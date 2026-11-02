"""Layout of a run directory: one JSON file per run record, plus the run manifest."""

from __future__ import annotations

from pathlib import Path

RUN_MANIFEST_NAME = "manifest.json"


def record_paths(runs_dir: Path) -> list[Path]:
    """Every run record directly inside `runs_dir`, sorted by path."""
    return sorted(p for p in runs_dir.glob("*.json") if p.name != RUN_MANIFEST_NAME)
