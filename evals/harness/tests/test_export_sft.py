"""Tests for evals/harness/export_sft.py: scored refusal, the pass filter and the JSONL line shape."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path
from types import ModuleType

EXPORT_PATH = Path(__file__).resolve().parents[1] / "export_sft.py"


def load_export_module() -> ModuleType:
    spec = importlib.util.spec_from_file_location("export_sft", EXPORT_PATH)
    if spec is None or spec.loader is None:
        raise ImportError(f"cannot load {EXPORT_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


export_sft = load_export_module()

CONVERSATION = {
    "messages": [
        {"role": "system", "content": "you are Gary"},
        {"role": "user", "content": "turn off pvp"},
        {
            "role": "assistant",
            "content": None,
            "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "edit_file", "arguments": "{}"}}],
            "reasoning_content": "the file says pvp=true",
        },
        {"role": "tool", "content": "edited", "tool_call_id": "c1"},
        {"role": "assistant", "content": "done"},
    ],
    "tools": [{"type": "function", "function": {"name": "edit_file", "parameters": {"type": "object"}}}],
}


def record(stem: str, outcome: str = "pass", split: str = "dev", harness: str = "gary", **extra) -> dict:
    return {"system_id": "gary-fw", "case_id": stem, "harness": harness, "split": split, "outcome": outcome, **extra}


def write_run(runs: Path, stem: str, body: dict, conversation: dict | None = CONVERSATION) -> None:
    runs.mkdir(parents=True, exist_ok=True)
    (runs / f"{stem}.json").write_text(json.dumps(body), encoding="utf-8")
    if conversation is not None:
        transcripts = runs / "transcripts"
        transcripts.mkdir(exist_ok=True)
        (transcripts / f"{stem}.messages.json").write_text(json.dumps(conversation), encoding="utf-8")


def run_main(argv: list[str]) -> tuple[int, str, str]:
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = export_sft.main(argv)
    return code, out.getvalue(), err.getvalue()


class ExportTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.runs = self.root / "runs"
        self.out = self.root / "train.jsonl"

    def lines(self) -> list[dict]:
        return [json.loads(line) for line in self.out.read_text(encoding="utf-8").splitlines()]

    def test_refuses_when_any_input_is_scored_and_writes_nothing(self):
        write_run(self.runs, "gary-fw__dev-case__r1", record("dev-case"))
        write_run(self.runs, "gary-fw__held-out__r1", record("held-out", split="scored"))
        code, _, err = run_main([str(self.runs), "-o", str(self.out)])
        self.assertNotEqual(code, 0)
        self.assertIn("scored-split", err)
        self.assertFalse(self.out.exists())

    def test_scored_record_refuses_even_when_filtered_out_by_pass(self):
        write_run(self.runs, "gary-fw__held-out__r1", record("held-out", outcome="fail", split="scored"))
        code, _, _ = run_main([str(self.runs), "-o", str(self.out), "--only-pass"])
        self.assertNotEqual(code, 0)
        self.assertFalse(self.out.exists())

    def test_only_pass_keeps_passing_dev_turns(self):
        write_run(self.runs, "gary-fw__a__r1", record("a", outcome="pass"))
        write_run(self.runs, "gary-fw__b__r1", record("b", outcome="fail"))
        write_run(self.runs, "gary-fw__c__r1", record("c", outcome="infra_error"))
        code, out, _ = run_main([str(self.runs), "-o", str(self.out), "--only-pass"])
        self.assertEqual(code, 0)
        self.assertEqual(len(self.lines()), 1)
        self.assertIn("exported 1 turns", out)
        self.assertIn("skipped 2: not a pass", out)

    def test_without_only_pass_every_dev_turn_with_a_transcript_is_exported(self):
        write_run(self.runs, "gary-fw__a__r1", record("a", outcome="pass"))
        write_run(self.runs, "gary-fw__b__r1", record("b", outcome="fail"))
        code, _, _ = run_main([str(self.runs), "-o", str(self.out)])
        self.assertEqual(code, 0)
        self.assertEqual(len(self.lines()), 2)

    def test_line_is_messages_and_tools_with_reasoning_kept(self):
        write_run(self.runs, "gary-fw__a__r1", record("a"))
        run_main([str(self.runs), "-o", str(self.out)])
        (line,) = self.lines()
        self.assertEqual(set(line), {"messages", "tools"})
        self.assertEqual(line["messages"], CONVERSATION["messages"])
        self.assertEqual(line["tools"], CONVERSATION["tools"])
        self.assertEqual(line["messages"][2]["reasoning_content"], "the file says pvp=true")

    def test_claude_code_and_non_dev_records_are_not_exported(self):
        write_run(self.runs, "cc-opus__a__r1", record("a", harness="claude_code"))
        write_run(self.runs, "gary-fw__b__r1", record("b", harness="gary", split="train"))
        code, out, _ = run_main([str(self.runs), "-o", str(self.out)])
        self.assertEqual(code, 0)
        self.assertEqual(self.lines(), [])
        self.assertIn("0 gary dev records", out)

    def test_turn_without_transcript_is_skipped_and_counted(self):
        write_run(self.runs, "gary-fw__a__r1", record("a", notes={"transcript": "unavailable"}), conversation=None)
        write_run(self.runs, "gary-fw__b__r1", record("b", outcome="infra_error"), conversation=None)
        code, out, _ = run_main([str(self.runs), "-o", str(self.out)])
        self.assertEqual(code, 0)
        self.assertEqual(self.lines(), [])
        self.assertIn("skipped 2: no transcript", out)

    def test_malformed_transcript_fails_the_export(self):
        write_run(self.runs, "gary-fw__a__r1", record("a"), conversation={"messages": []})
        code, _, err = run_main([str(self.runs), "-o", str(self.out)])
        self.assertEqual(code, 1)
        self.assertIn("lacks a messages list and a tools list", err)

    def test_missing_runs_directory_is_a_usage_error(self):
        code, _, err = run_main([str(self.root / "nope"), "-o", str(self.out)])
        self.assertEqual(code, 2)
        self.assertIn("runs directory not found", err)


if __name__ == "__main__":
    unittest.main()
