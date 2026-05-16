#!/usr/bin/env python3
"""Emit AGE-113 truth-table baseline JSON payloads."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any


EVAL_ID = "claude-pty-launch-shape"
EXPECTED_FIELDS = [
    "mcp_server_initialized",
    "tools_call_reached_server",
    "tool_returned_to_claude",
]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    subcommands = parser.add_subparsers(dest="command", required=True)

    matrix = subcommands.add_parser("matrix")
    matrix.add_argument("baseline")

    mode = subcommands.add_parser("mode")
    mode.add_argument("baseline")
    mode.add_argument("mode_cell")

    return parser.parse_args()


def decode_baseline_text(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def require_baseline_object(value: Any, path: Path | None = None) -> dict[str, Any]:
    if isinstance(value, dict):
        return value
    if path is not None:
        raise ValueError(f"{path} must contain a JSON object")
    raise ValueError("baseline must contain a JSON object")


def load_baseline(path: Path) -> dict[str, Any]:
    return require_baseline_object(decode_baseline_text(path), path)


def split_mode_text(text: str) -> list[str]:
    return text.split("-", 1)


def require_two_part_mode(parts: list[str], text: str | None = None) -> tuple[str, str]:
    if len(parts) == 2:
        return parts[0], parts[1]
    if text is not None:
        raise ValueError(f"mode cell must use MODE-COLUMN shape: {text}")
    raise ValueError("mode cell must use MODE-COLUMN shape")


def mode_parts(mode_cell: str) -> tuple[str, str]:
    return require_two_part_mode(split_mode_text(mode_cell), mode_cell)


def baseline_rows(baseline: dict[str, Any]) -> list[dict[str, Any]]:
    rows = baseline.get("matrix")
    if isinstance(rows, list) and all(isinstance(row, dict) for row in rows):
        return rows
    raise ValueError("baseline matrix must be a list of objects")


def rows_for(rows: list[dict[str, Any]], mode: str, column: str) -> list[dict[str, Any]]:
    return [row for row in rows if row.get("mode") == mode and row.get("column") == column]


def require_one_row(matches: list[dict[str, Any]], mode: str, column: str) -> dict[str, Any]:
    if matches:
        return matches[0]
    raise ValueError(f"baseline matrix is missing {mode}-{column}")


def matching_row(rows: list[dict[str, Any]], mode: str, column: str) -> dict[str, Any]:
    return require_one_row(rows_for(rows, mode, column), mode, column)


def expected_payload(row: dict[str, Any]) -> dict[str, Any]:
    return {field: row[field] for field in EXPECTED_FIELDS}


def mode_payload(
    baseline: dict[str, Any], mode_cell: str, mode: str, column: str, row: dict[str, Any]
) -> dict[str, Any]:
    return {
        "eval_id": EVAL_ID,
        "mode_cell": mode_cell,
        "claude_code_baseline": baseline["claude_code_baseline"],
        "mode": mode,
        "column": column,
        "tool_flag_shape": row["tool_flag_shape"],
        "expected": expected_payload(row),
    }


def matrix_payload(baseline: dict[str, Any]) -> dict[str, Any]:
    return {"eval_id": EVAL_ID, **baseline}


def emit_json(payload: dict[str, Any]) -> None:
    print(json.dumps(payload, sort_keys=True))


def emit_matrix_payload(baseline: dict[str, Any]) -> None:
    emit_json(matrix_payload(baseline))


def emit_mode_payload(payload: dict[str, Any]) -> None:
    emit_json(payload)


def main() -> int:
    args = parse_args()
    baseline_path = Path(args.baseline)
    baseline = load_baseline(baseline_path)
    if args.command == "matrix":
        emit_matrix_payload(baseline)
        return 0

    mode, column = mode_parts(args.mode_cell)
    row = matching_row(baseline_rows(baseline), mode, column)
    emit_mode_payload(mode_payload(baseline, args.mode_cell, mode, column, row))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
