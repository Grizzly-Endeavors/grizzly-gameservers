"""Tests for evals/harness/run.py: the dry-run flow on a fake cluster and the Claude Code usage parser."""

from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import threading
import unittest
from pathlib import Path
from types import ModuleType

RUN_PATH = Path(__file__).resolve().parents[1] / "run.py"


def load_run_module() -> ModuleType:
    spec = importlib.util.spec_from_file_location("run", RUN_PATH)
    if spec is None or spec.loader is None:
        raise ImportError(f"cannot load {RUN_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


run = load_run_module()

SYSTEM = {"system_id": "fake", "label": "Fake", "harness": "gary", "model": "m"}
PVP_CASE = {
    "id": "mc-pvp-d1",
    "scenario": "mc-pvp",
    "split": "dev",
    "game": "minecraft",
    "kind": "config",
    "task_area": "config",
    "prompt": "turn off pvp",
    "setup": [{"tool": "write_file", "args": {"path": "server.properties", "content": "pvp=true\n"}}],
    "checks": [
        {"type": "ready"},
        {"type": "file_regex", "path": "/data/server.properties", "pattern": "(?m)^pvp=false$"},
    ],
}

CLAUDE_FIXTURE = {
    "type": "result",
    "subtype": "success",
    "is_error": False,
    "result": "all fixed",
    "num_turns": 7,
    "duration_api_ms": 4200,
    "modelUsage": {
        "claude-opus-5-5": {
            "inputTokens": 100,
            "outputTokens": 50,
            "cacheReadInputTokens": 900,
            "cacheCreationInputTokens": 400,
            "costUSD": 0.12,
        }
    },
}


GARY_FIXTURE_MESSAGES = [{"role": "system", "content": "you are Gary"}, {"role": "user", "content": "hi"}]
GARY_FIXTURE_TOOLS = [{"type": "function", "function": {"name": "edit_file"}}]


def fixes_pvp(cluster, name):
    cluster.files[name]["server.properties"] = "pvp=false\n"


class RunFlowTest(unittest.TestCase):
    def run_one(self, case, effect=None, on_restart=None):
        cluster = run.FakeCluster()
        cluster.on_restart = on_restart
        executor = run.FakeExecutor(cluster, effect)
        record, transcript = run.run_job(cluster, executor, SYSTEM, case, 1)
        return cluster, record, transcript

    def test_pass_records_every_scorer_field_and_destroys_server(self):
        cluster, record, _ = self.run_one(PVP_CASE, fixes_pvp)
        self.assertEqual(record["outcome"], "pass")
        self.assertEqual([c["ok"] for c in record["checks"]], [True] * 4)
        self.assertEqual(cluster.restarts, [record["server"]])
        self.assertEqual(cluster.destroyed, [record["server"]])
        self.assertTrue(record["destroyed"])
        self.assertEqual(record["category"], "config")
        self.assertEqual(record["usage"], {"prompt_tokens": 10, "cached_tokens": 2, "completion_tokens": 5})
        self.assertTrue(record["claimed_done"])
        spec = importlib.util.spec_from_file_location("score", RUN_PATH.with_name("score.py"))
        score = importlib.util.module_from_spec(spec)
        sys.modules["score"] = score
        spec.loader.exec_module(score)
        score.validate_record(record, Path("record.json"))

    def test_untouched_server_fails_first_pass_without_restart(self):
        cluster, record, _ = self.run_one(PVP_CASE)
        self.assertEqual(record["outcome"], "fail")
        self.assertEqual(cluster.restarts, [])
        self.assertEqual(cluster.destroyed, [record["server"]])

    def test_change_lost_on_restart_fails_second_pass(self):
        def revert(name, cluster):
            cluster.files[name]["server.properties"] = "pvp=true\n"

        cluster, record, _ = self.run_one(PVP_CASE, fixes_pvp, on_restart=revert)
        self.assertEqual(record["outcome"], "fail")
        self.assertTrue(all(c["ok"] for c in record["checks"] if c["name"].startswith("after stop")))
        self.assertFalse(all(c["ok"] for c in record["checks"] if c["name"].startswith("after restart")))
        self.assertEqual(cluster.destroyed, [record["server"]])

    def test_failed_wait_is_a_staging_error_and_skips_the_system(self):
        case = dict(PVP_CASE, setup=[{"wait": "log", "pattern": "never appears", "timeout_s": 30}])
        ran = []
        cluster, record, _ = self.run_one(case, lambda c, n: ran.append(n))
        self.assertEqual(record["outcome"], "staging_error")
        self.assertEqual(ran, [])
        self.assertEqual(cluster.destroyed, [record["server"]])

    def test_wait_log_matches_when_the_pattern_is_present(self):
        case = dict(PVP_CASE, setup=[{"wait": "log", "pattern": "Mod .* requires", "timeout_s": 30}])
        cluster = run.FakeCluster()
        original = cluster.tool

        def tool(name, arguments, confirm=False):
            result = original(name, arguments, confirm)
            if name == "create_server":
                cluster.logs[arguments["name"]] = "Mod fabric requires lithium"
            return result

        cluster.tool = tool
        record, _ = run.run_job(cluster, run.FakeExecutor(cluster, fixes_pvp), SYSTEM, case, 1)
        self.assertEqual(record["outcome"], "pass")

    def test_infra_failure_in_the_system_is_recorded_and_server_destroyed(self):
        class Broken:
            def execute(self, system, case, name):
                return run.SystemRun(infra_error="operator gary failed: HTTP 503")

        cluster = run.FakeCluster()
        record, _ = run.run_job(cluster, Broken(), SYSTEM, PVP_CASE, 1)
        self.assertEqual(record["outcome"], "infra_error")
        self.assertIn("503", record["detail"])
        self.assertEqual(cluster.destroyed, [record["server"]])

    def test_run_all_writes_records_and_transcripts_apart(self):
        cluster = run.FakeCluster()
        executor = run.FakeExecutor(cluster, fixes_pvp)
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            records = run.run_all(cluster, executor, [SYSTEM], [PVP_CASE], 2, 2, out, {})
            self.assertEqual(len(records), 2)
            self.assertEqual(len(list(out.glob("*.json"))), 2)
            self.assertEqual(len(list((out / "transcripts").glob("*.json"))), 2)
            again = run.run_all(cluster, executor, [SYSTEM], [PVP_CASE], 2, 2, out, {})
            self.assertEqual(again, [])


class HelpersTest(unittest.TestCase):
    def test_server_name_is_a_short_label_and_stays_distinct(self):
        short = run.server_name("mc-pvp-d1", "gary-fw", 1)
        self.assertEqual(short, "ev-mc-pvp-d1-gary-fw-1")
        long_a = run.server_name("a-very-long-scenario-name-for-testing-d1", "gary-fw", 1)
        long_b = run.server_name("a-very-long-scenario-name-for-testing-d1", "gary-fw", 2)
        self.assertLessEqual(len(long_a), 40)
        self.assertNotEqual(long_a, long_b)
        self.assertRegex(long_a, r"^[a-z0-9]([a-z0-9-]*[a-z0-9])?$")

    def test_resolve_env_reads_dollar_values_and_reports_missing(self):
        resolved = run.resolve_env({"A": "$KEY", "B": "plain"}, {"KEY": "secret"})
        self.assertEqual(resolved, {"A": "secret", "B": "plain"})
        with self.assertRaises(ValueError) as caught:
            run.resolve_env({"A": "$NOPE_NOT_SET"}, {})
        self.assertNotIn("secret", str(caught.exception))

    def test_ready_timeout_is_one_configurable_value(self):
        cluster = run.FakeCluster()
        self.assertEqual(run.ready_timeout(cluster), 900)
        cluster.ready_timeout_s = 123.0
        self.assertEqual(run.ready_timeout(cluster), 123.0)


class FakeOperatorConn:
    """Stands in for OperatorConn: returns a canned Gary response."""

    channel = None
    timeout = 0.0

    def __init__(self, response):
        self.response = response

    def gary(self, text, access, **kwargs):
        return self.response


GARY_RESPONSE = {
    "reply": "done",
    "escalated": False,
    "rounds": 1,
    "tool_calls": [{"name": "edit_file"}],
    "usage": {"prompt_tokens": 10, "cached_tokens": 2, "completion_tokens": 5},
    "latency_ms": 7,
    "escalation_kind": None,
    "messages": GARY_FIXTURE_MESSAGES,
    "tools": GARY_FIXTURE_TOOLS,
}


class GaryConversationTest(unittest.TestCase):
    def gary_run(self, response):
        executor = run.RealExecutor(FakeOperatorConn(response), ["c1"], {})
        return executor._gary(SYSTEM, "turn off pvp", "c1")

    def test_conversation_is_split_out_of_the_transcript(self):
        run_result = self.gary_run(GARY_RESPONSE)
        self.assertEqual(run_result.conversation, {"messages": GARY_FIXTURE_MESSAGES, "tools": GARY_FIXTURE_TOOLS})
        self.assertNotIn("messages", run_result.transcript)
        self.assertNotIn("tools", run_result.transcript)
        self.assertEqual(run_result.transcript["reply"], "done")
        self.assertNotIn("transcript", run_result.notes)

    def test_response_without_messages_is_noted_unavailable(self):
        response = {key: value for key, value in GARY_RESPONSE.items() if key not in ("messages", "tools")}
        run_result = self.gary_run(response)
        self.assertIsNone(run_result.conversation)
        self.assertEqual(run_result.notes["transcript"], "unavailable")

    def test_run_all_writes_the_conversation_beside_the_transcript(self):
        class Gary:
            def execute(self, system, case, name):
                return run.SystemRun(reply="done", claimed_done=True,
                                     conversation={"messages": [{"role": "user", "content": "hi"}], "tools": []})

        cluster = run.FakeCluster()
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            run.run_all(cluster, Gary(), [SYSTEM], [PVP_CASE], 1, 1, out, {})
            messages = json.loads((out / "transcripts" / "fake__mc-pvp-d1__r1.messages.json").read_text())
            self.assertEqual(messages["tools"], [])
            transcript = json.loads((out / "transcripts" / "fake__mc-pvp-d1__r1.json").read_text())
            self.assertNotIn("conversation", transcript)


class ClaudeResultTest(unittest.TestCase):
    def test_usage_sums_cache_creation_into_prompt_tokens(self):
        parsed = run.parse_claude_result(json.dumps(CLAUDE_FIXTURE))
        self.assertEqual(parsed["prompt_tokens"], 100 + 900 + 400)
        self.assertEqual(parsed["cached_tokens"], 900)
        self.assertEqual(parsed["completion_tokens"], 50)
        self.assertEqual(parsed["turns"], 7)
        self.assertEqual(parsed["model_ms"], 4200)
        self.assertFalse(parsed["is_error"])
        self.assertEqual(parsed["reply"], "all fixed")

    def test_max_turns_subtype_is_not_done(self):
        fixture = dict(CLAUDE_FIXTURE, subtype="error_max_turns")
        self.assertTrue(run.parse_claude_result(json.dumps(fixture))["is_error"])

    def test_result_inside_a_list_and_garbage(self):
        self.assertEqual(run.parse_claude_result(json.dumps([{"type": "system"}, CLAUDE_FIXTURE]))["turns"], 7)
        with self.assertRaises(ValueError):
            run.parse_claude_result("not json")


class ClaudeStreamTests(unittest.TestCase):
    def test_stream_yields_events_and_result(self):
        events = [
            {"type": "system", "subtype": "init"},
            {"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "read_file"}]}},
            {"type": "user", "message": {"content": [{"type": "tool_result", "content": "x"}]}},
            CLAUDE_FIXTURE,
        ]
        raw = "\n".join(json.dumps(e) for e in events) + "\nnot json\n"
        got, parsed = run.parse_claude_stream(raw)
        self.assertEqual(len(got), 4)
        self.assertEqual(parsed["turns"], 7)
        self.assertEqual(parsed["reply"], "all fixed")
        self.assertEqual(parsed["prompt_tokens"], 1400)

    def test_stream_without_result_is_an_error(self):
        with self.assertRaises(ValueError):
            run.parse_claude_stream(json.dumps({"type": "system"}))

    def test_escalate_tool_use_is_detected(self):
        events = [
            {"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "read_file"}]}},
            {"type": "assistant", "message": {"content": [
                {"type": "tool_use", "name": "mcp__gary__escalate", "input": {"reason": "needs a human"}},
            ]}},
            CLAUDE_FIXTURE,
        ]
        raw = "\n".join(json.dumps(e) for e in events)
        got, _ = run.parse_claude_stream(raw)
        self.assertTrue(run.stream_escalated(got))

    def test_stream_without_escalate_is_not_escalated(self):
        events = [
            {"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "mcp__gary__read_file"}]}},
            {"type": "assistant", "message": {"content": [{"type": "text", "text": "escalate"}]}},
            {"type": "user", "message": {"content": [{"type": "tool_result", "content": "escalate"}]}},
            CLAUDE_FIXTURE,
        ]
        self.assertFalse(run.stream_escalated(events))


class ManifestTests(unittest.TestCase):
    def test_dry_run_writes_manifest_without_secrets(self):
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "o"
            cluster = run.FakeCluster()
            manifest = run.build_manifest(["--dry-run"], True)
            run.run_all(cluster, run.FakeExecutor(cluster), [], [], 1, 1, out, {}, manifest=manifest)
            written = json.loads((out / "manifest.json").read_text())
        self.assertEqual(written["args"], ["--dry-run"])
        self.assertIn("skipped", written["bot_image"])
        self.assertIsInstance(written["git_dirty"], bool)
        self.assertEqual(len(written["git_head"]), 40)
        self.assertIn("$ANTHROPIC_API_KEY", json.dumps(written["systems"]))
        self.assertTrue(written["models"])


ANY_OF_CASE = dict(
    PVP_CASE,
    checks=[
        {"type": "ready"},
        {
            "type": "any_of",
            "checks": [
                {"type": "file_regex", "path": "/data/server.properties", "pattern": "(?m)^pvp=false$"},
                {"type": "tool_regex", "tool": "send_command", "args": {"command": "gamerule pvp"},
                 "pattern": "(?i)false"},
            ],
        },
    ],
)


class AnyOfFlowTest(unittest.TestCase):
    def run_one(self, case, effect=None, replies=None):
        cluster = run.FakeCluster()
        cluster.tool_replies = dict(replies or {})
        record, _ = run.run_job(cluster, run.FakeExecutor(cluster, effect), SYSTEM, case, 1)
        return record

    def test_passes_when_only_the_gamerule_is_set(self):
        record = self.run_one(ANY_OF_CASE, replies={"send_command": "Gamerule pvp is currently set to: false"})
        self.assertEqual(record["outcome"], "pass")
        any_of = next(c for c in record["checks"] if c["name"].startswith("after stop: any_of"))
        self.assertTrue(any_of["ok"])
        self.assertIn("file_regex /data/server.properties failed", any_of["detail"])
        self.assertIn("tool_regex send_command passed", any_of["detail"])

    def test_passes_when_only_the_properties_file_is_set(self):
        record = self.run_one(ANY_OF_CASE, fixes_pvp, replies={"send_command": "Gamerule pvp is currently set to: true"})
        self.assertEqual(record["outcome"], "pass")

    def test_fails_when_no_nested_check_passes(self):
        record = self.run_one(ANY_OF_CASE, replies={"send_command": "Gamerule pvp is currently set to: true"})
        self.assertEqual(record["outcome"], "fail")
        any_of = next(c for c in record["checks"] if c["name"] == "after stop: any_of")
        self.assertFalse(any_of["ok"])
        self.assertIn("file_regex /data/server.properties failed", any_of["detail"])
        self.assertIn("tool_regex send_command failed", any_of["detail"])

    def test_unavailable_nested_check_counts_as_failed_and_others_still_decide(self):
        record = self.run_one(ANY_OF_CASE, fixes_pvp)
        self.assertEqual(record["outcome"], "pass")
        any_of = next(c for c in record["checks"] if c["name"] == "after stop: any_of")
        self.assertIn("cluster error", any_of["detail"])

    def test_empty_any_of_fails_with_a_reason(self):
        case = dict(PVP_CASE, checks=[{"type": "any_of", "checks": []}])
        record = self.run_one(case, fixes_pvp)
        self.assertEqual(record["outcome"], "fail")
        self.assertEqual(record["checks"][0]["detail"], "any_of has no checks")


class FakeToolConn:
    """Stands in for OperatorConn.tool: records each call and answers with a canned result."""

    def __init__(self, result=None, error=None):
        self.result = result
        self.error = error
        self.calls = []

    def tool(self, name, arguments, access="admin", confirm=False):
        self.calls.append((name, dict(arguments), access))
        if self.error is not None:
            raise self.error
        return {"result": self.result}


class ToolRegexTest(unittest.TestCase):
    CHECK = {"type": "tool_regex", "tool": "send_command", "args": {"command": "gamerule pvp"}, "pattern": "(?i)false"}

    def test_calls_the_tool_with_the_server_name_and_matches_the_result(self):
        conn = FakeToolConn(result="Gamerule pvp is currently set to: false")
        result = run.run_check(run.Cluster(conn), self.CHECK, "srv-1", "")
        self.assertTrue(result["ok"])
        self.assertEqual(result["name"], "tool_regex send_command")
        self.assertEqual(conn.calls, [("send_command", {"command": "gamerule pvp", "name": "srv-1"}, "admin")])
        self.assertEqual(self.CHECK["args"], {"command": "gamerule pvp"})

    def test_mismatch_fails_with_the_result_in_the_detail(self):
        conn = FakeToolConn(result="Gamerule pvp is currently set to: true")
        result = run.run_check(run.Cluster(conn), self.CHECK, "srv-1", "")
        self.assertFalse(result["ok"])
        self.assertIn("did not match", result["detail"])
        self.assertIn("set to: true", result["detail"])

    def test_present_false_passes_when_the_pattern_is_absent(self):
        conn = FakeToolConn(result="Gamerule pvp is currently set to: true")
        check = dict(self.CHECK, pattern="(?i)false", present=False)
        self.assertTrue(run.run_check(run.Cluster(conn), check, "srv-1", "")["ok"])

    def test_present_false_fails_when_the_pattern_matches(self):
        conn = FakeToolConn(result="Gamerule pvp is currently set to: false")
        check = dict(self.CHECK, present=False)
        self.assertFalse(run.run_check(run.Cluster(conn), check, "srv-1", "")["ok"])

    def test_operator_failure_is_a_failed_check_not_a_crash(self):
        conn = FakeToolConn(error=run.OperatorError(500, "rcon unavailable"))
        result = run.run_check(run.Cluster(conn), self.CHECK, "srv-1", "")
        self.assertFalse(result["ok"])
        self.assertIn("cluster error", result["detail"])
        self.assertIn("rcon unavailable", result["detail"])


if __name__ == "__main__":
    unittest.main()


class RerunOutcomesTest(unittest.TestCase):
    def write(self, out, repeat, outcome):
        (out / f"fake__mc-pvp-d1__r{repeat}.json").write_text(json.dumps({"outcome": outcome}), encoding="utf-8")

    def test_only_selected_outcomes_and_missing_rerun(self):
        cluster = run.FakeCluster()
        executor = run.FakeExecutor(cluster, fixes_pvp)
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            for repeat, outcome in ((1, "pass"), (2, "infra_error"), (3, "fail"), (4, "staging_error")):
                self.write(out, repeat, outcome)
            records = run.run_all(
                cluster, executor, [SYSTEM], [PVP_CASE], 5, 1, out, {},
                rerun_outcomes=run.parse_rerun_outcomes("infra_error,staging_error,missing"),
            )
            self.assertEqual(sorted(r["repeat"] for r in records), [2, 4, 5])
            self.assertEqual(json.loads((out / "fake__mc-pvp-d1__r1.json").read_text())["outcome"], "pass")
            self.assertEqual(json.loads((out / "fake__mc-pvp-d1__r3.json").read_text())["outcome"], "fail")
            self.assertEqual(json.loads((out / "fake__mc-pvp-d1__r2.json").read_text())["outcome"], "pass")

    def test_without_missing_absent_records_are_skipped(self):
        cluster = run.FakeCluster()
        executor = run.FakeExecutor(cluster, fixes_pvp)
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            self.write(out, 1, "infra_error")
            records = run.run_all(cluster, executor, [SYSTEM], [PVP_CASE], 2, 1, out, {},
                                  rerun_outcomes=frozenset({"infra_error"}))
            self.assertEqual([r["repeat"] for r in records], [1])

    def test_unknown_outcome_is_rejected(self):
        with self.assertRaises(ValueError):
            run.parse_rerun_outcomes("pass")


class AbortTest(unittest.TestCase):
    def test_streak_aborts_and_skips_remaining_jobs(self):
        cluster = run.FakeCluster()
        executor = run.FakeExecutor(cluster, fixes_pvp)
        cluster.failure_streak = run.ABORT_AFTER_FAILURES
        aborted = threading.Event()
        with tempfile.TemporaryDirectory() as tmp:
            records = run.run_all(cluster, executor, [SYSTEM], [PVP_CASE], 4, 1, Path(tmp), {}, aborted=aborted)
        self.assertTrue(aborted.is_set())
        self.assertEqual(len(records), 1)


class ChannelSelectionTest(unittest.TestCase):
    def test_eval_channels_prefers_arg_then_env_then_file(self):
        self.assertEqual(run.eval_channels("1, 2", {run.CHANNELS_KEY: "9"}, {}), ["1", "2"])
        self.assertEqual(run.eval_channels(None, {run.CHANNELS_KEY: "7,8"}, {run.CHANNELS_KEY: "9"}), ["7", "8"])
        self.assertEqual(run.eval_channels(None, {}, {run.CHANNELS_KEY: "5,6"}), ["5", "6"])
        self.assertEqual(run.eval_channels(None, {}, {}), [])

    def test_gary_turns_round_robin_while_the_connection_keeps_the_first(self):
        conn = FakeOperatorConn(GARY_RESPONSE)
        conn.channel = "first"
        executor = run.RealExecutor(conn, ["first", "second", "third"], {})
        self.assertEqual([executor._next_channel() for _ in range(4)], ["first", "second", "third", "first"])
        self.assertEqual(conn.channel, "first")
