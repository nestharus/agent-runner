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

values_equal() {
  [[ "$1" == "$2" ]]
}

assert_eq_failure_message() {
  local label="$1"
  local expected="$2"
  local actual="$3"

  printf '%s: expected <%s>, got <%s>' "$label" "$expected" "$actual"
}

assert_eq() {
  local actual="$1"
  local expected="$2"
  local label="$3"

  if ! values_equal "$actual" "$expected"; then
    fail "$(assert_eq_failure_message "$label" "$expected" "$actual")"
  fi
}

status_zero() {
  [[ "$1" -eq 0 ]]
}

status_zero_failure_message() {
  local label="$1"
  local status="$2"

  printf '%s: expected exit 0, got %s; stderr: %s' "$label" "$status" "$(cat "$RUN_STDERR")"
}

assert_status_zero() {
  local status="$1"
  local label="$2"

  if ! status_zero "$status"; then
    fail "$(status_zero_failure_message "$label" "$status")"
  fi
}

stdout_contains_pattern() {
  grep -Fq "$1" "$RUN_STDOUT"
}

stdout_contains_failure_message() {
  local label="$1"
  local pattern="$2"

  printf '%s: stdout did not contain <%s>; stdout: %s' "$label" "$pattern" "$(cat "$RUN_STDOUT")"
}

assert_stdout_contains() {
  local pattern="$1"
  local label="$2"

  if ! stdout_contains_pattern "$pattern"; then
    fail "$(stdout_contains_failure_message "$label" "$pattern")"
  fi
}

stdout_excludes_pattern() {
  ! grep -Fq "$1" "$RUN_STDOUT"
}

stdout_unexpected_failure_message() {
  local label="$1"
  local pattern="$2"

  printf '%s: stdout unexpectedly contained <%s>; stdout: %s' "$label" "$pattern" "$(cat "$RUN_STDOUT")"
}

assert_stdout_not_contains() {
  local pattern="$1"
  local label="$2"

  if ! stdout_excludes_pattern "$pattern"; then
    fail "$(stdout_unexpected_failure_message "$label" "$pattern")"
  fi
}

assert_current_export_records() {
  local label="$1"

  python3 - "$RUN_STDOUT" <<'PY' || fail "$label: current-format export records did not match"
import json
import sys

path = sys.argv[1]
with open(path, encoding="utf-8") as handle:
    records = [json.loads(line) for line in handle if line.strip()]

expected = [
    {
        "session_id": "ses_current_info",
        "turn_id": "msg_user_current",
        "timestamp": "2023-11-14T22:13:20Z",
        "role": "user",
        "body": [
            {
                "type": "text",
                "text": "[OULIPOLY-DELIVERY 123e4567-e89b-12d3-a456-426614174000] wake please",
            }
        ],
    },
    {
        "session_id": "ses_current_info",
        "turn_id": "msg_assistant_current",
        "timestamp": "2023-11-14T22:13:25Z",
        "role": "assistant",
        "body": [{"type": "text", "text": "confirmed"}],
    },
]

if records != expected:
    print("expected:", json.dumps(expected, indent=2), file=sys.stderr)
    print("actual:", json.dumps(records, indent=2), file=sys.stderr)
    raise SystemExit(1)
PY
}

assert_session_id_env_export_records() {
  local label="$1"

  python3 - "$RUN_STDOUT" <<'PY' || fail "$label: SESSION_ID-env export records did not match"
import json
import sys

path = sys.argv[1]
with open(path, encoding="utf-8") as handle:
    records = [json.loads(line) for line in handle if line.strip()]

expected = [
    {
        "session_id": "ses_env_requested",
        "turn_id": "msg_user_env",
        "timestamp": "2023-11-14T22:13:20Z",
        "role": "user",
        "body": [{"type": "text", "text": "wake via SESSION_ID env"}],
    },
    {
        "session_id": "ses_env_requested",
        "turn_id": "msg_assistant_env",
        "timestamp": "2023-11-14T22:13:25Z",
        "role": "assistant",
        "body": [{"type": "text", "text": "confirmed via env"}],
    },
]

if records != expected:
    print("expected:", json.dumps(expected, indent=2), file=sys.stderr)
    print("actual:", json.dumps(records, indent=2), file=sys.stderr)
    raise SystemExit(1)
PY
}

