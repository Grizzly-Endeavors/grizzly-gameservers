"""Reach the deployed bot's operator harness from eval scripts.

Mirrors what scripts/operator.sh does: port-forward the agent Service, read the
bearer token from the ESO-synced Secret, and read the report channel from the
gitignored repo-root .env. The token lives in memory only and is never printed
or included in an error message.
"""

from __future__ import annotations

import base64
import http.client
import json
import os
import socket
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any, Callable

DEFAULT_NAMESPACE = "game-servers"
SERVICE = "svc/grizzly-gameservers-agent"
REMOTE_PORT = 9360
SECRET_NAME = "grizzly-gameservers-operator"
SECRET_TOKEN_KEY = "operator_token"
ENV_NAMESPACE = "GAMESERVERS_NAMESPACE"
ENV_TOKEN = "GAMESERVERS_OPERATOR_TOKEN"
ENV_CHANNEL = "GAMESERVERS_OPERATOR_CHANNEL"
ENV_LOCAL_PORT = "GAMESERVERS_OPERATOR_LOCAL_PORT"

DEFAULT_TIMEOUT_S = 600.0
READY_TIMEOUT_S = 60.0
POLL_INTERVAL_S = 0.1
# A 503 means the Discord gateway is still connecting after a rollout; it
# resolves on its own, so a few spaced retries match scripts/operator.sh.
BUSY_ATTEMPTS = 3
BUSY_RETRY_DELAY_S = 3.0
# After a bot redeploy the port-forward is dead until it is re-established and
# the new pod answers /health. One delay per reconnect attempt, ~5 minutes in all.
RECONNECT_BACKOFF_S = (5.0, 15.0, 40.0, 80.0, 150.0)


class OperatorError(Exception):
    """The bot answered with a non-2xx status."""

    def __init__(self, status: int, message: str) -> None:
        super().__init__(f"HTTP {status}: {message}")
        self.status = status
        self.message = message


class OperatorConnError(Exception):
    """The harness could not be reached or configured."""


class _ConnectionFailure(Exception):
    """A transport failure; `maybe_sent` says whether the bot could have received the request."""

    def __init__(self, cause: BaseException, maybe_sent: bool) -> None:
        super().__init__(str(cause))
        self.cause = cause
        self.maybe_sent = maybe_sent


def _classify(err: BaseException) -> _ConnectionFailure | None:
    """Sort a transport error into refused (never sent), dropped (maybe sent) or neither."""
    reason = err.reason if isinstance(err, urllib.error.URLError) else err
    if isinstance(reason, ConnectionRefusedError):
        return _ConnectionFailure(err, maybe_sent=False)
    if isinstance(reason, (http.client.RemoteDisconnected, ConnectionResetError, BrokenPipeError)):
        return _ConnectionFailure(err, maybe_sent=True)
    return None


class _Link:
    """The port-forward behind a client, shared by every shallow copy of it.

    A run copies its client per Gary channel; they must all follow one reconnect
    rather than each holding the dead port.
    """

    def __init__(
        self,
        url: str,
        forward: subprocess.Popen[bytes] | None,
        log: Any,
        reopen: Callable[[], tuple[str, subprocess.Popen[bytes], Any]] | None,
    ) -> None:
        self.url = url
        self.forward = forward
        self.log = log
        self.reopen = reopen
        self.generation = 0
        self.failure_streak = 0
        self.lock = threading.Lock()

    def forward_exited(self) -> bool:
        return self.forward is not None and self.forward.poll() is not None

    def stop_forward(self) -> None:
        if self.forward is not None:
            self.forward.terminate()
            try:
                self.forward.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.forward.kill()
                self.forward.wait()
        if self.log is not None:
            self.log.close()
        self.forward = None
        self.log = None

    def reconnect(self, seen_generation: int) -> None:
        """Replace the forward unless another thread already did since `seen_generation`."""
        with self.lock:
            if self.generation != seen_generation:
                return
            if self.reopen is None:
                raise OperatorConnError("this client has no port-forward to re-establish")
            self.stop_forward()
            self.url, self.forward, self.log = self.reopen()
            self.generation += 1


