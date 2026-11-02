#!/usr/bin/env python3
"""Score eval run records into eval_results.json and a markdown summary.

Input is a directory of run records, one JSON file per (system, case, repeat).
Scoring rules are in evals/evals.md (sections 5-8); the output shape is
FR-4.3 in docs/fireworks-deploy-prd.md.

Usage: python3 evals/harness/score.py <runs_dir> [-o eval_results.json]
       [--cases <cases_dir>] [--baseline <system_id>] [--no-baseline]
       [--include-dev] [--human-estimate <file.json>]
"""

from __future__ import annotations

import argparse
import json
import math
import random
import statistics
import sys
from collections import defaultdict
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

import yaml

from run_layout import record_paths

RATES_PATH = Path(__file__).resolve().with_name("models.json")
Z_95 = 1.959963984540054
BOOTSTRAP_RESAMPLES = 2000
BOOTSTRAP_SEED = 0

RECORD_FIELDS = (
    "system_id",
    "label",
    "model",
    "harness",
    "case_id",
    "game",
    "task_area",
    "category",
    "prompt",
    "split",
    "repeat",
    "outcome",
    "claimed_done",
    "turns",
    "tool_calls",
    "usage",
    "latency_ms",
    "model_ms",
    "tool_ms",
    "checks",
)
CASE_FIELDS = ("game", "task_area", "category", "prompt")
COUNT_FIELDS = ("turns", "latency_ms", "model_ms", "tool_ms")
USAGE_FIELDS = ("prompt_tokens", "cached_tokens", "completion_tokens")
RATE_FIELDS = ("input_per_1m_usd", "cached_input_per_1m_usd", "output_per_1m_usd")
OUTCOMES = ("pass", "fail", "staging_error", "infra_error")
SPLITS = ("scored", "dev")
HARNESSES = ("gary", "claude_code")
TASK_AREAS = ("config", "troubleshooting")
ESTIMATE_FIELDS = (
    "system_id",
    "label",
    "kind",
    "model",
    "harness",
    "n_cases",
    "verified_success_rate",
    "success_ci_95",
    "unintended_change_rate",
    "mean_turns",
    "mean_tokens",
    "latency_p50_ms",
    "latency_p95_ms",
    "cost_per_1k_usd",
    "notes",
)
ESTIMATE_NUMBER_FIELDS = (
    "n_cases",
    "verified_success_rate",
    "unintended_change_rate",
    "mean_turns",
    "mean_tokens",
    "latency_p50_ms",
    "latency_p95_ms",
    "cost_per_1k_usd",
)


@dataclass(frozen=True)
class Row:
    """One case's result for one system, joined with its case metadata and cost.

    A `missing` row is a scored case the system has no run record for. It counts
    as a fail and carries no measurements, so its measurement fields are None.
    """

    system_id: str
    label: str
    model: str
    harness: str
    case_id: str
    game: str
    task_area: str
    category: str
    prompt: str
    repeat: int
    outcome: str
    claimed_done: bool
    turns: int | None
    tool_calls: tuple[str, ...]
    prompt_tokens: int | None
    cached_tokens: int | None
    completion_tokens: int | None
    latency_ms: int | None
    cost_usd: float | None

    @property
    def measured(self) -> bool:
        return self.outcome != "missing"

    @property
    def success(self) -> bool:
        return self.outcome == "pass"

    @property
    def tokens(self) -> int:
        return self.prompt_tokens + self.completion_tokens


def warn(message: str) -> None:
    print(f"warning: {message}", file=sys.stderr)


def read_structured(path: Path) -> object:
    try:
        text = path.read_text(encoding="utf-8")
        if path.suffix == ".json":
            return json.loads(text)
        return yaml.safe_load(text)
    except (OSError, ValueError, yaml.YAMLError) as err:
        raise ValueError(f"failed to read {path}") from err


def is_whole_number(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


def is_non_negative_number(value: object) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool) and value >= 0


