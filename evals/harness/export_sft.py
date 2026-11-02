#!/usr/bin/env python3
"""Export Gary dev-case turns as Fireworks fine-tuning JSONL.

Each line is one {"messages": [...], "tools": [...]} record: the chat messages the
bot exchanged during a turn (reasoning_content kept on assistant messages) and the
tools array offered. Only dev-split Gary records are exported. Any scored-split
record in the inputs aborts the export before anything is written, because scored
cases must never become training data.

Usage: python3 evals/harness/export_sft.py <runs_dir>... -o train.jsonl [--only-pass]
"""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))

from operator_conn import OperatorConnError
from scrub import dumps_scrubbed, known_ids

SCORED = "scored"
DEV = "dev"
GARY = "gary"
PASS = "pass"
UNAVAILABLE = "unavailable"
REFUSAL_SAMPLE = 5


class ExportError(Exception):
    pass


@dataclass
class Tally:
    records: int = 0
    gary_dev: int = 0
    selected: int = 0
    exported: int = 0
    skipped: dict[str, int] = field(default_factory=dict)

    def skip(self, reason: str) -> None:
        self.skipped[reason] = self.skipped.get(reason, 0) + 1


def load_records(runs_dirs: list[Path]) -> list[tuple[Path, dict[str, Any]]]:
    """Every run record directly inside the given runs directories, in path order."""
    found: list[tuple[Path, dict[str, Any]]] = []
    for runs_dir in runs_dirs:
        if not runs_dir.is_dir():
            raise ExportError(f"runs directory not found: {runs_dir}")
        for path in sorted(runs_dir.glob("*.json")):
            try:
                record = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError) as err:
                raise ExportError(f"cannot read run record {path}: {err}") from err
            if not isinstance(record, dict):
                raise ExportError(f"run record {path} is not a JSON object")
            found.append((path, record))
    return found


def scored_paths(records: list[tuple[Path, dict[str, Any]]]) -> list[Path]:
    return [path for path, record in records if record.get("split") == SCORED]


def load_conversation(path: Path, record: dict[str, Any]) -> dict[str, Any] | None:
    """The turn's {messages, tools} from the runs' transcripts directory, or None when there is none."""
    notes = record.get("notes") or {}
    if notes.get("transcript") == UNAVAILABLE:
        return None
    messages_path = path.parent / "transcripts" / f"{path.stem}.messages.json"
    if not messages_path.exists():
        return None
    try:
        conversation = json.loads(messages_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as err:
        raise ExportError(f"cannot read transcript {messages_path}: {err}") from err
    if not (
        isinstance(conversation, dict)
        and isinstance(conversation.get("messages"), list)
        and isinstance(conversation.get("tools"), list)
    ):
        raise ExportError(f"transcript {messages_path} lacks a messages list and a tools list")
    return conversation


def export(
    records: list[tuple[Path, dict[str, Any]]],
    out: Path,
    only_pass: bool,
    known: frozenset[str] = frozenset(),
) -> Tally:
    tally = Tally(records=len(records))
    lines: list[str] = []
    for path, record in records:
        if record.get("harness") != GARY or record.get("split", DEV) != DEV:
            continue
        tally.gary_dev += 1
        if only_pass and record.get("outcome") != PASS:
            tally.skip("not a pass")
            continue
        tally.selected += 1
        conversation = load_conversation(path, record)
        if conversation is None:
            tally.skip("no transcript (turn unavailable or never reached the bot)")
            continue
        lines.append(dumps_scrubbed(conversation, known, ensure_ascii=False))
    out.write_text("".join(f"{line}\n" for line in lines), encoding="utf-8")
    tally.exported = len(lines)
    return tally


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("runs_dirs", nargs="+", type=Path)
    parser.add_argument("-o", "--out", required=True, type=Path)
    parser.add_argument("--only-pass", action="store_true", help="export only records whose outcome is pass")
    args = parser.parse_args(argv)

    try:
        records = load_records(args.runs_dirs)
    except ExportError as err:
        print(f"export_sft: {err}", file=sys.stderr)
        return 2

    scored = scored_paths(records)
    if scored:
        sample = "\n".join(f"  {path}" for path in scored[:REFUSAL_SAMPLE])
        print(
            f"export_sft: refusing to export: {len(scored)} scored-split record(s) in the inputs; "
            f"scored cases must never become training data:\n{sample}",
            file=sys.stderr,
        )
        return 2

    try:
        known = known_ids()
        tally = export(records, args.out, args.only_pass, known)
    except (ExportError, OperatorConnError) as err:
        print(f"export_sft: {err}", file=sys.stderr)
        return 1

    print(f"read {tally.records} records; {tally.gary_dev} gary dev records; {tally.selected} selected")
    print(f"exported {tally.exported} turns to {args.out}")
    for reason, count in sorted(tally.skipped.items()):
        print(f"skipped {count}: {reason}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
