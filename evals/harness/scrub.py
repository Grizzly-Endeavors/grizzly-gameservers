#!/usr/bin/env python3
"""Scrub Discord identifiers out of eval outputs before they are written or committed.

Tool results and model replies carry real snowflakes (the restarting user, the channel,
the guild). Two passes remove them: every id the operator's .env names is replaced with
<discord-id>, and any other standalone 17-20 digit integer that decodes as a plausible
Discord snowflake is replaced too. Steam ids (17 digits starting 7656119) and floats are
left alone, as is anything outside the snowflake window.

Usage: python3 evals/harness/scrub.py <dir-or-file>... [--check]
Without --check the files are rewritten in place. With --check nothing is written and the
exit status is 1 when any file would change, so a pre-commit hook can refuse a leak.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import time
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))

from operator_conn import find_env_file, read_env_file

PLACEHOLDER = "<discord-id>"
DISCORD_EPOCH_MS = 1420070400000
EARLIEST_MS = DISCORD_EPOCH_MS
FUTURE_SLACK_MS = 24 * 60 * 60 * 1000
U64_LIMIT = 2**64
STEAM_PREFIX = "7656119"
SCRUBBED_SUFFIXES = (".json", ".jsonl", ".md")
KNOWN_ID_KEYS = ("GAMESERVERS_ADMIN_USER_IDS", "GAMESERVERS_OPERATOR_CHANNEL", "DISCORD_GUILD_ID", "GAMESERVERS_EVAL_CHANNELS")

# A digit run standing alone: not part of a longer number, and not the fraction of a float
# (`1.1384...`) or the integer part of one (`1384....5`). A sentence-ending period is fine.
CANDIDATE = re.compile(r"(?<!\d)(?<!\d\.)(\d{17,20})(?!\d)(?!\.\d)")


def _is_snowflake(value: int, now_ms: int) -> bool:
    if not 0 <= value < U64_LIMIT:
        return False
    created_ms = (value >> 22) + DISCORD_EPOCH_MS
    return EARLIEST_MS <= created_ms <= now_ms + FUTURE_SLACK_MS


def _is_steam_id(digits: str) -> bool:
    return len(digits) == 17 and digits.startswith(STEAM_PREFIX)


def _scrub_string(text: str, known_ids: frozenset[str]) -> tuple[str, int]:
    count = 0
    now_ms = int(time.time() * 1000)

    def replace_candidate(match: re.Match[str]) -> str:
        nonlocal count
        digits = match.group(1)
        if _is_steam_id(digits) or not _is_snowflake(int(digits), now_ms):
            return digits
        count += 1
        return PLACEHOLDER

    for known in known_ids:
        pattern = re.compile(rf"(?<!\d){re.escape(known)}(?!\d)")
        text, n = pattern.subn(PLACEHOLDER, text)
        count += n
    text = CANDIDATE.sub(replace_candidate, text)
    return text, count


def scrub_text(text: str, known_ids: frozenset[str]) -> str:
    """Return text with every known id and every snowflake-shaped integer replaced."""
    return _scrub_string(text, known_ids)[0]


def scrub_value(obj: Any, known_ids: frozenset[str]) -> tuple[Any, int]:
    """Scrub every string, dict key and integer inside a decoded JSON value.

    Working on the decoded value instead of the serialized text keeps the output valid
    JSON: a bare snowflake integer becomes a string placeholder rather than a bare token.
    """
    if isinstance(obj, str):
        return _scrub_string(obj, known_ids)
    if isinstance(obj, bool) or obj is None or isinstance(obj, float):
        return obj, 0
    if isinstance(obj, int):
        scrubbed, count = _scrub_string(str(obj), known_ids)
        if count == 0:
            return obj, 0
        return scrubbed, count
    if isinstance(obj, dict):
        total = 0
        out: dict[Any, Any] = {}
        for key, value in obj.items():
            new_key, key_count = scrub_value(key, known_ids)
            new_value, value_count = scrub_value(value, known_ids)
            out[new_key] = new_value
            total += key_count + value_count
        return out, total
    if isinstance(obj, (list, tuple)):
        total = 0
        items: list[Any] = []
        for item in obj:
            new_item, item_count = scrub_value(item, known_ids)
            items.append(new_item)
            total += item_count
        return items, total
    return obj, 0


def dumps_scrubbed(obj: Any, known_ids: frozenset[str], **dump_kwargs: Any) -> str:
    """json.dumps with every id scrubbed out of the value first."""
    scrubbed, _ = scrub_value(obj, known_ids)
    return json.dumps(scrubbed, **dump_kwargs)


def known_ids(env_file: Path | None = None) -> frozenset[str]:
    """Discord ids named by the operator's .env, read into memory and never printed.

    A missing .env yields no known ids; the snowflake pass still runs. A value that is not
    numeric is skipped with a warning that names only the key.
    """
    path = env_file or find_env_file()
    if not path.exists():
        print(f"scrub: no .env at {path}; only snowflake-shaped ids will be scrubbed", file=sys.stderr)
    values = read_env_file(path)
    ids: set[str] = set()
    for key in KNOWN_ID_KEYS:
        for part in re.split(r"[\s,]+", values.get(key, "")):
            if not part:
                continue
            if part.isdigit():
                ids.add(part)
            else:
                print(f"scrub: ignoring non-numeric value in {key}", file=sys.stderr)
    return frozenset(ids)


def _scrub_document(text: str, suffix: str, known: frozenset[str]) -> tuple[str, int]:
    """Scrub one file's text, keeping its line and JSON structure."""
    if suffix == ".json":
        try:
            decoded = json.loads(text)
        except ValueError:
            return _scrub_string(text, known)
        scrubbed, count = scrub_value(decoded, known)
        if count == 0:
            return text, 0
        rendered = json.dumps(scrubbed, indent=2)
        return (rendered + "\n" if text.endswith("\n") else rendered), count
    if suffix == ".jsonl":
        out_lines: list[str] = []
        total = 0
        for line in text.splitlines():
            if not line.strip():
                out_lines.append(line)
                continue
            try:
                decoded = json.loads(line)
            except ValueError:
                new_line, n = _scrub_string(line, known)
            else:
                scrubbed, n = scrub_value(decoded, known)
                new_line = json.dumps(scrubbed, ensure_ascii=False) if n else line
            out_lines.append(new_line)
            total += n
        if total == 0:
            return text, 0
        return "\n".join(out_lines) + ("\n" if text.endswith("\n") else ""), total
    return _scrub_string(text, known)