def validate_record(record: object, path: Path) -> dict:
    if not isinstance(record, dict):
        raise ValueError(f"run record at {path} is not an object")
    missing = [field for field in RECORD_FIELDS if field not in record]
    if missing:
        raise ValueError(f"run record at {path} is missing {', '.join(missing)}")
    if record["split"] not in SPLITS:
        raise ValueError(f"run record at {path} has unknown split {record['split']!r}")
    if record["outcome"] not in OUTCOMES:
        raise ValueError(f"run record at {path} has unknown outcome {record['outcome']!r}")
    if record["harness"] not in HARNESSES:
        raise ValueError(f"run record at {path} has unknown harness {record['harness']!r}")
    if record["task_area"] not in TASK_AREAS:
        raise ValueError(f"run record at {path} has unknown task_area {record['task_area']!r}")
    if not is_whole_number(record["repeat"]):
        raise ValueError(f"run record at {path} has a repeat that is not a whole number >= 0")
    for field in COUNT_FIELDS:
        if not is_whole_number(record[field]):
            raise ValueError(f"run record at {path} has a {field} that is not a whole number >= 0")
    if not isinstance(record["claimed_done"], bool):
        raise ValueError(f"run record at {path} has a claimed_done that is not true or false")
    if not isinstance(record["tool_calls"], list):
        raise ValueError(f"run record at {path} has a tool_calls that is not a list")
    usage = record["usage"]
    if not isinstance(usage, dict) or any(field not in usage for field in USAGE_FIELDS):
        raise ValueError(f"run record at {path} has a usage object without {', '.join(USAGE_FIELDS)}")
    for field in USAGE_FIELDS:
        if not is_whole_number(usage[field]):
            raise ValueError(f"run record at {path} has a usage.{field} that is not a whole number >= 0")
    if usage["cached_tokens"] > usage["prompt_tokens"]:
        raise ValueError(f"run record at {path} has more cached tokens than prompt tokens")
    return record


def check_system_identity(loaded: list[tuple[Path, dict]]) -> None:
    """Every record of one system must agree on its label, model and harness."""
    first: dict[str, tuple[Path, dict]] = {}
    for path, record in loaded:
        system_id = record["system_id"]
        if system_id not in first:
            first[system_id] = (path, record)
            continue
        first_path, reference = first[system_id]
        for field in ("label", "model", "harness"):
            if record[field] != reference[field]:
                raise ValueError(
                    f"system {system_id!r} has inconsistent {field}: "
                    f"{reference[field]!r} in {first_path} but {record[field]!r} in {path}"
                )


def load_records(runs_dir: Path) -> list[dict]:
    paths = record_paths(runs_dir)
    if not paths:
        raise ValueError(f"no run records found in {runs_dir}")
    loaded = [(path, validate_record(read_structured(path), path)) for path in paths]
    check_system_identity(loaded)
    return [record for _, record in loaded]


def load_rates(path: Path) -> dict[str, dict[str, float]]:
    raw = read_structured(path)
    if not isinstance(raw, dict):
        raise ValueError(f"rate table at {path} is not an object")
    rates = {}
    for model, entry in raw.items():
        if model.startswith("_"):
            continue
        if not isinstance(entry, dict) or any(not isinstance(entry.get(f), (int, float)) for f in RATE_FIELDS):
            raise ValueError(f"rate table entry {model!r} at {path} needs numeric {', '.join(RATE_FIELDS)}")
        rates[model] = {field: float(entry[field]) for field in RATE_FIELDS}
    return rates


def normalize_case_entry(entry: object) -> object:
    """Map the runner's case-file schema (evals/cases/README.md) onto the scorer's field names.

    A case file names its id `id` and its category `kind`; run.py writes them into
    run records as `case_id` and `category`, so the catalog must resolve the same way.
    """
    if not isinstance(entry, dict):
        return entry
    normalized = dict(entry)
    normalized.setdefault("case_id", entry.get("id"))
    if not normalized.get("category"):
        normalized["category"] = entry.get("kind", "")
    return normalized


def load_catalog(cases_dir: Path) -> dict[str, dict]:
    """Read case files (JSON or YAML; a file holds one case or a list) keyed by case_id."""
    catalog: dict[str, dict] = {}
    paths = sorted(p for p in cases_dir.iterdir() if p.suffix in (".json", ".yaml", ".yml"))
    for path in paths:
        content = read_structured(path)
        entries = content if isinstance(content, list) else [content]
        for raw in entries:
            entry = normalize_case_entry(raw)
            if not isinstance(entry, dict) or not isinstance(entry.get("case_id"), str):
                raise ValueError(f"case file {path} has an entry without a case_id")
            missing = [field for field in CASE_FIELDS if field not in entry]
            if missing:
                raise ValueError(f"case {entry['case_id']!r} in {path} is missing {', '.join(missing)}")
            if entry["case_id"] in catalog:
                raise ValueError(f"case {entry['case_id']!r} is defined more than once in {cases_dir}")
            catalog[entry["case_id"]] = entry
    if not catalog:
        raise ValueError(f"no case files found in {cases_dir}")
    return catalog


