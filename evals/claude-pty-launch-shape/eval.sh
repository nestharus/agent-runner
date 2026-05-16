#!/usr/bin/env bash
set -euo pipefail

eval_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
fixtures_dir="$eval_dir/fixtures"
baseline_json="$fixtures_dir/truth-table-baseline.json"
baseline_helper="$eval_dir/helpers/baseline_payload.py"
tmp_dir=""
child_pids=()

cleanup() {
  local status=$?
  for pid in "${child_pids[@]:-}"; do
    if kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
  done
  if [[ -n "$tmp_dir" && -d "$tmp_dir" ]]; then
    rm -rf "$tmp_dir"
  fi
  return "$status"
}
trap cleanup EXIT INT TERM

usage() {
  cat >&2 <<'EOF'
usage:
  eval.sh --dry-run --json --mode M3-C1|M3-C2|M3-C3
  eval.sh --dry-run --json --matrix
  eval.sh --json --mode M3-C1|M3-C2|M3-C3
EOF
}

dry_run=false
json=false
mode_cell=""
matrix=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run)
      dry_run=true
      shift
      ;;
    --json)
      json=true
      shift
      ;;
    --mode)
      if [[ $# -lt 2 ]]; then
        usage
        exit 2
      fi
      mode_cell="$2"
      shift 2
      ;;
    --matrix)
      matrix=true
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      usage
      exit 2
      ;;
  esac
done

if [[ "$json" != true ]]; then
  printf 'eval.sh currently emits the AGE-113 contract as JSON; pass --json.\n' >&2
  exit 2
fi

emit_matrix() {
  python3 "$baseline_helper" matrix "$baseline_json"
}

claude_binary_path() {
  printf '%s\n' "${CLAUDE_CODE_BINARY:-${AGE113_CLAUDE_BIN:-}}"
}

if [[ "$dry_run" == true ]]; then
  if [[ "$matrix" == true ]]; then
    if [[ -n "$mode_cell" ]]; then
      usage
      exit 2
    fi
    emit_matrix
    exit 0
  fi
  if [[ -z "$mode_cell" ]]; then
    usage
    exit 2
  fi
  exec "$fixtures_dir/run-mode.sh" --dry-run --json --mode "$mode_cell"
fi

claude_bin="$(claude_binary_path)"
if [[ -z "$claude_bin" ]]; then
  printf 'SKIP: live Claude replay requires CLAUDE_CODE_BINARY or AGE113_CLAUDE_BIN; dry-run remains available.\n' >&2
  exit 0
fi
if [[ ! -x "$claude_bin" ]]; then
  printf 'SKIP: configured Claude Code binary is unavailable or not executable: %s\n' "$claude_bin" >&2
  exit 0
fi
export CLAUDE_CODE_BINARY="$claude_bin"
export AGE113_CLAUDE_BIN="$claude_bin"

if [[ "$matrix" == true ]]; then
  printf 'SKIP: live matrix replay is not part of the default AGE-113 eval gate; use --dry-run --matrix for the contract baseline.\n' >&2
  exit 0
fi
if [[ -z "$mode_cell" ]]; then
  usage
  exit 2
fi

exec "$fixtures_dir/run-mode.sh" --json --mode "$mode_cell"
