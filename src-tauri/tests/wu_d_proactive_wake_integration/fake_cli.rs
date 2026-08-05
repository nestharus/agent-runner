//! ## Declared roles
//!
//! Roles: formatter.
//!
//! TEST: fake CLI script and notify-command formatters for proactive wake
//! integration cases.

use crate::CAPTURED_OPENCODE_SESSION;
use crate::fixtures::Fixture;
use crate::parse::{caller_chain, runner_bin};
use oulipoly_state::pid_identity::ProcessIdentity;
use serde_json::json;
use std::fs;
use std::process::{Command, Stdio};

pub(crate) fn notify_command(
    fixture: &Fixture,
    handle: &str,
    identity: &ProcessIdentity,
) -> Command {
    let state_dir = fixture.work_dir.join(format!("concurrent-{handle}"));
    fs::create_dir_all(&state_dir).unwrap();
    let meta = state_dir.join("meta.json");
    let log = state_dir.join("log");
    let rc = state_dir.join("rc");
    fs::write(
        &meta,
        serde_json::to_string(&caller_chain(identity)).unwrap(),
    )
    .unwrap();
    fs::write(&log, format!("log {handle}\n")).unwrap();
    fs::write(&rc, "0\n").unwrap();
    let mut cmd = Command::new(runner_bin());
    cmd.arg("notify")
        .arg("agent-bash-complete")
        .arg("--caller-ppid")
        .arg(std::process::id().to_string())
        .arg("--handle")
        .arg(handle)
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("--meta")
        .arg(&meta)
        .arg("--log")
        .arg(&log)
        .arg("--rc")
        .arg(&rc)
        .arg("--json")
        .env("XDG_CONFIG_HOME", &fixture.config_home)
        .env("XDG_DATA_HOME", &fixture.data_home)
        .env("HOME", &fixture.home_dir)
        .env("AGENT_BASH_AGENT_RUNNER_BIN", runner_bin())
        .env("WU_D_WORK_DIR", &fixture.work_dir)
        .env_remove("OULIPOLY_DATA_DIR")
        .env_remove("OULIPOLY_AUTO_WAKE_MAX")
        .env_remove("OULIPOLY_PARENT_INVOCATION")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(fixture.dir.path());
    cmd
}

pub(crate) fn provider_script(on_initial: &str, on_resume: &str, prompt_file: &str) -> String {
    let prompt_file = prompt_file.replace('"', "\\\"");
    format!(
        r#"runner="${{AGENT_BASH_AGENT_RUNNER_BIN:?missing}}"
work="${{WU_D_WORK_DIR:?missing}}"
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
provider_pid="$$"
boot_id="$(< /proc/sys/kernel/random/boot_id)"
stat_line="$(< "/proc/${{provider_pid}}/stat")"
after=
after="${{stat_line##*) }}"
read -r -a stat_fields <<< "$after"
start_ticks="${{stat_fields[19]}}"
notify_handle() {{
  handle="$1"
  rc_value="$2"
  state="$work/$handle"
  mkdir -p "$state"
  printf '{{"caller_chain":[{{"pid":%s,"boot_id":"%s","starttime_ticks":%s}}]}}\n' "$provider_pid" "$boot_id" "$start_ticks" > "$state/meta.json"
  printf 'log for %s\n' "$handle" > "$state/log"
  printf '%s\n' "$rc_value" > "$state/rc"
  "$runner" notify agent-bash-complete \
    --caller-ppid "$provider_pid" \
    --handle "$handle" \
    --state-dir "$state" \
    --meta "$state/meta.json" \
    --log "$state/log" \
    --rc "$state/rc" \
    --json > "$state/notify.json" 2> "$state/notify.err" || true
}}
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

pub(crate) fn opencode_capture_provider_script() -> String {
    let event = json!({
        "type": "step_start",
        "sessionID": CAPTURED_OPENCODE_SESSION,
    });
    provider_script(
        &format!(
            r#"printf '%s\n' '{}'
sleep 0.2
notify_handle h-capture-midturn 0
sleep 0.2"#,
            event
        ),
        r#"if [ "$resume" != "ses_capturemidturn" ]; then
  printf 'expected --session ses_capturemidturn, got %s\n' "$resume" >&2
  exit 66
fi"#,
        "opencode-capture-resumed.txt",
    )
}