def load_estimate(path: Path) -> dict:
    """Read the human estimate row: one JSON object in the FR-4.3 summary.systems shape."""
    raw = read_structured(path)
    if not isinstance(raw, dict):
        raise ValueError(f"human estimate at {path} is not an object")
    missing = [field for field in ESTIMATE_FIELDS if field not in raw]
    if missing:
        raise ValueError(f"human estimate at {path} is missing {', '.join(missing)}")
    if raw["kind"] != "estimated":
        raise ValueError(f"human estimate at {path} has kind {raw['kind']!r}; expected 'estimated'")
    if raw["harness"] != "human":
        raise ValueError(f"human estimate at {path} has harness {raw['harness']!r}; expected 'human'")
    for field in ESTIMATE_NUMBER_FIELDS:
        value = raw[field]
        if value is not None and not is_non_negative_number(value):
            raise ValueError(f"human estimate at {path} has a {field} that is not a number >= 0 or null")
    ci = raw["success_ci_95"]
    if ci is not None and not (isinstance(ci, list) and len(ci) == 2 and all(is_non_negative_number(x) for x in ci)):
        raise ValueError(f"human estimate at {path} has a success_ci_95 that is not [low, high] or null")
    return dict(raw)


def wilson_interval(successes: int, n: int) -> tuple[float, float] | None:
    if n == 0:
        return None
    p = successes / n
    z2 = Z_95 * Z_95
    denom = 1 + z2 / n
    centre = (p + z2 / (2 * n)) / denom
    half = Z_95 * math.sqrt(p * (1 - p) / n + z2 / (4 * n * n)) / denom
    return (max(0.0, centre - half), min(1.0, centre + half))


def mcnemar_exact_p(b: int, c: int) -> float:
    """Two-sided exact McNemar test on discordant counts b and c."""
    n = b + c
    if n == 0:
        return 1.0
    tail = sum(math.comb(n, i) for i in range(min(b, c) + 1))
    return min(1.0, 2 * tail / 2**n)


def mean(values: list[float]) -> float | None:
    if not values:
        return None
    return sum(values) / len(values)


def nearest_rank(values: list[float], pct: int) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    rank = max(1, math.ceil(pct * len(ordered) / 100))
    return ordered[rank - 1]


def percentile(ordered: list[float], q: float) -> float:
    pos = (len(ordered) - 1) * q / 100
    lo = math.floor(pos)
    hi = math.ceil(pos)
    return ordered[lo] + (ordered[hi] - ordered[lo]) * (pos - lo)


def cost_usd(usage: dict, rate: dict[str, float] | None) -> float | None:
    if rate is None:
        return None
    uncached = usage["prompt_tokens"] - usage["cached_tokens"]
    total = (
        uncached * rate["input_per_1m_usd"]
        + usage["cached_tokens"] * rate["cached_input_per_1m_usd"]
        + usage["completion_tokens"] * rate["output_per_1m_usd"]
    )
    return total / 1_000_000


def bootstrap_ratio_ci(pairs: list[tuple[float, float]]) -> list[float] | None:
    """Paired bootstrap 95% percentile interval for mean(a) / mean(b)."""
    n = len(pairs)
    rng = random.Random(BOOTSTRAP_SEED)
    ratios = []
    for _ in range(BOOTSTRAP_RESAMPLES):
        sample = [pairs[rng.randrange(n)] for _ in range(n)]
        denom = sum(b for _, b in sample)
        if denom == 0:
            continue
        ratios.append(sum(a for a, _ in sample) / denom)
    if not ratios:
        return None
    ratios.sort()
    return [percentile(ratios, 2.5), percentile(ratios, 97.5)]


def ratio_summary(pairs: list[tuple[float | None, float | None]]) -> dict | None:
    if not pairs or any(a is None or b is None for a, b in pairs):
        return None
    complete = [(float(a), float(b)) for a, b in pairs]
    denom = sum(b for _, b in complete)
    if denom == 0:
        return {"point": None, "ci_95": None}
    return {"point": sum(a for a, _ in complete) / denom, "ci_95": bootstrap_ratio_ci(complete)}


def unique_in_order(values) -> list[str]:
    return list(dict.fromkeys(values))


