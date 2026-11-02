"""Tests for evals/harness/score.py: statistics, cost, exclusion rules, and the end-to-end schema."""

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

SCORE_PATH = Path(__file__).resolve().parents[1] / "score.py"


def load_score_module() -> ModuleType:
    spec = importlib.util.spec_from_file_location("score", SCORE_PATH)
    if spec is None or spec.loader is None:
        raise ImportError(f"cannot load {SCORE_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


score = load_score_module()

OPUS_RATE = {"input_per_1m_usd": 2.0, "cached_input_per_1m_usd": 0.5, "output_per_1m_usd": 8.0}
RATES = {"opus": OPUS_RATE, "open": {"input_per_1m_usd": 1.0, "cached_input_per_1m_usd": 0.1, "output_per_1m_usd": 2.0}}


def make_record(
    system_id: str = "opus",
    case_id: str = "case-1",
    repeat: int = 0,
    outcome: str = "pass",
    split: str = "scored",
    model: str = "opus",
    harness: str = "claude_code",
    claimed_done: bool = False,
    latency_ms: int = 1000,
    prompt_tokens: int = 1000,
    cached_tokens: int = 0,
    completion_tokens: int = 100,
    game: str = "minecraft",
    category: str = "mods",
) -> dict:
    return {
        "system_id": system_id,
        "label": system_id.upper(),
        "model": model,
        "harness": harness,
        "case_id": case_id,
        "game": game,
        "task_area": "config",
        "category": category,
        "prompt": f"prompt for {case_id}",
        "split": split,
        "repeat": repeat,
        "outcome": outcome,
        "claimed_done": claimed_done,
        "turns": 3,
        "tool_calls": ["read_file", "edit_file"],
        "usage": {
            "prompt_tokens": prompt_tokens,
            "cached_tokens": cached_tokens,
            "completion_tokens": completion_tokens,
        },
        "latency_ms": latency_ms,
        "model_ms": latency_ms // 2,
        "tool_ms": latency_ms // 2,
        "checks": [{"name": "ready", "ok": outcome == "pass", "detail": ""}],
    }


def quietly(fn, *args, **kwargs):
    with contextlib.redirect_stderr(io.StringIO()):
        return fn(*args, **kwargs)


class WilsonIntervalTest(unittest.TestCase):
    def test_eight_of_ten_matches_known_interval(self) -> None:
        low, high = score.wilson_interval(8, 10)
        self.assertAlmostEqual(low, 0.490, places=3)
        self.assertAlmostEqual(high, 0.943, places=3)

    def test_no_cases_has_no_interval(self) -> None:
        self.assertIsNone(score.wilson_interval(0, 0))

    def test_bounds_stay_inside_unit_interval(self) -> None:
        low, high = score.wilson_interval(10, 10)
        self.assertGreaterEqual(low, 0.0)
        self.assertAlmostEqual(high, 1.0)


class McNemarTest(unittest.TestCase):
    def test_five_discordant_one_way_is_exact_binomial(self) -> None:
        # P(X <= 0) for Binomial(5, 0.5) is 1/32; two-sided doubles it.
        self.assertAlmostEqual(score.mcnemar_exact_p(5, 0), 0.0625)

    def test_symmetric_discordance_is_not_significant(self) -> None:
        self.assertEqual(score.mcnemar_exact_p(3, 3), 1.0)

    def test_three_and_one(self) -> None:
        # n = 4, tail P(X <= 1) = 5/16, two-sided = 10/16.
        self.assertAlmostEqual(score.mcnemar_exact_p(3, 1), 0.625)

    def test_no_discordant_pairs_gives_one(self) -> None:
        self.assertEqual(score.mcnemar_exact_p(0, 0), 1.0)


class NearestRankTest(unittest.TestCase):
    def test_p95_of_twenty_values_is_nineteenth(self) -> None:
        self.assertEqual(score.nearest_rank(list(range(1, 21)), 95), 19)

    def test_p95_of_ten_values_is_the_maximum(self) -> None:
        self.assertEqual(score.nearest_rank(list(range(1, 11)), 95), 10)

    def test_p50_of_ten_values_is_fifth(self) -> None:
        self.assertEqual(score.nearest_rank(list(range(1, 11)), 50), 5)

    def test_unsorted_input(self) -> None:
        self.assertEqual(score.nearest_rank([9, 1, 5], 50), 5)

    def test_empty_is_none(self) -> None:
        self.assertIsNone(score.nearest_rank([], 95))


class CostTest(unittest.TestCase):
    def test_cached_tokens_billed_at_cached_rate(self) -> None:
        usage = {"prompt_tokens": 1000, "cached_tokens": 400, "completion_tokens": 200}
        # uncached 600 * 2.0 + cached 400 * 0.5 + completion 200 * 8.0 = 3000 per 1M tokens
        self.assertAlmostEqual(score.cost_usd(usage, OPUS_RATE), 0.003)

    def test_missing_rate_gives_none(self) -> None:
        usage = {"prompt_tokens": 1, "cached_tokens": 0, "completion_tokens": 1}
        self.assertIsNone(score.cost_usd(usage, None))

    def test_cost_per_case_flows_into_system_summary(self) -> None:
        records = [make_record(prompt_tokens=1000, cached_tokens=400, completion_tokens=200, case_id="c1")]
        results = score.build_results(records, RATES, generated_at="t")
        self.assertAlmostEqual(results["summary"]["systems"][0]["mean_cost_usd"], 0.003)
        self.assertAlmostEqual(results["summary"]["systems"][0]["cost_per_1k_usd"], 3.0)

    def test_missing_rate_warns_and_nulls_cost(self) -> None:
        records = [make_record(model="unpriced", case_id="c1")]
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            results = score.build_results(records, RATES, no_baseline=True, generated_at="t")
        self.assertIn("unpriced", stderr.getvalue())
        system = results["summary"]["systems"][0]
        self.assertIsNone(system["cost_per_1k_usd"])
        self.assertIsNone(results["cases"][0]["results_by_system"]["opus"]["cost_usd"])

    def test_unfilled_placeholder_rate_warns(self) -> None:
        rates = {"unfilled": {"input_per_1m_usd": 0.0, "cached_input_per_1m_usd": 0.0, "output_per_1m_usd": 0.0}}
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            score.build_results([make_record(model="unfilled", case_id="c1")], rates, no_baseline=True, generated_at="t")
        self.assertIn("every rate for model 'unfilled' is zero", stderr.getvalue())


class ExclusionTest(unittest.TestCase):
    def test_staging_error_leaves_every_denominator(self) -> None:
        records = [
            make_record(case_id="c1", outcome="pass"),
            make_record(case_id="c2", outcome="fail"),
            make_record(case_id="c3", outcome="staging_error"),
        ]
        results = score.build_results(records, RATES, generated_at="t")
        system = results["summary"]["systems"][0]
        self.assertEqual(system["n_cases"], 2)
        self.assertAlmostEqual(system["verified_success_rate"], 0.5)
        self.assertEqual(system["staging_error_count"], 1)
        self.assertEqual([c["case_id"] for c in results["cases"]], ["c1", "c2"])
        self.assertEqual(
            results["excluded"],
            [{"system_id": "opus", "case_id": "c3", "repeat": 0, "reason": "staging_error"}],
        )

    def test_infra_error_is_a_fail_and_counted_separately(self) -> None:
        records = [
            make_record(case_id="c1", outcome="pass"),
            make_record(case_id="c2", outcome="infra_error"),
        ]
        results = score.build_results(records, RATES, generated_at="t")
        system = results["summary"]["systems"][0]
        self.assertEqual(system["n_cases"], 2)
        self.assertAlmostEqual(system["verified_success_rate"], 0.5)
        self.assertEqual(system["infra_error_count"], 1)
        self.assertNotIn("c2", [e["case_id"] for e in results["excluded"]])

    def test_dev_split_is_ignored(self) -> None:
        records = [make_record(case_id="c1"), make_record(case_id="dev-1", split="dev", outcome="fail")]
        results = score.build_results(records, RATES, generated_at="t")
        self.assertEqual(results["summary"]["systems"][0]["n_cases"], 1)
        self.assertEqual([c["case_id"] for c in results["cases"]], ["c1"])

    def test_headline_uses_repeat_zero_only(self) -> None:
        records = [
            make_record(case_id="c1", repeat=0, outcome="pass"),
            make_record(case_id="c1", repeat=1, outcome="fail"),
            make_record(case_id="c1", repeat=2, outcome="fail"),
        ]
        results = score.build_results(records, RATES, generated_at="t")
        self.assertEqual(results["summary"]["systems"][0]["n_cases"], 1)
        self.assertEqual(results["summary"]["systems"][0]["verified_success_rate"], 1.0)

    def test_duplicate_record_is_rejected(self) -> None:
        records = [make_record(case_id="c1"), make_record(case_id="c1")]
        with self.assertRaises(ValueError):
            score.build_results(records, RATES, generated_at="t")


class FalseCompletionTest(unittest.TestCase):
    def test_claimed_done_and_failed_counts(self) -> None:
        records = [
            make_record(case_id="c1", claimed_done=True, outcome="fail"),
            make_record(case_id="c2", claimed_done=True, outcome="pass"),
            make_record(case_id="c3", claimed_done=False, outcome="fail"),
            make_record(case_id="c4", claimed_done=False, outcome="pass"),
        ]
        results = score.build_results(records, RATES, generated_at="t")
        self.assertAlmostEqual(results["summary"]["systems"][0]["false_completion_rate"], 0.25)


class BreakdownTest(unittest.TestCase):
    def test_by_game_and_category(self) -> None:
        records = [
            make_record(case_id="c1", game="minecraft", category="mods", outcome="pass"),
            make_record(case_id="c2", game="minecraft", category="fix", outcome="fail"),
            make_record(case_id="c3", game="valheim", category="fix", outcome="pass"),
        ]
        system = score.build_results(records, RATES, generated_at="t")["summary"]["systems"][0]
        self.assertEqual(system["by_game"]["minecraft"], {"n": 2, "success_rate": 0.5})
        self.assertEqual(system["by_game"]["valheim"], {"n": 1, "success_rate": 1.0})
        self.assertEqual(system["by_category"]["fix"], {"n": 2, "success_rate": 0.5})


class ComparisonTest(unittest.TestCase):
    def build(self, sys_outcomes, base_outcomes, sys_latency=None, base_latency=None):
        records = []
        for i, (s, b) in enumerate(zip(sys_outcomes, base_outcomes)):
            case_id = f"c{i}"
            records.append(make_record("open", case_id, outcome=s, model="open", harness="gary",
                                       latency_ms=(sys_latency or {}).get(i, 1000)))
            records.append(make_record("opus", case_id, outcome=b,
                                       latency_ms=(base_latency or {}).get(i, 1000)))
        return score.build_results(records, RATES, baseline_id="opus", generated_at="t")

    def test_success_difference_and_mcnemar(self) -> None:
        results = self.build(["pass"] * 5 + ["fail"], ["fail"] * 5 + ["pass"])
        comparison = results["comparisons"][0]
        self.assertEqual(comparison["system_id"], "open")
        self.assertEqual(comparison["baseline_system_id"], "opus")
        self.assertEqual(comparison["n_paired"], 6)
        self.assertAlmostEqual(comparison["success_difference"], 5 / 6 - 1 / 6)
        # 5 discordant in favour of open, 1 in favour of opus: n = 6, tail P(X <= 1) = 7/64.
        self.assertAlmostEqual(comparison["mcnemar_p_two_sided"], 14 / 64)

    def test_cost_ratio_uses_each_systems_rates(self) -> None:
        # Same tokens on both sides. open: 1000 * 1.0 + 100 * 2.0 = 1200; opus: 1000 * 2.0 + 100 * 8.0 = 2800.
        results = self.build(["pass", "fail", "pass"], ["pass", "fail", "pass"])
        comparison = results["comparisons"][0]
        self.assertAlmostEqual(comparison["cost_ratio"]["point"], 1200 / 2800)
        low, high = comparison["cost_ratio"]["ci_95"]
        self.assertAlmostEqual(low, 1200 / 2800)
        self.assertAlmostEqual(high, 1200 / 2800)

    def test_identical_latencies_give_unit_ratio(self) -> None:
        results = self.build(["pass", "fail", "pass"], ["pass", "fail", "pass"])
        comparison = results["comparisons"][0]
        self.assertAlmostEqual(comparison["latency_ratio"]["point"], 1.0)
        self.assertAlmostEqual(comparison["latency_ratio"]["ci_95"][0], 1.0)
        self.assertAlmostEqual(comparison["latency_ratio"]["ci_95"][1], 1.0)

    def test_latency_ratio_is_system_over_baseline(self) -> None:
        results = self.build(["pass", "pass"], ["pass", "pass"], sys_latency={0: 500, 1: 500})
        self.assertAlmostEqual(results["comparisons"][0]["latency_ratio"]["point"], 0.5)

    def test_bootstrap_is_deterministic(self) -> None:
        pairs = [(float(i), float(i % 3 + 1)) for i in range(1, 12)]
        self.assertEqual(score.bootstrap_ratio_ci(pairs), score.bootstrap_ratio_ci(pairs))

    def test_default_baseline_is_first_claude_code_opus(self) -> None:
        records = [
            make_record("gary-open", "c1", model="fireworks/PLACEHOLDER", harness="gary"),
            make_record("opus", "c1"),
        ]
        results = quietly(score.build_results, records, RATES, generated_at="t")
        self.assertEqual(results["comparisons"][0]["baseline_system_id"], "opus")

    def test_unknown_baseline_is_an_error(self) -> None:
        with self.assertRaises(ValueError):
            score.build_results([make_record()], RATES, baseline_id="missing", generated_at="t")


class VarianceTest(unittest.TestCase):
    def test_success_rate_per_repeat_mean_and_stdev(self) -> None:
        records = [
            make_record(case_id="c1", repeat=0, outcome="pass"),
            make_record(case_id="c1", repeat=1, outcome="pass"),
            make_record(case_id="c2", repeat=0, outcome="fail"),
            make_record(case_id="c2", repeat=1, outcome="pass"),
            make_record(case_id="c3", repeat=0, outcome="pass"),
        ]
        section = score.build_results(records, RATES, generated_at="t")["variance"]["opus"]
        self.assertEqual(section["case_ids"], ["c1", "c2"])
        self.assertEqual(section["success_rate_by_repeat"], {"0": 0.5, "1": 1.0})
        self.assertAlmostEqual(section["mean_success_rate"], 0.75)
        self.assertAlmostEqual(section["stdev_success_rate"], 0.3535533905932738)

    def test_single_run_has_no_variance(self) -> None:
        section = score.build_results([make_record()], RATES, generated_at="t")["variance"]["opus"]
        self.assertEqual(section["case_ids"], [])
        self.assertIsNone(section["stdev_success_rate"])


class ValidationTest(unittest.TestCase):
    def test_missing_field_names_the_file(self) -> None:
        record = make_record()
        del record["latency_ms"]
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "bad.json"
            path.write_text(json.dumps(record), encoding="utf-8")
            with self.assertRaises(ValueError) as ctx:
                score.validate_record(record, path)
            self.assertIn("latency_ms", str(ctx.exception))
            self.assertIn("bad.json", str(ctx.exception))

    def test_cached_tokens_cannot_exceed_prompt_tokens(self) -> None:
        record = make_record(prompt_tokens=10, cached_tokens=20)
        with self.assertRaises(ValueError):
            score.validate_record(record, Path("x.json"))


class CaseCatalogTest(unittest.TestCase):
    def test_catalog_supplies_case_metadata(self) -> None:
        records = [make_record(case_id="c1", game="from-record")]
        catalog = {"c1": {"case_id": "c1", "game": "from-catalog", "task_area": "config",
                          "category": "mods", "prompt": "prompt for c1"}}
        results = quietly(score.build_results, records, RATES, catalog=catalog, generated_at="t")
        self.assertEqual(results["cases"][0]["game"], "from-catalog")

    def test_record_outside_catalog_is_an_error(self) -> None:
        with self.assertRaises(ValueError):
            score.build_results([make_record(case_id="nope")], RATES, catalog={}, generated_at="t")


class EndToEndTest(unittest.TestCase):
    def test_two_systems_three_cases_produce_schema(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            runs = Path(tmp) / "runs"
            runs.mkdir()
            n = 0
            for system_id, model, harness in (
                ("opus", "opus", "claude_code"),
                ("gary-open", "open", "gary"),
            ):
                for case_id in ("c1", "c2", "c3"):
                    n += 1
                    outcome = "pass" if case_id != "c3" or system_id == "opus" else "fail"
                    record = make_record(system_id, case_id, model=model, harness=harness,
                                         outcome=outcome, latency_ms=100 * n)
                    (runs / f"{system_id}-{case_id}-0.json").write_text(json.dumps(record), encoding="utf-8")
            staging = make_record("gary-open", "c4", model="open", harness="gary", outcome="staging_error")
            (runs / "gary-open-c4-0.json").write_text(json.dumps(staging), encoding="utf-8")
            opus_c4 = make_record("opus", "c4", outcome="pass", latency_ms=900)
            (runs / "opus-c4-0.json").write_text(json.dumps(opus_c4), encoding="utf-8")

            out = Path(tmp) / "eval_results.json"
            stdout = io.StringIO()
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(io.StringIO()):
                code = score.main([str(runs), "-o", str(out)])
            self.assertEqual(code, 0)

            results = json.loads(out.read_text(encoding="utf-8"))
            self.assertEqual(set(results), {"generated_at", "summary", "cases", "comparisons", "variance", "excluded"})
            systems = {s["system_id"]: s for s in results["summary"]["systems"]}
            self.assertEqual(set(systems), {"opus", "gary-open"})
            for field in (
                "system_id", "label", "kind", "model", "harness", "n_cases", "verified_success_rate",
                "success_ci_95", "unintended_change_rate", "mean_turns", "mean_tokens",
                "latency_p50_ms", "latency_p95_ms", "cost_per_1k_usd", "notes",
            ):
                self.assertIn(field, systems["opus"])
            self.assertEqual(systems["gary-open"]["n_cases"], 3)
            self.assertAlmostEqual(systems["gary-open"]["verified_success_rate"], 2 / 3)
            self.assertIsNone(systems["opus"]["unintended_change_rate"])

            # c4 is a staging error for gary-open only, so gary-open drops it and opus keeps it.
            self.assertEqual(len(results["cases"]), 4)
            case = results["cases"][0]
            self.assertEqual(
                set(case),
                {"case_id", "game", "task_area", "category", "prompt", "results_by_system"},
            )
            self.assertEqual(set(case["results_by_system"]), {"opus", "gary-open"})
            self.assertEqual(
                set(case["results_by_system"]["opus"]),
                {"outcome", "verified_success", "unintended_changes", "turns", "latency_ms", "cost_usd", "tool_calls"},
            )
            self.assertEqual(results["comparisons"][0]["baseline_system_id"], "opus")
            self.assertEqual(results["excluded"][0]["case_id"], "c4")
            self.assertIn("| System |", stdout.getvalue())

    def test_cli_reports_missing_runs_directory(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            stderr = io.StringIO()
            with contextlib.redirect_stderr(stderr), contextlib.redirect_stdout(io.StringIO()):
                code = score.main([str(Path(tmp) / "empty"), "-o", str(Path(tmp) / "out.json")])
            self.assertEqual(code, 1)
            self.assertIn("error:", stderr.getvalue())


def system_row(results: dict, system_id: str) -> dict:
    return next(s for s in results["summary"]["systems"] if s["system_id"] == system_id)


def catalog_entry(case_id: str) -> dict:
    return {"case_id": case_id, "game": "minecraft", "task_area": "config", "category": "mods",
            "prompt": f"prompt for {case_id}"}


def write_records(runs: Path, records: list[dict]) -> None:
    for record in records:
        name = f"{record['system_id']}-{record['case_id']}-{record['repeat']}.json"
        (runs / name).write_text(json.dumps(record), encoding="utf-8")


class MissingRecordTest(unittest.TestCase):
    def build(self) -> dict:
        records = [
            make_record("opus", "c1"),
            make_record("opus", "c2"),
            make_record("open", "c1", model="open", harness="gary"),
        ]
        return score.build_results(records, RATES, baseline_id="opus", generated_at="t")

    def test_absent_record_counts_as_fail_and_is_listed(self) -> None:
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            results = self.build()
        open_system = system_row(results, "open")
        self.assertEqual(open_system["n_cases"], 2)
        self.assertEqual(open_system["missing_cases"], ["c2"])
        self.assertAlmostEqual(open_system["verified_success_rate"], 0.5)
        self.assertIn("no run record for 1 scored case(s)", stderr.getvalue())
        self.assertIn("c2", stderr.getvalue())
        self.assertIn("no run record", open_system["notes"])
        self.assertEqual(system_row(results, "opus")["missing_cases"], [])

    def test_missing_case_appears_with_missing_outcome(self) -> None:
        results = quietly(self.build)
        c2 = next(c for c in results["cases"] if c["case_id"] == "c2")
        self.assertEqual(c2["results_by_system"]["open"]["outcome"], "missing")
        self.assertFalse(c2["results_by_system"]["open"]["verified_success"])
        self.assertIsNone(c2["results_by_system"]["open"]["cost_usd"])

    def test_missing_case_stays_in_the_paired_comparison_as_a_fail(self) -> None:
        records = [
            make_record("opus", "c1"),
            make_record("opus", "c2"),
            make_record("open", "c1", model="open", harness="gary"),
        ]
        results = quietly(score.build_results, records, RATES, baseline_id="opus", generated_at="t")
        comparison = results["comparisons"][0]
        self.assertEqual(comparison["n_paired"], 2)
        self.assertEqual(comparison["n_resource_paired"], 1)
        self.assertAlmostEqual(comparison["success_difference"], -0.5)

    def test_catalog_case_with_no_record_fails_for_every_system(self) -> None:
        catalog = {"c1": catalog_entry("c1"), "c9": catalog_entry("c9")}
        records = [make_record("opus", "c1"), make_record("open", "c1", model="open", harness="gary")]
        results = quietly(score.build_results, records, RATES, catalog=catalog, baseline_id="opus",
                          generated_at="t")
        for system_id in ("opus", "open"):
            system = system_row(results, system_id)
            self.assertEqual(system["staged_cases"], 2)
            self.assertEqual(system["n_cases"], 2)
            self.assertEqual(system["missing_cases"], ["c9"])
        self.assertEqual([c["case_id"] for c in results["cases"]], ["c1", "c9"])
        self.assertEqual(results["comparisons"][0]["n_paired"], 2)


class BaselineRequiredTest(unittest.TestCase):
    def test_no_baseline_is_an_error_naming_the_flag(self) -> None:
        records = [make_record("gary", "c1", model="open", harness="gary")]
        with self.assertRaises(ValueError) as ctx:
            score.build_results(records, RATES, generated_at="t")
        self.assertIn("no baseline system found", str(ctx.exception))
        self.assertIn("--no-baseline", str(ctx.exception))

    def test_no_baseline_flag_builds_without_comparisons(self) -> None:
        records = [make_record("gary", "c1", model="open", harness="gary")]
        results = quietly(score.build_results, records, RATES, no_baseline=True, generated_at="t")
        self.assertEqual(results["comparisons"], [])
        self.assertEqual(len(results["summary"]["systems"]), 1)

    def test_cli_exits_nonzero_without_baseline_unless_flagged(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            runs = Path(tmp) / "runs"
            runs.mkdir()
            write_records(runs, [make_record("gary", "c1", model="open", harness="gary")])
            out = Path(tmp) / "out.json"
            stderr = io.StringIO()
            with contextlib.redirect_stderr(stderr), contextlib.redirect_stdout(io.StringIO()):
                code = score.main([str(runs), "-o", str(out)])
            self.assertEqual(code, 1)
            self.assertIn("error: no baseline system found", stderr.getvalue())
            self.assertFalse(out.exists())
            with contextlib.redirect_stderr(io.StringIO()), contextlib.redirect_stdout(io.StringIO()):
                code = score.main([str(runs), "-o", str(out), "--no-baseline"])
            self.assertEqual(code, 0)


class SystemIdentityTest(unittest.TestCase):
    def test_inconsistent_field_within_a_system_names_both_files(self) -> None:
        changes = {"model": "fireworks/other", "label": "OTHER", "harness": "claude_code"}
        for field, value in changes.items():
            with self.subTest(field=field), tempfile.TemporaryDirectory() as tmp:
                runs = Path(tmp)
                first = make_record("open", "c1", model="fireworks/a", harness="gary")
                second = make_record("open", "c2", model="fireworks/a", harness="gary")
                second[field] = value
                write_records(runs, [first, second])
                with self.assertRaises(ValueError) as ctx:
                    score.load_records(runs)
                message = str(ctx.exception)
                self.assertIn(f"inconsistent {field}", message)
                self.assertIn("open-c1-0.json", message)
                self.assertIn("open-c2-0.json", message)

    def test_consistent_system_loads(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            runs = Path(tmp)
            write_records(runs, [make_record("open", "c1", model="open", harness="gary"),
                                 make_record("open", "c2", model="open", harness="gary")])
            self.assertEqual(len(score.load_records(runs)), 2)


class NumericFieldTest(unittest.TestCase):
    def test_null_or_wrongly_typed_fields_name_the_field(self) -> None:
        bad_values = {
            "latency_ms": None,
            "turns": True,
            "model_ms": "12",
            "tool_ms": -1,
        }
        for field, bad in bad_values.items():
            with self.subTest(field=field):
                record = make_record()
                record[field] = bad
                with self.assertRaises(ValueError) as ctx:
                    score.validate_record(record, Path("bad.json"))
                self.assertIn(field, str(ctx.exception))
                self.assertIn("bad.json", str(ctx.exception))

    def test_null_usage_count_names_the_usage_field(self) -> None:
        record = make_record()
        record["usage"]["cached_tokens"] = None
        with self.assertRaises(ValueError) as ctx:
            score.validate_record(record, Path("bad.json"))
        self.assertIn("usage.cached_tokens", str(ctx.exception))
        self.assertIn("bad.json", str(ctx.exception))

    def test_claimed_done_must_be_a_bool(self) -> None:
        record = make_record()
        record["claimed_done"] = "false"
        with self.assertRaises(ValueError) as ctx:
            score.validate_record(record, Path("bad.json"))
        self.assertIn("claimed_done", str(ctx.exception))

    def test_tool_calls_must_be_a_list(self) -> None:
        record = make_record()
        record["tool_calls"] = "read_file"
        with self.assertRaises(ValueError) as ctx:
            score.validate_record(record, Path("bad.json"))
        self.assertIn("tool_calls", str(ctx.exception))

    def test_bad_record_fails_the_cli_with_an_error_line(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            runs = Path(tmp) / "runs"
            runs.mkdir()
            record = make_record()
            record["latency_ms"] = None
            write_records(runs, [record])
            stderr = io.StringIO()
            with contextlib.redirect_stderr(stderr), contextlib.redirect_stdout(io.StringIO()):
                code = score.main([str(runs), "-o", str(Path(tmp) / "out.json")])
            self.assertEqual(code, 1)
            self.assertIn("latency_ms", stderr.getvalue())
            self.assertIn("opus-case-1-0.json", stderr.getvalue())


class HumanEstimateTest(unittest.TestCase):
    def estimate(self, **overrides) -> dict:
        row = {
            "system_id": "human-t1-t2",
            "label": "Human T1/T2",
            "kind": "estimated",
            "model": "human",
            "harness": "human",
            "n_cases": None,
            "verified_success_rate": 0.9,
            "success_ci_95": [0.8, 0.95],
            "unintended_change_rate": None,
            "mean_turns": None,
            "mean_tokens": None,
            "latency_p50_ms": None,
            "latency_p95_ms": None,
            "cost_per_1k_usd": None,
            "notes": "Industry estimate.",
        }
        row.update(overrides)
        return row

    def records(self) -> list[dict]:
        return [make_record("opus", "c1"), make_record("open", "c1", model="open", harness="gary")]

    def test_estimate_row_is_appended_and_kept_out_of_comparisons(self) -> None:
        results = quietly(score.build_results, self.records(), RATES, baseline_id="opus",
                          estimate=self.estimate(), generated_at="t")
        systems = results["summary"]["systems"]
        self.assertEqual([s["system_id"] for s in systems], ["opus", "open", "human-t1-t2"])
        self.assertEqual(systems[-1]["kind"], "estimated")
        self.assertEqual(systems[-1]["verified_success_rate"], 0.9)
        self.assertEqual([c["system_id"] for c in results["comparisons"]], ["open"])
        for case in results["cases"]:
            self.assertNotIn("human-t1-t2", case["results_by_system"])
        self.assertNotIn("human-t1-t2", results["variance"])

    def test_estimate_row_renders_marked_as_estimated(self) -> None:
        results = quietly(score.build_results, self.records(), RATES, baseline_id="opus",
                          estimate=self.estimate(), generated_at="t")
        self.assertIn("Human T1/T2 (estimated)", score.render_markdown(results))

    def test_load_estimate_accepts_a_valid_file(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "human.json"
            path.write_text(json.dumps(self.estimate()), encoding="utf-8")
            loaded = score.load_estimate(path)
            self.assertEqual(loaded["kind"], "estimated")
            self.assertEqual(loaded["success_ci_95"], [0.8, 0.95])

    def test_load_estimate_rejects_a_measured_kind(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "human.json"
            path.write_text(json.dumps(self.estimate(kind="measured")), encoding="utf-8")
            with self.assertRaises(ValueError) as ctx:
                score.load_estimate(path)
            self.assertIn("expected 'estimated'", str(ctx.exception))
            self.assertIn("human.json", str(ctx.exception))

    def test_load_estimate_rejects_a_missing_field(self) -> None:
        row = self.estimate()
        del row["cost_per_1k_usd"]
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "human.json"
            path.write_text(json.dumps(row), encoding="utf-8")
            with self.assertRaises(ValueError) as ctx:
                score.load_estimate(path)
            self.assertIn("cost_per_1k_usd", str(ctx.exception))

    def test_load_estimate_rejects_a_negative_number(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "human.json"
            path.write_text(json.dumps(self.estimate(mean_turns=-2)), encoding="utf-8")
            with self.assertRaises(ValueError) as ctx:
                score.load_estimate(path)
            self.assertIn("mean_turns", str(ctx.exception))
            self.assertIn("human.json", str(ctx.exception))


class StagingCaseSetTest(unittest.TestCase):
    def test_staging_error_on_one_system_warns_and_shrinks_only_that_system(self) -> None:
        records = [
            make_record("opus", "c1"),
            make_record("opus", "c2"),
            make_record("open", "c1", model="open", harness="gary"),
            make_record("open", "c2", model="open", harness="gary", outcome="staging_error"),
        ]
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            results = score.build_results(records, RATES, baseline_id="opus", generated_at="t")
        self.assertIn("headline case sets differ across systems", stderr.getvalue())
        self.assertIn("'open' excludes c2", stderr.getvalue())
        open_system = system_row(results, "open")
        self.assertEqual(open_system["staged_cases"], 2)
        self.assertEqual(open_system["n_cases"], 1)
        self.assertEqual(open_system["missing_cases"], [])
        self.assertEqual(system_row(results, "opus")["n_cases"], 2)
        self.assertEqual(results["comparisons"][0]["n_paired"], 1)

    def test_matching_staging_sets_do_not_warn(self) -> None:
        records = [
            make_record("opus", "c1"),
            make_record("opus", "c2", outcome="staging_error"),
            make_record("open", "c1", model="open", harness="gary"),
            make_record("open", "c2", model="open", harness="gary", outcome="staging_error"),
        ]
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            score.build_results(records, RATES, baseline_id="opus", generated_at="t")
        self.assertNotIn("headline case sets differ", stderr.getvalue())


class IncludeDevTest(unittest.TestCase):
    def records(self):
        return [make_record("opus", "c1"), make_record("opus", "d1", split="dev")]

    def test_dev_records_are_left_out_by_default(self) -> None:
        results = score.build_results(self.records(), RATES, no_baseline=True, generated_at="t")
        self.assertEqual([c["case_id"] for c in results["cases"]], ["c1"])
        self.assertEqual(results["summary"]["systems"][0]["n_cases"], 1)

    def test_include_dev_counts_dev_records_alongside_scored(self) -> None:
        results = score.build_results(
            self.records(), RATES, no_baseline=True, include_dev=True, generated_at="t"
        )
        self.assertEqual([c["case_id"] for c in results["cases"]], ["c1", "d1"])
        self.assertEqual(results["summary"]["systems"][0]["n_cases"], 2)

    def test_dev_only_run_prints_a_table_with_include_dev(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            runs = Path(tmp) / "runs"
            runs.mkdir()
            record = make_record("opus", "d1", split="dev")
            (runs / "opus-d1-0.json").write_text(json.dumps(record), encoding="utf-8")
            out = Path(tmp) / "out.json"
            stdout = io.StringIO()
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(io.StringIO()):
                code = score.main([str(runs), "-o", str(out), "--no-baseline", "--include-dev"])
            self.assertEqual(code, 0)
            self.assertIn("| OPUS |", stdout.getvalue())
            results = json.loads(out.read_text(encoding="utf-8"))
            self.assertEqual(results["summary"]["systems"][0]["n_cases"], 1)


if __name__ == "__main__":
    unittest.main()
