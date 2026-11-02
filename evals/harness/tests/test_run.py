"""Tests for evals/harness/run.py: the dry-run flow on a fake cluster and the Claude Code usage parser."""

from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
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

    def test_ready_timeout_by_game(self):
        self.assertEqual(run.ready_timeout("palworld"), 900)
        self.assertEqual(run.ready_timeout("minecraft"), 400)


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


if __name__ == "__main__":
    unittest.main()