def breakdown(rows: list[Row], attr: str) -> dict[str, dict]:
    groups: dict[str, list[Row]] = defaultdict(list)
    for row in rows:
        groups[getattr(row, attr)].append(row)
    return {
        key: {"n": len(group), "success_rate": sum(r.success for r in group) / len(group)}
        for key, group in sorted(groups.items())
    }


def missing_row(identity: Row, case_id: str, meta: dict) -> Row:
    """A scored case with no run record for this system: a fail with nothing measured."""
    return Row(
        system_id=identity.system_id,
        label=identity.label,
        model=identity.model,
        harness=identity.harness,
        case_id=case_id,
        game=meta["game"],
        task_area=meta["task_area"],
        category=meta["category"],
        prompt=meta["prompt"],
        repeat=0,
        outcome="missing",
        claimed_done=False,
        turns=None,
        tool_calls=(),
        prompt_tokens=None,
        cached_tokens=None,
        completion_tokens=None,
        latency_ms=None,
        cost_usd=None,
    )


def summarize_system(headline: list[Row], scored: list[Row], staged_cases: int) -> dict:
    """Headline measures for one system.

    `headline` has one row per case the system was scored on, missing cases included.
    `scored` is every scored record of the system, for identity and error counts.
    """
    identity = scored[0]
    n = len(headline)
    successes = sum(r.success for r in headline)
    ci = wilson_interval(successes, n)
    measured = [r for r in headline if r.measured]
    costs = [r.cost_usd for r in measured]
    mean_cost = None if None in costs else mean(costs)
    latencies = [r.latency_ms for r in measured]
    missing = [r.case_id for r in headline if not r.measured]
    notes = ["Unintended changes are not measured."]
    if missing:
        notes.append(
            f"{len(missing)} case(s) have no run record and count as fails; "
            "turns, tokens, latency and cost use the measured cases only."
        )
    return {
        "system_id": identity.system_id,
        "label": identity.label,
        "kind": "measured",
        "model": identity.model,
        "harness": identity.harness,
        "n_cases": n,
        "staged_cases": staged_cases,
        "missing_cases": missing,
        "verified_success_rate": successes / n if n else None,
        "success_ci_95": list(ci) if ci else None,
        "unintended_change_rate": None,
        "mean_turns": mean([r.turns for r in measured]),
        "mean_tokens": mean([r.tokens for r in measured]),
        "latency_p50_ms": nearest_rank(latencies, 50),
        "latency_p95_ms": nearest_rank(latencies, 95),
        "cost_per_1k_usd": mean_cost * 1000 if mean_cost is not None else None,
        "notes": " ".join(notes),
        "mean_cost_usd": mean_cost,
        "false_completion_rate": (
            sum(1 for r in measured if r.claimed_done and r.outcome == "fail") / len(measured)
            if measured
            else None
        ),
        "infra_error_count": sum(r.outcome == "infra_error" for r in scored),
        "staging_error_count": sum(r.outcome == "staging_error" for r in scored),
        "by_game": breakdown(headline, "game"),
        "by_category": breakdown(headline, "category"),
    }


def compare_to_baseline(
    system_id: str,
    baseline_id: str,
    headline_by_case: dict[str, dict[str, Row]],
) -> dict:
    """Compare on the shared cases. A missing case is a fail on the side that lacks it."""
    system_rows = headline_by_case[system_id]
    baseline_rows = headline_by_case[baseline_id]
    shared = sorted(set(system_rows) & set(baseline_rows))
    pairs = [(system_rows[c], baseline_rows[c]) for c in shared]
    n = len(pairs)
    success_difference = None
    mcnemar_p = None
    if n:
        system_passes = sum(s.success for s, _ in pairs)
        baseline_passes = sum(b.success for _, b in pairs)
        success_difference = (system_passes - baseline_passes) / n
        only_system = sum(1 for s, b in pairs if s.success and not b.success)
        only_baseline = sum(1 for s, b in pairs if b.success and not s.success)
        mcnemar_p = mcnemar_exact_p(only_system, only_baseline)
    both_measured = [(s, b) for s, b in pairs if s.measured and b.measured]
    return {
        "system_id": system_id,
        "baseline_system_id": baseline_id,
        "n_paired": n,
        "n_resource_paired": len(both_measured),
        "success_difference": success_difference,
        "mcnemar_p_two_sided": mcnemar_p,
        "cost_ratio": ratio_summary([(s.cost_usd, b.cost_usd) for s, b in both_measured]),
        "latency_ratio": ratio_summary([(s.latency_ms, b.latency_ms) for s, b in both_measured]),
    }


