#!/usr/bin/env python3
"""Check that the dev and scored case sets are separate and match the scenario catalog.

Fails (exit 1) when any scored case shares an id or a prompt text with a dev
case, or shares its scenario, setup and checks with one (the ask is what
separates two cases on the same healthy baseline, so identical setup alone is
not overlap), when a case file sits in the wrong split directory, when a case
names a scenario that is not in the catalog, when a scenario marked `dropped`
still has cases, when a scenario's dev or scored case count differs from
`split` in scenarios.yaml, or when a `smoke-` case appears in the scored split.
Procedure: evals/evals.md section 3.4.

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
SMOKE_PREFIX = "smoke-"


def normalize_prompt(text: object) -> str:
    return re.sub(r"\s+", " ", str(text)).strip().lower()


def fingerprint(value: object) -> str:
    return json.dumps(value, sort_keys=True, default=str)


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


def load_catalog(scenarios_path: Path) -> tuple[dict[str, dict[str, int]], set[str]]:
    """Return the expected per-split counts and the ids of dropped scenarios."""
    raw = yaml.safe_load(scenarios_path.read_text(encoding="utf-8"))
    expected: dict[str, dict[str, int]] = {}
    dropped: set[str] = set()
    for scenario in raw["scenarios"]:
        if scenario.get("dropped"):
            dropped.add(scenario["id"])
            continue
        split = scenario.get("split") or {}
        expected[scenario["id"]] = {name: int(split.get(name, 0)) for name in SPLITS}
    return expected, dropped


def find_problems(
    cases: dict[str, list[dict]],
    expected: dict[str, dict[str, int]],
    dropped: frozenset[str] = frozenset(),
) -> list[str]:
    problems: list[str] = []

    for split, split_cases in cases.items():
        for case in split_cases:
            scenario = str(case.get("scenario"))
            if case.get("split") != split:
                problems.append(f"{case['_file']}: split is {case.get('split')!r} but the file is in {split}/")
            if scenario in dropped:
                problems.append(f"{case['_file']}: scenario {scenario} is dropped in scenarios.yaml")
            elif scenario not in expected and not scenario.startswith(SMOKE_PREFIX):
                problems.append(f"{case['_file']}: scenario {scenario!r} is not in scenarios.yaml")
            elif scenario.startswith(SMOKE_PREFIX) and split == "scored":
                problems.append(f"{case['_file']}: smoke case {case.get('id')!r} is in the scored split")
        duplicates = [i for i, n in Counter(c.get("id") for c in split_cases).items() if n > 1]
        problems.extend(f"{split}: case id {i!r} is defined more than once" for i in duplicates)

    dev_ids = {c.get("id") for c in cases["dev"]}
    dev_prompts: dict[str, str] = {}
    dev_asks: dict[tuple[str, str, str], str] = {}
    for case in cases["dev"]:
        dev_prompts.setdefault(normalize_prompt(case.get("prompt", "")), str(case.get("id")))
        key = (str(case.get("scenario")), fingerprint(case.get("setup")), fingerprint(case.get("checks")))
        dev_asks.setdefault(key, str(case.get("id")))

    for case in cases["scored"]:
        case_id = str(case.get("id"))
        if case_id in dev_ids:
            problems.append(f"scored case {case_id} shares its id with a dev case")
        twin = dev_prompts.get(normalize_prompt(case.get("prompt", "")))
        if twin:
            problems.append(f"scored case {case_id} has the same prompt as dev case {twin}")
        twin = dev_asks.get(
            (str(case.get("scenario")), fingerprint(case.get("setup")), fingerprint(case.get("checks")))
        )
        if twin:
            problems.append(f"scored case {case_id} has the same scenario, setup and checks as dev case {twin}")

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
        expected, dropped = load_catalog(args.scenarios)
    except (OSError, ValueError, KeyError, yaml.YAMLError) as err:
        print(f"error: {err}", file=sys.stderr)
        return 2

    problems = find_problems(cases, expected, frozenset(dropped))
    print(
        f"{len(cases['dev'])} dev cases, {len(cases['scored'])} scored cases, "
        f"{len(expected)} scenarios in the catalog, {len(dropped)} dropped"
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
