#!/usr/bin/env python3
"""Minimal MCP stdio server that exposes Claude Code Agent/Task launch tools.

Claudex attaches this server to Grok/Cursor ACP sessions so the model can invoke
Agent/Task. The adapter bridges those tool calls into Anthropic tool_use for
Claude Code; this server only acknowledges the launch so the provider turn can
end promptly.
"""

from __future__ import annotations

import json
import sys
from typing import Any

PROTOCOL_VERSION = "2024-11-05"
SERVER_NAME = "claudex-launch"
SERVER_VERSION = "1.0.0"

AGENT_SCHEMA: dict[str, Any] = {
    "type": "object",
    "additionalProperties": True,
    "properties": {
        "description": {
            "type": "string",
            "description": "Short 3-5 word description of the task",
        },
        "prompt": {
            "type": "string",
            "description": "The task for the agent to perform",
        },
        "subagent_type": {
            "type": "string",
            "description": "Claudex worker type from selected_workers (e.g. claudex-cursor)",
        },
        "run_in_background": {
            "type": "boolean",
            "description": "Prefer true so Claude Code tracks the worker in the agents panel",
        },
        "claudex_model": {
            "type": "string",
            "description": "Exact worker model id from selected_workers",
        },
        "claudex_effort": {
            "type": "string",
            "description": "Worker effort from selected_workers",
        },
    },
    "required": ["description", "prompt"],
}

TOOLS: list[dict[str, Any]] = [
    {
        "name": "Agent",
        "description": (
            "Launch a Claude Code SubAgent through Claudex. Prefer "
            "run_in_background=true and selected_workers subagent_type + claudex_model. "
            "After launch, end the turn; do not poll."
        ),
        "inputSchema": AGENT_SCHEMA,
    },
    {
        "name": "Task",
        "description": (
            "Launch a Claude Code Task SubAgent through Claudex. Prefer "
            "run_in_background=true. After launch, end the turn; do not poll."
        ),
        "inputSchema": AGENT_SCHEMA,
    },
]


def read_message() -> dict[str, Any] | None:
    headers: dict[str, str] = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        decoded = line.decode("utf-8", errors="replace").strip()
        if not decoded or ":" not in decoded:
            continue
        key, value = decoded.split(":", 1)
        headers[key.strip().lower()] = value.strip()
    length = int(headers.get("content-length", "0"))
    if length <= 0:
        return None
    body = sys.stdin.buffer.read(length)
    if not body:
        return None
    return json.loads(body.decode("utf-8"))


def write_message(message: dict[str, Any]) -> None:
    body = json.dumps(message, separators=(",", ":")).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("ascii"))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()


def handle(message: dict[str, Any]) -> None:
    method = message.get("method")
    msg_id = message.get("id")
    if method == "initialize":
        write_message(
            {
                "jsonrpc": "2.0",
                "id": msg_id,
                "result": {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION},
                },
            }
        )
        return
    if method == "notifications/initialized":
        return
    if method == "tools/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"tools": TOOLS}})
        return
    if method == "tools/call":
        write_message(
            {
                "jsonrpc": "2.0",
                "id": msg_id,
                "result": {
                    "content": [
                        {
                            "type": "text",
                            "text": (
                                "Claudex: SubAgent launch handed to Claude Code. "
                                "End the turn; do not poll TaskOutput."
                            ),
                        }
                    ],
                    "isError": False,
                },
            }
        )
        return
    if method == "ping":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {}})
        return
    if msg_id is not None:
        write_message(
            {
                "jsonrpc": "2.0",
                "id": msg_id,
                "error": {"code": -32601, "message": f"Method not found: {method}"},
            }
        )


def main() -> None:
    while True:
        message = read_message()
        if message is None:
            break
        handle(message)


if __name__ == "__main__":
    main()
