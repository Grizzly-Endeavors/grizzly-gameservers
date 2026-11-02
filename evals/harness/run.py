#!/usr/bin/env python3
"""Run eval cases against live game servers and write the run records score.py reads.

Per (system, case, repeat) the runner creates a fresh server, stages the case,
hands the customer message to the system, grades the result before and after
one runner-triggered restart, and always destroys the server. Procedure and
budget: evals/evals.md sections 2, 4 and 5. Case format: evals/cases (YAML).

Usage: python3 evals/harness/run.py --cases evals/cases/dev --systems gary-ollama-dev
       [--case ID ...] [--repeats N] [--concurrency N] [--out DIR] [--dry-run]
       [--game-cap game=N ...] [--allow-missing-extras] [--ready-timeout S]
       [--rerun-outcomes infra_error,staging_error,missing] [--force]

Gary channels: GAMESERVERS_EVAL_CHANNELS (comma list) in the repo-root .env,
else the single operator channel. A Gary turn holds a per-channel lock because
new_session clears the whole channel session.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import os
import re
import shlex
import subprocess
import sys
import tempfile
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parent))

from operator_conn import (
    OperatorConn,
    OperatorConnError,
    OperatorError,
    find_env_file,
    read_env_file,
)
from scrub import dumps_scrubbed, known_ids, scrub_text

REPO_ROOT = Path(__file__).resolve().parents[2]
HARNESS_DIR = Path(__file__).resolve().parent
SYSTEMS_PATH = HARNESS_DIR / "systems.json"
GARY_MCP_PATH = HARNESS_DIR / "gary_mcp.py"

NAMESPACE = "game-servers"
SLOW_GAMES = frozenset({"valheim", "palworld", "enshrouded"})
DEFAULT_READY_S = 900.0
ABORT_AFTER_FAILURES = 5
RERUNNABLE_OUTCOMES = ("infra_error", "staging_error", "missing")
DEFAULT_SLOW_GAME_CAP = 2
POLL_INTERVAL_S = 5.0
WALL_CLOCK_S = 1200.0
MAX_MODEL_CALLS = 40
MAX_NAME_LEN = 40
LOG_LINES = 400
CHANNELS_KEY = "GAMESERVERS_EVAL_CHANNELS"
CC_EXCLUDED_TOOLS = "remember,forget"
CASE_REQUIRED = ("id", "game", "prompt", "checks")
UNKNOWN_FIELD_STATUSES = (400, 422)
AUTH_FAILURE = re.compile(r"(?i)api key|log ?in|authenticat|unauthori[sz]ed|auth token")


class ClusterError(Exception):
    """kubectl or the operator route failed in a way that is not the system's fault."""


class StagingError(Exception):
    """The case could not be put into its starting condition."""


# --------------------------------------------------------------------------
# cases and systems


def load_cases(cases_dir: Path, only: list[str] | None) -> list[dict[str, Any]]:
    cases = []
    for path in sorted(cases_dir.glob("*.y*ml")):
        case = yaml.safe_load(path.read_text(encoding="utf-8"))
        if not isinstance(case, dict):
            raise ValueError(f"case file {path} is not a mapping")
        missing = [key for key in CASE_REQUIRED if key not in case]
        if missing:
            raise ValueError(f"case file {path} is missing {', '.join(missing)}")
        if only and case["id"] not in only:
            continue
        cases.append(case)
    ids = [case["id"] for case in cases]
    if len(ids) != len(set(ids)):
        raise ValueError(f"duplicate case ids in {cases_dir}")
    unknown = sorted(set(only or []) - set(ids))
    if unknown:
        raise ValueError(f"no such case in {cases_dir}: {', '.join(unknown)}")
    if not cases:
        raise ValueError(f"no cases found in {cases_dir}")
    return cases


def load_systems(path: Path, wanted: list[str]) -> list[dict[str, Any]]:
    everything = {entry["system_id"]: entry for entry in json.loads(path.read_text(encoding="utf-8"))}
    unknown = [name for name in wanted if name not in everything]
    if unknown:
        raise ValueError(f"unknown system {', '.join(unknown)}; known: {', '.join(sorted(everything))}")
    return [everything[name] for name in wanted]


def eval_channels(arg: str | None, environ: Any, env_file: dict[str, str]) -> list[str]:
    """Gary channels: --channels, else GAMESERVERS_EVAL_CHANNELS. The first also carries every operator tool call.

    Servers are stamped with the guild of the channel that created them, so tool calls must
    all use one channel; Gary turns round-robin over the whole list.
    """
    listed = arg or environ.get(CHANNELS_KEY) or env_file.get(CHANNELS_KEY) or ""
    return [c.strip() for c in listed.split(",") if c.strip()]


def resolve_env(spec: dict[str, str], env_file: dict[str, str]) -> dict[str, str]:
    """Expand `$VAR` values from the process environment, then the repo-root .env."""
    resolved = {}
    for key, value in spec.items():
        if value.startswith("$"):
            source = value[1:]
            found = os.environ.get(source) or env_file.get(source)
            if not found:
                raise ValueError(f"{key} needs {source}, which is in neither the environment nor .env")
            resolved[key] = found
        else:
            resolved[key] = value
    return resolved


def server_name(case_id: str, system_id: str, repeat: int) -> str:
    """An RFC1123 label of at most 40 chars; a hash keeps truncated names distinct."""
    full = f"ev-{case_id}-{system_id}-{repeat}"
    label = re.sub(r"-+", "-", re.sub(r"[^a-z0-9]+", "-", full.lower())).strip("-")
    if len(label) <= MAX_NAME_LEN:
        return label
    digest = hashlib.sha1(full.encode("utf-8")).hexdigest()[:6]
    return label[: MAX_NAME_LEN - 7].rstrip("-") + "-" + digest


