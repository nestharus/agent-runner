#!/usr/bin/env bash
set -euo pipefail

eval_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$eval_dir"
exec python3 -m unittest contract_tests "$@"
