#!/usr/bin/env bash
#
# Run from the repo root:
#   cd <repo-root> && scripts/tests/anthropic-usage.test.sh

set -euo pipefail

SCRIPT="$PWD/scripts/anthropic-usage"
FIXTURE_DIR="$PWD/scripts/tests/fixtures/anthropic-usage"

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

if [[ -z "${ANTHROPIC_USAGE_MOCK_RESPONSE_FILE:-}" ]]; then
  echo "ANTHROPIC_USAGE_MOCK_RESPONSE_FILE is not set" >&2
  exit 64
fi

out_file=""
while [[ "$#" -gt 0 ]]; do
  case "$1" in
    -o)
      out_file="$2"
      shift 2
      ;;
    -w)
      shift 2
      ;;
    --max-time)
      shift 2
      ;;
    -H)
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done

if [[ -z "$out_file" ]]; then
  echo "mock curl did not receive -o" >&2
  exit 65
fi

cat "$ANTHROPIC_USAGE_MOCK_RESPONSE_FILE" >"$out_file"
printf '%s' "${ANTHROPIC_USAGE_MOCK_HTTP_CODE:-200}"
MOCK_CURL

  chmod +x "$path"
}

write_valid_credentials_file() {
  local path="$1"

  cat >"$path" <<'JSON'
{
  "claudeAiOauth": {
    "accessToken": "test-access-token"
  }
}
JSON
}

run_anthropic_usage() {
  local response_fixture="$1"
  local creds_file="$2"
  local tmpdir="$3"
  local mock_bin="$tmpdir/mock-bin"

  mkdir -p "$mock_bin"
  write_mock_curl "$mock_bin/curl"

  RUN_STDOUT="$tmpdir/stdout.json"
  RUN_STDERR="$tmpdir/stderr.txt"

  set +e
  PATH="$mock_bin:$PATH" \
    ANTHROPIC_USAGE_MOCK_RESPONSE_FILE="$response_fixture" \
    ANTHROPIC_USAGE_MOCK_HTTP_CODE="${ANTHROPIC_USAGE_MOCK_HTTP_CODE:-200}" \
    bash "$SCRIPT" "$creds_file" >"$RUN_STDOUT" 2>"$RUN_STDERR"
  RUN_STATUS=$?
  set -e
}

assert_weekly_window() {
  local index="$1"
  local used_percent="$2"
  local resets_at="$3"
  local label="$4"

  assert_jq_eq ".windows[$index].label" "weekly" "$label label"
  assert_jq_eq ".windows[$index].used_percent" "$used_percent" "$label used_percent"
  assert_jq_eq ".windows[$index].resets_at" "$resets_at" "$label resets_at"
  assert_jq_eq ".windows[$index] | has(\"limit\")" "false" "$label limit omitted"
  assert_jq_eq ".windows[$index] | has(\"remaining\")" "false" "$label remaining omitted"
}

assert_five_hour_window() {
  local index="$1"
  local used_percent="$2"
  local resets_at="$3"
  local label="$4"

  assert_jq_eq ".windows[$index].label" "5h-burst" "$label label"
  assert_jq_eq ".windows[$index].used_percent" "$used_percent" "$label used_percent"
  assert_jq_eq ".windows[$index].resets_at" "$resets_at" "$label resets_at"
  assert_jq_eq ".windows[$index] | has(\"limit\")" "false" "$label limit omitted"
  assert_jq_eq ".windows[$index] | has(\"remaining\")" "false" "$label remaining omitted"
}

test_anthropic_usage_emits_labeled_percent_windows_on_normal_response() {
  local tmpdir
  tmpdir="$(mktemp -d)"
  trap "rm -rf '$tmpdir'" EXIT

  local creds_file="$tmpdir/credentials.json"
  write_valid_credentials_file "$creds_file"

  run_anthropic_usage "$FIXTURE_DIR/normal-response.json" "$creds_file" "$tmpdir"

  assert_status_zero "$RUN_STATUS" "$FUNCNAME"
  assert_jq_eq '.windows | length' "2" "$FUNCNAME window count"
  assert_weekly_window 0 "46" "2026-04-28T18:13:20Z" "$FUNCNAME weekly window"
  assert_five_hour_window 1 "13" "2026-04-22T01:06:40Z" "$FUNCNAME five-hour window"
}

test_anthropic_usage_emits_empty_windows_on_empty_response() {
  local tmpdir
  tmpdir="$(mktemp -d)"
  trap "rm -rf '$tmpdir'" EXIT

  local creds_file="$tmpdir/credentials.json"
  write_valid_credentials_file "$creds_file"

  run_anthropic_usage "$FIXTURE_DIR/empty-response.json" "$creds_file" "$tmpdir"

  assert_status_zero "$RUN_STATUS" "$FUNCNAME"
  assert_jq_eq '.windows | length' "0" "$FUNCNAME window count"
}

test_anthropic_usage_error_response_exits_nonzero() {
  local tmpdir
  tmpdir="$(mktemp -d)"
  trap "rm -rf '$tmpdir'" EXIT

  local creds_file="$tmpdir/credentials.json"
  write_valid_credentials_file "$creds_file"

  ANTHROPIC_USAGE_MOCK_HTTP_CODE=401 \
    run_anthropic_usage "$FIXTURE_DIR/error-401.json" "$creds_file" "$tmpdir"

  assert_status_nonzero "$RUN_STATUS" "$FUNCNAME"
  assert_stdout_empty "$FUNCNAME"
  assert_stderr_matches '(401|auth|invalid|token)' "$FUNCNAME"
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
  done < <(compgen -A function | grep -E '^test_' | LC_ALL=C sort)

  if [[ "${#failed[@]}" -eq 0 ]]; then
    echo "All anthropic-usage tests passed."
    return 0
  fi

  echo
  echo "${#failed[@]} anthropic-usage test(s) failed:"
  printf ' - %s\n' "${failed[@]}"
  return 1
}

main "$@"
