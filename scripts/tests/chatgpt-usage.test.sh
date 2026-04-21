#!/usr/bin/env bash
#
# Run from the repo root:
#   cd <repo-root> && scripts/tests/chatgpt-usage.test.sh

set -euo pipefail

ROOT="$PWD"
SCRIPT="$PWD/scripts/chatgpt-usage"
FIXTURE_DIR="$PWD/scripts/tests/fixtures/chatgpt-usage"

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

assert_status_nonzero() {
  local status="$1"
  local label="$2"

  if [[ "$status" -eq 0 ]]; then
    fail "$label: expected non-zero exit"
  fi
}

assert_stdout_empty() {
  local label="$1"

  if [[ -s "$RUN_STDOUT" ]]; then
    fail "$label: expected empty stdout, got: $(cat "$RUN_STDOUT")"
  fi
}

assert_stderr_matches() {
  local pattern="$1"
  local label="$2"

  if ! grep -Eiq "$pattern" "$RUN_STDERR"; then
    fail "$label: stderr did not match /$pattern/; stderr: $(cat "$RUN_STDERR")"
  fi
}

assert_jq_eq() {
  local filter="$1"
  local expected="$2"
  local label="$3"
  local actual

  actual="$(jq -r "$filter" "$RUN_STDOUT")" || fail "$label: jq failed for filter $filter; stdout: $(cat "$RUN_STDOUT")"
  assert_eq "$actual" "$expected" "$label"
}

write_mock_curl() {
  local path="$1"

  cat >"$path" <<'MOCK_CURL'
#!/usr/bin/env bash
set -euo pipefail

if [[ -z "${CHATGPT_USAGE_MOCK_RESPONSE_FILE:-}" ]]; then
  echo "CHATGPT_USAGE_MOCK_RESPONSE_FILE is not set" >&2
  exit 64
fi

cat "$CHATGPT_USAGE_MOCK_RESPONSE_FILE"
MOCK_CURL

  chmod +x "$path"
}

write_valid_auth_file() {
  local path="$1"

  cat >"$path" <<'JSON'
{
  "tokens": {
    "access_token": "test-access-token",
    "account_id": "test-account-id"
  }
}
JSON
}

run_chatgpt_usage() {
  local response_fixture="$1"
  local auth_file="$2"
  local tmpdir="$3"
  local mock_bin="$tmpdir/mock-bin"

  mkdir -p "$mock_bin"
  write_mock_curl "$mock_bin/curl"

  RUN_STDOUT="$tmpdir/stdout.json"
  RUN_STDERR="$tmpdir/stderr.txt"

  set +e
  PATH="$mock_bin:$PATH" \
    CHATGPT_USAGE_MOCK_RESPONSE_FILE="$response_fixture" \
    "$SCRIPT" "$auth_file" >"$RUN_STDOUT" 2>"$RUN_STDERR"
  RUN_STATUS=$?
  set -e
}

assert_weekly_window() {
  local index="$1"
  local used_percent="$2"
  local resets_at="$3"
  local label="$4"

  assert_jq_eq ".windows[$index].used_percent" "$used_percent" "$label used_percent"
  assert_jq_eq ".windows[$index].resets_at" "$resets_at" "$label resets_at"
}

assert_five_hour_window() {
  local index="$1"
  local used_percent="$2"
  local resets_at="$3"
  local label="$4"

  assert_jq_eq ".windows[$index].used_percent" "$used_percent" "$label used_percent"
  assert_jq_eq ".windows[$index].resets_at" "$resets_at" "$label resets_at"
}

test_chatgpt_usage_emits_two_windows_on_normal_response() {
  local tmpdir
  tmpdir="$(mktemp -d)"
  trap "rm -rf '$tmpdir'" EXIT

  local auth_file="$tmpdir/auth.json"
  write_valid_auth_file "$auth_file"

  run_chatgpt_usage "$FIXTURE_DIR/normal-response.json" "$auth_file" "$tmpdir"

  assert_status_zero "$RUN_STATUS" "$FUNCNAME"
  assert_jq_eq '.windows | length' "2" "$FUNCNAME window count"
  # Fixture uses unix `reset_at`; script converts via jq `todate` to RFC3339.
  assert_weekly_window 0 "42" "2026-04-27T14:26:40Z" "$FUNCNAME weekly window"
  assert_five_hour_window 1 "17" "2026-04-21T22:20:00Z" "$FUNCNAME five-hour window"
}

