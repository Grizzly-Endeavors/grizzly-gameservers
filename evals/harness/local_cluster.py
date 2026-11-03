"""A cluster backend for run.py made of plain Docker containers and a local Gary binary.

Each eval server is one `docker run` of the game's composite image with its catalog
env, a named volume at the data dir and the supervisor's control port published on
loopback. Gary's tools and turns go through `grizzly-gameservers gary-local`, which
takes one JSON request on stdin and answers with the body the operator route would.
Creating and destroying servers is the harness's own business, done here through
docker, because the binary does not offer those tools.
"""

from __future__ import annotations

import json
import os
import re
import secrets
import socket
import subprocess
import threading
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

import yaml

from cluster_errors import ClusterError

REPO_ROOT = Path(__file__).resolve().parents[2]
GAMES_DIR = REPO_ROOT / "games"
CONTROL_PORT = 9359
DEFAULT_IMAGE_PREFIX = "localhost:5000/grizzly-gameservers-"
IMAGE_TAG = "dev"
IMAGE_PREFIX_KEY = "GARY_LOCAL_IMAGE_PREFIX"
BINARY_KEY = "GARY_LOCAL_BIN"
ENV_FILE_KEY = "GAMESERVERS_ENV_FILE"
BINARY_NAME = "grizzly-gameservers"
DUMMY_AGENT_URL = "http://127.0.0.1:1/ingame-trigger"
RUN_LABEL = "grizzly-eval.run"
SERVER_LABEL = "grizzly-eval.server"
PORT_ENV_PREFIX = "grizzly-gameservers.grizzly-endeavors.com/port-env."
STATUS_TIMEOUT_S = 5.0
DOCKER_TIMEOUT_S = 300.0
GARY_TIMEOUT_S = 1200.0
QUANTITY = re.compile(r"^([0-9]+(?:\.[0-9]+)?)([KMGTE]i?|[kmgte])?$")
BINARY_SUFFIX = {"Ki": 2**10, "Mi": 2**20, "Gi": 2**30, "Ti": 2**40, "Ei": 2**60}
DECIMAL_SUFFIX = {"k": 10**3, "K": 10**3, "M": 10**6, "G": 10**9, "T": 10**12, "E": 10**18}

Runner = Callable[..., "subprocess.CompletedProcess[str]"]


class GaryLocalTimeout(ClusterError):
    """The binary did not answer within its budget."""


# --------------------------------------------------------------------------
# reading the catalog


def load_template(game: str, games_dir: Path = GAMES_DIR) -> tuple[dict[str, Any], dict[str, Any] | None]:
    """The game's GameServer template and, when it has one, its Service template."""
    game_dir = games_dir / game
    template = yaml.safe_load((game_dir / "gameserver.yaml").read_text(encoding="utf-8"))
    service_path = game_dir / "service.yaml"
    service = yaml.safe_load(service_path.read_text(encoding="utf-8")) if service_path.exists() else None
    return template, service


def game_container(template: dict[str, Any]) -> dict[str, Any]:
    """The container that runs the game: the one `spec.container` names, else the first."""
    containers = template["spec"]["template"]["spec"]["containers"]
    wanted = template["spec"].get("container")
    return next((c for c in containers if c.get("name") == wanted), containers[0])


def container_env(
    template: dict[str, Any], service: dict[str, Any] | None, name: str, game: str
) -> dict[str, str]:
    """The game container's env as the bot would render it, minus what only a cluster can supply.

    Secret-sourced values are dropped (each is optional in the catalog). A fieldRef to the
    GameServer's own name becomes `name`. Advertised ports get their container port, since
    nothing outside the container dials them. The in-game chat loop gets a dead agent URL
    because a missing one fails the supervisor's startup, and the auto-updater is off so a
    run never races an upstream release.
    """
    env: dict[str, str] = {}
    for entry in game_container(template).get("env") or []:
        if "value" in entry:
            env[entry["name"]] = str(entry["value"])
            continue
        field_path = ((entry.get("valueFrom") or {}).get("fieldRef") or {}).get("fieldPath", "")
        if "agones.dev/gameserver" in field_path or field_path == "metadata.name":
            env[entry["name"]] = name
    env.update(advertised_port_env(service))
    if "SUPERVISOR_AGENT_URL" in env or game == "minecraft":
        env["SUPERVISOR_AGENT_URL"] = DUMMY_AGENT_URL
        env["SUPERVISOR_GAMESERVER_NAME"] = name
    env["SUPERVISOR_AUTO_UPDATE"] = "false"
    return env