def ready_timeout(cluster: Any) -> float:
    """PVC provisioning dominates create time for every game, so one timeout serves all."""
    return float(getattr(cluster, "ready_timeout_s", DEFAULT_READY_S))


def context_line(name: str, game: str) -> str:
    return f"The customer's server is named {name} (game: {game})."


# --------------------------------------------------------------------------
# cluster access


class Cluster:
    """The live cluster: operator tool route for Gary's tools, kubectl for the pod."""

    def __init__(
        self, conn: OperatorConn, namespace: str = NAMESPACE, ready_timeout_s: float = DEFAULT_READY_S
    ) -> None:
        self.conn = conn
        self.ready_timeout_s = ready_timeout_s
        self.namespace = namespace
        self._pods: dict[str, tuple[str, str]] = {}

    @property
    def failure_streak(self) -> int:
        return self.conn.failure_streak

    def now(self) -> float:
        return time.monotonic()

    def sleep(self, seconds: float) -> None:
        time.sleep(seconds)

    def tool(self, tool: str, arguments: dict[str, Any], confirm: bool = False) -> str:
        try:
            return str(self.conn.tool(tool, arguments, access="admin", confirm=confirm)["result"])
        except (OperatorError, OperatorConnError) as err:
            raise ClusterError(f"operator tool {tool} failed: {err}") from err

    def _kubectl(self, args: list[str], timeout: float = 120.0) -> subprocess.CompletedProcess[str]:
        try:
            return subprocess.run(
                ["kubectl", "-n", self.namespace, *args], capture_output=True, text=True, timeout=timeout
            )
        except (OSError, subprocess.SubprocessError) as err:
            raise ClusterError(f"kubectl {args[0]} failed: {err}") from err

    def state(self, name: str) -> str:
        """The GameServer's Agones state, or `absent` while it does not exist yet."""
        done = self._kubectl(["get", "gameserver", name, "-o", "jsonpath={.status.state}"])
        if done.returncode != 0:
            if "NotFound" in done.stderr:
                return "absent"
            raise ClusterError(f"kubectl get gameserver {name} failed: {done.stderr.strip()[-300:]}")
        return done.stdout.strip() or "unknown"

    def _pod(self, name: str) -> tuple[str, str]:
        cached = self._pods.get(name)
        if cached:
            return cached
        done = self._kubectl(
            [
                "get", "pod", "-l", f"agones.dev/gameserver={name}", "-o",
                "jsonpath={.items[0].metadata.name} {.items[0].spec.containers[0].name}",
            ]
        )
        parts = done.stdout.split()
        if done.returncode != 0 or len(parts) != 2:
            raise ClusterError(f"no pod found for server {name}: {done.stderr.strip()[-300:]}")
        self._pods[name] = (parts[0], parts[1])
        return self._pods[name]

    def exec(self, name: str, argv: list[str], timeout: float = 120.0) -> tuple[int, str]:
        pod, container = self._pod(name)
        done = self._kubectl(["exec", pod, "-c", container, "--", *argv], timeout)
        return done.returncode, done.stdout if done.returncode == 0 else done.stdout + done.stderr

    def put(self, name: str, src: Path, dst: str) -> None:
        pod, container = self._pod(name)
        done = self._kubectl(["cp", "-c", container, str(src), f"{self.namespace}/{pod}:{dst}"], 300.0)
        if done.returncode != 0:
            raise ClusterError(f"kubectl cp {src} failed: {done.stderr.strip()[-300:]}")

    def forget(self, name: str) -> None:
        self._pods.pop(name, None)


# --------------------------------------------------------------------------
# waiting, staging, grading (all cluster-agnostic: they only use Cluster's interface)


def wait_for(cluster: Any, ready: Callable[[], bool], timeout_s: float) -> bool:
    deadline = cluster.now() + timeout_s
    while True:
        if ready():
            return True
        if cluster.now() >= deadline:
            return False
        cluster.sleep(POLL_INTERVAL_S)


def wait_ready(cluster: Any, name: str) -> bool:
    return wait_for(cluster, lambda: cluster.state(name) == "Ready", ready_timeout(cluster))


def apply_setup(cluster: Any, case: dict[str, Any], name: str) -> list[str]:
    """Run the setup steps in order; any failure raises StagingError. Returns a step log."""
    log = []
    for index, step in enumerate(case.get("setup") or []):
        label = f"setup[{index}] {next(iter(step), '?')}"
        try:
            if "tool" in step:
                args = dict(step.get("args") or {})
                args["name"] = name
                log.append(f"{label} {step['tool']}: {cluster.tool(step['tool'], args)[:300]}")
            elif "pod_exec" in step:
                rc, out = cluster.exec(name, shlex.split(step["pod_exec"]))
                if rc != 0:
                    raise StagingError(f"{label} exited {rc}: {out.strip()[-300:]}")
                log.append(f"{label} ok")
            elif "pod_put" in step:
                put = step["pod_put"]
                cluster.put(name, REPO_ROOT / put["src"], put["dst"])
                log.append(f"{label} ok")
            elif "restart" in step:
                log.append(f"{label}: {cluster.tool('restart_server', {'name': name})[:300]}")
            elif "wait" in step:
                wait_step(cluster, step, name)
                log.append(f"{label} {step['wait']} ok")
            else:
                raise StagingError(f"{label} is not a known step kind")
        except ClusterError as err:
            raise StagingError(f"{label} failed: {err}") from err
    return log


