#!/usr/bin/env bash
set -euo pipefail

# prototype-pending: implementation pending in https://linear.app/oulipoly/issue/AGE-101/wire-proxy-pty-driver-for-claude-code-one-shot-execution; remove marker and make this test pass
# prototype-pending: implementation pending in https://linear.app/oulipoly/issue/AGE-103/extend-providerstoml-with-invocation-mode-schema; remove marker and make this test pass
# prototype-pending: implementation pending in https://linear.app/oulipoly/issue/AGE-113/harden-claude-proxy-launch-shape-regression-coverage; remove marker and make this test pass
# prototype-pending: implementation pending in https://linear.app/oulipoly/issue/AGE-112/decide-orchestrator-mode-completion-scope-until-pty-mcp-task-is-fixed; remove marker and make this test pass
#
# Acceptance criterion: remove the prototype-pending: markers in the listed test
# files, make these tests pass against production code, and preserve the
# original assertions unless the manifest, spawned ticket payload, or Phase 6
# Step 6b output index records a strictly stronger equivalent supersession.
#
# See ../../planning/prototype-age-104-pty-mcp-gap/dossier/answer.md and
# ../../planning/prototype-age-104-pty-mcp-gap/dossier/test-publication-manifest.md
# for the full prototype context and the per-ticket mapping in
# ./MARKERS.md alongside this script.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HARNESS_DIR="${SCRIPT_DIR}/p2-truth-table-harness"
FAILURES=0

run_cell() {
  local column="$1"
  AGE104P2_RUN_SET=proof "$HARNESS_DIR/run-mode.sh" M3 "$column"
}

assert_cell() {
  local column="$1"
  local expected="$2"
  local result="$HARNESS_DIR/logs/proof/M3/$column/result.json"
  local actual
  actual="$(python3 - "$result" "$expected" <<'PY'
from __future__ import annotations

import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
expected = sys.argv[2]
row = json.loads(path.read_text(encoding="utf-8"))
success = row["tools_call_reached_server"] and row["tool_returned_to_claude"]
failure = (not row["tools_call_reached_server"]) or row["no_such_tool"]
actual = "success" if success else "failure" if failure else "indeterminate"
print(actual)
PY
)"
  if [[ "$actual" != "$expected" ]]; then
    FAILURES=$((FAILURES + 1))
    printf 'ASSERTION FAILED M3 %s\n' "$column" >&2
    diff -u <(printf 'expected=%s\n' "$expected") <(printf 'actual=%s\nresult=%s\n' "$actual" "$result") >&2 || true
  fi
}

rm -rf "$HARNESS_DIR/logs/proof"

run_cell C1
assert_cell C1 failure

run_cell C2
assert_cell C2 success

run_cell C3
assert_cell C3 success

if (( FAILURES > 0 )); then
  exit 1
fi

printf 'P2 proof controls passed: M3-C1 failed as expected; M3-C2 and M3-C3 succeeded.\n'