def variance_for_system(rows: list[Row]) -> dict:
    """Success rate per repeat index, over the cases that were run more than once."""
    by_case: dict[str, dict[int, Row]] = defaultdict(dict)
    for row in rows:
        by_case[row.case_id][row.repeat] = row
    multi = {case_id: reps for case_id, reps in by_case.items() if len(reps) > 1}
    rate_by_repeat: dict[str, float] = {}
    for repeat in sorted({rep for reps in multi.values() for rep in reps}):
        in_repeat = [reps[repeat] for reps in multi.values() if repeat in reps]
        rate_by_repeat[str(repeat)] = sum(r.success for r in in_repeat) / len(in_repeat)
    rates = list(rate_by_repeat.values())
    return {
        "case_ids": sorted(multi),
        "success_rate_by_repeat": rate_by_repeat,
        "mean_success_rate": mean(rates),
        "stdev_success_rate": statistics.stdev(rates) if len(rates) > 1 else None,
    }


def resolve_case(record: dict, catalog: dict[str, dict] | None) -> dict:
    """Case metadata from the catalog when one is given, else from the record."""
    fields = {field: record[field] for field in CASE_FIELDS}
    if catalog is None:
        return fields
    entry = catalog.get(record["case_id"])
    if entry is None:
        raise ValueError(
            f"case {record['case_id']!r} of system {record['system_id']!r} is not in the case catalog"
        )
    for field in CASE_FIELDS:
        if record[field] != entry[field]:
            warn(f"case {record['case_id']!r} field {field} differs from the case catalog; using the catalog")
    return {field: entry[field] for field in CASE_FIELDS}


def case_result(row: Row) -> dict:
    return {
        "outcome": row.outcome,
        "verified_success": row.success,
        "unintended_changes": None,
        "turns": row.turns,
        "latency_ms": row.latency_ms,
        "cost_usd": row.cost_usd,
        "tool_calls": list(row.tool_calls),
    }


