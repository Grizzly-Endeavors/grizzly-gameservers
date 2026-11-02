#!/usr/bin/env python3
"""Check that the dev and scored case sets are separate and match the scenario catalog.

Fails (exit 1) when any scored case shares an id, a prompt text, or a scenario
plus an identical setup with a dev case, when a case file sits in the wrong
split directory, when a case names a scenario that is not in the catalog, or
when a scenario's dev or scored case count differs from `split` in
scenarios.yaml. Procedure: evals/evals.md section 3.4.

Usage: python3 evals/harness/check_split.py [--cases evals/cases] [--scenarios evals/scenarios.yaml]
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections import Counter
from pathlib import Path

import yaml

EVALS_DIR = Path(__file__).resolve().parents[1]
SPLITS = ("dev", "scored")


def normalize_prompt(text: object) -> str:
    return re.sub(r"\s+", " ", str(text)).strip().lower()


def setup_fingerprint(setup: object) -> str:
    return json.dumps(setup, sort_keys=True, default=str)


def load_cases(cases_dir: Path) -> dict[str, list[dict]]:
    """Read every case file under cases_dir/<split>/, keyed by split directory name."""
    loaded: dict[str, list[dict]] = {}
    for split in SPLITS:
        split_dir = cases_dir / split
        if not split_dir.is_dir():
            raise ValueError(f"missing case directory {split_dir}")
        cases = []
        for path in sorted(split_dir.glob("*.yaml")):
            case = yaml.safe_load(path.read_text(encoding="utf-8"))
            if not isinstance(case, dict):
                raise ValueError(f"case file {path} is not a mapping")
            case["_file"] = path.name
            cases.append(case)
        loaded[split] = cases
    return loaded


def load_expected_counts(scenarios_path: Path) -> dict[str, dict[str, int]]:
    raw = yaml.safe_load(scenarios_path.read_text(encoding="utf-8"))
    expected = {}
    for scenario in raw["scenarios"]:
        split = scenario.get("split") or {}
        expected[scenario["id"]] = {name: int(split.get(name, 0)) for name in SPLITS}
    return expected


def find_problems(cases: dict[str, list[dict]], expected: dict[str, dict[str, int]]) -> list[str]:
    problems: list[str] = []

    for split, split_cases in cases.items():
        for case in split_cases:
            if case.get("split") != split:
                problems.append(f"{case['_file']}: split is {case.get('split')!r} but the file is in {split}/")
            if case.get("scenario") not in expected:
                problems.append(f"{case['_file']}: scenario {case.get('scenario')!r} is not in scenarios.yaml")
        duplicates = [i for i, n in Counter(c.get("id") for c in split_cases).items() if n > 1]
        problems.extend(f"{split}: case id {i!r} is defined more than once" for i in duplicates)

    dev_ids = {c.get("id") for c in cases["dev"]}
    dev_prompts: dict[str, str] = {}
    dev_setups: dict[tuple[str, str], str] = {}
    for case in cases["dev"]:
        dev_prompts.setdefault(normalize_prompt(case.get("prompt", "")), str(case.get("id")))
        key = (str(case.get("scenario")), setup_fingerprint(case.get("setup")))
        dev_setups.setdefault(key, str(case.get("id")))

    for case in cases["scored"]:
        case_id = str(case.get("id"))
        if case_id in dev_ids:
            problems.append(f"scored case {case_id} shares its id with a dev case")
        twin = dev_prompts.get(normalize_prompt(case.get("prompt", "")))
        if twin:
            problems.append(f"scored case {case_id} has the same prompt as dev case {twin}")
        twin = dev_setups.get((str(case.get("scenario")), setup_fingerprint(case.get("setup"))))
        if twin:
            problems.append(f"scored case {case_id} has the same scenario and setup as dev case {twin}")

    for split, split_cases in cases.items():
        found = Counter(c.get("scenario") for c in split_cases)
        for scenario, counts in expected.items():
            if found.get(scenario, 0) != counts[split]:
                problems.append(
                    f"scenario {scenario}: {found.get(scenario, 0)} {split} cases, scenarios.yaml says {counts[split]}"
                )
    return problems


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--cases", type=Path, default=EVALS_DIR / "cases")
    parser.add_argument("--scenarios", type=Path, default=EVALS_DIR / "scenarios.yaml")
    args = parser.parse_args(argv)

    try:
        cases = load_cases(args.cases)
        expected = load_expected_counts(args.scenarios)
    except (OSError, ValueError, KeyError, yaml.YAMLError) as err:
        print(f"error: {err}", file=sys.stderr)
        return 2

    problems = find_problems(cases, expected)
    print(
        f"{len(cases['dev'])} dev cases, {len(cases['scored'])} scored cases, "
        f"{len(expected)} scenarios in the catalog"
    )
    if problems:
        print(f"FAIL: {len(problems)} problem(s)")
        for problem in problems:
            print(f"  - {problem}")
        return 1
    print("OK: zero overlap between dev and scored, and per-scenario counts match scenarios.yaml")
    return 0


if __name__ == "__main__":
    sys.exit(main())
