#!/usr/bin/env bash
set -euo pipefail

HARNESS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
EVIDENCE_DIR="$(cd "$HARNESS_DIR/.." && pwd)"
RESULTS_JSONL="$HARNESS_DIR/logs/matrix/results.jsonl"
TABLE_MD="$EVIDENCE_DIR/p2-truth-table.md"

rm -rf "$HARNESS_DIR/logs/matrix"
mkdir -p "$(dirname "$RESULTS_JSONL")"
: > "$RESULTS_JSONL"

for mode in M1 M2 M3 M4; do
  for column in C1 C2 C3; do
    printf 'RUN %s %s\n' "$mode" "$column"
    AGE104P2_RUN_SET=matrix "$HARNESS_DIR/run-mode.sh" "$mode" "$column" | tee -a "$RESULTS_JSONL"
  done
done

python3 - "$RESULTS_JSONL" "$TABLE_MD" <<'PY'
from __future__ import annotations

import json
import sys
from pathlib import Path

results_path = Path(sys.argv[1])
table_path = Path(sys.argv[2])
labels = {
    "M1": "claude -p raw print",
    "M2": "claude --print stream-json",
    "M3": "interactive PTY with hooks",
    "M4": "interactive PTY without hooks",
    "C1": "--tools mcp__age104p2__Task,mcp__age104p2__Echo",
    "C2": "--allowedTools mcp__age104p2__Task,mcp__age104p2__Echo",
    "C3": "no tool filter",
}

rows = []
for line in results_path.read_text(encoding="utf-8").splitlines():
    line = line.strip()
    if not line.startswith("{"):
        continue
    rows.append(json.loads(line))

by_key = {(row["mode"], row["column"]): row for row in rows}

def yn(value: bool) -> str:
    return "yes" if value else "no"

lines = [
    "# AGE-104 P2 Truth Table",
    "",
    "Claude binary: `/home/nes/.local/share/claude/versions/2.1.143`.",
    "",
    "| Mode | Tool-flag shape | mcp_server_initialized | tools_call_reached_server | tool_returned_to_claude | evidence_path |",
    "|---|---|---:|---:|---:|---|",
]
for mode in ["M1", "M2", "M3", "M4"]:
    for column in ["C1", "C2", "C3"]:
        row = by_key[(mode, column)]
        lines.append(
            "| {mode} - {mode_label} | {column} - `{column_label}` | {listed} | {called} | {returned} | `{evidence}` |".format(
                mode=mode,
                mode_label=labels[mode],
                column=column,
                column_label=labels[column],
                listed=yn(row["mcp_server_initialized"]),
                called=yn(row["tools_call_reached_server"]),
                returned=yn(row["tool_returned_to_claude"]),
                evidence=row["evidence_path"],
            )
        )

lines += [
    "",
    "Expected control pattern: print modes call the tool for all three shapes; PTY modes fail only for C1 (`--tools`) and succeed for C2 (`--allowedTools`) and C3 (no filter).",
]
table_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
print(f"WROTE {table_path}")
PY
