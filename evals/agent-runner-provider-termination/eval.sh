#!/usr/bin/env bash
set -euo pipefail

eval_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
contract_tests_dir="$eval_dir/contract_tests"

tests=(
  "T1:$contract_tests_dir/t01_claude_quota.sh"
  "T2:$contract_tests_dir/t02_claude_status_matrix.sh"
  "T3:$contract_tests_dir/t03_codex_quota.sh"
  "T4:$contract_tests_dir/t04_codex_status_matrix.sh"
  "T5:$contract_tests_dir/t05_openai_compat_quota.sh"
  "T6:$contract_tests_dir/t06_openai_compat_status_matrix.sh"
  "T7:$contract_tests_dir/t07_dispatch_table.sh"
  "T8:$contract_tests_dir/t08_network_boundary.sh"
  "T9:$contract_tests_dir/t09_marker_payload_schema.sh"
  "T10:$contract_tests_dir/t10_fixture_roundtrip.sh"
  "T11:$contract_tests_dir/t11_schema_ids.sh"
  "T12:$contract_tests_dir/t12_privacy_bounds.sh"
  "T13:$contract_tests_dir/t13_coupling_declarations.sh"
)

validate_test_script() {
  local script="$1"

  [[ -x "$script" ]]
}

parse_colon_entry() {
  local entry="$1"

  printf '%s\n' "${entry%%:*}"
  printf '%s\n' "${entry#*:}"
}

validate_environment() {
  local overall=0
  local entry script
  local -a parsed_entry

  for entry in "${tests[@]}"; do
    readarray -t parsed_entry < <(parse_colon_entry "$entry")
    script="${parsed_entry[1]}"
    validate_test_script "$script" || overall=1
  done

  return "$overall"
}

run_contract_test() {
  local test_id="$1"
  local script="$2"
  local evidence

  if ! validate_test_script "$script"; then
    format_missing_helper "$test_id" "$script"
    return 1
  fi

  if evidence="$("$script" 2>&1)"; then
    format_test_result "$test_id" "PASS" "$evidence"
  else
    format_test_result "$test_id" "FAIL" "$evidence"
    return 1
  fi
}

run_all_contract_tests() {
  local overall=0
  local entry test_id script
  local -a parsed_entry

  for entry in "${tests[@]}"; do
    readarray -t parsed_entry < <(parse_colon_entry "$entry")
    test_id="${parsed_entry[0]}"
    script="${parsed_entry[1]}"
    run_contract_test "$test_id" "$script" || overall=1
  done

  return "$overall"
}

format_missing_helper() {
  local test_id="$1"
  local script="$2"

  printf '%s: FAIL — missing or non-executable helper: %s\n' "$test_id" "$script"
}

format_test_result() {
  local test_id="$1"
  local status="$2"
  local evidence="$3"

  printf '%s: %s — %s\n' "$test_id" "$status" "$evidence"
}

format_result() {
  local overall="$1"

  if [[ "$overall" -eq 0 ]]; then
    printf 'EVAL_RESULT: PASS\n'
  else
    printf 'EVAL_RESULT: FAIL\n'
  fi
}

main() {
  local overall=0

  validate_environment || overall=1
  run_all_contract_tests || overall=1
  format_result "$overall"
  return "$overall"
}

main