def wait_step(cluster: Any, step: dict[str, Any], name: str) -> None:
    kind = step["wait"]
    timeout = float(step.get("timeout_s", ready_timeout(cluster)))
    if kind == "ready":
        ok = wait_for(cluster, lambda: cluster.state(name) == "Ready", timeout)
    elif kind == "not_ready":
        ok = wait_for(cluster, lambda: cluster.state(name) != "Ready", timeout)
    elif kind == "log":
        pattern = re.compile(step["pattern"])
        ok = wait_for(
            cluster,
            lambda: bool(pattern.search(cluster.tool("read_logs", {"name": name, "lines": LOG_LINES}))),
            timeout,
        )
    else:
        raise StagingError(f"wait kind {kind!r} is not known")
    if not ok:
        raise StagingError(f"wait {kind} did not hold within {timeout:.0f}s")


def read_pod_file(cluster: Any, name: str, path: str) -> tuple[bool, str]:
    rc, out = cluster.exec(name, ["cat", path])
    return rc == 0, out


def check_label(check: dict[str, Any]) -> str:
    if "path" in check:
        return f"{check['type']} {check['path']}"
    if "tool" in check:
        return f"{check['type']} {check['tool']}"
    return check["type"]


def run_check(cluster: Any, check: dict[str, Any], name: str, reply: str) -> dict[str, Any]:
    try:
        ok, detail = _evaluate(cluster, check, name, reply)
    except ClusterError as err:
        ok, detail = False, f"cluster error: {err}"
    return {"name": check_label(check), "ok": ok, "detail": detail}


def _evaluate(cluster: Any, check: dict[str, Any], name: str, reply: str) -> tuple[bool, str]:
    kind = check["type"]
    if kind == "ready":
        state = cluster.state(name)
        return state == "Ready", f"state {state}"
    if kind == "reply_regex":
        return bool(re.search(check["pattern"], reply)), ""
    if kind == "any_of":
        subs = [run_check(cluster, sub, name, reply) for sub in check["checks"]]
        if not subs:
            return False, "any_of has no checks"
        detail = "; ".join(f"{sub['name']} {'passed' if sub['ok'] else 'failed'}: {sub['detail']}" for sub in subs)
        return any(sub["ok"] for sub in subs), detail
    if kind == "tool_regex":
        args = dict(check.get("args") or {})
        args["name"] = name
        result = cluster.tool(check["tool"], args)
        matched = bool(re.search(check["pattern"], result))
        present = check.get("present", True)
        verdict = "matched" if matched else "did not match"
        return matched == present, f"pattern {check['pattern']!r} {verdict}; result: {result.strip()[-200:]}"
    if kind == "pod_exec":
        rc, out = cluster.exec(name, shlex.split(check["cmd"]))
        expect = check.get("expect_exit", 0)
        return rc == expect, f"exit {rc}, wanted {expect}: {out.strip()[-200:]}"
    if kind == "file_regex":
        present = check.get("present", True)
        found, text = read_pod_file(cluster, name, check["path"])
        matched = found and bool(re.search(check["pattern"], text))
        if not found and present:
            return False, f"could not read file: {text.strip()[-200:]}"
        return matched == present, "" if matched == present else f"pattern {'absent' if present else 'present'}"
    if kind == "json_equals":
        found, text = read_pod_file(cluster, name, check["path"])
        if not found:
            return False, f"could not read file: {text.strip()[-200:]}"
        try:
            value: Any = json.loads(text)
            for part in str(check["key"]).split("."):
                value = value[int(part)] if isinstance(value, list) else value[part]
        except (ValueError, KeyError, IndexError, TypeError) as err:
            return False, f"key {check['key']} not readable: {err}"
        return value == check["equals"], f"{check['key']} is {value!r}"
    return False, f"unknown check type {kind}"


def run_checks(cluster: Any, case: dict[str, Any], name: str, reply: str, prefix: str) -> list[dict[str, Any]]:
    results = [run_check(cluster, check, name, reply) for check in case["checks"]]
    return [{**result, "name": f"{prefix}: {result['name']}"} for result in results]


# --------------------------------------------------------------------------
# running a system


@dataclass
class SystemRun:
    reply: str = ""
    claimed_done: bool = False
    turns: int = 0
    tool_calls: list[str] = field(default_factory=list)
    prompt_tokens: int = 0
    cached_tokens: int = 0
    completion_tokens: int = 0
    model_ms: int = 0
    wall_ms: int = 0
    infra_error: str | None = None
    notes: dict[str, Any] = field(default_factory=dict)
    transcript: Any = None
    conversation: dict[str, Any] | None = None

    @property
    def tool_ms(self) -> int:
        return max(0, self.wall_ms - self.model_ms)


CONVERSATION_KEYS = ("messages", "tools")


def conversation_of(response: dict[str, Any]) -> dict[str, Any] | None:
    """The fine-tuning record a Gary response carries, or None when the bot did not send one."""
    messages = response.get("messages")
    tools = response.get("tools")
    if isinstance(messages, list) and isinstance(tools, list):
        return {"messages": messages, "tools": tools}
    return None


