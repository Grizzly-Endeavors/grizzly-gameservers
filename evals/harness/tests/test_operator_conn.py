"""Unit tests for the pure .env parser in operator_conn."""

from __future__ import annotations

import sys
import http.client
import json
import tempfile
import unittest
import urllib.error
from unittest import mock
from importlib import import_module
from pathlib import Path

HARNESS_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HARNESS_DIR))
operator_conn = import_module("operator_conn")
read_env_file = operator_conn.read_env_file


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


class _Response:
    def __init__(self, payload):
        self.payload = json.dumps(payload).encode()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False

    def read(self):
        return self.payload


def make_conn(reopens):
    def reopen():
        reopens.append(1)
        return f"http://127.0.0.1:{9000 + len(reopens)}", None, None

    sleeps = []
    conn = operator_conn.OperatorConn(
        "http://127.0.0.1:9000", "t", "c", reopen=reopen, backoff=(1.0, 2.0, 3.0), sleep=sleeps.append
    )
    return conn, sleeps


class ReconnectTest(unittest.TestCase):
    def test_refused_then_reconnect_then_success(self):
        reopens = []
        conn, sleeps = make_conn(reopens)
        calls = []

        def urlopen(request, timeout):
            calls.append(request.full_url)
            if len(calls) == 1:
                raise urllib.error.URLError(ConnectionRefusedError())
            return _Response({"result": "ok"})

        with mock.patch.object(operator_conn.urllib.request, "urlopen", urlopen):
            self.assertEqual(conn.tool("x"), {"result": "ok"})
        self.assertEqual(len(reopens), 1)
        self.assertEqual(sleeps, [1.0])
        self.assertTrue(calls[1].startswith("http://127.0.0.1:9001"))
        self.assertEqual(conn.failure_streak, 0)

    def test_tool_call_dropped_mid_flight_is_retried(self):
        conn, _ = make_conn([])
        calls = []

        def urlopen(request, timeout):
            calls.append(1)
            if len(calls) == 1:
                raise http.client.RemoteDisconnected("closed")
            return _Response({"result": "ok"})

        with mock.patch.object(operator_conn.urllib.request, "urlopen", urlopen):
            self.assertEqual(conn.tool("x"), {"result": "ok"})

    def test_gary_dropped_mid_flight_is_not_retried(self):
        reopens = []
        conn, _ = make_conn(reopens)
        calls = []

        def urlopen(request, timeout):
            calls.append(1)
            raise http.client.RemoteDisconnected("closed")

        with mock.patch.object(operator_conn.urllib.request, "urlopen", urlopen):
            with self.assertRaises(operator_conn.OperatorConnError):
                conn.gary("hi")
        self.assertEqual(len(calls), 1)
        self.assertEqual(reopens, [])

    def test_gary_refused_is_retried(self):
        conn, _ = make_conn([])
        calls = []

        def urlopen(request, timeout):
            calls.append(1)
            if len(calls) == 1:
                raise urllib.error.URLError(ConnectionRefusedError())
            return _Response({"reply": "ok"})

        with mock.patch.object(operator_conn.urllib.request, "urlopen", urlopen):
            self.assertEqual(conn.gary("hi"), {"reply": "ok"})

    def test_gives_up_after_the_backoff_is_spent_and_counts_the_streak(self):
        reopens = []
        conn, sleeps = make_conn(reopens)

        def urlopen(request, timeout):
            raise urllib.error.URLError(ConnectionRefusedError())

        with mock.patch.object(operator_conn.urllib.request, "urlopen", urlopen):
            with self.assertRaises(operator_conn.OperatorConnError):
                conn.tool("x")
        self.assertEqual(sleeps, [1.0, 2.0, 3.0])
        self.assertEqual(conn.failure_streak, 1)


if __name__ == "__main__":
    unittest.main()