test_chatgpt_usage_emits_one_window_when_only_weekly_present() {
  local tmpdir
  tmpdir="$(mktemp -d)"
  trap "rm -rf '$tmpdir'" EXIT

  local auth_file="$tmpdir/auth.json"
  write_valid_auth_file "$auth_file"

  run_chatgpt_usage "$FIXTURE_DIR/only-weekly.json" "$auth_file" "$tmpdir"

  assert_status_zero "$RUN_STATUS" "$FUNCNAME"
  assert_jq_eq '.windows | length' "1" "$FUNCNAME window count"
  assert_weekly_window 0 "64" "2026-04-28T18:13:20Z" "$FUNCNAME weekly window"
}

test_chatgpt_usage_emits_one_window_when_only_five_hour_present() {
  local tmpdir
  tmpdir="$(mktemp -d)"
  trap "rm -rf '$tmpdir'" EXIT

  local auth_file="$tmpdir/auth.json"
  write_valid_auth_file "$auth_file"

  run_chatgpt_usage "$FIXTURE_DIR/only-five-hour.json" "$auth_file" "$tmpdir"

  assert_status_zero "$RUN_STATUS" "$FUNCNAME"
  assert_jq_eq '.windows | length' "1" "$FUNCNAME window count"
  assert_five_hour_window 0 "23" "2026-04-22T01:06:40Z" "$FUNCNAME five-hour window"
}

assert_credential_failure() {
  local auth_file="$1"
  local stderr_pattern="$2"
  local label="$3"
  local tmpdir="$4"

  mkdir -p "$tmpdir"
  run_chatgpt_usage "$FIXTURE_DIR/normal-response.json" "$auth_file" "$tmpdir"

  assert_status_nonzero "$RUN_STATUS" "$label"
  assert_stdout_empty "$label"
  assert_stderr_matches "$stderr_pattern" "$label"
}

test_chatgpt_usage_credential_failure_exits_nonzero() {
  local tmpdir
  tmpdir="$(mktemp -d)"
  trap "rm -rf '$tmpdir'" EXIT

  assert_credential_failure "/tmp/nonexistent-chatgpt-usage-$$-$RANDOM" \
    '(credential|auth|readable)' \
    "$FUNCNAME unreadable auth file" \
    "$tmpdir/unreadable"

  assert_credential_failure "$FIXTURE_DIR/empty-tokens.json" \
    '(token|account|credential|auth|missing)' \
    "$FUNCNAME missing token fields" \
    "$tmpdir/missing-tokens"
}

scripts_readme_references_chatgpt_usage_adapter() {
  # Anchor assertions on the *install command* and the *adapter table row*
  # so a mention in unrelated prose (e.g. "removed the chatgpt-usage script")
  # doesn't falsely pass. The README rows we rely on:
  #   README.md:
  #     | `chatgpt-usage ~/.codex/auth.json` | ...
  #     install -m 755 scripts/anthropic-usage scripts/chatgpt-usage scripts/zai-usage ~/.local/bin/
  #   scripts/README.md:
  #     `chatgpt-usage ~/.codex/auth.json` (weekly + 5h)
  grep -Eq '^install .*scripts/chatgpt-usage' "$ROOT/README.md" \
    || { echo "README.md: install command does not reference scripts/chatgpt-usage" >&2; return 1; }
  grep -Eq '^\| `chatgpt-usage ' "$ROOT/README.md" \
    || { echo "README.md: adapter table row for chatgpt-usage missing" >&2; return 1; }
  grep -Fq 'chatgpt-usage ~/.codex/auth.json' "$ROOT/scripts/README.md" \
    || { echo "scripts/README.md: inventory entry for chatgpt-usage missing" >&2; return 1; }
}

main() {
  local failed=()
  local test_name

  while IFS= read -r test_name; do
    if ( "$test_name" ); then
      echo "PASS $test_name"
    else
      echo "FAIL $test_name"
      failed+=("$test_name")
    fi
  done < <(compgen -A function | grep -E '^(test_|scripts_readme_)' | LC_ALL=C sort)

  if [[ "${#failed[@]}" -eq 0 ]]; then
    echo "All chatgpt-usage tests passed."
    return 0
  fi

  echo
  echo "${#failed[@]} chatgpt-usage test(s) failed:"
  printf ' - %s\n' "${failed[@]}"
  return 1
}

main "$@"
