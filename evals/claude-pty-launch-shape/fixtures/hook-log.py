#!/usr/bin/env python3
from __future__ import annotations

import json
import os
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat()


def hook_event(argv: list[str]) -> str:
    return argv[1] if len(argv) > 1 else "unknown"


def hook_payload(raw: str) -> Any:
    try:
        return json.loads(raw) if raw.strip() else None
    except json.JSONDecodeError:
        return None


def hook_dir_from_env() -> Path:
    return Path(os.environ["AGE113_CLAUDE_PTY_RUN_DIR"]) / "hooks"


def process_context() -> dict[str, Any]:
    return {
        "ts": utc_now(),
        "epoch_ns": time.time_ns(),
        "pid": os.getpid(),
        "cwd": os.getcwd(),
    }


def hook_record(event: str, raw: str, payload: Any, context: dict[str, Any]) -> dict[str, Any]:
    return {
        "kind": "hook",
        "event": event,
        **context,
        "payload": payload,
        "payload_raw": raw,
    }


def json_line(record: dict[str, Any]) -> str:
    return json.dumps(record, sort_keys=True) + "\n"


def json_document(record: dict[str, Any]) -> str:
    return json.dumps(record, indent=2, sort_keys=True) + "\n"


def write_hook_record(hook_dir: Path, event: str, line: str, document: str) -> None:
    hook_dir.mkdir(parents=True, exist_ok=True)
    with (hook_dir / "hooks.jsonl").open("a", encoding="utf-8") as handle:
        handle.write(line)
    (hook_dir / f"{event}.last.json").write_text(document, encoding="utf-8")


def main() -> int:
    event = hook_event(sys.argv)
    raw = sys.stdin.read()
    payload = hook_payload(raw)
    record = hook_record(event, raw, payload, process_context())
    write_hook_record(hook_dir_from_env(), event, json_line(record), json_document(record))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