file_size_bytes() {
  local path="$1"

  if [[ ! -f "$path" ]]; then
    missing_file_size_bytes
    return
  fi
  existing_file_size_bytes "$path"
}

existing_file_size_bytes() {
  local path="$1"

  wc -c <"$path" | tr -d ' '
}

missing_file_size_bytes() {
  echo 0
}

marker_size_sample() {
  local marker="$1"

  file_size_bytes "$marker"
  sleep 1
  file_size_bytes "$marker"
}

marker_size_stable() {
  [[ "$1" == "$2" ]]
}

marker_growth_failure_message() {
  local label="$1"
  local before="$2"
  local after="$3"

  printf '%s: descendant marker kept growing after timeout kill (%s -> %s bytes)' "$label" "$before" "$after"
}

assert_marker_stopped_growing() {
  local marker="$1"
  local label="$2"
  local -a sizes
  local before after

  mapfile -t sizes < <(marker_size_sample "$marker")
  before="${sizes[0]}"
  after="${sizes[1]}"
  if ! marker_size_stable "$before" "$after"; then
    fail "$(marker_growth_failure_message "$label" "$before" "$after")"
  fi
}

process_state() {
  local pid="$1"

  if ! process_exists "$pid"; then
    absent_process_state
    return
  fi
  read_process_state "$pid"
}

process_exists() {
  local pid="$1"

  kill -0 "$pid" 2>/dev/null
}

read_process_state() {
  local pid="$1"

  ps -o stat= -p "$pid" 2>/dev/null | tr -d ' ' || true
}

absent_process_state() {
  echo absent
}

process_state_allowed() {
  local state="$1"

  [[ "$state" == absent || "$state" == Z* ]]
}

process_running_failure_message() {
  local label="$1"
  local pid="$2"
  local state="$3"

  printf '%s: descendant process %s survived timeout kill with state <%s>' "$label" "$pid" "$state"
}

assert_process_not_running() {
  local pid="$1"
  local label="$2"
  local state

  state="$(process_state "$pid")"
  if process_state_allowed "$state"; then
    return
  fi
  fail "$(process_running_failure_message "$label" "$pid" "$state")"
}

write_executable_mock() {
  local path="$1"
  local body_writer="$2"

  "$body_writer" >"$path"
  chmod +x "$path"
}

emit_timestampless_cap_mock_body() {
  cat <<'MOCK_OPENCODE'
#!/usr/bin/env bash
set -euo pipefail

if [[ "$1" == "session" && "$2" == "list" && "${3:-}" == "--json" ]]; then
  cat <<'JSON'
[
  {"id":"ses_cap_one"},
  {"id":"ses_cap_two"},
  {"id":"ses_cap_three"},
  {"id":"ses_cap_four"},
  {"id":"ses_cap_five"}
]
JSON
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
}

write_timestampless_cap_mock() {
  local path="$1"

  write_executable_mock "$path" emit_timestampless_cap_mock_body
}

emit_window_filter_mock_body() {
  cat <<'MOCK_OPENCODE'
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
}

write_window_filter_mock() {
  local path="$1"

  write_executable_mock "$path" emit_window_filter_mock_body
}

emit_timeout_mock_body() {
  cat <<'MOCK_OPENCODE'
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
}

write_timeout_mock() {
  local path="$1"

  write_executable_mock "$path" emit_timeout_mock_body
}

emit_descendant_timeout_mock_body() {
  cat <<'MOCK_OPENCODE'
#!/usr/bin/env bash
set -euo pipefail

if [[ "$1" == "session" && "$2" == "list" && "${3:-}" == "--json" ]]; then
  printf '[{"id":"ses_descendant"}]\n'
  exit 0
fi

if [[ "$1" == "export" ]]; then
  session_id="$2"
  printf '%s\n' "$session_id" >>"$OPENCODE_TURNS_EXPORT_LOG"
  (
    printf tick >>"$OPENCODE_TURNS_DESCENDANT_MARKER"
    while true; do
      sleep 0.2
      printf tick >>"$OPENCODE_TURNS_DESCENDANT_MARKER"
    done
  ) &
  printf '%s\n' "$!" >"$OPENCODE_TURNS_DESCENDANT_PID"
  sleep 30
  exit 0
fi

echo "unexpected opencode args: $*" >&2
exit 64
MOCK_OPENCODE
}