def parse_claude_result(raw: str) -> dict[str, Any]:
    """Pull reply, turns and token usage out of `claude -p --output-format json` output."""
    parsed = json.loads(raw)
    if isinstance(parsed, list):
        parsed = next((item for item in parsed if isinstance(item, dict) and item.get("type") == "result"), {})
    if not isinstance(parsed, dict):
        raise ValueError("claude result is not a JSON object")

    def pick(entry: dict[str, Any], *keys: str) -> int:
        return next((int(entry[key]) for key in keys if isinstance(entry.get(key), (int, float))), 0)

    fresh = cached = created = output = 0
    sources = list((parsed.get("modelUsage") or {}).values())
    if not sources and isinstance(parsed.get("usage"), dict):
        sources = [parsed["usage"]]
    for entry in sources:
        fresh += pick(entry, "inputTokens", "input_tokens")
        cached += pick(entry, "cacheReadInputTokens", "cache_read_input_tokens")
        created += pick(entry, "cacheCreationInputTokens", "cache_creation_input_tokens")
        output += pick(entry, "outputTokens", "output_tokens")
    subtype = str(parsed.get("subtype") or "")
    return {
        "reply": str(parsed.get("result") or ""),
        "is_error": bool(parsed.get("is_error")) or subtype.startswith("error"),
        "subtype": subtype,
        "turns": int(parsed.get("num_turns") or 0),
        "prompt_tokens": fresh + cached + created,
        "cached_tokens": cached,
        "completion_tokens": output,
        "model_ms": int(parsed.get("duration_api_ms") or 0),
    }


def parse_claude_stream(raw: str) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    """Split `--output-format stream-json` stdout into its events and the parsed final result."""
    events: list[dict[str, Any]] = []
    for line in raw.splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(event, dict):
            events.append(event)
    final = next((e for e in reversed(events) if e.get("type") == "result"), None)
    if final is None:
        raise ValueError("claude stream has no result event")
    return events, parse_claude_result(json.dumps(final))


ESCALATE_TOOL_SUFFIX = "escalate"


def stream_escalated(events: list[dict[str, Any]]) -> bool:
    """True when the stream contains a tool_use of Gary's `escalate` tool (MCP names end in it)."""
    for event in events:
        message = event.get("message")
        content = message.get("content") if isinstance(message, dict) else None
        if not isinstance(content, list):
            continue
        for block in content:
            if (
                isinstance(block, dict)
                and block.get("type") == "tool_use"
                and str(block.get("name") or "").endswith(ESCALATE_TOOL_SUFFIX)
            ):
                return True
    return False


