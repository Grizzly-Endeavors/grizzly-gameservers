#!/usr/bin/env python3
"""MCP stdio server that exposes Gary's operator tool surface to Claude Code.

Each tool the bot advertises at GET /operator/tools for the configured access
tier becomes an MCP tool, and a call runs through POST /operator/tool, so the
Claude Code baseline in an eval calls exactly the tools Gary calls, through the
same dispatcher and the same tier gates.

Configuration is read from the environment:

- GARY_OPERATOR_URL / GARY_OPERATOR_TOKEN: a reachable operator endpoint. When
  GARY_OPERATOR_URL is unset, the server port-forwards the deployed bot itself
  (see operator_conn.py) and reads the token from the cluster.
- GARY_CHANNEL: the Discord channel id replies are attributed to. Required when
  GARY_OPERATOR_URL is set; otherwise overrides GAMESERVERS_OPERATOR_CHANNEL.
- GARY_ACCESS: access tier, read-only | manager | admin (default admin).
- GARY_EXCLUDE_TOOLS: comma-separated tool names to hide from the listing.
- GARY_SERVER: the instance a case is about. Informational only; it is never
  sent to the bot.
- GARY_TOOL_LOG: a path to append one JSON line per tools/call to.

Protocol traffic is newline-delimited JSON-RPC 2.0 on stdout. Anything
diagnostic goes to stderr, which Claude Code keeps in its MCP log.
"""

from __future__ import annotations

import json
import os
import sys
import time
from pathlib import Path
from typing import Any, TextIO

from operator_conn import OperatorConn, OperatorConnError, OperatorError

PROTOCOL_VERSION = "2025-06-18"
SERVER_NAME = "gary"
SERVER_VERSION = "0.1.0"
HTTP_TIMEOUT_S = 600.0

PARSE_ERROR = -32700
INVALID_REQUEST = -32600
METHOD_NOT_FOUND = -32601
INVALID_PARAMS = -32602
INTERNAL_ERROR = -32603

_NOTIFICATIONS = frozenset({"notifications/initialized", "notifications/cancelled"})
_EMPTY_OBJECT_SCHEMA: dict[str, Any] = {"type": "object", "properties": {}}


class RpcError(Exception):
    """A JSON-RPC error to send back to the client in place of a result."""

    def __init__(self, code: int, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.message = message


class GaryMcpServer:
    """Routes JSON-RPC messages to the tool surface fetched at startup."""

    def __init__(
        self,
        conn: OperatorConn,
        access: str,
        tools: dict[str, dict[str, Any]],
        tool_log: Path | None,
    ) -> None:
        self._conn = conn
        self._access = access
        self._tools = tools
        self._tool_log = tool_log

    def handle(self, message: Any) -> dict[str, Any] | None:
        """Return the response for a message, or None when it needs no reply."""
        if not isinstance(message, dict):
            return _error_response(None, INVALID_REQUEST, "request must be a JSON object")
        request_id = message.get("id")
        method = message.get("method")
        is_notification = "id" not in message
        if not isinstance(method, str):
            if is_notification:
                return None
            return _error_response(request_id, INVALID_REQUEST, "request has no method")
        if is_notification and method in _NOTIFICATIONS:
            return None
        try:
            result = self._dispatch(method, message.get("params") or {})
        except RpcError as err:
            if is_notification:
                return None
            return _error_response(request_id, err.code, err.message)
        except OSError as err:
            # A tool log that cannot be written would silently drop eval data,
            # so the failure goes back to the client rather than being swallowed.
            if is_notification:
                return None
            return _error_response(request_id, INTERNAL_ERROR, f"internal error: {err}")
        if is_notification:
            return None
        return {"jsonrpc": "2.0", "id": request_id, "result": result}

    def _dispatch(self, method: str, params: Any) -> dict[str, Any]:
        if not isinstance(params, dict):
            raise RpcError(INVALID_PARAMS, "params must be an object")
        if method == "initialize":
            return self._initialize()
        if method == "ping":
            return {}
        if method == "tools/list":
            return {"tools": [self._mcp_tool(tool) for tool in self._tools.values()]}
        if method == "tools/call":
            return self._call_tool(params)
        raise RpcError(METHOD_NOT_FOUND, f"method not found: {method}")

    def _initialize(self) -> dict[str, Any]:
        return {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"tools": {"listChanged": False}},
            "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION},
        }

    @staticmethod
    def _mcp_tool(tool: dict[str, Any]) -> dict[str, Any]:
        schema = tool.get("parameters")
        return {
            "name": tool["name"],
            "description": tool.get("description", ""),
            "inputSchema": schema if isinstance(schema, dict) else _EMPTY_OBJECT_SCHEMA,
        }

    def _call_tool(self, params: dict[str, Any]) -> dict[str, Any]:
        name = params.get("name")
        arguments = params.get("arguments") or {}
        if not isinstance(name, str) or name not in self._tools:
            raise RpcError(INVALID_PARAMS, f"unknown tool: {name!r}")
        if not isinstance(arguments, dict):
            raise RpcError(INVALID_PARAMS, "tool arguments must be an object")

        started = time.monotonic()
        try:
            response = self._conn.tool(name, arguments, access=self._access)
            text = _result_text(response)
            is_error = False
        except OperatorError as err:
            text = err.message
            is_error = True
        except OperatorConnError as err:
            text = str(err)
            is_error = True
        elapsed_ms = int((time.monotonic() - started) * 1000)
        self._log_call(name, arguments, text, is_error, elapsed_ms)
        return {"content": [{"type": "text", "text": text}], "isError": is_error}

    def _log_call(
        self, name: str, arguments: dict[str, Any], text: str, is_error: bool, elapsed_ms: int
    ) -> None:
        if self._tool_log is None:
            return
        line = {
            "name": name,
            "arguments": arguments,
            "result_bytes": len(text.encode("utf-8")),
            "is_error": is_error,
            "ms": elapsed_ms,
        }
        with self._tool_log.open("a", encoding="utf-8") as log:
            log.write(json.dumps(line) + "\n")