write_descendant_timeout_mock() {
  local path="$1"

  write_executable_mock "$path" emit_descendant_timeout_mock_body
}

emit_command_derivation_mock_body() {
  cat <<'MOCK_OPENCODE'
#!/usr/bin/env bash
set -euo pipefail

printf '%s\n' "$(basename "$0")" >>"$OPENCODE_TURNS_COMMAND_LOG"

if [[ "$1" == "export" ]]; then
  session_id="$2"
  printf '%s\n' "$session_id" >>"$OPENCODE_TURNS_EXPORT_LOG"
  printf '[{"sessionID":"%s","messageID":"turn-%s","timestamp":"2099-01-01T00:00:00Z","role":"user","content":"submitted"}]\n' "$session_id" "$session_id"
  exit 0
fi

echo "unexpected opencode args: $*" >&2
exit 64
MOCK_OPENCODE
}

write_command_derivation_mock() {
  local path="$1"

  write_executable_mock "$path" emit_command_derivation_mock_body
}

emit_current_export_mock_body() {
  cat <<'MOCK_OPENCODE'
#!/usr/bin/env bash
set -euo pipefail

if [[ "$1" == "session" && "$2" == "list" && "${3:-}" == "--json" ]]; then
  cat <<'JSON'
[
  {"id":"ses_current_requested"}
]
JSON
  exit 0
fi

if [[ "$1" == "export" ]]; then
  session_id="$2"
  printf '%s\n' "$session_id" >>"$OPENCODE_TURNS_EXPORT_LOG"
  cat <<'JSON'
{
  "info": {
    "id": "ses_current_info",
    "time": {"created": 1700000000000}
  },
  "messages": [
    {
      "info": {
        "role": "user",
        "time": {"created": 1700000000000},
        "id": "msg_user_current",
        "sessionID": "ses_current_info"
      },
      "parts": [
        {
          "type": "text",
          "text": "[OULIPOLY-DELIVERY 123e4567-e89b-12d3-a456-426614174000] wake please"
        }
      ]
    },
    {
      "info": {
        "role": "assistant",
        "time": {"created": 1700000005000},
        "id": "msg_assistant_current",
        "sessionID": "ses_current_info"
      },
      "parts": [{"type": "text", "text": "confirmed"}]
    }
  ]
}
JSON
  exit 0
fi

echo "unexpected opencode args: $*" >&2
exit 64
MOCK_OPENCODE
}

write_current_export_mock() {
  local path="$1"

  write_executable_mock "$path" emit_current_export_mock_body
}

emit_session_id_env_mock_body() {
  cat <<'MOCK_OPENCODE'
#!/usr/bin/env bash
set -euo pipefail

if [[ "$1" == "session" && "$2" == "list" && "${3:-}" == "--json" ]]; then
  printf '[]\n'
  exit 0
fi

if [[ "$1" == "export" ]]; then
  session_id="$2"
  printf '%s\n' "$session_id" >>"$OPENCODE_TURNS_EXPORT_LOG"
  if [[ "$session_id" != "ses_env_requested" ]]; then
    printf '[]\n'
    exit 0
  fi
  cat <<'JSON'
{
  "info": {
    "id": "ses_env_requested",
    "time": {"created": 1700000000000}
  },
  "messages": [
    {
      "info": {
        "role": "user",
        "time": {"created": 1700000000000},
        "id": "msg_user_env",
        "sessionID": "ses_env_requested"
      },
      "parts": [{"type": "text", "text": "wake via SESSION_ID env"}]
    },
    {
      "info": {
        "role": "assistant",
        "time": {"created": 1700000005000},
        "id": "msg_assistant_env",
        "sessionID": "ses_env_requested"
      },
      "parts": [{"type": "text", "text": "confirmed via env"}]
    }
  ]
}
JSON
  exit 0
fi

echo "unexpected opencode args: $*" >&2
exit 64
MOCK_OPENCODE
}

