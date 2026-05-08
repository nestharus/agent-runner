#!/usr/bin/env bash
set -euo pipefail
if [ -n "${AGE27_EFFECTIVE_ARGV_DUMP:-}" ]; then
  printf '%s\n' "$@" > "$AGE27_EFFECTIVE_ARGV_DUMP"
fi
if [ -n "${AGE27_EFFECTIVE_STDIN_DUMP:-}" ]; then
  cat > "$AGE27_EFFECTIVE_STDIN_DUMP"
else
  cat >/dev/null
fi
printf 'effective stdout\n'
