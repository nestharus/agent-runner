#!/usr/bin/env python3
"""AGE-113 stdio MCP server fixture inherited from the AGE-104 P2 proof."""

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


class UnknownToolError(ValueError):
    pass


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
                "description": "Known AGE-113 probe payload.",
            }
        },
        "required": ["message"],
        "additionalProperties": False,
    }


def result_response(request_id: Any, result: dict[str, Any]) -> dict[str, Any]:
    return {"jsonrpc": "2.0", "id": request_id, "result": result}


def error_response(request_id: Any, code: int, message: str) -> dict[str, Any]:
    return {"jsonrpc": "2.0", "id": request_id, "error": {"code": code, "message": message}}


def initialize_payload(request: dict[str, Any]) -> dict[str, Any]:
    requested_protocol = (request.get("params") or {}).get("protocolVersion") or "2025-11-25"
    return {
        "protocolVersion": requested_protocol,
        "capabilities": {"tools": {}},
        "serverInfo": {"name": "age104p2", "version": "0.1.0"},
    }


def call_params(request: dict[str, Any]) -> tuple[Any, dict[str, Any]]:
    params = request.get("params") or {}
    return params.get("name"), params.get("arguments") or {}


def known_tool_name(name: Any) -> bool:
    return name in TOOLS


def echo_output(message: str) -> str:
    return f"ECHO_OK:{message}"


def task_command_for(name: str) -> str:
    return f"TASK_OK:{name}"


def run_command_text(cmd: str) -> str:
    proc = subprocess.run(
        ["/bin/echo", cmd],
        check=True,
        text=True,
        capture_output=True,
    )
    return proc.stdout.strip()


def task_output(message: str) -> str:
    return run_command_text(task_command_for(message))


def unknown_tool_output(name: str) -> str:
    return f"UNKNOWN_TOOL:{name}"


def tool_output(name: str, message: str) -> str:
    if name == "Echo":
        return echo_output(message)
    if name == "Task":
        return task_output(message)
    return unknown_tool_output(name)


def request_line(raw_line: str) -> str:
    return raw_line.strip()


def has_request_content(line: str) -> bool:
    return bool(line)


def parse_request(line: str) -> dict[str, Any]:
    return json.loads(line)


def exception_response(exc: Exception) -> dict[str, Any]:
    return error_response(None, -32603, repr(exc))


def response_text(response: dict[str, Any]) -> str:
    return json.dumps(response, sort_keys=True) + "\n"


def format_log_record(kind: str, fields: dict[str, Any]) -> str:
    record = {
        "kind": kind,
        "wall_time": utc_now(),
        "epoch_ns": time.time_ns(),
        "pid": os.getpid(),
        **fields,
    }
    return json.dumps(record, sort_keys=True) + "\n"


def write_log_record(log_path: Path, record: str) -> None:
    with log_path.open("a", encoding="utf-8") as handle:
        handle.write(record)


def validate_tool_name(name: Any) -> None:
    if not known_tool_name(name):
        raise UnknownToolError(f"unknown tool {name}")


def format_tool_response(
    request_id: Any, result: dict[str, Any] | None = None, error: UnknownToolError | None = None
) -> dict[str, Any]:
    if error is not None:
        return error_response(request_id, -32602, str(error))
    return result_response(request_id, result or {})


def format_response_line(response: dict[str, Any]) -> str:
    return response_text(response)


def emit_response_line(line: str) -> None:
    sys.stdout.write(line)
    sys.stdout.flush()


def parse_request_line(text: str) -> dict[str, Any]:
    return parse_request(text)


def format_exception_response(exc: Exception) -> dict[str, Any]:
    return exception_response(exc)


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
        write_log_record(self.log_path, format_log_record(kind, fields))

    def tool_schema(self) -> list[dict[str, Any]]:
        return [
            {
                "name": "Echo",
                "description": "AGE-113 echo control tool.",
                "inputSchema": input_schema(),
            },
            {
                "name": "Task",
                "description": "AGE-113 task control tool that returns TASK_OK via /bin/echo.",
                "inputSchema": input_schema(),
            },
        ]

    def record_tool_start(self, name: str, args: dict[str, Any]) -> None:
        self.log("tool_call_start", name=name, args=args)
        self.touch("tool-call-start.sentinel")

    def record_tool_end(self, name: str, output: str) -> None:
        self.log("tool_call_end", name=name, output=output)
        self.touch("tool-call-end.sentinel")

    def call_tool(self, name: str, args: dict[str, Any]) -> dict[str, Any]:
        message = str(args.get("message", ""))
        self.record_tool_start(name, args)
        output = tool_output(name, message)
        self.record_tool_end(name, output)
        return text_result(output)

    def route_request(
        self, method: Any, request_id: Any, request: dict[str, Any]
    ) -> dict[str, Any] | None:
        if method == "initialize":
            self.touch("mcp-initialize.sentinel")
            return result_response(request_id, initialize_payload(request))
        if method == "notifications/initialized":
            return None
        if method == "tools/list":
            self.touch("tools-list.sentinel")
            return result_response(request_id, {"tools": self.tool_schema()})
        if method == "tools/call":
            return self.handle_tool_call(request_id, request)
        if request_id is None:
            return None
        return error_response(request_id, -32601, f"method not found: {method}")

    def handle_tool_call(self, request_id: Any, request: dict[str, Any]) -> dict[str, Any]:
        name, args = call_params(request)
        try:
            validate_tool_name(name)
        except UnknownToolError as exc:
            return self.format_tool_response(request_id, error=exc)
        return self.format_tool_response(request_id, result=self.dispatch_tool(name, args))

    def dispatch_tool(self, name: str, args: dict[str, Any]) -> dict[str, Any]:
        return self.call_tool(name, args)

    def format_tool_response(
        self, request_id: Any, result: dict[str, Any] | None = None, error: UnknownToolError | None = None
    ) -> dict[str, Any]:
        return format_tool_response(request_id, result, error)

    def handle(self, request: dict[str, Any]) -> dict[str, Any] | None:
        method = request.get("method")
        request_id = request.get("id")
        self.log("request", method=method, id=request_id, request=request)
        return self.route_request(method, request_id, request)

    def stop(self, *_args: Any) -> None:
        self.running = False
        self.log("stop")
        self.touch("server-stop.sentinel")

    def write_response(self, response: dict[str, Any]) -> None:
        emit_response_line(format_response_line(response))
        self.log("response", response=response)

    def response_for_line(self, line: str) -> dict[str, Any] | None:
        try:
            return self.handle_request(parse_request_line(line))
        except Exception as exc:
            self.log("exception", error=repr(exc), raw=line)
            return format_exception_response(exc)

    def handle_request(self, request: dict[str, Any]) -> dict[str, Any] | None:
        return self.handle(request)

    def process_line(self, line: str) -> None:
        response = self.response_for_line(line)
        if response is not None:
            self.write_response(response)

    def serve(self) -> None:
        self.log("server_start", tools=TOOLS)
        self.touch("server-start.sentinel")
        for raw_line in sys.stdin:
            if not self.running:
                break
            line = request_line(raw_line)
            if not has_request_content(line):
                continue
            self.process_line(line)
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