def advertised_port_env(service: dict[str, Any] | None) -> dict[str, str]:
    """Each `port-env.<port>` annotation's vars, set to that port's container port."""
    if not service:
        return {}
    annotations = (service.get("metadata") or {}).get("annotations") or {}
    ports = {port.get("name"): port for port in (service.get("spec") or {}).get("ports") or []}
    env: dict[str, str] = {}
    for key, value in annotations.items():
        if not key.startswith(PORT_ENV_PREFIX):
            continue
        port = ports.get(key.removeprefix(PORT_ENV_PREFIX))
        if port is None:
            continue
        number = port.get("targetPort") or port.get("port")
        for var in str(value).split(","):
            if var.strip():
                env[var.strip()] = str(number)
    return env


def data_mount(template: dict[str, Any]) -> str:
    """Where the game's persistent volume is mounted."""
    volumes = template["spec"]["template"]["spec"].get("volumes") or []
    claimed = {v["name"] for v in volumes if "persistentVolumeClaim" in v}
    mounts = game_container(template).get("volumeMounts") or []
    mount = next((m for m in mounts if m.get("name") in claimed), None)
    if mount is None:
        declared = {e["name"]: e.get("value") for e in game_container(template).get("env") or []}
        return str(declared.get("SUPERVISOR_DATA_DIR") or "/data")
    return str(mount["mountPath"])


def memory_bytes(quantity: str) -> int:
    """A Kubernetes memory quantity (`6Gi`, `2G`, `512Mi`, `1000`) in bytes."""
    match = QUANTITY.match(str(quantity).strip())
    if not match:
        raise ValueError(f"cannot read memory quantity {quantity!r}")
    number, suffix = float(match.group(1)), match.group(2)
    scale = 1 if suffix is None else BINARY_SUFFIX.get(suffix) or DECIMAL_SUFFIX[suffix]
    return int(number * scale)


def memory_limit(template: dict[str, Any]) -> int | None:
    limits = (game_container(template).get("resources") or {}).get("limits") or {}
    return memory_bytes(limits["memory"]) if "memory" in limits else None


def security_flags(template: dict[str, Any]) -> list[str]:
    """The container's capability set and privilege-escalation bar as docker flags."""
    context = game_container(template).get("securityContext") or {}
    flags: list[str] = []
    caps = context.get("capabilities") or {}
    flags += [f"--cap-drop={cap}" for cap in caps.get("drop") or []]
    flags += [f"--cap-add={cap}" for cap in caps.get("add") or []]
    if context.get("allowPrivilegeEscalation") is False:
        flags += ["--security-opt", "no-new-privileges"]
    return flags


def run_prefix(host: str | None = None) -> str:
    """A container-name prefix unique to this host and this process."""
    raw = (host or socket.gethostname()).lower()
    label = re.sub(r"-+", "-", re.sub(r"[^a-z0-9]+", "-", raw)).strip("-")[:20] or "host"
    return f"gev-{label}-{secrets.token_hex(3)}"


# --------------------------------------------------------------------------
# the Gary binary


