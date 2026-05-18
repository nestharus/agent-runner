#!/usr/bin/env bash
set -euo pipefail

# ## Declared roles
# - orchestration
# - validator
# - formatter
# - parser
# - accessor
# - mapper
# - filter
# - predicate

contract_tests_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
eval_dir="$(cd "$contract_tests_dir/.." && pwd)"
repo_root="$(cd "$eval_dir/../.." && pwd)"

run_contract_test() {
  local test_id="$1"
  EVAL_DIR="$eval_dir" REPO_ROOT="$repo_root" python3 "$contract_tests_dir/verifier.py" "$test_id"
}