class RealExecutor:
    """Hands the customer message to a system on the live bot."""

    def __init__(
        self,
        conn: OperatorConn,
        channels: list[str],
        env_file: dict[str, str],
        allow_missing_extras: bool = False,
    ) -> None:
        self.conn = conn
        self.channels = channels
        self.env_file = env_file
        self.allow_missing_extras = allow_missing_extras
        self._locks = {channel: threading.Lock() for channel in channels}
        self._turn = 0
        self._guard = threading.Lock()
        self._bare: dict[str, bool] = {}

    def _next_channel(self) -> str:
        with self._guard:
            channel = self.channels[self._turn % len(self.channels)]
            self._turn += 1
        return channel

    def execute(self, system: dict[str, Any], case: dict[str, Any], name: str) -> SystemRun:
        text = f"{context_line(name, case['game'])}\n\n{case['prompt']}"
        channel = self._next_channel()
        started = time.monotonic()
        if system["harness"] == "gary":
            run = self._gary(system, text, channel)
        else:
            run = self._claude_code(system, text, name, channel)
        run.wall_ms = int((time.monotonic() - started) * 1000)
        return run

    def _gary(self, system: dict[str, Any], text: str, channel: str) -> SystemRun:
        conn = copy.copy(self.conn)
        conn.channel = channel
        conn.timeout = WALL_CLOCK_S
        extra: dict[str, Any] = {"memory": False}
        if system.get("model_profile"):
            extra["model_profile"] = system["model_profile"]
        run = SystemRun()
        with self._locks[channel]:
            try:
                try:
                    response = conn.gary(text, "admin", new_session=True, confirm=True, extra=extra)
                except OperatorError as err:
                    if err.status not in UNKNOWN_FIELD_STATUSES or not self.allow_missing_extras:
                        raise
                    run.notes["extras_dropped"] = str(err)[:200]
                    response = conn.gary(text, "admin", new_session=True, confirm=True)
            except OperatorError as err:
                run.infra_error = f"operator gary failed: {err}"
                return run
            except OperatorConnError as err:
                if "timed out" in str(err):
                    run.notes["budget_exceeded"] = "wall clock"
                else:
                    run.infra_error = f"operator gary unreachable: {err}"
                return run
        usage = response.get("usage") or {}
        run.reply = str(response.get("reply") or "")
        run.claimed_done = not response.get("escalated")
        if response.get("escalated"):
            run.notes["escalated"] = True
        run.turns = int(response.get("rounds") or 0)
        run.tool_calls = [call["name"] for call in response.get("tool_calls") or []]
        run.prompt_tokens = int(usage.get("prompt_tokens") or 0)
        run.cached_tokens = int(usage.get("cached_tokens") or 0)
        run.completion_tokens = int(usage.get("completion_tokens") or 0)
        run.model_ms = int(response.get("latency_ms") or 0)
        run.notes["escalation_kind"] = response.get("escalation_kind")
        run.transcript = {key: value for key, value in response.items() if key not in CONVERSATION_KEYS}
        run.conversation = conversation_of(response)
        if run.conversation is None:
            run.notes["transcript"] = "unavailable"
        return run

    def _claude_code(self, system: dict[str, Any], text: str, name: str, channel: str) -> SystemRun:
        try:
            env_extra = resolve_env(system.get("env") or {}, self.env_file)
        except ValueError as err:
            return SystemRun(infra_error=str(err))
        with tempfile.TemporaryDirectory(prefix="eval-cc-") as scratch:
            root = Path(scratch)
            workdir, config_dir = root / "work", root / "config"
            workdir.mkdir()
            config_dir.mkdir()
            tool_log = root / "tools.jsonl"
            mcp_config = root / "mcp.json"
            mcp_config.touch(mode=0o600)
            mcp_config.write_text(
                json.dumps(
                    {
                        "mcpServers": {
                            "gary": {
                                "command": sys.executable,
                                "args": [str(GARY_MCP_PATH)],
                                "env": {
                                    "GARY_OPERATOR_URL": self.conn.url,
                                    "GARY_OPERATOR_TOKEN": self.conn.token,
                                    "GARY_CHANNEL": channel,
                                    "GARY_ACCESS": "admin",
                                    "GARY_SERVER": name,
                                    "GARY_TOOL_LOG": str(tool_log),
                                    "GARY_EXCLUDE_TOOLS": CC_EXCLUDED_TOOLS,
                                },
                            }
                        }
                    }
                ),
                encoding="utf-8",
            )
            env = {
                key: value
                for key, value in os.environ.items()
                if not key.startswith(("ANTHROPIC_", "CLAUDE_"))
            }
            env.update(
                CLAUDE_CONFIG_DIR=str(config_dir),
                CLAUDE_CODE_DISABLE_CLAUDE_MDS="1",
                CLAUDE_CODE_DISABLE_AUTO_MEMORY="1",
            )
            env.update(env_extra)
            use_bare = self._bare.get(system["system_id"], True)
            run = self._claude_once(system, text, workdir, mcp_config, env, tool_log, use_bare)
            if use_bare and run.notes.get("auth_rejected"):
                self._bare[system["system_id"]] = False
                tool_log.unlink(missing_ok=True)
                run = self._claude_once(system, text, workdir, mcp_config, env, tool_log, False)
            run.notes["mode"] = "bare" if self._bare.get(system["system_id"], True) else "no_bare"
            return run

    def _claude_once(
        self,
        system: dict[str, Any],
        text: str,
        workdir: Path,
        mcp_config: Path,
        env: dict[str, str],
        tool_log: Path,
        bare: bool,
    ) -> SystemRun:
        command = ["claude", "-p", text]
        if bare:
            command.append("--bare")
        command += [
            "--output-format", "stream-json", "--verbose",
            "--model", system["model"],
            "--strict-mcp-config", "--mcp-config", str(mcp_config),
            "--tools", "",
            "--max-turns", str(MAX_MODEL_CALLS),
            "--permission-mode", "bypassPermissions",
        ]
        run = SystemRun()
        try:
            done = subprocess.run(
                command, cwd=workdir, env=env, capture_output=True, text=True, timeout=WALL_CLOCK_S
            )
        except subprocess.TimeoutExpired:
            run.notes["budget_exceeded"] = "wall clock"
            run.tool_calls = _read_tool_log(tool_log)
            return run
        except OSError as err:
            run.infra_error = f"could not start claude: {err}"
            return run
        run.tool_calls = _read_tool_log(tool_log)
        try:
            events, parsed = parse_claude_stream(done.stdout)
        except ValueError as err:
            combined = (done.stderr + done.stdout)[-400:]
            run.notes["auth_rejected"] = bool(AUTH_FAILURE.search(combined))
            run.infra_error = f"claude exit {done.returncode} with unreadable output ({err}): {combined}"
            return run
        run.reply = parsed["reply"]
        run.turns = parsed["turns"]
        run.prompt_tokens = parsed["prompt_tokens"]
        run.cached_tokens = parsed["cached_tokens"]
        run.completion_tokens = parsed["completion_tokens"]
        run.model_ms = parsed["model_ms"]
        escalated = stream_escalated(events)
        run.claimed_done = done.returncode == 0 and not parsed["is_error"] and not escalated
        if escalated:
            run.notes["escalated"] = True
        run.notes["subtype"] = parsed["subtype"]
        if parsed["is_error"] and parsed["prompt_tokens"] == 0 and AUTH_FAILURE.search(run.reply):
            run.notes["auth_rejected"] = True
            run.infra_error = f"claude rejected the credentials: {run.reply[:200]}"
        run.transcript = {"events": events, "stderr": done.stderr[-2000:]}
        return run


def _read_tool_log(path: Path) -> list[str]:
    if not path.exists():
        return []
    names = []
    for line in path.read_text(encoding="utf-8").splitlines():
        try:
            names.append(str(json.loads(line)["name"]))
        except (ValueError, KeyError):
            continue
    return names


# --------------------------------------------------------------------------
# one job


