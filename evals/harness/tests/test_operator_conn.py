"""Unit tests for the pure .env parser in operator_conn."""

from __future__ import annotations

import sys
import tempfile
import unittest
from importlib import import_module
from pathlib import Path

HARNESS_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HARNESS_DIR))
read_env_file = import_module("operator_conn").read_env_file


class ReadEnvFileTest(unittest.TestCase):
    def test_parses_keys_quotes_export_and_comments(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            env = Path(tmp) / ".env"
            env.write_text(
                "# comment\n"
                "\n"
                "GAMESERVERS_OPERATOR_CHANNEL=123456\n"
                'export QUOTED="a value"\n'
                "SINGLE='x'\n"
                "NO_EQUALS_LINE\n",
                encoding="utf-8",
            )
            values = read_env_file(env)
        self.assertEqual(
            values,
            {"GAMESERVERS_OPERATOR_CHANNEL": "123456", "QUOTED": "a value", "SINGLE": "x"},
        )

    def test_missing_file_is_empty(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(read_env_file(Path(tmp) / "absent.env"), {})


if __name__ == "__main__":
    unittest.main()