def find_env_file() -> Path:
    """Return the repo-root .env, which a git worktree does not carry.

    The file is gitignored, so a worktree has none of its own; the main
    checkout's copy is the one the operator's machine actually holds.
    """
    here = Path(__file__).resolve().parents[2]
    own = here / ".env"
    if own.exists():
        return own
    try:
        common = subprocess.run(
            ["git", "rev-parse", "--path-format=absolute", "--git-common-dir"],
            cwd=here,
            capture_output=True,
            text=True,
            check=True,
            timeout=30,
        ).stdout.strip()
    except (OSError, subprocess.SubprocessError) as err:
        raise OperatorConnError(f"no .env at {own} and git could not locate the main checkout: {err}") from err
    return Path(common).parent / ".env"


def read_env_file(path: Path) -> dict[str, str]:
    """Parse KEY=VALUE lines, ignoring blanks, comments and surrounding quotes."""
    values: dict[str, str] = {}
    if not path.exists():
        return values
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("export "):
            line = line[len("export ") :]
        key, sep, value = line.partition("=")
        if not sep:
            continue
        value = value.strip()
        if len(value) >= 2 and value[0] == value[-1] and value[0] in ("'", '"'):
            value = value[1:-1]
        values[key.strip()] = value
    return values


def _free_local_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


def _error_text(raw: str) -> str:
    try:
        parsed = json.loads(raw)
    except ValueError:
        parsed = None
    if isinstance(parsed, dict) and isinstance(parsed.get("error"), str):
        return parsed["error"]
    return raw.strip() or "(empty body)"


def _poll_until(ready: Any, process: subprocess.Popen[bytes], what: str, log: Any) -> None:
    """Call `ready` until it returns True, failing if the forward dies or time runs out."""
    deadline = time.monotonic() + READY_TIMEOUT_S
    while True:
        if process.poll() is not None:
            raise OperatorConnError(f"{what} port-forward exited: {_tail(log)}")
        if ready():
            return
        if time.monotonic() >= deadline:
            raise OperatorConnError(
                f"{what} port-forward never answered; is the kubeconfig context the cluster? {_tail(log)}"
            )
        time.sleep(POLL_INTERVAL_S)


def _tail(log: Any) -> str:
    log.seek(0)
    text = log.read().decode("utf-8", "replace").strip()
    return f"kubectl said: {text[-400:]}" if text else "kubectl printed nothing"