def run_job(
    cluster: Any,
    executor: Any,
    system: dict[str, Any],
    case: dict[str, Any],
    repeat: int,
) -> tuple[dict[str, Any], dict[str, Any]]:
    """Run one (system, case, repeat). Returns the run record and its transcript."""
    name = server_name(case["id"], system["system_id"], repeat)
    game = case["game"]
    record: dict[str, Any] = {
        "system_id": system["system_id"],
        "label": system["label"],
        "model": system["model"],
        "harness": system["harness"],
        "case_id": case["id"],
        "game": game,
        "task_area": case.get("task_area", "config"),
        "category": case.get("category") or case.get("kind", ""),
        "prompt": case["prompt"],
        "split": case.get("split", "dev"),
        "repeat": repeat,
        "outcome": "infra_error",
        "claimed_done": False,
        "turns": 0,
        "tool_calls": [],
        "usage": {"prompt_tokens": 0, "cached_tokens": 0, "completion_tokens": 0},
        "latency_ms": 0,
        "model_ms": 0,
        "tool_ms": 0,
        "checks": [],
        "server": name,
        "destroyed": False,
        "detail": "",
        "started_at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
    }
    transcript: dict[str, Any] = {"server": name, "setup": [], "system": None}
    create_attempted = False
    try:
        create_attempted = True
        created = cluster.tool("create_server", {"game": game, "name": name})
        if not created.lower().startswith("created"):
            record["detail"] = f"create_server refused: {created[:300]}"
            return record, transcript
        if not wait_ready(cluster, name):
            record.update(outcome="staging_error", detail="server never became Ready after create")
            return record, transcript
        transcript["setup"] = apply_setup(cluster, case, name)
        run = executor.execute(system, case, name)
        transcript["system"] = run.transcript
        transcript["conversation"] = run.conversation
        _copy_run(record, run)
        if run.infra_error:
            record["detail"] = run.infra_error
            return record, transcript
        record["outcome"] = _grade(cluster, case, name, run.reply, record)
    except StagingError as err:
        record.update(outcome="staging_error", detail=str(err))
    except ClusterError as err:
        record.update(outcome="infra_error", detail=str(err))
    finally:
        if create_attempted:
            record["destroyed"], destroy_detail = _destroy(cluster, name)
            if destroy_detail:
                record["detail"] = (record["detail"] + "; " if record["detail"] else "") + destroy_detail
        record["finished_at"] = datetime.now(timezone.utc).isoformat(timespec="seconds")
    return record, transcript


def _copy_run(record: dict[str, Any], run: SystemRun) -> None:
    record.update(
        claimed_done=run.claimed_done,
        turns=run.turns,
        tool_calls=run.tool_calls,
        usage={
            "prompt_tokens": run.prompt_tokens,
            "cached_tokens": min(run.cached_tokens, run.prompt_tokens),
            "completion_tokens": run.completion_tokens,
        },
        latency_ms=run.wall_ms,
        model_ms=run.model_ms,
        tool_ms=run.tool_ms,
    )
    if run.notes:
        record["notes"] = run.notes
        if run.notes.get("budget_exceeded"):
            record["detail"] = f"budget exceeded: {run.notes['budget_exceeded']}"


def _grade(cluster: Any, case: dict[str, Any], name: str, reply: str, record: dict[str, Any]) -> str:
    wait_ready(cluster, name)
    first = run_checks(cluster, case, name, reply, "after stop")
    record["checks"] = first
    if not all(item["ok"] for item in first):
        return "fail"
    cluster.tool("restart_server", {"name": name})
    wait_ready(cluster, name)
    second = run_checks(cluster, case, name, reply, "after restart")
    record["checks"] = first + second
    return "pass" if all(item["ok"] for item in second) else "fail"


def _destroy(cluster: Any, name: str) -> tuple[bool, str]:
    try:
        result = cluster.tool("destroy_server", {"name": name}, confirm=True)
    except ClusterError as err:
        return False, f"destroy failed: {err}"
    cluster.forget(name)
    ok = result.lower().startswith("deleted")
    return ok, "" if ok else f"destroy unclear: {result[:200]}"


# --------------------------------------------------------------------------
# orchestration


def run_all(
    cluster: Any,
    executor: Any,
    systems: list[dict[str, Any]],
    cases: list[dict[str, Any]],
    repeats: int,
    concurrency: int,
    out_dir: Path,
    game_caps: dict[str, int],
    force: bool = False,
    known: frozenset[str] = frozenset(),
    rerun_outcomes: frozenset[str] | None = None,
    aborted: threading.Event | None = None,
    manifest: dict[str, Any] | None = None,
) -> list[dict[str, Any]]:
    """Run every job that is not already recorded; `aborted` is set if the operator stays unreachable."""
    aborted = aborted if aborted is not None else threading.Event()
    out_dir.mkdir(parents=True, exist_ok=True)
    transcript_dir = out_dir / "transcripts"
    transcript_dir.mkdir(exist_ok=True)
    if manifest is not None:
        (out_dir / "manifest.json").write_text(dumps_scrubbed(manifest, known, indent=2, default=str), encoding="utf-8")
    semaphores: dict[str, threading.Semaphore] = {}
    for game in {case["game"] for case in cases}:
        cap = game_caps.get(game, DEFAULT_SLOW_GAME_CAP if game in SLOW_GAMES else concurrency)
        semaphores[game] = threading.Semaphore(max(1, cap))
    print_lock = threading.Lock()

    def one(job: tuple[dict[str, Any], dict[str, Any], int]) -> dict[str, Any] | None:
        system, case, repeat = job
        stem = f"{system['system_id']}__{case['id']}__r{repeat}"
        target = out_dir / f"{stem}.json"
        if aborted.is_set() or not should_run(target, force, rerun_outcomes):
            return None
        with semaphores[case["game"]]:
            if aborted.is_set():
                return None
            record, transcript = run_job(cluster, executor, system, case, repeat)
        if getattr(cluster, "failure_streak", 0) >= ABORT_AFTER_FAILURES:
            aborted.set()
        # The conversation is written beside the transcript rather than inside it: it is the
        # bulk of a Gary turn and the only part the fine-tuning export reads.
        conversation = transcript.pop("conversation", None)
        system_part = transcript.get("system")
        # A Claude Code run is saved as its bare event array: every assistant message,
        # tool_use and tool_result, in order.
        saved: Any = system_part["events"] if isinstance(system_part, dict) and "events" in system_part else transcript
        (transcript_dir / f"{stem}.json").write_text(
            dumps_scrubbed(saved, known, indent=2, default=str), encoding="utf-8"
        )
        if conversation is not None:
            (transcript_dir / f"{stem}.messages.json").write_text(
                dumps_scrubbed(conversation, known, indent=2, default=str), encoding="utf-8"
            )
        temp = target.with_suffix(".tmp")
        temp.write_text(dumps_scrubbed(record, known, indent=2), encoding="utf-8")
        temp.replace(target)
        with print_lock:
            print(f"{stem}: {record['outcome']} {scrub_text(record['detail'][:120], known)}", flush=True)
        return record

    jobs = [(s, c, r) for s in systems for c in cases for r in range(1, repeats + 1)]
    with ThreadPoolExecutor(max_workers=max(1, concurrency)) as pool:
        records = [record for record in pool.map(one, jobs) if record is not None]
    if aborted.is_set():
        _sweep_undestroyed(cluster, records)
    return records


