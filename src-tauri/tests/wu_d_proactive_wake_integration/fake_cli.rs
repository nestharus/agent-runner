//! ## Declared roles
//!
//! Roles: formatter.
//!
//! TEST: fake provider CLI script formatter for proactive wake integration
//! cases.

use std::path::Path;

pub(crate) fn provider_script(on_initial: &str, on_resume: &str, prompt_file: &str) -> String {
    let prompt_file = prompt_file.replace('"', "\\\"");
    format!(
        r#"work="${{WU_D_WORK_DIR:?missing}}"
session=""
resume=""
for ((i=1; i <= $#; i++)); do
  arg="${{!i}}"
  if [ "$arg" = "--session-id" ]; then
    j=$((i + 1))
    session="${{!j}}"
  fi
  if [ "$arg" = "--resume" ]; then
    j=$((i + 1))
    resume="${{!j}}"
  fi
  if [ "$arg" = "--session" ]; then
    j=$((i + 1))
    resume="${{!j}}"
  fi
done
last="${{@: -1}}"
if [ -n "$resume" ]; then
  target="$work/{prompt_file}"
  mkdir -p "$(dirname "$target")"
  printf '%s' "$last" > "$target"
  {on_resume}
  python3 - "$work/session-turns" "$resume" "${{OULIPOLY_AUTO_WAKE_COUNT:-manual}}" "$last" <<'PY'
import hashlib
import json
import os
import sys

turns_dir, session_id, wake_count, prompt = sys.argv[1:]
turn_id = f"wu-d-delivery-{{session_id}}-{{wake_count}}"
record = {{
    "session_id": session_id,
    "turn_id": turn_id,
    "timestamp": "2026-07-29T12:00:00Z",
    "role": "user",
    "body": [{{"type": "text", "text": prompt}}],
}}
os.makedirs(turns_dir, exist_ok=True)
path = os.path.join(turns_dir, hashlib.sha256(turn_id.encode()).hexdigest() + ".jsonl")
with open(path, "w", encoding="utf-8") as out:
    out.write(json.dumps(record, separators=(",", ":")) + "\n")
PY
  exit 0
fi
{on_initial}
exit 0
"#
    )
}

pub(crate) fn delayed_agent_bash_provider_script(agent_bash_bin: &Path) -> String {
    let agent_bash_bin = shell_single_quote(&agent_bash_bin.to_string_lossy());
    provider_script(
        &format!(
            r#"runner="${{AGENT_BASH_AGENT_RUNNER_BIN:?missing}}"
owner_invocation="$(python3 -c 'import json, os; print(json.loads(os.environ["OULIPOLY_PARENT_INVOCATION"])["id"])')"
AGENT_BASH_OWNER_SESSION_ID="$session" \
AGENT_BASH_OWNER_INVOCATION_UUID="$owner_invocation" \
AGENT_BASH_AGENT_RUNNER_BIN="$runner" \
{agent_bash_bin} run --completion-scope tree --delivery async -- \
  bash -lc '( sleep 1; printf nested-tree-complete ) &' \
  > "$work/agent-bash-dispatch.json" \
  2> "$work/agent-bash-dispatch.err""#,
        ),
        "",
        "acr329-resumed-input.txt",
    )
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
