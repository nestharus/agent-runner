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

overall=0

for entry in "${tests[@]}"; do
  test_id="${entry%%:*}"
  script="${entry#*:}"
  if [[ ! -x "$script" ]]; then
    printf '%s: FAIL — missing or non-executable helper: %s\n' "$test_id" "$script"
    overall=1
    continue
  fi

  if evidence="$("$script" 2>&1)"; then
    printf '%s: PASS — %s\n' "$test_id" "$evidence"
  else
    printf '%s: FAIL — %s\n' "$test_id" "$evidence"
    overall=1
  fi
done

if [[ "$overall" -eq 0 ]]; then
  printf 'EVAL_RESULT: PASS\n'
else
  printf 'EVAL_RESULT: FAIL\n'
fi

exit "$overall"
