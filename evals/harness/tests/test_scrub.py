"""Tests for evals/harness/scrub.py: snowflake and known-id replacement, the keep-list, JSON safety and --check."""

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
from unittest import mock

HARNESS = Path(__file__).resolve().parents[1]
SCRUB_PATH = HARNESS / "scrub.py"
EXPORT_PATH = HARNESS / "export_sft.py"

# 138445055974178816 decodes to 2024; it is the id the bot's tool results quote.
REAL_SNOWFLAKE = "138445055974178816"
# 19 digits, but the timestamp it encodes lies beyond now+1 day, so only a known id can scrub it.
OUT_OF_WINDOW_KNOWN = "9999999999999999999"


def load_module(name: str, path: Path) -> ModuleType:
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise ImportError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


scrub = load_module("scrub", SCRUB_PATH)
export_sft = load_module("export_sft", EXPORT_PATH)

NO_KNOWN: frozenset[str] = frozenset()


class ScrubTextTest(unittest.TestCase):
    def test_snowflake_is_replaced(self) -> None:
        self.assertEqual(
            scrub.scrub_text(f"restarted by {REAL_SNOWFLAKE}", NO_KNOWN),
            "restarted by <discord-id>",
        )

    def test_sentence_final_period_after_snowflake_is_still_replaced(self) -> None:
        self.assertEqual(scrub.scrub_text(f"by {REAL_SNOWFLAKE}.", NO_KNOWN), "by <discord-id>.")

    def test_steam_id_is_kept(self) -> None:
        steam = "76561198012345678"
        self.assertEqual(scrub.scrub_text(f"steam {steam}", NO_KNOWN), f"steam {steam}")

    def test_float_is_kept(self) -> None:
        self.assertEqual(scrub.scrub_text("ratio 0.13844505597417881", NO_KNOWN), "ratio 0.13844505597417881")
        self.assertEqual(scrub.scrub_text("ratio 1.138445055974178816", NO_KNOWN), "ratio 1.138445055974178816")

    def test_u64_max_is_kept(self) -> None:
        self.assertEqual(scrub.scrub_text("cap 18446744073709551615", NO_KNOWN), "cap 18446744073709551615")

    def test_digits_inside_a_longer_number_are_kept(self) -> None:
        longer = REAL_SNOWFLAKE + "999"
        self.assertEqual(scrub.scrub_text(longer, NO_KNOWN), longer)

    def test_short_numbers_are_kept(self) -> None:
        self.assertEqual(scrub.scrub_text("ticks 1700000000000 port 9361", NO_KNOWN), "ticks 1700000000000 port 9361")

    def test_known_id_is_replaced_even_outside_the_snowflake_window(self) -> None:
        known = frozenset({OUT_OF_WINDOW_KNOWN})
        self.assertEqual(scrub.scrub_text(f"user {OUT_OF_WINDOW_KNOWN}", known), "user <discord-id>")

    def test_known_id_inside_a_longer_number_is_kept(self) -> None:
        known = frozenset({OUT_OF_WINDOW_KNOWN})
        longer = OUT_OF_WINDOW_KNOWN + "999"
        self.assertEqual(scrub.scrub_text(longer, known), longer)


class ScrubValueTest(unittest.TestCase):
    def test_known_id_inside_json_string_with_escapes_stays_valid(self) -> None:
        known = frozenset({OUT_OF_WINDOW_KNOWN})
        decoded = {"detail": f'said "{OUT_OF_WINDOW_KNOWN}"\nthen\tleft \\ {OUT_OF_WINDOW_KNOWN}'}
        text = scrub.dumps_scrubbed(decoded, known, indent=2)
        self.assertNotIn(OUT_OF_WINDOW_KNOWN, text)
        self.assertEqual(json.loads(text)["detail"], 'said "<discord-id>"\nthen\tleft \\ <discord-id>')

    def test_bare_snowflake_integer_becomes_a_placeholder_string(self) -> None:
        scrubbed, count = scrub.scrub_value({"user": int(REAL_SNOWFLAKE), "ticks": 42}, NO_KNOWN)
        self.assertEqual(scrubbed, {"user": "<discord-id>", "ticks": 42})
        self.assertEqual(count, 1)

    def test_steam_and_float_values_pass_through(self) -> None:
        value = {"steam": 76561198012345678, "ratio": 0.5, "flag": True, "none": None}
        scrubbed, count = scrub.scrub_value(value, NO_KNOWN)
        self.assertEqual(scrubbed, value)
        self.assertEqual(count, 0)


