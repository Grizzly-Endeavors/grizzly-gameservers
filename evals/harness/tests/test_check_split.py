"""Tests for evals/harness/check_split.py: overlap detection and scenario count agreement."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path
from types import ModuleType

CHECK_PATH = Path(__file__).resolve().parents[1] / "check_split.py"


def load_check_module() -> ModuleType:
    spec = importlib.util.spec_from_file_location("check_split", CHECK_PATH)
    if spec is None or spec.loader is None:
        raise ImportError(f"cannot load {CHECK_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


check_split = load_check_module()

EXPECTED = {"mc-access": {"dev": 1, "scored": 1}}


CHECKS = [{"type": "ready"}]


def case(case_id: str, split: str, prompt: str, setup: object, checks: object = CHECKS, scenario: str = "mc-access") -> dict:
    return {
        "id": case_id,
        "scenario": scenario,
        "split": split,
        "prompt": prompt,
        "setup": setup,
        "checks": checks,
        "_file": f"{case_id}.yaml",
    }


def clean_cases() -> dict[str, list[dict]]:
    return {
        "dev": [case("mc-access-d1", "dev", "make Rowan an op", [{"tool": "write_file", "args": {"path": "a"}}])],
        "scored": [case("mc-access-s1", "scored", "can you op Fenwick", [{"tool": "write_file", "args": {"path": "b"}}])],
    }


class FindProblemsTest(unittest.TestCase):
    def test_clean_split_has_no_problems(self) -> None:
        self.assertEqual(check_split.find_problems(clean_cases(), EXPECTED), [])

    def test_shared_id_is_reported(self) -> None:
        cases = clean_cases()
        cases["scored"][0]["id"] = "mc-access-d1"
        problems = check_split.find_problems(cases, EXPECTED)
        self.assertTrue(any("shares its id" in p for p in problems))

    def test_same_prompt_ignoring_case_and_spacing_is_reported(self) -> None:
        cases = clean_cases()
        cases["scored"][0]["prompt"] = "  Make  ROWAN an op "
        problems = check_split.find_problems(cases, EXPECTED)
        self.assertTrue(any("same prompt" in p for p in problems))

    def test_same_scenario_setup_and_checks_is_reported(self) -> None:
        cases = clean_cases()
        cases["scored"][0]["setup"] = [{"args": {"path": "a"}, "tool": "write_file"}]
        cases["scored"][0]["checks"] = [{"type": "ready"}]
        problems = check_split.find_problems(cases, EXPECTED)
        self.assertTrue(any("same scenario, setup and checks" in p for p in problems))

    def test_same_setup_with_different_checks_is_not_overlap(self) -> None:
        cases = clean_cases()
        cases["scored"][0]["setup"] = [{"args": {"path": "a"}, "tool": "write_file"}]
        cases["scored"][0]["checks"] = [{"type": "ready"}, {"type": "pod_exec", "cmd": "true"}]
        self.assertEqual(check_split.find_problems(cases, EXPECTED), [])

    def test_same_checks_with_different_setup_is_not_overlap(self) -> None:
        cases = clean_cases()
        cases["scored"][0]["setup"] = [{"args": {"path": "z"}, "tool": "write_file"}]
        self.assertEqual(check_split.find_problems(cases, EXPECTED), [])

    def test_smoke_scenario_absent_from_catalog_is_allowed_in_dev(self) -> None:
        cases = clean_cases()
        cases["dev"].append(case("smoke-mc-pvp-d1", "dev", "pvp on", [], scenario="smoke-mc-pvp"))
        self.assertEqual(check_split.find_problems(cases, EXPECTED), [])

    def test_smoke_case_in_scored_split_is_reported(self) -> None:
        cases = clean_cases()
        cases["scored"].append(case("smoke-mc-pvp-s1", "scored", "pvp on", [], scenario="smoke-mc-pvp"))
        problems = check_split.find_problems(cases, EXPECTED)
        self.assertTrue(any("smoke case" in p and "scored split" in p for p in problems))

    def test_dropped_scenario_with_no_cases_is_skipped(self) -> None:
        cases = clean_cases()
        expected = {**EXPECTED, "tr-admin": {"dev": 1, "scored": 2}}
        self.assertTrue(any("tr-admin" in p for p in check_split.find_problems(cases, expected)))
        self.assertEqual(check_split.find_problems(cases, EXPECTED, frozenset({"tr-admin"})), [])

    def test_case_naming_dropped_scenario_is_reported(self) -> None:
        cases = clean_cases()
        cases["dev"].append(case("tr-admin-d1", "dev", "make Quillon an admin", [], scenario="tr-admin"))
        problems = check_split.find_problems(cases, EXPECTED, frozenset({"tr-admin"}))
        self.assertTrue(any("is dropped" in p for p in problems))

    def test_count_mismatch_is_reported(self) -> None:
        cases = clean_cases()
        cases["scored"].append(case("mc-access-s2", "scored", "op Dusk", [{"tool": "write_file", "args": {"path": "c"}}]))
        problems = check_split.find_problems(cases, EXPECTED)
        self.assertTrue(any("2 scored cases, scenarios.yaml says 1" in p for p in problems))

    def test_case_in_wrong_split_directory_is_reported(self) -> None:
        cases = clean_cases()
        cases["scored"][0]["split"] = "dev"
        problems = check_split.find_problems(cases, EXPECTED)
        self.assertTrue(any("but the file is in scored/" in p for p in problems))

    def test_unknown_scenario_is_reported(self) -> None:
        cases = clean_cases()
        cases["dev"][0]["scenario"] = "not-a-scenario"
        problems = check_split.find_problems(cases, EXPECTED)
        self.assertTrue(any("not in scenarios.yaml" in p for p in problems))


class LoadCatalogTest(unittest.TestCase):
    def test_dropped_scenarios_are_separated_from_counted_ones(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "scenarios.yaml"
            path.write_text(
                "scenarios:\n"
                "  - id: mc-access\n"
                "    split: { dev: 1, scored: 4 }\n"
                "  - id: tr-admin\n"
                '    dropped: "no way to change TShock groups"\n'
                "    split: { dev: 0, scored: 0 }\n",
                encoding="utf-8",
            )
            expected, dropped = check_split.load_catalog(path)
        self.assertEqual(expected, {"mc-access": {"dev": 1, "scored": 4}})
        self.assertEqual(dropped, {"tr-admin"})


if __name__ == "__main__":
    unittest.main()
