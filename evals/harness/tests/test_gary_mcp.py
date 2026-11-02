"""Drive gary_mcp.py as a subprocess against an in-process fake of the operator routes."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from typing import Any

HARNESS_DIR = Path(__file__).resolve().parents[1]
GARY_MCP = HARNESS_DIR / "gary_mcp.py"
TOKEN = "test-token"
CHANNEL = "C-1"

ADVERTISED = {
    "list_servers": "List the game servers.\nSecond line of detail.",
    "fail_tool": "Always refused by the bot.",
    "hidden_tool": "Hidden by GARY_EXCLUDE_TOOLS.",
}


def _tool_listing() -> list[dict[str, Any]]:
    return [
        {
            "type": "function",
            "function": {
                "name": name,
                "description": description,
                "parameters": {"type": "object", "properties": {"name": {"type": "string"}}},
            },
        }
        for name, description in ADVERTISED.items()
    ]


class FakeOperatorHandler(BaseHTTPRequestHandler):
    """Answers the two operator routes the way the bot does, and records POST bodies.

    Each test subclasses this to get its own `posted` list, so the recorded bodies
    are reachable from the test without reaching through the server object.
    """

    posted: list[dict[str, Any]] = []

    def log_message(self, *args: object) -> None:
        return

    def _send(self, status: int, body: Any) -> None:
        data = json.dumps(body).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _authorized(self) -> bool:
        if self.headers.get("Authorization") == f"Bearer {TOKEN}":
            return True
        self._send(401, {"error": "unauthorized"})
        return False

    def do_GET(self) -> None:
        if not self._authorized():
            return
        if self.path.startswith("/operator/tools"):
            self._send(200, _tool_listing())
        else:
            self._send(404, {"error": "no route"})

    def do_POST(self) -> None:
        if not self._authorized():
            return
        if self.path != "/operator/tool":
            self._send(404, {"error": "no route"})
            return
        length = int(self.headers.get("Content-Length", "0"))
        body = json.loads(self.rfile.read(length))
        self.posted.append(body)
        if body["tool"] == "fail_tool":
            self._send(400, {"error": "no such server: ghost"})
            return
        arguments = json.dumps(body["arguments"], sort_keys=True)
        self._send(200, {"tool": body["tool"], "result": f"ran {body['tool']} {arguments}"})


class GaryMcpTest(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        tmp = Path(self._tmp.name)
        self.tool_log = tmp / "tool.jsonl"
        self.stderr_path = tmp / "stderr.txt"

        handler = type("Handler", (FakeOperatorHandler,), {"posted": []})
        self.posted: list[dict[str, Any]] = handler.posted
        self.fake = HTTPServer(("127.0.0.1", 0), handler)
        serving = threading.Thread(target=self.fake.serve_forever, daemon=True)
        serving.start()
        self.addCleanup(serving.join, 30)
        self.addCleanup(self.fake.server_close)
        self.addCleanup(self.fake.shutdown)

        env = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith("GARY_") and key != "GAMESERVERS_OPERATOR_CHANNEL"
        }
        env.update(
            {
                "GARY_OPERATOR_URL": f"http://127.0.0.1:{self.fake.server_address[1]}",
                "GARY_OPERATOR_TOKEN": TOKEN,
                "GARY_CHANNEL": CHANNEL,
                "GARY_ACCESS": "admin",
                "GARY_EXCLUDE_TOOLS": "hidden_tool",
                "GARY_TOOL_LOG": str(self.tool_log),
            }
        )
        self._stderr = self.stderr_path.open("w")
        self.addCleanup(self._stderr.close)
        self.proc = subprocess.Popen(
            [sys.executable, str(GARY_MCP)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=self._stderr,
            env=env,
            text=True,
        )
        self.addCleanup(self._stop_process)
        self._next_id = 0

    def _stop_process(self) -> None:
        assert self.proc.stdin is not None and self.proc.stdout is not None
        self.proc.stdin.close()
        self.proc.wait(timeout=30)
        self.proc.stdout.close()

    def _send(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        self._next_id += 1
        message: dict[str, Any] = {"jsonrpc": "2.0", "id": self._next_id, "method": method}
        if params is not None:
            message["params"] = params
        assert self.proc.stdin is not None and self.proc.stdout is not None
        self.proc.stdin.write(json.dumps(message) + "\n")
        self.proc.stdin.flush()
        # Blocks until the server answers this request, so the test waits on the reply itself.
        line = self.proc.stdout.readline()
        if not line:
            self.fail(f"gary_mcp exited without answering: {self.stderr_path.read_text()}")
        response = json.loads(line)
        self.assertEqual(response["id"], self._next_id)
        return response

    def _initialize(self) -> None:
        response = self._send(
            "initialize",
            {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}},
        )
        self.assertEqual(response["result"]["protocolVersion"], "2025-06-18")
        self.assertIn("tools", response["result"]["capabilities"])
        # A notification has no id and must get no reply. The next request's response
        # carries its own id, so an unexpected reply here would fail the id check.
        assert self.proc.stdin is not None
        self.proc.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        self.proc.stdin.flush()

    def test_initialize_then_tools_list_returns_fake_tools_minus_excluded(self) -> None:
        self._initialize()
        response = self._send("tools/list")
        tools = {tool["name"]: tool for tool in response["result"]["tools"]}
        self.assertEqual(sorted(tools), ["fail_tool", "list_servers"])
        self.assertEqual(tools["list_servers"]["description"], ADVERTISED["list_servers"])
        self.assertEqual(tools["list_servers"]["inputSchema"]["properties"], {"name": {"type": "string"}})

    def test_tools_call_round_trips_arguments_and_returns_text(self) -> None:
        self._initialize()
        response = self._send("tools/call", {"name": "list_servers", "arguments": {"name": "mc"}})
        result = response["result"]
        self.assertFalse(result["isError"])
        expected_text = 'ran list_servers {"name": "mc"}'
        self.assertEqual(result["content"], [{"type": "text", "text": expected_text}])
        self.assertEqual(
            self.posted,
            [{"channel": CHANNEL, "access": "admin", "tool": "list_servers", "arguments": {"name": "mc"}}],
        )

        records = [json.loads(line) for line in self.tool_log.read_text().splitlines()]
        self.assertEqual(len(records), 1)
        self.assertEqual(records[0]["name"], "list_servers")
        self.assertEqual(records[0]["arguments"], {"name": "mc"})
        self.assertEqual(records[0]["result_bytes"], len(expected_text.encode("utf-8")))
        self.assertFalse(records[0]["is_error"])
        self.assertIsInstance(records[0]["ms"], int)

    def test_http_400_becomes_is_error_result_with_bot_message(self) -> None:
        self._initialize()
        response = self._send("tools/call", {"name": "fail_tool", "arguments": {"server": "ghost"}})
        self.assertNotIn("error", response)
        self.assertTrue(response["result"]["isError"])
        self.assertEqual(response["result"]["content"], [{"type": "text", "text": "no such server: ghost"}])
        records = [json.loads(line) for line in self.tool_log.read_text().splitlines()]
        self.assertTrue(records[-1]["is_error"])

    def test_excluded_tool_is_not_callable(self) -> None:
        self._initialize()
        response = self._send("tools/call", {"name": "hidden_tool", "arguments": {}})
        self.assertEqual(response["error"]["code"], -32602)
        self.assertEqual(self.posted, [])

    def test_unknown_method_is_method_not_found(self) -> None:
        self._initialize()
        response = self._send("resources/list")
        self.assertEqual(response["error"]["code"], -32601)

    def test_ping_answers_empty_result(self) -> None:
        self._initialize()
        self.assertEqual(self._send("ping")["result"], {})


if __name__ == "__main__":
    unittest.main()
