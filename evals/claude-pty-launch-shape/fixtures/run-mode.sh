#!/usr/bin/env bash
set -euo pipefail

fixtures_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
baseline_json="$fixtures_dir/truth-table-baseline.json"
baseline_helper="$fixtures_dir/../helpers/baseline_payload.py"
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
  run-mode.sh --dry-run --json --mode M3-C1|M3-C2|M3-C3
  run-mode.sh --json --mode M3-C1|M3-C2|M3-C3
EOF
}

dry_run=false
json=false
mode_cell=""

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

if [[ "$json" != true || -z "$mode_cell" ]]; then
  usage
  exit 2
fi

case "$mode_cell" in
  M3-C1|M3-C2|M3-C3) ;;
  *)
    printf 'unsupported AGE-113 mode cell: %s\n' "$mode_cell" >&2
    exit 2
    ;;
esac

emit_mode() {
  python3 "$baseline_helper" mode "$baseline_json" "$mode_cell"
}

claude_binary_path() {
  printf '%s\n' "${CLAUDE_CODE_BINARY:-${AGE113_CLAUDE_BIN:-}}"
}

if [[ "$dry_run" == true ]]; then
  emit_mode
  exit 0
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

printf 'SKIP: live single-mode replay for %s is intentionally outside the default AGE-113 gate; dry-run emits the pinned 2.1.143 baseline.\n' "$mode_cell" >&2