def build_results(
    records: list[dict],
    rates: dict[str, dict[str, float]],
    catalog: dict[str, dict] | None = None,
    baseline_id: str | None = None,
    generated_at: str | None = None,
    estimate: dict | None = None,
    no_baseline: bool = False,
    include_dev: bool = False,
) -> dict:
    seen: set[tuple[str, str, int]] = set()
    for record in records:
        key = (record["system_id"], record["case_id"], record["repeat"])
        if key in seen:
            raise ValueError(f"more than one run record for system {key[0]!r}, case {key[1]!r}, repeat {key[2]}")
        seen.add(key)

    counted_splits = ("scored", "dev") if include_dev else ("scored",)
    scored_records = [r for r in records if r["split"] in counted_splits]
    priced_models = {r["model"] for r in scored_records if r["outcome"] != "staging_error"}
    for model in sorted(priced_models - rates.keys()):
        warn(f"no rate for model {model!r} in models.json; its cost is reported as null")
    for model in sorted(m for m in priced_models & rates.keys() if not any(rates[m].values())):
        warn(f"every rate for model {model!r} is zero in models.json; its cost is reported as 0")

    case_meta: dict[str, dict] = {}
    if catalog is not None:
        case_meta = {case_id: {f: entry[f] for f in CASE_FIELDS} for case_id, entry in catalog.items()}
    scored: list[Row] = []
    for record in scored_records:
        meta = resolve_case(record, catalog)
        case_meta.setdefault(record["case_id"], meta)
        usage = record["usage"]
        scored.append(
            Row(
                system_id=record["system_id"],
                label=record["label"],
                model=record["model"],
                harness=record["harness"],
                case_id=record["case_id"],
                game=meta["game"],
                task_area=meta["task_area"],
                category=meta["category"],
                prompt=meta["prompt"],
                repeat=record["repeat"],
                outcome=record["outcome"],
                claimed_done=record["claimed_done"],
                turns=record["turns"],
                tool_calls=tuple(record["tool_calls"]),
                prompt_tokens=usage["prompt_tokens"],
                cached_tokens=usage["cached_tokens"],
                completion_tokens=usage["completion_tokens"],
                latency_ms=record["latency_ms"],
                cost_usd=cost_usd(usage, rates.get(record["model"])),
            )
        )

    # The headline run of each (system, case) is its first repeat, whatever the runner numbers it from.
    first_repeat: dict[tuple[str, str], Row] = {}
    for r in scored:
        key = (r.system_id, r.case_id)
        if key not in first_repeat or r.repeat < first_repeat[key].repeat:
            first_repeat[key] = r
    # The scored case set: the catalog when one is given, else every case any system ran.
    expected = sorted(catalog) if catalog is not None else sorted({case_id for _, case_id in first_repeat})
    system_order = unique_in_order(r.system_id for r in scored)
    scored_by_system = {sid: [r for r in scored if r.system_id == sid] for sid in system_order}

    headline_by_case: dict[str, dict[str, Row]] = {}
    for sid in system_order:
        identity = scored_by_system[sid][0]
        staged: dict[str, Row] = {}
        for case_id in expected:
            row = first_repeat.get((sid, case_id))
            if row is None:
                staged[case_id] = missing_row(identity, case_id, case_meta[case_id])
            elif row.outcome != "staging_error":
                staged[case_id] = row
        headline_by_case[sid] = staged

    for sid in system_order:
        missing_ids = [case_id for case_id, row in headline_by_case[sid].items() if not row.measured]
        if missing_ids:
            warn(
                f"system {sid!r} has no run record for {len(missing_ids)} scored case(s), "
                f"counted as fails: {', '.join(missing_ids)}"
            )
    staging_excluded = {
        sid: sorted(set(expected) - set(headline_by_case[sid])) for sid in system_order
    }
    if len({tuple(ids) for ids in staging_excluded.values()}) > 1:
        differences = "; ".join(
            f"{sid!r} excludes {', '.join(ids) or 'none'}" for sid, ids in staging_excluded.items()
        )
        warn(f"headline case sets differ across systems because of staging errors: {differences}")

    if baseline_id is None:
        baseline_id = next(
            (
                sid
                for sid in system_order
                if scored_by_system[sid][0].harness == "claude_code"
                and "opus" in scored_by_system[sid][0].model.lower()
            ),
            None,
        )
        if baseline_id is None and not no_baseline:
            raise ValueError(
                "no baseline system found (looked for harness claude_code with an opus model); "
                "pass --baseline <system_id> or --no-baseline"
            )
    elif baseline_id not in system_order:
        raise ValueError(f"baseline system {baseline_id!r} has no scored run records")

    if estimate is not None and estimate["system_id"] in system_order:
        raise ValueError(f"human estimate system_id {estimate['system_id']!r} collides with a measured system")

    cases = []
    for case_id in expected:
        results = {
            sid: headline_by_case[sid][case_id] for sid in system_order if case_id in headline_by_case[sid]
        }
        if not results:
            continue
        meta = case_meta[case_id]
        cases.append(
            {
                "case_id": case_id,
                "game": meta["game"],
                "task_area": meta["task_area"],
                "category": meta["category"],
                "prompt": meta["prompt"],
                "results_by_system": {sid: case_result(row) for sid, row in results.items()},
            }
        )

    comparisons = (
        [
            compare_to_baseline(sid, baseline_id, headline_by_case)
            for sid in system_order
            if sid != baseline_id
        ]
        if baseline_id is not None
        else []
    )

    systems = [
        summarize_system(list(headline_by_case[sid].values()), scored_by_system[sid], len(expected))
        for sid in system_order
    ]
    if estimate is not None:
        systems.append(dict(estimate))

    excluded = [
        {"system_id": r.system_id, "case_id": r.case_id, "repeat": r.repeat, "reason": "staging_error"}
        for r in scored
        if r.outcome == "staging_error"
    ]
    excluded.sort(key=lambda e: (e["system_id"], e["case_id"], e["repeat"]))

    valid = [r for r in scored if r.outcome != "staging_error"]
    return {
        "generated_at": generated_at or datetime.now(timezone.utc).isoformat(),
        "summary": {"systems": systems},
        "cases": cases,
        "comparisons": comparisons,
        "variance": {
            sid: variance_for_system([r for r in valid if r.system_id == sid]) for sid in system_order
        },
        "excluded": excluded,
    }


def fmt_pct(value: float | None) -> str:
    return "n/a" if value is None else f"{value * 100:.1f}%"


def fmt_num(value: float | None, digits: int = 0) -> str:
    return "n/a" if value is None else f"{value:.{digits}f}"