class OperatorConn:
    """A client for the bot's /operator/* routes, bound to one report channel.

    Built either directly from a URL and token (`OperatorConn(...)`) or by
    `OperatorConn.connect()`, which owns a port-forward and tears it down on
    `close()` or context-manager exit.
    """

    def __init__(
        self,
        url: str,
        token: str,
        channel: str,
        *,
        timeout: float = DEFAULT_TIMEOUT_S,
        forward: subprocess.Popen[bytes] | None = None,
        forward_log: Any = None,
        reopen: Callable[[], tuple[str, subprocess.Popen[bytes], Any]] | None = None,
        backoff: tuple[float, ...] = RECONNECT_BACKOFF_S,
        sleep: Callable[[float], None] = time.sleep,
    ) -> None:
        self.token = token
        self.channel = channel
        self.timeout = timeout
        self._link = _Link(url.rstrip("/"), forward, forward_log, reopen)
        self._backoff = backoff
        self._sleep = sleep

    @property
    def url(self) -> str:
        return self._link.url

    @property
    def failure_streak(self) -> int:
        """Consecutive requests that failed to reach the bot even after reconnecting."""
        return self._link.failure_streak

    @classmethod
    def connect(cls, *, channel: str | None = None, timeout: float = DEFAULT_TIMEOUT_S) -> OperatorConn:
        """Port-forward the deployed bot and return a client for it.

        `channel` overrides GAMESERVERS_OPERATOR_CHANNEL. The environment wins
        over .env for both values, matching scripts/operator.sh's precedence for
        the token.
        """
        env_file = read_env_file(find_env_file())

        def setting(key: str) -> str | None:
            return os.environ.get(key) or env_file.get(key) or None

        resolved_channel = channel or setting(ENV_CHANNEL)
        if not resolved_channel:
            raise OperatorConnError(
                f"no report channel: set {ENV_CHANNEL} in the repo-root .env or pass one explicitly"
            )
        namespace = setting(ENV_NAMESPACE) or DEFAULT_NAMESPACE
        token = setting(ENV_TOKEN) or _read_token_secret(namespace)
        fixed_port = setting(ENV_LOCAL_PORT)

        def open_forward() -> tuple[str, subprocess.Popen[bytes], Any]:
            """A reconnect takes a fresh port: the old one may still be held by the dying forward."""
            return _open_forward(namespace, _free_local_port())

        url, process, log = _open_forward(namespace, int(fixed_port) if fixed_port else _free_local_port())
        return cls(
            url, token, resolved_channel, timeout=timeout, forward=process, forward_log=log, reopen=open_forward
        )

    def __enter__(self) -> OperatorConn:
        return self

    def __exit__(self, *exc_info: object) -> None:
        self.close()

    def close(self) -> None:
        """Stop the port-forward this client owns, if it owns one."""
        with self._link.lock:
            self._link.stop_forward()

    def tools(self, access: str = "admin") -> list[dict[str, Any]]:
        """The `function` object of every tool the tier may call, with its JSON schema."""
        listing = self._request("GET", "/operator/tools", query={"access": access})
        return [entry["function"] for entry in listing]

    def tool(
        self,
        name: str,
        arguments: dict[str, Any] | None = None,
        access: str = "admin",
        confirm: bool = False,
    ) -> dict[str, Any]:
        """Run one tool directly, with no model in the loop."""
        body: dict[str, Any] = {
            "channel": self.channel,
            "access": access,
            "tool": name,
            "arguments": arguments or {},
        }
        if confirm:
            body["confirm"] = True
        return self._request("POST", "/operator/tool", body)

    def gary(
        self,
        prompt: str,
        access: str = "admin",
        new_session: bool = False,
        confirm: bool = False,
        extra: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        """Run a full model turn. `extra` is merged into the body for fields this client does not name."""
        body: dict[str, Any] = {
            "channel": self.channel,
            "access": access,
            "prompt": prompt,
            "new_session": new_session,
        }
        if confirm:
            body["confirm"] = True
        body.update(extra or {})
        return self._request("POST", "/operator/gary", body)

    def _request(
        self,
        method: str,
        path: str,
        body: dict[str, Any] | None = None,
        query: dict[str, str] | None = None,
    ) -> Any:
        data = json.dumps(body).encode("utf-8") if body is not None else None
        headers = {"Authorization": f"Bearer {self.token}"}
        if data is not None:
            headers["Content-Type"] = "application/json"
        # A Gary turn is not idempotent: once it may have reached the bot, a second send
        # would run the model twice against the same session and servers.
        replayable = path != "/operator/gary"

        reconnects = 0
        while True:
            generation = self._link.generation
            try:
                if self._link.forward_exited():
                    raise _ConnectionFailure(OSError("port-forward process exited"), maybe_sent=False)
                result = self._send(method, path, data, headers, query)
            except _ConnectionFailure as failure:
                if self._link.reopen is None or (failure.maybe_sent and not replayable):
                    self._link.failure_streak += 1
                    raise OperatorConnError(f"{method} {path} failed: {failure.cause}") from failure.cause
                if reconnects >= len(self._backoff):
                    self._link.failure_streak += 1
                    raise OperatorConnError(
                        f"{method} {path} failed after {reconnects} reconnects: {failure.cause}"
                    ) from failure.cause
                self._sleep(self._backoff[reconnects])
                reconnects += 1
                try:
                    self._link.reconnect(generation)
                except OperatorConnError:
                    # The bot is likely still rolling out; the next backoff step tries again.
                    continue
                continue
            self._link.failure_streak = 0
            return result

    def _send(
        self,
        method: str,
        path: str,
        data: bytes | None,
        headers: dict[str, str],
        query: dict[str, str] | None,
    ) -> Any:
        url = self.url + path
        if query:
            url += "?" + urllib.parse.urlencode(query)
        for attempt in range(1, BUSY_ATTEMPTS + 1):
            request = urllib.request.Request(url, data=data, method=method, headers=headers)
            try:
                with urllib.request.urlopen(request, timeout=self.timeout) as response:
                    payload = response.read()
                return json.loads(payload) if payload else None
            except urllib.error.HTTPError as err:
                message = _error_text(err.read().decode("utf-8", "replace"))
                if err.code == 503 and attempt < BUSY_ATTEMPTS:
                    time.sleep(BUSY_RETRY_DELAY_S)
                    continue
                raise OperatorError(err.code, message) from None
            except (OSError, http.client.HTTPException) as err:
                failure = _classify(err)
                if failure is not None:
                    raise failure from err
                raise OperatorConnError(f"{method} {path} failed: {err}") from err
        raise AssertionError("unreachable: the final attempt either returns or raises")


def _open_forward(namespace: str, local_port: int) -> tuple[str, subprocess.Popen[bytes], Any]:
    """Start `kubectl port-forward` and wait until the bot answers /health through it."""
    log = tempfile.TemporaryFile()
    try:
        process = subprocess.Popen(
            ["kubectl", "port-forward", "-n", namespace, SERVICE, f"{local_port}:{REMOTE_PORT}"],
            stdout=subprocess.DEVNULL,
            stderr=log,
        )
    except OSError as err:
        log.close()
        raise OperatorConnError(f"could not start kubectl port-forward: {err}") from err
    url = f"http://127.0.0.1:{local_port}"
    try:
        _poll_until(lambda: _accepts_connections(local_port), process, "the deployed bot", log)
        _poll_until(lambda: _health_ok(url), process, "the deployed bot", log)
    except BaseException:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        log.close()
        raise
    return url, process, log


def _read_token_secret(namespace: str) -> str:
    """Read and decode the operator token from the ESO-synced Secret, in memory only."""
    try:
        result = subprocess.run(
            [
                "kubectl",
                "-n",
                namespace,
                "get",
                "secret",
                SECRET_NAME,
                "-o",
                f"jsonpath={{.data.{SECRET_TOKEN_KEY}}}",
            ],
            capture_output=True,
            text=True,
            timeout=60,
        )
    except (OSError, subprocess.SubprocessError) as err:
        raise OperatorConnError(f"could not run kubectl to read secret {SECRET_NAME}: {err}") from err
    if result.returncode != 0:
        raise OperatorConnError(
            f"kubectl could not read secret {SECRET_NAME} in {namespace}: {result.stderr.strip()[-400:]}"
        )
    encoded = result.stdout.strip()
    if not encoded:
        raise OperatorConnError(
            f"secret {SECRET_NAME} in {namespace} has no {SECRET_TOKEN_KEY}; the bot will not mount /operator/*"
        )
    try:
        return base64.b64decode(encoded, validate=True).decode("utf-8")
    except (ValueError, UnicodeDecodeError) as err:
        raise OperatorConnError(f"secret {SECRET_NAME} {SECRET_TOKEN_KEY} is not valid base64") from err


def _accepts_connections(port: int) -> bool:
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=1):
            return True
    except OSError:
        return False


def _health_ok(url: str) -> bool:
    try:
        with urllib.request.urlopen(url + "/health", timeout=2) as response:
            return bool(response.status == 200)
    except (OSError, http.client.HTTPException):
        return False