def _sweep_undestroyed(cluster: Any, records: list[dict[str, Any]]) -> None:
    """After an abort, retry the destroy of every server this process created and could not remove."""
    for record in records:
        if record["destroyed"]:
            continue
        record["destroyed"], _ = _destroy(cluster, record["server"])
        print(f"abort sweep: {record['server']} destroyed={record['destroyed']}", flush=True)


def should_run(target: Path, force: bool, rerun_outcomes: frozenset[str] | None) -> bool:
    """Whether the job whose record would be `target` still needs running."""
    if force:
        return True
    if not target.exists():
        return rerun_outcomes is None or "missing" in rerun_outcomes
    if rerun_outcomes is None:
        return False
    try:
        outcome = json.loads(target.read_text(encoding="utf-8")).get("outcome")
    except (ValueError, OSError, AttributeError):
        return "missing" in rerun_outcomes
    return outcome in rerun_outcomes


def parse_rerun_outcomes(text: str) -> frozenset[str]:
    wanted = frozenset(part.strip() for part in text.split(",") if part.strip())
    unknown = sorted(wanted - set(RERUNNABLE_OUTCOMES))
    if unknown or not wanted:
        raise ValueError(f"--rerun-outcomes wants some of {','.join(RERUNNABLE_OUTCOMES)}, got {text!r}")
    return wanted


def _git_output(args: list[str]) -> str | None:
    try:
        done = subprocess.run(["git", *args], cwd=REPO_ROOT, capture_output=True, text=True, timeout=30)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return done.stdout.strip() if done.returncode == 0 else None


def live_bot_image() -> str:
    try:
        done = subprocess.run(
            ["kubectl", "-n", "game-servers", "get", "deploy", "-o",
             "jsonpath={.items[*].spec.template.spec.containers[*].image}"],
            capture_output=True, text=True, timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired) as err:
        return f"unavailable: {err}"
    return done.stdout.strip() if done.returncode == 0 else f"unavailable: {done.stderr.strip()[:200]}"


def build_manifest(argv: list[str], dry_run: bool) -> dict[str, Any]:
    """Describe the run: configs (secrets stay as $VAR names), code revision, live bot image, args."""
    status = _git_output(["status", "--porcelain"])
    return {
        "started_at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "args": argv,
        "git_head": _git_output(["rev-parse", "HEAD"]),
        "git_dirty": None if status is None else bool(status),
        "bot_image": "skipped in --dry-run" if dry_run else live_bot_image(),
        "systems": json.loads(SYSTEMS_PATH.read_text(encoding="utf-8")),
        "models": json.loads((HARNESS_DIR / "models.json").read_text(encoding="utf-8")),
    }


def parse_game_caps(items: list[str]) -> dict[str, int]:
    caps = {}
    for item in items:
        game, sep, count = item.partition("=")
        if not sep or not count.isdigit():
            raise ValueError(f"--game-cap wants game=N, got {item!r}")
        caps[game] = int(count)
    return caps


# --------------------------------------------------------------------------
# dry run: an in-memory cluster and a do-nothing system


class FakeCluster:
    """A virtual-clock cluster for tests and --dry-run; no kubectl, no operator."""

    def __init__(self) -> None:
        self.clock = 0.0
        self.states: dict[str, str] = {}
        self.files: dict[str, dict[str, str]] = {}
        self.logs: dict[str, str] = {}
        self.destroyed: list[str] = []
        self.restarts: list[str] = []
        self.ready_timeout_s = DEFAULT_READY_S
        self.failure_streak = 0
        self.on_restart: Callable[[str, "FakeCluster"], None] | None = None
        self.tool_replies: dict[str, str] = {}
        self._lock = threading.Lock()

    def now(self) -> float:
        return self.clock

    def sleep(self, seconds: float) -> None:
        self.clock += seconds

    def tool(self, tool: str, arguments: dict[str, Any], confirm: bool = False) -> str:
        name = arguments.get("name", "")
        with self._lock:
            if tool == "create_server":
                self.states[name] = "Ready"
                self.files[name] = {}
                return f"created {name}; it'll be reachable at 127.0.0.1:7000 once it finishes booting"
            if tool in ("write_file", "edit_file"):
                self.files[name][arguments["path"]] = arguments.get("content", arguments.get("new_text", ""))
                return "written"
            if tool == "read_logs":
                return self.logs.get(name, "")
            if tool == "restart_server":
                self.restarts.append(name)
                if self.on_restart:
                    self.on_restart(name, self)
                return f"{name} is back up"
            if tool == "destroy_server":
                self.destroyed.append(name)
                self.states.pop(name, None)
                return f"deleted {name} and its world"
            if tool in self.tool_replies:
                return self.tool_replies[tool]
        raise ClusterError(f"fake cluster has no tool {tool}")

    def state(self, name: str) -> str:
        return self.states.get(name, "absent")

    def exec(self, name: str, argv: list[str], timeout: float = 120.0) -> tuple[int, str]:
        if argv[:1] == ["cat"]:
            path = argv[1]
            data = self.files.get(name, {})
            key = path if path in data else path.removeprefix("/data/")
            return (0, data[key]) if key in data else (1, f"cat: {path}: No such file or directory")
        return 0, ""

    def put(self, name: str, src: Path, dst: str) -> None:
        self.files[name][dst] = ""

    def forget(self, name: str) -> None:
        return None


