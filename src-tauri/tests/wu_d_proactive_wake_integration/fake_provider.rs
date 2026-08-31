//! ## Declared roles
//!
//! Roles: formatter.
//!
//! TEST: native external-provider fixture formatter for proactive wake
//! integration cases.

use crate::SESSION;
use std::path::Path;

pub(crate) fn provider_script(on_initial: &str, on_resume: &str, prompt_file: &str) -> String {
    PROVIDER_TEMPLATE
        .replace("__WU_D_SESSION__", &serde_json::to_string(SESSION).unwrap())
        .replace(
            "__WU_D_ON_INITIAL__",
            &serde_json::to_string(on_initial).unwrap(),
        )
        .replace(
            "__WU_D_ON_RESUME__",
            &serde_json::to_string(on_resume).unwrap(),
        )
        .replace(
            "__WU_D_PROMPT_FILE__",
            &serde_json::to_string(prompt_file).unwrap(),
        )
}

pub(crate) fn delayed_agent_bash_provider_script(agent_bash_bin: &Path) -> String {
    let agent_bash_bin = shell_single_quote(&agent_bash_bin.to_string_lossy());
    provider_script(
        &format!(
            r#"runner="${{AGENT_BASH_AGENT_RUNNER_BIN:?missing}}"
owner_invocation="$(python3 -c 'import json, os; print(json.loads(os.environ["OULIPOLY_PARENT_INVOCATION"])["id"])')"
writer_ready="$work/pid-sidecar-writer-ready"
python3 - "$OULIPOLY_DATA_DIR/pid-identity.db" "$owner_invocation" "$writer_ready" <<'PY' &
import sqlite3
import sys
import time

path, owner_invocation, ready = sys.argv[1:]
connection = sqlite3.connect(path, timeout=0.1)
admission_deadline = time.monotonic() + 5
revision = 0
while True:
    try:
        cursor = connection.execute(
            "UPDATE pid_identity SET recorded_at = ? WHERE invocation_uuid = ?",
            ("owner-lookup-admission", owner_invocation),
        )
        connection.commit()
    except sqlite3.OperationalError as error:
        connection.rollback()
        if "locked" not in str(error).lower() or time.monotonic() >= admission_deadline:
            raise
        time.sleep(0.01)
        continue
    if cursor.rowcount == 1:
        break
    if time.monotonic() >= admission_deadline:
        raise RuntimeError("owner identity did not appear before write burst")
    time.sleep(0.01)

open(ready, "w", encoding="utf-8").close()
deadline = time.monotonic() + 1
while time.monotonic() < deadline:
    try:
        cursor = connection.execute(
            "UPDATE pid_identity SET recorded_at = ? WHERE invocation_uuid = ?",
            ("owner-lookup-burst-" + str(revision), owner_invocation),
        )
        connection.commit()
    except sqlite3.OperationalError as error:
        connection.rollback()
        if "locked" not in str(error).lower():
            raise
        time.sleep(0.001)
        continue
    if cursor.rowcount != 1:
        raise RuntimeError("owner identity disappeared during write burst")
    revision += 1
PY
writer_pid=$!
for _ in $(seq 1 200); do
  [ -e "$writer_ready" ] && break
  sleep 0.01
done
[ -e "$writer_ready" ]
if AGENT_BASH_AGENT_RUNNER_BIN="$runner" \
   {agent_bash_bin} run --completion-scope tree --delivery async -- \
     bash -lc '( sleep 1; printf nested-tree-complete ) &' \
     > "$work/agent-bash-dispatch.json" \
     2> "$work/agent-bash-dispatch.err"; then
  :
else
  rc=$?
  cat "$work/agent-bash-dispatch.err" >&2
  wait "$writer_pid" || true
  exit "$rc"
fi
wait "$writer_pid"
"#,
        ),
        "",
        "acr329-resumed-input.txt",
    )
}

