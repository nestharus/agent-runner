#!/usr/bin/env bash
set -euo pipefail
cat > "${AGE27_DIAGNOSTIC_PROMPT_DUMP:?}"
printf '%s\n' "${AGE27_DIAGNOSTIC_CATEGORY:-network_error}"
printf '%s\n' "${AGE27_DIAGNOSTIC_SUMMARY:-Diagnostic model saw network trouble}"
