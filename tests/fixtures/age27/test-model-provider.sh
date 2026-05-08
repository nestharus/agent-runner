#!/usr/bin/env bash
set -euo pipefail
if [ -n "${AGE27_TEST_MODEL_ARGV_DUMP:-}" ]; then
  printf '%s\n' "$@" > "$AGE27_TEST_MODEL_ARGV_DUMP"
fi
if [ -n "${AGE27_TEST_MODEL_STDIN_DUMP:-}" ]; then
  cat > "$AGE27_TEST_MODEL_STDIN_DUMP"
else
  cat >/dev/null
fi
if [ -n "${AGE27_TEST_MODEL_STDOUT:-}" ]; then
  printf '%s\n' "$AGE27_TEST_MODEL_STDOUT"
fi
if [ -n "${AGE27_TEST_MODEL_STDERR:-}" ]; then
  printf '%s\n' "$AGE27_TEST_MODEL_STDERR" >&2
fi
exit "${AGE27_TEST_MODEL_EXIT:-0}"