class KnownIdsTest(unittest.TestCase):
    def test_reads_listed_keys_and_skips_non_numeric_values(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            env = Path(tmp) / ".env"
            env.write_text(
                "GAMESERVERS_ADMIN_USER_IDS=111111111111111111,222222222222222222\n"
                "GAMESERVERS_OPERATOR_CHANNEL=333333333333333333\n"
                "DISCORD_GUILD_ID=not-a-number\n"
                "GAMESERVERS_EVAL_CHANNELS=444444444444444444 555555555555555555\n"
                "UNRELATED=999999999999999999\n",
                encoding="utf-8",
            )
            err = io.StringIO()
            with contextlib.redirect_stderr(err):
                ids = scrub.known_ids(env)
        self.assertEqual(
            ids,
            frozenset({"111111111111111111", "222222222222222222", "333333333333333333", "444444444444444444", "555555555555555555"}),
        )
        self.assertIn("DISCORD_GUILD_ID", err.getvalue())
        self.assertNotIn("not-a-number", err.getvalue())

    def test_missing_env_file_yields_no_ids(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            err = io.StringIO()
            with contextlib.redirect_stderr(err):
                ids = scrub.known_ids(Path(tmp) / "absent.env")
        self.assertEqual(ids, NO_KNOWN)
        self.assertIn("no .env", err.getvalue())


class ScrubFileTest(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.known = frozenset({OUT_OF_WINDOW_KNOWN})

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def test_json_file_is_rewritten_and_stays_parseable(self) -> None:
        path = self.root / "record.json"
        path.write_text(json.dumps({"detail": f"by {REAL_SNOWFLAKE} via {OUT_OF_WINDOW_KNOWN}"}, indent=2), encoding="utf-8")
        count = scrub.scrub_file(path, self.known, write=True)
        self.assertEqual(count, 2)
        self.assertEqual(json.loads(path.read_text(encoding="utf-8"))["detail"], "by <discord-id> via <discord-id>")

    def test_jsonl_file_keeps_one_record_per_line(self) -> None:
        path = self.root / "train.jsonl"
        lines = [{"messages": [{"role": "user", "content": f"id {REAL_SNOWFLAKE}"}]}, {"messages": []}]
        path.write_text("".join(json.dumps(line, ensure_ascii=False) + "\n" for line in lines), encoding="utf-8")
        count = scrub.scrub_file(path, self.known, write=True)
        self.assertEqual(count, 1)
        rewritten = path.read_text(encoding="utf-8").splitlines()
        self.assertEqual(len(rewritten), 2)
        self.assertEqual(json.loads(rewritten[0])["messages"][0]["content"], "id <discord-id>")

    def test_markdown_file_is_scrubbed_as_text(self) -> None:
        path = self.root / "notes.md"
        path.write_text(f"Operator {OUT_OF_WINDOW_KNOWN} restarted it.\n", encoding="utf-8")
        self.assertEqual(scrub.scrub_file(path, self.known, write=True), 1)
        self.assertEqual(path.read_text(encoding="utf-8"), "Operator <discord-id> restarted it.\n")

    def test_check_mode_counts_without_writing(self) -> None:
        path = self.root / "notes.md"
        original = f"by {REAL_SNOWFLAKE}\n"
        path.write_text(original, encoding="utf-8")
        self.assertEqual(scrub.scrub_file(path, self.known, write=False), 1)
        self.assertEqual(path.read_text(encoding="utf-8"), original)


class MainTest(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.known = frozenset({OUT_OF_WINDOW_KNOWN})
        self.patch = mock.patch.object(scrub, "known_ids", return_value=self.known)
        self.patch.start()

    def tearDown(self) -> None:
        self.patch.stop()
        self._tmp.cleanup()

    def run_main(self, argv: list[str]) -> tuple[int, str]:
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            code = scrub.main(argv)
        return code, out.getvalue()

    def test_check_exits_one_when_something_would_change_and_writes_nothing(self) -> None:
        results = self.root / "results"
        results.mkdir()
        leaky = results / "record.json"
        leaky.write_text(json.dumps({"detail": f"by {REAL_SNOWFLAKE}"}), encoding="utf-8")
        before = leaky.read_text(encoding="utf-8")
        code, out = self.run_main([str(results), "--check"])
        self.assertEqual(code, 1)
        self.assertEqual(leaky.read_text(encoding="utf-8"), before)
        self.assertIn("would replace 1", out)
        self.assertNotIn(REAL_SNOWFLAKE, out)

    def test_check_exits_zero_on_clean_tree(self) -> None:
        results = self.root / "results"
        results.mkdir()
        (results / "clean.json").write_text(json.dumps({"detail": "steam 76561198012345678"}), encoding="utf-8")
        code, _ = self.run_main([str(results), "--check"])
        self.assertEqual(code, 0)

    def test_rewrite_mode_scrubs_in_place_and_reports_counts_only(self) -> None:
        results = self.root / "results"
        results.mkdir()
        leaky = results / "notes.md"
        leaky.write_text(f"by {REAL_SNOWFLAKE}\n", encoding="utf-8")
        code, out = self.run_main([str(results)])
        self.assertEqual(code, 0)
        self.assertEqual(leaky.read_text(encoding="utf-8"), "by <discord-id>\n")
        self.assertIn("replaced 1", out)
        self.assertNotIn(REAL_SNOWFLAKE, out)

    def test_missing_path_exits_two(self) -> None:
        code, _ = self.run_main([str(self.root / "absent")])
        self.assertEqual(code, 2)


class ExportWiringTest(unittest.TestCase):
    def test_exported_conversation_has_ids_scrubbed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            record = {"harness": "gary", "split": "dev", "outcome": "pass"}
            conversation = {"messages": [{"role": "tool", "content": f"restarted by {REAL_SNOWFLAKE}"}], "tools": []}
            run_dir = root / "runs"
            (run_dir / "transcripts").mkdir(parents=True)
            (run_dir / "transcripts" / "gary__case__r1.messages.json").write_text(json.dumps(conversation), encoding="utf-8")
            (run_dir / "gary__case__r1.json").write_text(json.dumps(record), encoding="utf-8")
            out = root / "train.jsonl"
            records = export_sft.load_records([run_dir])
            export_sft.export(records, out, False, frozenset({OUT_OF_WINDOW_KNOWN}))
            text = out.read_text(encoding="utf-8")
        self.assertNotIn(REAL_SNOWFLAKE, text)
        self.assertIn("restarted by <discord-id>", json.loads(text)["messages"][0]["content"])


if __name__ == "__main__":
    unittest.main()