class FakeExecutor:
    """A system that runs `effect(cluster, server)` and claims it is done."""

    def __init__(self, cluster: FakeCluster, effect: Callable[[FakeCluster, str], None] | None = None) -> None:
        self.cluster = cluster
        self.effect = effect

    def execute(self, system: dict[str, Any], case: dict[str, Any], name: str) -> SystemRun:
        if self.effect:
            self.effect(self.cluster, name)
        return SystemRun(reply="done", claimed_done=True, turns=1, tool_calls=["edit_file"], prompt_tokens=10,
                         cached_tokens=2, completion_tokens=5, wall_ms=3, model_ms=1)


# --------------------------------------------------------------------------
# CLI


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--cases", required=True, type=Path)
    parser.add_argument("--systems", required=True, help="comma-separated system ids from systems.json")
    parser.add_argument("--case", action="append", default=[], help="only this case id (repeatable)")
    parser.add_argument("--repeats", type=int, default=1)
    parser.add_argument("--concurrency", type=int, default=4)
    parser.add_argument("--out", type=Path)
    parser.add_argument("--game-cap", action="append", default=[], help="game=N concurrent servers")
    parser.add_argument("--channels", help=f"comma list of Discord channel ids (default: {CHANNELS_KEY} or the operator channel)")
    parser.add_argument("--allow-missing-extras", action="store_true",
                        help="retry a Gary turn without model_profile/memory when the bot rejects them (smoke only)")
    parser.add_argument("--force", action="store_true", help="rerun jobs whose record already exists")
    parser.add_argument("--ready-timeout", type=float, default=DEFAULT_READY_S,
                        help="seconds to wait for a created server to become Ready (default: %(default)s)")
    parser.add_argument("--rerun-outcomes", type=parse_rerun_outcomes,
                        help="with an existing --out, rerun only records with these outcomes "
                             f"({','.join(RERUNNABLE_OUTCOMES)}); everything else is skipped")
    parser.add_argument("--dry-run", action="store_true", help="fake cluster and systems; no cluster or model calls")
    raw_args = list(sys.argv[1:] if argv is None else argv)
    args = parser.parse_args(argv)

    try:
        cases = load_cases(args.cases, args.case)
        systems = load_systems(SYSTEMS_PATH, [s for s in args.systems.split(",") if s])
        caps = parse_game_caps(args.game_cap)
    except (ValueError, OSError, yaml.YAMLError) as err:
        print(f"run: {err}", file=sys.stderr)
        return 2
    run_id = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    out_dir = args.out or REPO_ROOT / "evals" / "results" / "runs" / run_id

    try:
        known = known_ids()
    except OperatorConnError as err:
        print(f"run: {err}", file=sys.stderr)
        return 2

    if args.dry_run:
        cluster: Any = FakeCluster()
        executor: Any = FakeExecutor(cluster)
        records = run_all(cluster, executor, systems, cases, args.repeats, args.concurrency, out_dir, caps, args.force, known, args.rerun_outcomes,
                           manifest=build_manifest(raw_args, True))
        print(f"dry run: {len(records)} records in {out_dir}")
        return 0

    env_file = read_env_file(find_env_file())
    try:
        for system in systems:
            if system["harness"] == "claude_code":
                resolve_env(system.get("env") or {}, env_file)
        channels = eval_channels(args.channels, os.environ, env_file)
        conn = OperatorConn.connect(channel=channels[0] if channels else None, timeout=WALL_CLOCK_S)
    except (ValueError, OperatorConnError) as err:
        print(f"run: {err}", file=sys.stderr)
        return 2
    with conn:
        executor = RealExecutor(conn, channels or [conn.channel], env_file, args.allow_missing_extras)
        aborted = threading.Event()
        cluster = Cluster(conn, ready_timeout_s=args.ready_timeout)
        records = run_all(
            cluster, executor, systems, cases, args.repeats, args.concurrency, out_dir, caps,
            args.force, known, args.rerun_outcomes, aborted, build_manifest(raw_args, False),
        )
    tally: dict[str, int] = {}
    for record in records:
        tally[record["outcome"]] = tally.get(record["outcome"], 0) + 1
    if aborted.is_set():
        print(
            f"ABORTED: the operator stayed unreachable after {ABORT_AFTER_FAILURES} consecutive failures; "
            f"{len(records)} records written to {out_dir} {tally}. Rerun with --out {out_dir} to finish.",
            file=sys.stderr,
        )
        return 1
    print(f"done: {len(records)} records in {out_dir} {tally}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
