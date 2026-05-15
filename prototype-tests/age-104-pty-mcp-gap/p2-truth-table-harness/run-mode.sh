#!/usr/bin/env bash
set -euo pipefail

HARNESS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CLAUDE_BIN="/home/nes/.local/share/claude/versions/2.1.143"
WORKTREE="/home/nes/projects/agent-runner/worktrees/prototype-age-104-pty-mcp-gap"
SERVER_NAME="age104p2"
PROMPT_FILE="$HARNESS_DIR/prompts/call-task.md"
MCP_CONFIG="$HARNESS_DIR/mcp.json"
HOOK_SETTINGS="$HARNESS_DIR/settings.json"
TIMEOUT_S="${AGE104P2_TIMEOUT_S:-180}"
RUN_SET="${AGE104P2_RUN_SET:-manual}"

usage() {
  printf 'usage: %s <M1|M2|M3|M4> <C1|C2|C3>\n' "$0" >&2
}

if [[ $# -ne 2 ]]; then
  usage
  exit 2
fi

mode="$1"
column="$2"
case "$mode" in
  M1|M2|M3|M4) ;;
  *) usage; exit 2 ;;
esac
case "$column" in
  C1|C2|C3) ;;
  *) usage; exit 2 ;;
esac

run_dir="$HARNESS_DIR/logs/$RUN_SET/$mode/$column"
rm -rf "$run_dir"
mkdir -p "$run_dir"

stdout_log="$run_dir/stdout.log"
stderr_log="$run_dir/stderr.log"
transcript_log="$run_dir/transcript.log"
debug_log="$run_dir/claude-debug.log"
server_log="$run_dir/server.jsonl"
command_log="$run_dir/command.txt"
result_json="$run_dir/result.json"
empty_settings="$run_dir/settings-no-hooks.json"
inner_script="$run_dir/pty-inner.sh"

printf '{}\n' > "$empty_settings"

tool_csv="mcp__${SERVER_NAME}__Task,mcp__${SERVER_NAME}__Echo"
tool_args=()
case "$column" in
  C1) tool_args=(--tools "$tool_csv") ;;
  C2) tool_args=(--allowedTools "$tool_csv") ;;
  C3) tool_args=() ;;
esac

settings="$empty_settings"
if [[ "$mode" == "M3" ]]; then
  settings="$HOOK_SETTINGS"
fi

prompt_text="$(< "$PROMPT_FILE")"

export AGE104P2_HARNESS="$HARNESS_DIR"
export AGE104P2_RUN_DIR="$run_dir"

base_args=(
  "$CLAUDE_BIN"
  --model sonnet
  --permission-mode bypassPermissions
  --settings "$settings"
  --strict-mcp-config
  --mcp-config "$MCP_CONFIG"
  "${tool_args[@]}"
  --debug-file "$debug_log"
)

quote_cmd() {
  printf '%q ' "$@"
  printf '\n'
}

write_inner_script() {
  {
    printf '#!/usr/bin/env bash\n'
    printf 'set -euo pipefail\n'
    printf 'export AGE104P2_HARNESS=%q\n' "$HARNESS_DIR"
    printf 'export AGE104P2_RUN_DIR=%q\n' "$run_dir"
    printf 'exec '
    quote_cmd "${base_args[@]}" -- "$prompt_text"
  } > "$inner_script"
  chmod +x "$inner_script"
}

wait_for_pty_sentinel() {
  local pid="$1"
  local started
  started="$(date +%s)"
  local terminal_reason=""

  while kill -0 "$pid" 2>/dev/null; do
    if [[ -f "$run_dir/tool-call-end.sentinel" && -f "$transcript_log" ]] && grep -q 'TASK_OK:AGE104_P2_SENTINEL' "$transcript_log"; then
      terminal_reason="tool-return-visible"
      break
    fi
    if [[ -f "$run_dir/hook-Stop.sentinel" || -f "$run_dir/hook-SessionEnd.sentinel" ]]; then
      terminal_reason="hook-stop"
      break
    fi
    if [[ -f "$transcript_log" ]] && grep -qi 'No such tool available' "$transcript_log"; then
      terminal_reason="no-such-tool"
      break
    fi
    if [[ -f "$transcript_log" ]] && grep -q 'Task.tool.disabled\\|Task tool disabled' "$transcript_log"; then
      terminal_reason="assistant-disabled-tool-result"
      break
    fi
    if [[ -f "$transcript_log" ]] && (( $(grep -ao 'PROBE_RESULT:' "$transcript_log" | wc -l) >= 2 )); then
      terminal_reason="assistant-result"
      break
    fi
    if [[ ! -f "$run_dir/tool-call-start.sentinel" && -f "$transcript_log" ]] && (( $(grep -ao 'AGE104_P2_SENTINEL' "$transcript_log" | wc -l) >= 2 )); then
      terminal_reason="assistant-result-no-tool-call"
      break
    fi
    if [[ -f "$stdout_log" ]] && grep -qi 'No such tool available' "$stdout_log"; then
      terminal_reason="no-such-tool"
      break
    fi
    if (( $(date +%s) - started >= TIMEOUT_S )); then
      terminal_reason="wall-timeout"
      break
    fi
    sleep 0.2
  done

  printf '%s\n' "${terminal_reason:-process-exited}" > "$run_dir/terminal-reason.txt"
  if kill -0 "$pid" 2>/dev/null; then
    kill -TERM "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null || true
  fi
}