write_session_id_env_mock() {
  local path="$1"

  write_executable_mock "$path" emit_session_id_env_mock_body
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

run_opencode_turns_with_session_id_env_on_path() {
  local tmpdir="$1"
  local session_id="$2"

  RUN_STDOUT="$tmpdir/stdout.jsonl"
  RUN_STDERR="$tmpdir/stderr.txt"
  OPENCODE_TURNS_EXPORT_LOG="$tmpdir/exports.log"
  export OPENCODE_TURNS_EXPORT_LOG
  : >"$OPENCODE_TURNS_EXPORT_LOG"

  set +e
  env -u OPENCODE_BIN \
    PATH="$tmpdir/bin:$PATH" \
    SESSION_ID="$session_id" \
    OPENCODE_TURNS_WINDOW_HOURS=6 \
    OPENCODE_TURNS_MAX_SESSIONS=25 \
    OPENCODE_TURNS_CALL_TIMEOUT=20 \
    OPENCODE_TURNS_DEADLINE=60 \
    "$SCRIPT" "$tmpdir/opencode-root" >"$RUN_STDOUT" 2>"$RUN_STDERR"
  RUN_STATUS=$?
  set -e
}

test_timestampless_session_list_applies_max_sessions_cap() {
  local tmpdir
  tmpdir="$(mktemp -d)"
  trap "rm -rf '$tmpdir'" RETURN

  mkdir -p "$tmpdir/bin"
  write_timestampless_cap_mock "$tmpdir/bin/opencode"

  OPENCODE_TURNS_MAX_SESSIONS=3 run_opencode_turns "$tmpdir" "$tmpdir/bin/opencode"

  assert_status_zero "$RUN_STATUS" "$FUNCNAME"
  assert_eq "$(cat "$OPENCODE_TURNS_EXPORT_LOG")" $'ses_cap_one\nses_cap_two\nses_cap_three' "$FUNCNAME exports"
  assert_eq "$(wc -l <"$OPENCODE_TURNS_EXPORT_LOG" | tr -d ' ')" "3" "$FUNCNAME export count"
  assert_stdout_contains '"session_id":"ses_cap_one"' "$FUNCNAME cap one"
  assert_stdout_contains '"session_id":"ses_cap_three"' "$FUNCNAME cap three"
  assert_stdout_not_contains 'ses_cap_four' "$FUNCNAME cap excludes fourth"
  assert_stdout_not_contains 'ses_cap_five' "$FUNCNAME cap excludes fifth"
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

test_timeout_kills_opencode_process_group_descendant() {
  local tmpdir
  tmpdir="$(mktemp -d)"
  trap "rm -rf '$tmpdir'" RETURN

  mkdir -p "$tmpdir/bin"
  write_descendant_timeout_mock "$tmpdir/bin/opencode"
  OPENCODE_TURNS_DESCENDANT_MARKER="$tmpdir/descendant-marker"
  OPENCODE_TURNS_DESCENDANT_PID="$tmpdir/descendant.pid"
  export OPENCODE_TURNS_DESCENDANT_MARKER OPENCODE_TURNS_DESCENDANT_PID

  OPENCODE_TURNS_CALL_TIMEOUT=1 OPENCODE_TURNS_DEADLINE=3 run_opencode_turns "$tmpdir" "$tmpdir/bin/opencode"

  assert_status_zero "$RUN_STATUS" "$FUNCNAME"
  assert_stdout_contains '"degraded":true' "$FUNCNAME degraded marker"
  assert_stdout_contains '"count":0' "$FUNCNAME no completed assistant records"
  [[ -s "$OPENCODE_TURNS_DESCENDANT_PID" ]] || fail "$FUNCNAME: descendant pid file was not written"
  [[ -s "$OPENCODE_TURNS_DESCENDANT_MARKER" ]] || fail "$FUNCNAME: descendant marker was not written"

  local descendant_pid
  descendant_pid="$(cat "$OPENCODE_TURNS_DESCENDANT_PID")"
  assert_marker_stopped_growing "$OPENCODE_TURNS_DESCENDANT_MARKER" "$FUNCNAME"
  assert_process_not_running "$descendant_pid" "$FUNCNAME"
}

test_base_dir_suffix_selects_matching_opencode_command() {
  local tmpdir
  tmpdir="$(mktemp -d)"
  trap "rm -rf '$tmpdir'" RETURN

  mkdir -p "$tmpdir/bin" "$tmpdir/.opencode3/opencode"
  write_command_derivation_mock "$tmpdir/bin/opencode3"
  RUN_STDOUT="$tmpdir/stdout.jsonl"
  RUN_STDERR="$tmpdir/stderr.txt"
  OPENCODE_TURNS_EXPORT_LOG="$tmpdir/exports.log"
  OPENCODE_TURNS_COMMAND_LOG="$tmpdir/commands.log"
  export OPENCODE_TURNS_EXPORT_LOG OPENCODE_TURNS_COMMAND_LOG
  : >"$OPENCODE_TURNS_EXPORT_LOG"
  : >"$OPENCODE_TURNS_COMMAND_LOG"

  set +e
  env -u OPENCODE_BIN \
    PATH="$tmpdir/bin:$PATH" \
    SESSION_ID=ses_env_ignored \
    OPENCODE_TURNS_WINDOW_HOURS=6 \
    OPENCODE_TURNS_MAX_SESSIONS=25 \
    OPENCODE_TURNS_CALL_TIMEOUT=20 \
    OPENCODE_TURNS_DEADLINE=60 \
    "$SCRIPT" "$tmpdir/.opencode3/opencode" ses_account >"$RUN_STDOUT" 2>"$RUN_STDERR"
  RUN_STATUS=$?
  set -e

  assert_status_zero "$RUN_STATUS" "$FUNCNAME"
  assert_eq "$(cat "$OPENCODE_TURNS_COMMAND_LOG")" "opencode3" "$FUNCNAME command"
  assert_eq "$(cat "$OPENCODE_TURNS_EXPORT_LOG")" "ses_account" "$FUNCNAME export"
  assert_stdout_contains '"role":"user"' "$FUNCNAME user record"
}

test_current_export_info_metadata_emits_turns() {
  local tmpdir
  tmpdir="$(mktemp -d)"
  trap "rm -rf '$tmpdir'" RETURN

  mkdir -p "$tmpdir/bin"
  write_current_export_mock "$tmpdir/bin/opencode"

  run_opencode_turns "$tmpdir" "$tmpdir/bin/opencode"

  assert_status_zero "$RUN_STATUS" "$FUNCNAME"
  assert_eq "$(cat "$OPENCODE_TURNS_EXPORT_LOG")" "ses_current_requested" "$FUNCNAME export"
  assert_current_export_records "$FUNCNAME"
}

test_session_id_env_used_when_no_positional_sessions() {
  local tmpdir
  tmpdir="$(mktemp -d)"
  trap "rm -rf '$tmpdir'" RETURN

  mkdir -p "$tmpdir/bin"
  write_session_id_env_mock "$tmpdir/bin/opencode"

  run_opencode_turns_with_session_id_env_on_path "$tmpdir" "ses_env_requested"

  assert_status_zero "$RUN_STATUS" "$FUNCNAME"
  assert_eq "$(cat "$OPENCODE_TURNS_EXPORT_LOG")" "ses_env_requested" "$FUNCNAME export"
  assert_session_id_env_export_records "$FUNCNAME"
}

test_timestampless_session_list_applies_max_sessions_cap
test_exports_only_recent_window_sessions
test_timeout_emits_degraded_best_effort_and_exits_zero
test_timeout_kills_opencode_process_group_descendant
test_base_dir_suffix_selects_matching_opencode_command
test_current_export_info_metadata_emits_turns
test_session_id_env_used_when_no_positional_sessions

echo "opencode-turns tests passed"
