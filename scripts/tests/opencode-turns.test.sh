#!/usr/bin/env bash
#
# Run from the repo root:
#   cd <repo-root> && scripts/tests/opencode-turns.test.sh

set -euo pipefail

SCRIPT="$PWD/scripts/opencode-turns"

fail() {
  echo "$*" >&2
  exit 1
}

assert_eq() {
  local actual="$1"
  local expected="$2"
  local label="$3"

  if [[ "$actual" != "$expected" ]]; then
    fail "$label: expected <$expected>, got <$actual>"
  fi
}

assert_status_zero() {
  local status="$1"
  local label="$2"

  if [[ "$status" -ne 0 ]]; then
    fail "$label: expected exit 0, got $status; stderr: $(cat "$RUN_STDERR")"
  fi
}

assert_stdout_contains() {
  local pattern="$1"
  local label="$2"

  if ! grep -Fq "$pattern" "$RUN_STDOUT"; then
    fail "$label: stdout did not contain <$pattern>; stdout: $(cat "$RUN_STDOUT")"
  fi
}

assert_stdout_not_contains() {
  local pattern="$1"
  local label="$2"

  if grep -Fq "$pattern" "$RUN_STDOUT"; then
    fail "$label: stdout unexpectedly contained <$pattern>; stdout: $(cat "$RUN_STDOUT")"
  fi
}

write_window_filter_mock() {
  local path="$1"

  cat >"$path" <<'MOCK_OPENCODE'
#!/usr/bin/env bash
set -euo pipefail

if [[ "$1" == "session" && "$2" == "list" && "${3:-}" == "--json" ]]; then
  python3 - <<'PY'
import json
from datetime import datetime, timedelta, timezone

now = datetime.now(timezone.utc)
sessions = [
    {"id": "ses_recent_one", "updatedAt": (now - timedelta(hours=1)).isoformat().replace("+00:00", "Z")},
    {"sessionID": "ses_recent_two", "createdAt": (now - timedelta(hours=5)).isoformat().replace("+00:00", "Z")},
    {"sessionId": "ses_old", "updatedAt": (now - timedelta(hours=9)).isoformat().replace("+00:00", "Z")},
    {"id": "ses_without_timestamp"},
]
print(json.dumps(sessions))
PY
  exit 0
fi

if [[ "$1" == "export" ]]; then
  session_id="$2"
  printf '%s\n' "$session_id" >>"$OPENCODE_TURNS_EXPORT_LOG"
  printf '[{"sessionID":"%s","messageID":"turn-%s","timestamp":"2099-01-01T00:00:00Z","role":"assistant","content":"ok"}]\n' "$session_id" "$session_id"
  exit 0
fi

echo "unexpected opencode args: $*" >&2
exit 64
MOCK_OPENCODE

  chmod +x "$path"
}

write_timeout_mock() {
  local path="$1"

  cat >"$path" <<'MOCK_OPENCODE'
#!/usr/bin/env bash
set -euo pipefail

if [[ "$1" == "session" && "$2" == "list" && "${3:-}" == "--json" ]]; then
  cat <<'JSON'
[
  {"id":"ses_fast"},
  {"id":"ses_slow"}
]
JSON
  exit 0
fi

if [[ "$1" == "export" ]]; then
  session_id="$2"
  printf '%s\n' "$session_id" >>"$OPENCODE_TURNS_EXPORT_LOG"
  if [[ "$session_id" == "ses_slow" ]]; then
    sleep 5
  fi
  printf '[{"sessionID":"%s","messageID":"turn-%s","timestamp":"2099-01-01T00:00:00Z","role":"assistant","content":"ok"}]\n' "$session_id" "$session_id"
  exit 0
fi

echo "unexpected opencode args: $*" >&2
exit 64
MOCK_OPENCODE

  chmod +x "$path"
}

run_opencode_turns() {
  local tmpdir="$1"
  local opencode_bin="$2"

  RUN_STDOUT="$tmpdir/stdout.jsonl"
  RUN_STDERR="$tmpdir/stderr.txt"
  OPENCODE_TURNS_EXPORT_LOG="$tmpdir/exports.log"
  export OPENCODE_TURNS_EXPORT_LOG
  : >"$OPENCODE_TURNS_EXPORT_LOG"

  set +e
  OPENCODE_BIN="$opencode_bin" \
    OPENCODE_TURNS_WINDOW_HOURS="${OPENCODE_TURNS_WINDOW_HOURS:-6}" \
    OPENCODE_TURNS_MAX_SESSIONS="${OPENCODE_TURNS_MAX_SESSIONS:-25}" \
    OPENCODE_TURNS_CALL_TIMEOUT="${OPENCODE_TURNS_CALL_TIMEOUT:-20}" \
    OPENCODE_TURNS_DEADLINE="${OPENCODE_TURNS_DEADLINE:-60}" \
    "$SCRIPT" "$tmpdir/opencode-root" >"$RUN_STDOUT" 2>"$RUN_STDERR"
  RUN_STATUS=$?
  set -e
}

test_exports_only_recent_window_sessions() {
  local tmpdir
  tmpdir="$(mktemp -d)"
  trap "rm -rf '$tmpdir'" RETURN

  mkdir -p "$tmpdir/bin"
  write_window_filter_mock "$tmpdir/bin/opencode"

  run_opencode_turns "$tmpdir" "$tmpdir/bin/opencode"

  assert_status_zero "$RUN_STATUS" "$FUNCNAME"
  assert_eq "$(cat "$OPENCODE_TURNS_EXPORT_LOG")" $'ses_recent_one\nses_recent_two' "$FUNCNAME exports"
  assert_stdout_contains '"session_id":"ses_recent_one"' "$FUNCNAME recent one"
  assert_stdout_contains '"session_id":"ses_recent_two"' "$FUNCNAME recent two"
  assert_stdout_not_contains 'ses_old' "$FUNCNAME old excluded"
  assert_stdout_not_contains 'ses_without_timestamp' "$FUNCNAME timestampless excluded when list has timestamps"
  assert_stdout_not_contains '"degraded":true' "$FUNCNAME not degraded"
}

test_timeout_emits_degraded_best_effort_and_exits_zero() {
  local tmpdir
  tmpdir="$(mktemp -d)"
  trap "rm -rf '$tmpdir'" RETURN

  mkdir -p "$tmpdir/bin"
  write_timeout_mock "$tmpdir/bin/opencode"

  local start end elapsed
  start="$(date +%s)"
  OPENCODE_TURNS_CALL_TIMEOUT=1 OPENCODE_TURNS_DEADLINE=3 run_opencode_turns "$tmpdir" "$tmpdir/bin/opencode"
  end="$(date +%s)"
  elapsed=$((end - start))

  assert_status_zero "$RUN_STATUS" "$FUNCNAME"
  assert_stdout_contains '"session_id":"ses_fast"' "$FUNCNAME fast record"
  assert_stdout_contains '"degraded":true' "$FUNCNAME degraded marker"
  assert_stdout_contains '"count":1' "$FUNCNAME best count"
  if [[ "$elapsed" -gt 4 ]]; then
    fail "$FUNCNAME: expected deadline-bounded execution, took ${elapsed}s"
  fi
}

test_exports_only_recent_window_sessions
test_timeout_emits_degraded_best_effort_and_exits_zero

echo "opencode-turns tests passed"