def resolve_binary(environ: Any = os.environ, repo_root: Path = REPO_ROOT) -> Path:
    """`GARY_LOCAL_BIN`, else the release build, else the debug build."""
    named = environ.get(BINARY_KEY)
    if named:
        path = Path(named)
        if not path.is_file():
            raise ValueError(f"{BINARY_KEY} names {path}, which is not a file")
        return path
    for profile in ("release", "debug"):
        path = repo_root / "target" / profile / BINARY_NAME
        if path.is_file():
            return path
    raise ValueError(
        f"no gary-local binary: set {BINARY_KEY} or build it (cargo build --release -p grizzly-gameservers-bot)"
    )


class GaryLocal:
    """One `gary-local` invocation per request: JSON in on stdin, JSON out on stdout."""

    def __init__(self, binary: Path, env_file: Path | None, runner: Runner = subprocess.run) -> None:
        self.binary = binary
        self.env_file = env_file
        self.runner = runner

    def call(self, body: dict[str, Any], timeout: float) -> dict[str, Any]:
        env = dict(os.environ)
        if self.env_file is not None:
            env[ENV_FILE_KEY] = str(self.env_file)
        try:
            done = self.runner(
                [str(self.binary), "gary-local"],
                input=json.dumps(body),
                capture_output=True,
                text=True,
                timeout=timeout,
                env=env,
            )
        except subprocess.TimeoutExpired as err:
            raise GaryLocalTimeout(f"gary-local did not answer within {timeout:.0f}s") from err
        except (OSError, subprocess.SubprocessError) as err:
            raise ClusterError(f"could not run {self.binary}: {err}") from err
        if done.returncode != 0:
            raise ClusterError(f"gary-local exited {done.returncode}: {done.stderr.strip()[-400:]}")
        try:
            parsed = json.loads(done.stdout)
        except json.JSONDecodeError as err:
            raise ClusterError(f"gary-local wrote unreadable output ({err}): {done.stdout[-300:]}") from err
        if not isinstance(parsed, dict):
            raise ClusterError(f"gary-local answered with a {type(parsed).__name__}, not an object")
        return parsed


# --------------------------------------------------------------------------
# the cluster


@dataclass
class LocalServer:
    name: str
    game: str
    container: str
    volume: str
    control_url: str

    def entry(self) -> dict[str, str]:
        return {"name": self.name, "game": self.game, "control_url": self.control_url}


def fetch_status(url: str) -> dict[str, Any] | None:
    """The supervisor's `GET /status` body, or None while it does not answer."""
    try:
        with urllib.request.urlopen(url, timeout=STATUS_TIMEOUT_S) as response:
            parsed = json.loads(response.read().decode("utf-8"))
    except (urllib.error.URLError, OSError, ValueError):
        return None
    return parsed if isinstance(parsed, dict) else None