pub(crate) fn late_consumed_agent_bash_provider_script(agent_bash_bin: &Path) -> String {
    let agent_bash_bin = shell_single_quote(&agent_bash_bin.to_string_lossy());
    provider_script(
        &format!(
            r#"runner="${{AGENT_BASH_AGENT_RUNNER_BIN:?missing}}"
owner_invocation="$(python3 -c 'import json, os; print(json.loads(os.environ["OULIPOLY_PARENT_INVOCATION"])["id"])')"
dispatch="$work/late-consumed-dispatch.json"
AGENT_BASH_AGENT_RUNNER_BIN="$runner" \
AGENT_BASH_CONSUMER_GRACE_MS=0 \
{agent_bash_bin} run --completion-scope root --delivery async -- \
  bash -lc 'printf nested-root-complete' > "$dispatch"
handle="$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["handle"])' "$dispatch")"
state_dir="$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["state_dir"])' "$dispatch")"
found=""
for _ in $(seq 1 200); do
  mailbox="$($runner mailbox list --session-id "$session" --json)"
  if printf '%s' "$mailbox" | grep -Fq "$handle"; then
    found=1
    break
  fi
  sleep 0.05
done
[ -n "$found" ]
{agent_bash_bin} status "$handle" > "$work/late-consumed-poll.txt"
: > "$state_dir/consumed""#,
        ),
        "",
        "late-consumed-resumed-input.txt",
    )
}

