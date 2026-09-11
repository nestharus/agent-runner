#!/usr/bin/env bash
# Cargo's Linux test runner: only the runtime unit harness needs a global TTY.
# Runner client tests must stay pipe-backed; their output assertions rely on it.
set -euo pipefail
export SHELL=/bin/bash
case "$(basename "$1")" in
  oulipoly_runtime-*)
    printf -v command '%q ' "$@"
    # util-linux script returns the test status, including signal exits. The
    # workflow step's deadline bounds this command; do not background the child.
    exec script --quiet --return --command \
      "stty rows 24 cols 80 && stty size && exec $command" /dev/null
    ;;
  *) exec "$@" ;;
esac