status=0
start_ns="$(date +%s%N)"
case "$mode" in
  M1)
    cmd=(timeout "$TIMEOUT_S" "${base_args[@]}" -p "$prompt_text")
    quote_cmd "${cmd[@]}" > "$command_log"
    "${cmd[@]}" > "$stdout_log" 2> "$stderr_log" || status=$?
    cp "$stdout_log" "$transcript_log"
    ;;
  M2)
    cmd=(timeout "$TIMEOUT_S" "${base_args[@]}" --print --verbose --output-format stream-json "$prompt_text")
    quote_cmd "${cmd[@]}" > "$command_log"
    "${cmd[@]}" > "$stdout_log" 2> "$stderr_log" || status=$?
    cp "$stdout_log" "$transcript_log"
    ;;
  M3|M4)
    write_inner_script
    quote_cmd timeout "$TIMEOUT_S" script -qfec "$inner_script" "$transcript_log" > "$command_log"
    setsid timeout "$TIMEOUT_S" script -qfec "$inner_script" "$transcript_log" > "$stdout_log" 2> "$stderr_log" &
    script_pid=$!
    wait_for_pty_sentinel "$script_pid"
    wait "$script_pid" || status=$?
    ;;
esac
end_ns="$(date +%s%N)"

python3 - "$result_json" "$HARNESS_DIR" "$run_dir" "$mode" "$column" "$status" "$start_ns" "$end_ns" <<'PY'
from __future__ import annotations

import json
import sys
from pathlib import Path

result_path = Path(sys.argv[1])
harness = Path(sys.argv[2])
run_dir = Path(sys.argv[3])
mode = sys.argv[4]
column = sys.argv[5]
status = int(sys.argv[6])
start_ns = int(sys.argv[7])
end_ns = int(sys.argv[8])

server_log = run_dir / "server.jsonl"
transcript = run_dir / "transcript.log"
stdout = run_dir / "stdout.log"
stderr = run_dir / "stderr.log"
debug = run_dir / "claude-debug.log"
terminal_reason = (run_dir / "terminal-reason.txt").read_text(errors="replace").strip() if (run_dir / "terminal-reason.txt").exists() else ""

server_text = server_log.read_text(errors="replace") if server_log.exists() else ""
combined_text = "\n".join(
    path.read_text(errors="replace")
    for path in [transcript, stdout, stderr, debug]
    if path.exists()
)

mcp_server_initialized = '"method": "tools/list"' in server_text or "tools/list" in server_text
tools_call_reached_server = '"method": "tools/call"' in server_text or '"kind": "tool_call_start"' in server_text
tool_returned_to_claude = "TASK_OK:AGE104_P2_SENTINEL" in combined_text
no_such_tool = "no such tool available" in combined_text.lower()

payload = {
    "mode": mode,
    "column": column,
    "status": status,
    "elapsed_s": round((end_ns - start_ns) / 1_000_000_000, 3),
    "terminal_reason": terminal_reason,
    "mcp_server_initialized": mcp_server_initialized,
    "tools_call_reached_server": tools_call_reached_server,
    "tool_returned_to_claude": tool_returned_to_claude,
    "no_such_tool": no_such_tool,
    "evidence_path": str((run_dir / "result.json").relative_to(harness.parent)),
    "server_log": str(server_log.relative_to(harness.parent)),
    "transcript": str(transcript.relative_to(harness.parent)),
    "debug_log": str(debug.relative_to(harness.parent)),
    "command": str((run_dir / "command.txt").relative_to(harness.parent)),
}
result_path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
print(json.dumps(payload, sort_keys=True))
PY