def scrub_file(path: Path, known: frozenset[str], write: bool) -> int:
    """Scrub one file; return the number of replacements, writing only when write is set and something changed."""
    original = path.read_text(encoding="utf-8")
    rewritten, count = _scrub_document(original, path.suffix, known)
    if count and write:
        path.write_text(rewritten, encoding="utf-8")
    return count


def iter_targets(paths: list[Path]) -> list[Path]:
    targets: list[Path] = []
    for path in paths:
        if path.is_file():
            targets.append(path)
        elif path.is_dir():
            targets.extend(sorted(p for p in path.rglob("*") if p.is_file() and p.suffix in SCRUBBED_SUFFIXES))
        else:
            raise FileNotFoundError(f"no such file or directory: {path}")
    return targets


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("paths", nargs="+", type=Path)
    parser.add_argument("--check", action="store_true", help="report only; exit 1 if anything would be replaced")
    args = parser.parse_args(argv)

    try:
        targets = iter_targets(args.paths)
        known = known_ids()
    except (OSError, ValueError) as err:
        print(f"scrub: {err}", file=sys.stderr)
        return 2

    dirty = 0
    replaced = 0
    for target in targets:
        try:
            count = scrub_file(target, known, write=not args.check)
        except (OSError, UnicodeDecodeError) as err:
            print(f"scrub: failed to read {target}: {err}", file=sys.stderr)
            return 2
        if count:
            dirty += 1
            replaced += count
            verb = "would replace" if args.check else "replaced"
            print(f"{target}: {verb} {count}")
    if args.check:
        if dirty:
            print(f"scrub: {replaced} identifier(s) in {dirty} file(s) would be replaced", file=sys.stderr)
            return 1
        print(f"scrub: {len(targets)} file(s) clean")
        return 0
    print(f"scrub: replaced {replaced} identifier(s) in {dirty} of {len(targets)} file(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