class DockerCluster:
    """The interface run.py's `Cluster` offers, over local containers and the gary-local binary."""

    def __init__(
        self,
        gary: GaryLocal | None = None,
        *,
        runner: Runner = subprocess.run,
        status: Callable[[str], dict[str, Any] | None] = fetch_status,
        prefix: str | None = None,
        image_prefix: str | None = None,
        games_dir: Path = GAMES_DIR,
        ready_timeout_s: float = 900.0,
    ) -> None:
        self.gary = gary
        self.runner = runner
        self.status = status
        self.prefix = prefix or run_prefix()
        self.image_prefix = image_prefix or os.environ.get(IMAGE_PREFIX_KEY) or DEFAULT_IMAGE_PREFIX
        self.games_dir = games_dir
        self.ready_timeout_s = ready_timeout_s
        self.failure_streak = 0
        self._servers: dict[str, LocalServer] = {}
        self._lock = threading.Lock()

    def now(self) -> float:
        return time.monotonic()

    def sleep(self, seconds: float) -> None:
        time.sleep(seconds)

    def container_name(self, name: str) -> str:
        return f"{self.prefix}-{name}"

    def volume_name(self, name: str) -> str:
        return f"{self.container_name(name)}-data"

    def image(self, game: str) -> str:
        return f"{self.image_prefix}{game}:{IMAGE_TAG}"

    def _docker(self, args: list[str], timeout: float = DOCKER_TIMEOUT_S) -> subprocess.CompletedProcess[str]:
        try:
            done = self.runner(["docker", *args], capture_output=True, text=True, timeout=timeout)
        except subprocess.TimeoutExpired as err:
            raise ClusterError(f"docker {args[0]} timed out after {timeout:.0f}s") from err
        except (OSError, subprocess.SubprocessError) as err:
            self.failure_streak += 1
            raise ClusterError(f"docker {args[0]} failed: {err}") from err
        self.failure_streak = 0
        return done

    def _server(self, name: str) -> LocalServer:
        with self._lock:
            server = self._servers.get(name)
        if server is None:
            raise ClusterError(f"no local server named {name}")
        # `docker restart` republishes the control port on a new host port, so the URL
        # recorded at create time goes stale; resolve it again on every lookup.
        try:
            server.control_url = self._control_url(name)
        except ClusterError:
            pass
        return server

    # ---- lifecycle the harness owns

    def run_args(self, game: str, name: str) -> list[str]:
        """The `docker run` arguments that start `name` as a fresh instance of `game`."""
        template, service = load_template(game, self.games_dir)
        args = [
            "run", "-d",
            "--name", self.container_name(name),
            "--label", f"{RUN_LABEL}={self.prefix}",
            "--label", f"{SERVER_LABEL}={name}",
            "--mount", f"type=volume,src={self.volume_name(name)},dst={data_mount(template)},volume-nocopy",
            "-p", f"127.0.0.1::{CONTROL_PORT}",
        ]
        limit = memory_limit(template)
        if limit is not None:
            args += ["--memory", str(limit)]
        args += security_flags(template)
        for key, value in container_env(template, service, name, game).items():
            args += ["-e", f"{key}={value}"]
        args.append(self.image(game))
        return args

    def create(self, game: str, name: str) -> str:
        try:
            args = self.run_args(game, name)
        except (OSError, KeyError, IndexError, ValueError, yaml.YAMLError) as err:
            raise ClusterError(f"cannot read the catalog entry for {game}: {err}") from err
        volume = self._docker(
            ["volume", "create", "--label", f"{RUN_LABEL}={self.prefix}", self.volume_name(name)]
        )
        if volume.returncode != 0:
            raise ClusterError(f"docker volume create failed: {volume.stderr.strip()[-300:]}")
        started = self._docker(args)
        if started.returncode != 0:
            raise ClusterError(f"docker run {self.image(game)} failed: {started.stderr.strip()[-300:]}")
        url = self._control_url(name)
        with self._lock:
            self._servers[name] = LocalServer(name, game, self.container_name(name), self.volume_name(name), url)
        return f"created {name}; supervisor control at {url}"

    def _control_url(self, name: str) -> str:
        done = self._docker(["port", self.container_name(name), f"{CONTROL_PORT}/tcp"])
        first = done.stdout.strip().splitlines()[0] if done.stdout.strip() else ""
        host, _, port = first.rpartition(":")
        if done.returncode != 0 or not port.isdigit():
            raise ClusterError(f"no published control port for {name}: {(done.stderr or done.stdout).strip()[-300:]}")
        return f"http://{host.strip('[]') or '127.0.0.1'}:{port}"

    def destroy(self, name: str) -> str:
        """Remove the container and its volume; a server that is already gone counts as removed."""
        removed = self._docker(["rm", "-f", "-v", self.container_name(name)], 180.0)
        if removed.returncode != 0 and "No such container" not in removed.stderr:
            return f"could not remove container: {removed.stderr.strip()[-200:]}"
        volume = self._docker(["volume", "rm", "-f", self.volume_name(name)], 180.0)
        if volume.returncode != 0 and "no such volume" not in volume.stderr.lower():
            return f"could not remove volume: {volume.stderr.strip()[-200:]}"
        with self._lock:
            self._servers.pop(name, None)
        return f"deleted {name} and its world"

    def sweep(self) -> list[str]:
        """Remove every container and volume this cluster's prefix labelled; returns what it removed."""
        removed: list[str] = []
        label = f"label={RUN_LABEL}={self.prefix}"
        containers = self._docker(["ps", "-aq", "--filter", label])
        ids = containers.stdout.split() if containers.returncode == 0 else []
        if ids and self._docker(["rm", "-f", "-v", *ids], 300.0).returncode == 0:
            removed += ids
        volumes = self._docker(["volume", "ls", "-q", "--filter", label])
        names = volumes.stdout.split() if volumes.returncode == 0 else []
        if names and self._docker(["volume", "rm", "-f", *names], 300.0).returncode == 0:
            removed += names
        with self._lock:
            self._servers.clear()
        return removed

    # ---- the interface run.py grades through

    def tool(self, tool: str, arguments: dict[str, Any], confirm: bool = False) -> str:
        if tool == "create_server":
            return self.create(arguments["game"], arguments["name"])
        if tool == "destroy_server":
            return self.destroy(arguments["name"])
        server = self._server(arguments.get("name", ""))
        body = {"servers": [server.entry()], "tool": tool, "arguments": arguments, "access": "admin"}
        response = self._call_gary(body)
        if "result" not in response:
            raise ClusterError(f"gary-local tool {tool} returned no result: {json.dumps(response)[:300]}")
        return str(response["result"])

    def gary_turn(
        self, name: str, prompt: str, model_profile: dict[str, Any] | None, timeout: float = GARY_TIMEOUT_S
    ) -> dict[str, Any]:
        """One full Gary turn over `name`; the body is what `POST /operator/gary` returns."""
        body: dict[str, Any] = {
            "servers": [self._server(name).entry()],
            "prompt": prompt,
            "access": "admin",
            "memory": False,
        }
        if model_profile:
            body["model_profile"] = model_profile
        return self._call_gary(body, timeout)

    def _call_gary(self, body: dict[str, Any], timeout: float = GARY_TIMEOUT_S) -> dict[str, Any]:
        if self.gary is None:
            raise ClusterError("this local cluster has no gary-local binary")
        try:
            response = self.gary.call(body, timeout)
        except GaryLocalTimeout:
            raise
        except ClusterError:
            self.failure_streak += 1
            raise
        self.failure_streak = 0
        return response

    def state(self, name: str) -> str:
        """`Ready` once the supervisor reports the game running and ready, `absent` with no container."""
        done = self._docker(["inspect", "-f", "{{.State.Status}}", self.container_name(name)], 60.0)
        if done.returncode != 0:
            if "No such" in done.stderr:
                return "absent"
            raise ClusterError(f"docker inspect {name} failed: {done.stderr.strip()[-300:]}")
        container_state = done.stdout.strip()
        if container_state in ("created", "restarting"):
            return "Scheduled"
        if container_state != "running":
            return "Unhealthy"
        try:
            server = self._server(name)
        except ClusterError:
            return "unknown"
        status = self.status(f"{server.control_url}/status")
        if status is None:
            return "Scheduled"
        phase = str(status.get("process") or "unknown")
        if phase == "running" and status.get("ready"):
            return "Ready"
        return "Starting" if phase == "running" else phase.capitalize()

    def exec(self, name: str, argv: list[str], timeout: float = 120.0) -> tuple[int, str]:
        done = self._docker(["exec", self.container_name(name), *argv], timeout)
        return done.returncode, done.stdout if done.returncode == 0 else done.stdout + done.stderr

    def put(self, name: str, src: Path, dst: str) -> None:
        done = self._docker(["cp", str(src), f"{self.container_name(name)}:{dst}"], 300.0)
        if done.returncode != 0:
            raise ClusterError(f"docker cp {src} failed: {done.stderr.strip()[-300:]}")

    def forget(self, name: str) -> None:
        return None