pub(crate) fn mixed_consumed_agent_bash_provider_script(agent_bash_bin: &Path) -> String {
    let agent_bash_bin = shell_single_quote(&agent_bash_bin.to_string_lossy());
    provider_script(
        &format!(
            r#"runner="${{AGENT_BASH_AGENT_RUNNER_BIN:?missing}}"
owner_invocation="$(python3 -c 'import json, os; print(json.loads(os.environ["OULIPOLY_PARENT_INVOCATION"])["id"])')"
run_job() {{
  local dispatch="$1"
  AGENT_BASH_AGENT_RUNNER_BIN="$runner" \
  AGENT_BASH_CONSUMER_GRACE_MS=0 \
  {agent_bash_bin} run --completion-scope root --delivery async -- \
    bash -lc 'printf nested-root-complete' > "$dispatch"
}}
consumed_dispatch="$work/mixed-consumed-dispatch.json"
unpolled_dispatch="$work/mixed-unpolled-dispatch.json"
run_job "$consumed_dispatch"
run_job "$unpolled_dispatch"
consumed_handle="$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["handle"])' "$consumed_dispatch")"
unpolled_handle="$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["handle"])' "$unpolled_dispatch")"
consumed_state_dir="$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["state_dir"])' "$consumed_dispatch")"
found=""
for _ in $(seq 1 200); do
  mailbox="$($runner mailbox list --session-id "$session" --json)"
  if printf '%s' "$mailbox" | grep -Fq "$consumed_handle" && \
     printf '%s' "$mailbox" | grep -Fq "$unpolled_handle"; then
    found=1
    break
  fi
  sleep 0.05
done
[ -n "$found" ]
{agent_bash_bin} status "$consumed_handle" > "$work/mixed-consumed-poll.txt"
: > "$consumed_state_dir/consumed""#,
        ),
        "",
        "mixed-resumed-input.txt",
    )
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

const PROVIDER_TEMPLATE: &str = r#"#!/usr/bin/env python3
import base64
import hashlib
import json
import os
import pathlib
import subprocess
import sys
import time

CONTRACT = "oulipoly.provider/v1"
PAGING_PROTOCOL = "oulipoly.session_turn_pages/v1"
SESSION = __WU_D_SESSION__
ON_INITIAL = __WU_D_ON_INITIAL__
ON_RESUME = __WU_D_ON_RESUME__
PROMPT_FILE = __WU_D_PROMPT_FILE__

def envelope(request, result):
    return {"contract": CONTRACT, "request_id": request["request_id"], "ok": True, "result": result}

def event(request, seq, kind, **fields):
    value = {"contract": CONTRACT, "request_id": request["request_id"], "seq": seq, "time_unix_ms": 1000 + seq, "kind": kind}
    value.update(fields)
    print(json.dumps(value, separators=(",", ":")), flush=True)

def next_resume_index(work):
    lock = work / "provider-resume-sequence.lock"
    while True:
        try:
            lock.mkdir()
            break
        except FileExistsError:
            time.sleep(0.01)
    try:
        sequence_file = work / "provider-resume-sequence.txt"
        index = int(sequence_file.read_text() or "0") if sequence_file.exists() else 0
        index += 1
        sequence_file.write_text(str(index))
        return index
    finally:
        lock.rmdir()

def write_turn(work, session, prompt, index):
    turns = work / "session-turns"
    turns.mkdir(parents=True, exist_ok=True)
    record = {
        "session_id": session,
        "turn_id": "wu-d-delivery-" + session + "-" + str(index),
        "timestamp": "2026-07-29T12:00:00Z",
        "role": "user",
        "body": [{"type": "text", "text": prompt}],
        "source_sequence": index,
    }
    (turns / (str(index).zfill(20) + ".json")).write_text(json.dumps(record, separators=(",", ":")) + "\n")

def launch(request):
    params = request.get("params", {})
    launch_session = params.get("session", {})
    session = launch_session.get("known_provider_session_id") or SESSION
    prompt = params.get("model", {}).get("inputs", {}).get("prompt", "")
    work = pathlib.Path(params.get("env", {}).get("WU_D_WORK_DIR") or os.environ["WU_D_WORK_DIR"])
    env = os.environ.copy()
    env.update(params.get("env") or {})
    env.update({"work": str(work), "session": session, "resume": session, "last": prompt})
    resumed = bool(launch_session.get("known_provider_session_id"))
    hook = ON_RESUME if resumed else ON_INITIAL
    index = None
    if resumed:
        index = next_resume_index(work)
        env["WU_D_PROVIDER_RESUME_INDEX"] = str(index)
        target_name = PROMPT_FILE.replace("${WU_D_PROVIDER_RESUME_INDEX}", str(index))
        target = work / target_name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(prompt)
    seq = 1
    event(request, seq, "marker", name="oulipoly.provider_session", value={"provider_session_id": session})
    seq += 1
    completed = subprocess.run(["bash", "-c", hook], cwd=work, env=env, capture_output=True) if hook else None
    code = completed.returncode if completed else 0
    stdout = completed.stdout if completed else b""
    stderr = completed.stderr if completed else b""
    if resumed and code == 0:
        write_turn(work, session, prompt, index)

    data_event_count = 0
    if stdout:
        event(request, seq, "stdout", data_base64=base64.b64encode(stdout).decode("ascii"))
        seq += 1
        data_event_count += 1
    if stderr:
        event(request, seq, "stderr", data_base64=base64.b64encode(stderr).decode("ascii"))
        seq += 1
        data_event_count += 1
    event(request, seq, "marker", name="oulipoly.launch_output_complete/v1", value={
        "protocol": "oulipoly.launch_output/v1",
        "stdout": {"bytes": len(stdout), "sha256": hashlib.sha256(stdout).hexdigest()},
        "stderr": {"bytes": len(stderr), "sha256": hashlib.sha256(stderr).hexdigest()},
        "data_event_count": data_event_count,
    })
    seq += 1
    event(request, seq, "exit",
        status={"kind": "exited", "code": code},
        terminal_signal={"kind": "clean_exit" if code == 0 else "nonzero_exit", "evidence": "wu-d native fixture", "observed_at_unix_ms": 1000 + seq},
        session={"provider_session_id": session, "state": {"fixture": "wu-d"}})

def load_turns(work, session):
    turns_dir = work / "session-turns"
    if not turns_dir.exists():
        return []
    records = []
    for path in sorted(turns_dir.glob("*.json")):
        record = json.loads(path.read_text())
        if record.get("session_id") == session:
            records.append(record)
    return records

def token_index(token):
    return int(token.rsplit(":", 1)[1]) if token else 0

def normalized_text(record):
    text = "".join(part.get("text", "") for part in record.get("body", []) if part.get("type") == "text")
    return text.replace("\r\n", "\n").replace("\r", "\n").strip()

def session_turn_page(request):
    params = request.get("params", {})
    session = params.get("session_id") or SESSION
    work = pathlib.Path(os.environ["WU_D_WORK_DIR"])
    records = load_turns(work, session)
    projection = params.get("turn_projection")
    if params.get("start_mode") == "tail":
        base = len(records)
        snapshot_count = base
        start = base
        page_index = 0
    elif params.get("snapshot_id"):
        snapshot_parts = params["snapshot_id"].split(":")
        base = int(snapshot_parts[-2])
        snapshot_count = int(snapshot_parts[-1])
        start = token_index(params.get("page_token"))
        page_index = int(params.get("page_token", "page:0:0").split(":")[-2])
    else:
        base = token_index(params.get("after_token"))
        snapshot_count = len(records)
        start = base
        page_index = 0
    limit = int(params.get("max_turns", 1))
    selected = records[start:min(snapshot_count, start + limit)]
    page_start_sequence = start if projection == "canonical_ingest" else start - base
    turns = []
    for offset, record in enumerate(selected):
        body = record.get("body")
        body_bytes = len(json.dumps(body, separators=(",", ":")).encode("utf-8"))
        text = normalized_text(record)
        canonical_sha = hashlib.sha256(text.encode("utf-8")).hexdigest()
        inline = projection == "canonical_ingest"
        turns.append({
            "session_id": session,
            "turn_id": record["turn_id"],
            "snapshot_sequence": page_start_sequence + offset,
            "timestamp": record["timestamp"],
            "role": record["role"],
            "parent_turn_id": None,
            "is_sidechain": False,
            "is_compaction_boundary": False,
            "body_state": "inline" if inline else "omitted_oversize",
            "body": body if inline else None,
            "body_bytes": body_bytes,
            "body_sha256": hashlib.sha256(json.dumps(body, separators=(",", ":")).encode("utf-8")).hexdigest() if inline else None,
            "canonical_text_sha256": canonical_sha,
        })
    next_index = start + len(selected)
    complete = next_index >= snapshot_count
    return envelope(request, {
        "read_protocol": PAGING_PROTOCOL,
        "provider_instance_id": request.get("provider_instance_id"),
        "settings_id": params.get("settings_id"),
        "session_id": session,
        "turn_projection": projection,
        "snapshot_id": "snapshot:" + str(base) + ":" + str(snapshot_count),
        "page_index": page_index,
        "page_start_sequence": page_start_sequence,
        "turns": turns,
        "page_turn_count": len(turns),
        "source_bytes_examined": sum(len(json.dumps(record, separators=(",", ":")).encode("utf-8")) for record in selected),
        "scan_progress": False,
        "snapshot_complete": complete,
        "next_page_token": None if complete else "page:" + str(page_index + 1) + ":" + str(next_index),
        "resume_token": "resume:" + str(snapshot_count) if complete else None,
        "source_final": False,
        "warnings": [],
    })

request = json.loads(sys.stdin.read() or "{}")
method = sys.argv[1] if len(sys.argv) > 1 else ""
if method == "describe":
    print(json.dumps(envelope(request, {
        "provider_id": "wu-d-native-fixture",
        "display_name": "WU-D Native Fixture",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": {"launch": True, "launch_output_v1": True, "policy": True, "quota": False, "session": True, "session_turn_pages_v1": True, "terminal": False, "rotation": False, "discovery": False, "settings": False, "setup_brain": False, "setup": False, "migration": False, "prompt_acceptance_v1": False},
    })))
elif method == "policy.evaluate":
    print(json.dumps(envelope(request, {"accepted": True, "env": {}, "stdin": None, "prompt": None, "diagnostics": [], "markers": []})))
elif method == "launch":
    launch(request)
elif method == "session.capture":
    params = request.get("params", {})
    print(json.dumps(envelope(request, {"provider_session_id": params.get("session_id") or SESSION, "state": {"captured": True}, "artifacts": []})))
elif method == "session.read_turns":
    print(json.dumps(session_turn_page(request)))
else:
    print(json.dumps({"contract": CONTRACT, "request_id": request.get("request_id", "missing"), "ok": False, "error": {"category": "failed", "code": "unsupported_subcommand", "message": method, "retryable": False}}))
"#;