def _result_text(response: dict[str, Any]) -> str:
    result = response.get("result")
    if isinstance(result, str):
        return result
    return json.dumps(response)


def _error_response(request_id: Any, code: int, message: str) -> dict[str, Any]:
    return {"jsonrpc": "2.0", "id": request_id, "error": {"code": code, "message": message}}


def serve(server: GaryMcpServer, reader: TextIO, writer: TextIO) -> None:
    """Answer newline-delimited JSON-RPC messages until the client closes stdin."""
    for raw in reader:
        line = raw.strip()
        if not line:
            continue
        try:
            message = json.loads(line)
        except ValueError as err:
            response: dict[str, Any] | None = _error_response(None, PARSE_ERROR, f"parse error: {err}")
        else:
            response = server.handle(message)
        if response is not None:
            writer.write(json.dumps(response) + "\n")
            writer.flush()


def _die(message: str) -> int:
    print(f"gary_mcp: {message}", file=sys.stderr)
    return 1


def _open_connection(environ: dict[str, str]) -> OperatorConn:
    channel = environ.get("GARY_CHANNEL") or None
    url = environ.get("GARY_OPERATOR_URL")
    if not url:
        return OperatorConn.connect(channel=channel, timeout=HTTP_TIMEOUT_S)
    token = environ.get("GARY_OPERATOR_TOKEN")
    if not token:
        raise OperatorConnError("GARY_OPERATOR_URL is set but GARY_OPERATOR_TOKEN is not")
    if not channel:
        raise OperatorConnError("GARY_OPERATOR_URL is set but GARY_CHANNEL is not")
    return OperatorConn(url, token, channel, timeout=HTTP_TIMEOUT_S)


def main() -> int:
    environ = dict(os.environ)
    access = environ.get("GARY_ACCESS") or "admin"
    excluded = {name.strip() for name in environ.get("GARY_EXCLUDE_TOOLS", "").split(",") if name.strip()}
    tool_log_path = environ.get("GARY_TOOL_LOG")

    try:
        conn = _open_connection(environ)
    except OperatorConnError as err:
        return _die(str(err))

    with conn:
        try:
            advertised = conn.tools(access)
        except (OperatorError, OperatorConnError) as err:
            return _die(f"could not list tools for access tier {access!r}: {err}")
        tools = {tool["name"]: tool for tool in advertised if tool["name"] not in excluded}
        print(f"gary_mcp: serving {len(tools)} tools at access tier {access!r}", file=sys.stderr)

        server = GaryMcpServer(
            conn,
            access,
            tools,
            Path(tool_log_path) if tool_log_path else None,
        )
        serve(server, sys.stdin, sys.stdout)
    return 0


if __name__ == "__main__":
    sys.exit(main())