def fmt_ratio(summary: dict | None) -> str:
    if summary is None or summary["point"] is None:
        return "n/a"
    ci = summary["ci_95"]
    if ci is None:
        return f"{summary['point']:.2f}"
    return f"{summary['point']:.2f} [{ci[0]:.2f}, {ci[1]:.2f}]"


def render_markdown(results: dict) -> str:
    lines = [
        "| System | Harness | Model | n | Success (95% CI) | Mean turns | Mean tokens "
        "| p50 ms | p95 ms | USD / 1k | False completion | Infra errors | Staging errors |",
        "|---|---|---|---|---|---|---|---|---|---|---|---|---|",
    ]
    for s in results["summary"]["systems"]:
        ci = s["success_ci_95"]
        ci_text = "n/a" if ci is None else f"[{fmt_pct(ci[0])}, {fmt_pct(ci[1])}]"
        label = f"{s['label']} (estimated)" if s["kind"] == "estimated" else s["label"]
        lines.append(
            f"| {label} | {s['harness']} | {s['model']} | {fmt_num(s['n_cases'])} "
            f"| {fmt_pct(s['verified_success_rate'])} {ci_text} "
            f"| {fmt_num(s['mean_turns'], 1)} | {fmt_num(s['mean_tokens'])} "
            f"| {fmt_num(s['latency_p50_ms'])} | {fmt_num(s['latency_p95_ms'])} "
            f"| {fmt_num(s['cost_per_1k_usd'], 2)} | {fmt_pct(s.get('false_completion_rate'))} "
            f"| {s.get('infra_error_count', 'n/a')} | {s.get('staging_error_count', 'n/a')} |"
        )
    if results["comparisons"]:
        lines += [
            "",
            "Compared with the baseline (ratios are system / baseline; cost and latency show 95% CI).",
            "",
            "| System | Baseline | Paired n | Success difference | McNemar p | Cost ratio | Latency ratio |",
            "|---|---|---|---|---|---|---|",
        ]
        for c in results["comparisons"]:
            diff = c["success_difference"]
            p = c["mcnemar_p_two_sided"]
            lines.append(
                f"| {c['system_id']} | {c['baseline_system_id']} | {c['n_paired']} "
                f"| {'n/a' if diff is None else f'{diff * 100:+.1f} pts'} "
                f"| {'n/a' if p is None else f'{p:.3f}'} "
                f"| {fmt_ratio(c['cost_ratio'])} | {fmt_ratio(c['latency_ratio'])} |"
            )
    missing_total = sum(len(s.get("missing_cases", [])) for s in results["summary"]["systems"])
    lines.append("")
    lines.append(f"Missing run records, counted as fails: {missing_total}.")
    lines.append(f"Staging errors excluded from their system's denominator: {len(results['excluded'])}.")
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("runs_dir", type=Path, help="directory of run record JSON files")
    parser.add_argument("-o", "--output", type=Path, default=Path("eval_results.json"))
    parser.add_argument("--cases", dest="cases_dir", type=Path, help="directory of case files; metadata comes from it")
    parser.add_argument("--baseline", help="system_id to compare against (default: first Claude Code Opus system)")
    parser.add_argument(
        "--no-baseline",
        action="store_true",
        help="allow a run with no baseline; the comparisons section is left empty",
    )
    parser.add_argument(
        "--include-dev",
        action="store_true",
        help="also summarize dev-split records (smoke and dev runs); scored-only by default",
    )
    parser.add_argument(
        "--human-estimate",
        type=Path,
        help="JSON file holding the kind-estimated human row, appended to summary.systems",
    )
    args = parser.parse_args(argv)

    try:
        records = load_records(args.runs_dir)
        rates = load_rates(RATES_PATH)
        catalog = load_catalog(args.cases_dir) if args.cases_dir else None
        estimate = load_estimate(args.human_estimate) if args.human_estimate else None
        results = build_results(
            records,
            rates,
            catalog=catalog,
            baseline_id=args.baseline,
            estimate=estimate,
            no_baseline=args.no_baseline,
            include_dev=args.include_dev,
        )
    except ValueError as err:
        print(f"error: {err}", file=sys.stderr)
        return 1

    args.output.write_text(json.dumps(results, indent=2) + "\n", encoding="utf-8")
    print(render_markdown(results))
    return 0


if __name__ == "__main__":
    sys.exit(main())
