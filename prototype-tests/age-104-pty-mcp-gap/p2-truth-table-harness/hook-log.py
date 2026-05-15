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


def main() -> int:
    event = sys.argv[1] if len(sys.argv) > 1 else "unknown"
    raw = sys.stdin.read()
    try:
        payload: Any = json.loads(raw) if raw.strip() else None
    except json.JSONDecodeError:
        payload = None

    run_dir = Path(os.environ["AGE104P2_RUN_DIR"])
    hook_dir = run_dir / "hooks"
    hook_dir.mkdir(parents=True, exist_ok=True)
    record = {
        "kind": "hook",
        "event": event,
        "ts": utc_now(),
        "epoch_ns": time.time_ns(),
        "pid": os.getpid(),
        "cwd": os.getcwd(),
        "payload": payload,
        "payload_raw": raw,
    }
    with (hook_dir / "hooks.jsonl").open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(record, sort_keys=True) + "\n")
    (hook_dir / f"{event}.last.json").write_text(
        json.dumps(record, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
