#!/usr/bin/env python3
"""AGE-104 P2 minimal stdio MCP server.

The server intentionally uses only Python stdlib. It exposes two tools:
Echo returns the supplied message in-process, and Task subprocess-runs
`/bin/echo TASK_OK:<message>`.
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


TOOLS = ["Echo", "Task"]


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat()


def text_result(text: str) -> dict[str, Any]:
    return {"content": [{"type": "text", "text": text}], "isError": False}


def input_schema() -> dict[str, Any]:
    return {
        "type": "object",
        "properties": {
            "message": {
                "type": "string",
                "description": "Known AGE-104 probe payload.",
            }
        },
        "required": ["message"],
        "additionalProperties": False,
    }


class Server:
    def __init__(self, log_path: Path, sentinel_dir: Path | None):
        self.log_path = log_path
        self.sentinel_dir = sentinel_dir
        self.running = True
        self.log_path.parent.mkdir(parents=True, exist_ok=True)
        if self.sentinel_dir:
            self.sentinel_dir.mkdir(parents=True, exist_ok=True)

    def touch(self, name: str) -> None:
        if self.sentinel_dir:
            (self.sentinel_dir / name).write_text(utc_now() + "\n", encoding="utf-8")

    def log(self, kind: str, **fields: Any) -> None:
        record = {
            "kind": kind,
            "wall_time": utc_now(),
            "epoch_ns": time.time_ns(),
            "pid": os.getpid(),
            **fields,
        }
        with self.log_path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(record, sort_keys=True) + "\n")

    def tool_schema(self) -> list[dict[str, Any]]:
        return [
            {
                "name": "Echo",
                "description": "AGE-104 P2 echo control tool.",
                "inputSchema": input_schema(),
            },
            {
                "name": "Task",
                "description": "AGE-104 P2 task control tool that returns TASK_OK via /bin/echo.",
                "inputSchema": input_schema(),
            },
        ]

    def call_tool(self, name: str, args: dict[str, Any]) -> dict[str, Any]:
        message = str(args.get("message", ""))
        self.log("tool_call_start", name=name, args=args)
        self.touch("tool-call-start.sentinel")
        if name == "Echo":
            output = f"ECHO_OK:{message}"
        elif name == "Task":
            proc = subprocess.run(
                ["/bin/echo", f"TASK_OK:{message}"],
                check=True,
                text=True,
                capture_output=True,
            )
            output = proc.stdout.strip()
        else:
            output = f"UNKNOWN_TOOL:{name}"
        self.log("tool_call_end", name=name, output=output)
        self.touch("tool-call-end.sentinel")
        return text_result(output)

    def handle(self, request: dict[str, Any]) -> dict[str, Any] | None:
        method = request.get("method")
        request_id = request.get("id")
        self.log("request", method=method, id=request_id, request=request)

        if method == "initialize":
            self.touch("mcp-initialize.sentinel")
            requested_protocol = (request.get("params") or {}).get("protocolVersion") or "2025-11-25"
            return {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "protocolVersion": requested_protocol,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "age104p2", "version": "0.1.0"},
                },
            }
        if method == "notifications/initialized":
            return None
        if method == "tools/list":
            self.touch("tools-list.sentinel")
            return {"jsonrpc": "2.0", "id": request_id, "result": {"tools": self.tool_schema()}}
        if method == "tools/call":
            params = request.get("params") or {}
            name = params.get("name")
            args = params.get("arguments") or {}
            if name not in TOOLS:
                return {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": {"code": -32602, "message": f"unknown tool {name}"},
                }
            return {"jsonrpc": "2.0", "id": request_id, "result": self.call_tool(name, args)}
        if request_id is None:
            return None
        return {
            "jsonrpc": "2.0",
            "id": request_id,
            "error": {"code": -32601, "message": f"method not found: {method}"},
        }

    def stop(self, *_args: Any) -> None:
        self.running = False
        self.log("stop")
        self.touch("server-stop.sentinel")

    def serve(self) -> None:
        self.log("server_start", tools=TOOLS)
        self.touch("server-start.sentinel")
        for line in sys.stdin:
            if not self.running:
                break
            line = line.strip()
            if not line:
                continue
            try:
                response = self.handle(json.loads(line))
            except Exception as exc:  # pragma: no cover - diagnostic path
                self.log("exception", error=repr(exc), raw=line)
                response = {
                    "jsonrpc": "2.0",
                    "id": None,
                    "error": {"code": -32603, "message": repr(exc)},
                }
            if response is not None:
                sys.stdout.write(json.dumps(response, sort_keys=True) + "\n")
                sys.stdout.flush()
                self.log("response", response=response)
        self.log("server_exit")
        self.touch("server-exit.sentinel")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--log", required=True)
    parser.add_argument("--sentinel-dir")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    server = Server(
        Path(args.log),
        Path(args.sentinel_dir) if args.sentinel_dir else None,
    )
    signal.signal(signal.SIGTERM, server.stop)
    signal.signal(signal.SIGINT, server.stop)
    server.serve()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
