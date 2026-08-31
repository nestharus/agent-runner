#![cfg(unix)]
//! AGE-166 one-shot zero-turn quota-detection CLI fixture tests.
//!
//! ## Declared roles
//!
//! `validator`, `orchestration`, `formatter`
//!
//! The tests drive `oulipoly-agent-runner` one-shot CLI invocations through
//! the `Age153Fixture::run_one_shot_with_env` harness (orchestration), assert
//! the typed `MaybeQuotaExhausted` marker shape + `provider_quotas` state
//! transitions (validator), and produce native provider fixtures + capture-pool
//! model/provider TOML (formatter).

mod age153_support;

use age153_support::{
    Age153Fixture, assert_no_terminal_marker_on_stdout, assert_result_envelope_shape, line_count,
    parse_terminal_signal_line, success_body, terminal_signal_lines, toml_string,
};
use rusqlite::params;
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Output;

const FORCE_KIND: &str = "OULIPOLY_AGE153_FORCE_TERMINAL_SIGNAL_KIND";

fn write_capture_pool(
    fixture: &Age153Fixture,
    model_name: &str,
    providers: &[(&str, &Path)],
    transcript_paths: &[(&str, &Path)],
) {
    let mut model = String::new();
    let mut providers_toml = String::new();
    let provider_path = fixture.dir.path().join("age166-native-provider.py");
    fs::write(
        &provider_path,
        native_provider_script(providers, transcript_paths),
    )
    .unwrap();
    let mut permissions = fs::metadata(&provider_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&provider_path, permissions).unwrap();

    model.push_str(&format!(
        "provider = {{ path = {} }}\nprompt_mode = \"arg\"\n\n",
        toml_string(&provider_path.display().to_string())
    ));

    for (provider, command) in providers {
        model.push_str(&format!(
            r#"[[providers]]
name = "{provider}"
args = []

"#
        ));
        providers_toml.push_str(&format!(
            r#"[{provider}]
command = {}
args = []
interactive_args = ["interactive"]
prompt_mode = "arg"

"#,
            toml_string(&command.display().to_string())
        ));
    }

    fs::write(fixture.models_dir.join(format!("{model_name}.toml")), model).unwrap();
    fs::write(
        fixture.app_config_dir.join("providers.toml"),
        providers_toml,
    )
    .unwrap();
}

fn native_provider_script(providers: &[(&str, &Path)], transcripts: &[(&str, &Path)]) -> String {
    let commands: Vec<_> = providers
        .iter()
        .map(|(provider, path)| (*provider, path.display().to_string()))
        .collect();
    let transcripts: Vec<_> = transcripts
        .iter()
        .map(|(provider, path)| (*provider, path.display().to_string()))
        .collect();

    NATIVE_PROVIDER_TEMPLATE
        .replace(
            "__AGE166_COMMANDS__",
            &serde_json::to_string(&commands).unwrap(),
        )
        .replace(
            "__AGE166_TRANSCRIPTS__",
            &serde_json::to_string(&transcripts).unwrap(),
        )
}

const NATIVE_PROVIDER_TEMPLATE: &str = r#"#!/usr/bin/env python3
import base64
import hashlib
import json
import pathlib
import subprocess
import sys

CONTRACT = "oulipoly.provider/v1"
PAGING_PROTOCOL = "oulipoly.session_turn_pages/v1"
COMMANDS = dict(__AGE166_COMMANDS__)
TRANSCRIPTS = dict(__AGE166_TRANSCRIPTS__)

def envelope(request, result):
    return {"contract": CONTRACT, "request_id": request["request_id"], "ok": True, "result": result}

def event(request, seq, kind, **fields):
    value = {"contract": CONTRACT, "request_id": request["request_id"], "seq": seq, "time_unix_ms": 1000 + seq, "kind": kind}
    value.update(fields)
    print(json.dumps(value, separators=(",", ":")), flush=True)

def account_name(request):
    candidate = request.get("provider_instance_id")
    if candidate in COMMANDS:
        return candidate
    candidate = request.get("params", {}).get("settings_id")
    if candidate in COMMANDS:
        return candidate
    raise RuntimeError("missing AGE-166 provider account identity")

def session_id(account):
    return "age166-session-" + account

def launch(request):
    account = account_name(request)
    params = request.get("params", {})
    session = params.get("session", {}).get("known_provider_session_id") or session_id(account)
    completed = subprocess.run(
        [COMMANDS[account], "--session-id", session],
        capture_output=True,
    )
    seq = 1
    event(request, seq, "marker", name="oulipoly.provider_session", value={"provider_session_id": session})
    seq += 1
    data_event_count = 0
    if completed.stdout:
        event(request, seq, "stdout", data_base64=base64.b64encode(completed.stdout).decode("ascii"))
        seq += 1
        data_event_count += 1
    if completed.stderr:
        event(request, seq, "stderr", data_base64=base64.b64encode(completed.stderr).decode("ascii"))
        seq += 1
        data_event_count += 1
    event(request, seq, "marker", name="oulipoly.launch_output_complete/v1", value={
        "protocol": "oulipoly.launch_output/v1",
        "stdout": {"bytes": len(completed.stdout), "sha256": hashlib.sha256(completed.stdout).hexdigest()},
        "stderr": {"bytes": len(completed.stderr), "sha256": hashlib.sha256(completed.stderr).hexdigest()},
        "data_event_count": data_event_count,
    })
    seq += 1
    event(request, seq, "exit",
        status={"kind": "exited", "code": completed.returncode},
        terminal_signal={
            "kind": "clean_exit" if completed.returncode == 0 else "nonzero_exit",
            "evidence": "AGE-166 native fixture",
            "observed_at_unix_ms": 1000 + seq,
        },
        session={"provider_session_id": session, "state": {"captured": True}})

def transcript_records(account, session):
    path = pathlib.Path(TRANSCRIPTS[account])
    records = []
    if path.exists():
        for line in path.read_text().splitlines():
            if not line:
                continue
            record = json.loads(line)
            if record.get("session_id") == session:
                records.append(record)
    return records

def token_index(token):
    return int(token.rsplit(":", 1)[1]) if token else 0

def session_turn_page(request):
    account = account_name(request)
    params = request.get("params", {})
    session = params.get("session_id") or session_id(account)
    records = transcript_records(account, session)
    projection = params.get("turn_projection")
    if params.get("start_mode") == "tail":
        base = len(records)
        snapshot_count = base
        start = base
        page_index = 0
    elif params.get("snapshot_id"):
        parts = params["snapshot_id"].split(":")
        base = int(parts[-2])
        snapshot_count = int(parts[-1])
        start = token_index(params.get("page_token"))
        page_index = int(params.get("page_token", "page:0:0").split(":")[-2])
    else:
        base = token_index(params.get("after_token"))
        snapshot_count = len(records)
        start = base
        page_index = 0
    selected = records[start:min(snapshot_count, start + int(params.get("max_turns", 1)))]
    page_start_sequence = start if projection == "canonical_ingest" else start - base
    turns = []
    for offset, record in enumerate(selected):
        text = record.get("text") or record.get("role", "")
        body = [{"type": "text", "text": text}]
        body_json = json.dumps(body, separators=(",", ":")).encode("utf-8")
        normalized = text.replace("\r\n", "\n").replace("\r", "\n").strip()
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
            "body_bytes": len(body_json),
            "body_sha256": hashlib.sha256(body_json).hexdigest() if inline else None,
            "canonical_text_sha256": hashlib.sha256(normalized.encode("utf-8")).hexdigest(),
        })
    next_index = start + len(selected)
    complete = next_index >= snapshot_count
    return envelope(request, {
        "read_protocol": PAGING_PROTOCOL,
        "provider_instance_id": request.get("provider_instance_id"),
        "settings_id": params.get("settings_id"),
        "session_id": session,
        "turn_projection": projection,
        "snapshot_id": "age166-snapshot:" + str(base) + ":" + str(snapshot_count),
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
        "provider_id": "age166-native-provider",
        "display_name": "AGE-166 Native Provider",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": {"launch": True, "launch_output_v1": True, "policy": True, "quota": False, "session": True, "session_turn_pages_v1": True, "terminal": False, "rotation": False, "discovery": False, "settings": False, "setup_brain": False, "setup": False, "migration": False, "prompt_acceptance_v1": False},
    })))
elif method == "policy.evaluate":
    print(json.dumps(envelope(request, {"accepted": True, "env": {}, "stdin": None, "prompt": None, "diagnostics": [], "markers": []})))
elif method == "launch":
    launch(request)
elif method == "session.capture":
    account = account_name(request)
    params = request.get("params", {})
    print(json.dumps(envelope(request, {"provider_session_id": params.get("session_id") or session_id(account), "state": {"captured": True}, "artifacts": []})))
elif method == "session.read_turns":
    print(json.dumps(session_turn_page(request)))
else:
    print(json.dumps({"contract": CONTRACT, "request_id": request.get("request_id", "missing"), "ok": False, "error": {"category": "failed", "code": "unsupported_subcommand", "message": method, "retryable": False}}))
"#;

fn zero_turn_capture_body(marker: &Path, exit_code: i32) -> String {
    format!(
        r#"session_id=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--session-id" ]; then
    session_id="$2"
    shift 2
  else
    shift
  fi
done
printf '%s\n' ran >> {}
printf '{{"type":"system","subtype":"init","session_id":"%s"}}\n' "$session_id"
exit {exit_code}"#,
        toml_string(&marker.display().to_string())
    )
}

fn assistant_turn_capture_body(marker: &Path, transcript: &Path, exit_code: i32) -> String {
    format!(
        r#"session_id=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--session-id" ]; then
    session_id="$2"
    shift 2
  else
    shift
  fi
done
printf '%s\n' ran >> {}
printf '{{"session_id":"%s","turn_id":"turn-%s","timestamp":"2026-04-17T08:00:01Z","role":"assistant"}}\n' "$session_id" "$$" >> {}
printf '{{"type":"system","subtype":"init","session_id":"%s"}}\n' "$session_id"
exit {exit_code}"#,
        toml_string(&marker.display().to_string()),
        toml_string(&transcript.display().to_string())
    )
}

fn assistant_turn_stdout_capture_body(
    marker: &Path,
    transcript: &Path,
    stdout: &str,
    exit_code: i32,
) -> String {
    format!(
        r#"session_id=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--session-id" ]; then
    session_id="$2"
    shift 2
  else
    shift
  fi
done
printf '%s\n' ran >> {}
printf '{{"session_id":"%s","turn_id":"turn-%s","timestamp":"2026-04-17T08:00:01Z","role":"assistant"}}\n' "$session_id" "$$" >> {}
printf '%s\n' {}
exit {exit_code}"#,
        toml_string(&marker.display().to_string()),
        toml_string(&transcript.display().to_string()),
        toml_string(stdout)
    )
}

fn zero_turn_then_assistant_turn_capture_body(
    marker: &Path,
    transcript: &Path,
    counter: &Path,
) -> String {
    format!(
        r#"session_id=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--session-id" ]; then
    session_id="$2"
    shift 2
  else
    shift
  fi
done
count=0
if [ -f {} ]; then
  count="$(cat {})"
fi
count=$((count + 1))
printf '%s\n' "$count" > {}
printf '%s\n' ran >> {}
printf '{{"type":"system","subtype":"init","session_id":"%s"}}\n' "$session_id"
if [ "$count" -ge 2 ]; then
  printf '{{"session_id":"%s","turn_id":"turn-%s","timestamp":"2026-04-17T08:00:01Z","role":"assistant"}}\n' "$session_id" "$$" >> {}
  exit 0
fi
exit 1"#,
        toml_string(&counter.display().to_string()),
        toml_string(&counter.display().to_string()),
        toml_string(&counter.display().to_string()),
        toml_string(&marker.display().to_string()),
        toml_string(&transcript.display().to_string())
    )
}

fn output_text(output: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn assert_maybe_marker_without_zero_turn_evidence(stderr: &str) -> Value {
    let maybe_lines: Vec<_> = terminal_signal_lines(stderr)
        .into_iter()
        .filter(|line| line.contains("\"kind\":\"MaybeQuotaExhausted\""))
        .collect();
    assert!(
        !maybe_lines.is_empty(),
        "expected MaybeQuotaExhausted marker in stderr:\n{stderr}"
    );
    let value = parse_terminal_signal_line(maybe_lines[0]);
    let evidence = value["evidence"]["excerpt"]
        .as_str()
        .expect("marker evidence excerpt");
    assert!(!evidence.contains("provider_session_id="), "{evidence}");
    assert!(
        !evidence.contains("baseline_assistant_turns="),
        "{evidence}"
    );
    assert!(!evidence.contains("current_assistant_turns="), "{evidence}");
    assert!(!evidence.contains("new_assistant_turns="), "{evidence}");
    value
}

fn assert_no_maybe_marker(stderr: &str) {
    assert!(
        !stderr.contains("\"kind\":\"MaybeQuotaExhausted\""),
        "stderr must not contain MaybeQuotaExhausted marker:\n{stderr}"
    );
}

fn latest_invocation_terminal_reason(fixture: &Age153Fixture, provider: &str) -> Option<String> {
    fixture
        .conn()
        .query_row(
            "SELECT terminal_reason
             FROM invocations
             WHERE provider_name = ?1
               AND finished_at IS NOT NULL
             ORDER BY id DESC
             LIMIT 1",
            params![provider],
            |row| row.get(0),
        )
        .unwrap()
}

fn latest_provider_session_id(fixture: &Age153Fixture, provider: &str) -> String {
    fixture
        .conn()
        .query_row(
            "SELECT provider_session_id
             FROM invocations
             WHERE provider_name = ?1
               AND provider_session_id IS NOT NULL
             ORDER BY id DESC
             LIMIT 1",
            params![provider],
            |row| row.get(0),
        )
        .unwrap()
}

fn failed_invocation_error_category(
    fixture: &Age153Fixture,
    provider: &str,
    terminal_reason: &str,
) -> Option<String> {
    fixture
        .conn()
        .query_row(
            "SELECT error_category
             FROM invocations
             WHERE provider_name = ?1
               AND terminal_reason = ?2
               AND finished_at IS NOT NULL
             ORDER BY id DESC
             LIMIT 1",
            params![provider, terminal_reason],
            |row| row.get(0),
        )
        .unwrap()
}

fn failed_invocation_count_with_error_category(
    fixture: &Age153Fixture,
    provider: &str,
    terminal_reason: &str,
    error_category: &str,
) -> i64 {
    fixture
        .conn()
        .query_row(
            "SELECT COUNT(*)
             FROM invocations
             WHERE provider_name = ?1
               AND terminal_reason = ?2
               AND error_category = ?3
               AND finished_at IS NOT NULL",
            params![provider, terminal_reason, error_category],
            |row| row.get(0),
        )
        .unwrap()
}

fn provider_invocation_sessions(fixture: &Age153Fixture, provider: &str) -> Vec<Option<String>> {
    let conn = fixture.conn();
    let mut stmt = conn
        .prepare(
            "SELECT provider_session_id
             FROM invocations
             WHERE provider_name = ?1
             ORDER BY id",
        )
        .unwrap();
    stmt.query_map(params![provider], |row| row.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

#[test]
fn one_shot_lagging_first_zero_turn_does_not_retry_or_mark_exhausted() {
    let fixture = Age153Fixture::new();
    let transcript = fixture.dir.path().join("one-shot-first-zero-turn.jsonl");
    fs::write(&transcript, "").unwrap();
    let marker = fixture.dir.path().join("one-shot-first-zero-turn.txt");
    let counter = fixture
        .dir
        .path()
        .join("one-shot-first-zero-turn-count.txt");
    let provider = fixture.write_script(
        "one-shot-first-zero-turn.sh",
        &zero_turn_then_assistant_turn_capture_body(&marker, &transcript, &counter),
    );
    write_capture_pool(
        &fixture,
        "age166-first-zero-turn",
        &[("claude-age166-a", &provider)],
        &[("claude-age166-a", &transcript)],
    );

    let output = fixture.run_one_shot_with_env(
        "age166-first-zero-turn",
        &[(FORCE_KIND, "MaybeQuotaExhausted")],
    );

    assert_no_terminal_marker_on_stdout(&output);
    let (_, stderr) = output_text(&output);
    assert_maybe_marker_without_zero_turn_evidence(&stderr);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(
        latest_invocation_terminal_reason(&fixture, "claude-age166-a").as_deref(),
        Some("maybe_quota_exhausted")
    );
    assert_eq!(
        fixture.exhausted_row_count(&["cla", "ude-age166-a"].concat()),
        0
    );
    let sessions = provider_invocation_sessions(&fixture, "claude-age166-a");
    assert_eq!(sessions.len(), 1, "{sessions:?}");
    assert!(sessions[0].is_some(), "{sessions:?}");
    assert_eq!(line_count(&marker), 1);
}

#[test]
fn one_shot_lagging_zero_turn_does_not_confirm_quota_or_migrate() {
    let fixture = Age153Fixture::new();
    let first_transcript = fixture.dir.path().join("one-shot-second-a.jsonl");
    let sibling_transcript = fixture.dir.path().join("one-shot-second-b.jsonl");
    fs::write(&first_transcript, "").unwrap();
    fs::write(&sibling_transcript, "").unwrap();
    let first_marker = fixture.dir.path().join("one-shot-second-a.txt");
    let sibling_marker = fixture.dir.path().join("one-shot-second-b.txt");
    let first = fixture.write_script(
        "one-shot-second-a.sh",
        &zero_turn_capture_body(&first_marker, 1),
    );
    let sibling = fixture.write_script(
        "one-shot-second-b.sh",
        &assistant_turn_stdout_capture_body(
            &sibling_marker,
            &sibling_transcript,
            "next provider ran",
            17,
        ),
    );
    write_capture_pool(
        &fixture,
        "age166-second-zero-turn",
        &[("claude-age166-a", &first), ("claude-age166-b", &sibling)],
        &[
            ("claude-age166-a", &first_transcript),
            ("claude-age166-b", &sibling_transcript),
        ],
    );

    let output = fixture.run_one_shot_with_env(
        "age166-second-zero-turn",
        &[(FORCE_KIND, "MaybeQuotaExhausted")],
    );

    assert_no_terminal_marker_on_stdout(&output);
    let (_, stderr) = output_text(&output);
    assert_maybe_marker_without_zero_turn_evidence(&stderr);
    assert_eq!(
        fixture.exhausted_row_count(&["cla", "ude-age166-a"].concat()),
        0
    );
    assert_eq!(
        failed_invocation_count_with_error_category(
            &fixture,
            "claude-age166-a",
            "maybe_quota_exhausted",
            "quota_exhausted"
        ),
        0
    );
    assert_eq!(
        failed_invocation_error_category(&fixture, "claude-age166-a", "maybe_quota_exhausted")
            .as_deref(),
        None
    );
    let sessions = provider_invocation_sessions(&fixture, "claude-age166-a");
    assert_eq!(sessions.len(), 1, "{sessions:?}");
    assert!(sessions[0].is_some(), "{sessions:?}");
    assert_eq!(line_count(&first_marker), 1);
    assert_eq!(line_count(&sibling_marker), 0);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
}

#[test]
fn one_shot_clean_exit_with_turn_preserves_status_zero_no_terminal_reason() {
    let fixture = Age153Fixture::new();
    let transcript = fixture.dir.path().join("one-shot-clean-turn.jsonl");
    fs::write(&transcript, "").unwrap();
    let marker = fixture.dir.path().join("one-shot-clean-turn.txt");
    let provider = fixture.write_script(
        "one-shot-clean-turn.sh",
        &assistant_turn_capture_body(&marker, &transcript, 0),
    );
    write_capture_pool(
        &fixture,
        "age166-clean-turn",
        &[("claude-age166-clean", &provider)],
        &[("claude-age166-clean", &transcript)],
    );

    let output = fixture.run_one_shot_with_env("age166-clean-turn", &[]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    let (stdout, stderr) = output_text(&output);
    assert_no_maybe_marker(&stderr);
    let expected_stdout = format!(
        "{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"age166-session-{}-age166-clean\"}}\n",
        ["cla", "ude"].concat()
    );
    assert_eq!(
        stdout, expected_stdout,
        "authoritative provider output must remain byte-clean"
    );
    assert_eq!(
        latest_invocation_terminal_reason(&fixture, "claude-age166-clean"),
        None
    );
    assert_eq!(fixture.exhausted_row_count("claude-age166-clean"), 0);
    assert_eq!(
        fixture.successful_invocation_count_without_terminal_reason("claude-age166-clean"),
        1
    );
    assert_eq!(line_count(&marker), 1);
}

#[test]
fn one_shot_failed_invocation_does_not_synchronously_scan_turns() {
    let fixture = Age153Fixture::new();
    let transcript = fixture.dir.path().join("one-shot-nonzero-turn.jsonl");
    fs::write(&transcript, "").unwrap();
    let marker = fixture.dir.path().join("one-shot-nonzero-turn.txt");
    let provider = fixture.write_script(
        "one-shot-nonzero-turn.sh",
        &assistant_turn_capture_body(&marker, &transcript, 7),
    );
    write_capture_pool(
        &fixture,
        "age166-nonzero-turn",
        &[("claude-age166-nonzero", &provider)],
        &[("claude-age166-nonzero", &transcript)],
    );

    let output =
        fixture.run_one_shot_with_env("age166-nonzero-turn", &[(FORCE_KIND, "NonzeroExit")]);

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    let (_, stderr) = output_text(&output);
    assert_no_maybe_marker(&stderr);
    assert_eq!(
        fixture.failed_invocation_count("claude-age166-nonzero", "exit_nonzero"),
        1
    );
    assert_eq!(
        failed_invocation_error_category(&fixture, "claude-age166-nonzero", "exit_nonzero")
            .as_deref(),
        None
    );
    let provider_session_id = latest_provider_session_id(&fixture, "claude-age166-nonzero");
    let counts = fixture
        .open_db()
        .count_session_turns("claude-age166-nonzero", &provider_session_id)
        .unwrap();
    assert_eq!(counts.assistant, 0);
    assert_eq!(fixture.exhausted_row_count("claude-age166-nonzero"), 0);
    assert_eq!(line_count(&marker), 1);
}

#[test]
fn one_shot_provider_without_session_id_does_not_emit_maybe_quota() {
    let fixture = Age153Fixture::new();
    let marker = fixture.dir.path().join("one-shot-no-session-id.txt");
    fixture.write_model("age166-no-session-id", &["openai-compatible-age166"]);
    fixture.write_providers_with_bodies(&[(
        "openai-compatible-age166",
        &success_body(&marker, "ordinary sessionless success"),
    )]);

    let output = fixture.run_one_shot_with_env("age166-no-session-id", &[]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    let (stdout, stderr) = output_text(&output);
    assert_no_maybe_marker(&stderr);
    let result = assert_result_envelope_shape(&stdout);
    assert_eq!(result["success"], true);
    assert!(result["terminal_reason"].is_null(), "{result}");
    assert_eq!(fixture.exhausted_row_count("openai-compatible-age166"), 0);
    assert_eq!(
        fixture.successful_invocation_count_without_terminal_reason("openai-compatible-age166"),
        1
    );
}

#[test]
fn quota_exhausted_inband_semantics_regression_e2e() {
    let fixture = Age153Fixture::new();
    let first_marker = fixture.dir.path().join("one-shot-inband-a.txt");
    let sibling_marker = fixture.dir.path().join("one-shot-inband-b.txt");
    fixture.write_model(
        "age166-inband-regression",
        &["claude-age166-inband-a", "claude-age166-inband-b"],
    );
    fixture.write_providers_with_bodies(&[
        (
            "claude-age166-inband-a",
            &zero_turn_capture_body(&first_marker, 42),
        ),
        (
            "claude-age166-inband-b",
            &success_body(&sibling_marker, "inband sibling ran"),
        ),
    ]);

    let output = fixture.run_one_shot_with_env(
        "age166-inband-regression",
        &[(FORCE_KIND, "QuotaExhaustedInband")],
    );

    assert_no_terminal_marker_on_stdout(&output);
    let (_, stderr) = output_text(&output);
    assert!(
        terminal_signal_lines(&stderr)
            .iter()
            .any(|line| line.contains("\"kind\":\"QuotaExhaustedInband\"")),
        "{stderr}"
    );
    assert_eq!(fixture.exhausted_row_count("claude-age166-inband-a"), 1);
    assert_eq!(
        fixture.failed_invocation_count("claude-age166-inband-a", "quota_exhausted_inband"),
        1
    );
    assert_eq!(
        failed_invocation_error_category(
            &fixture,
            "claude-age166-inband-a",
            "quota_exhausted_inband"
        )
        .as_deref(),
        Some("quota_exhausted")
    );
    assert_eq!(line_count(&sibling_marker), 1);
}

// AGE-166 F3: openai_compat follows the same conservative freshness rule as
// the Claude path. A newly captured session without a caught-up canonical
// checkpoint cannot produce conclusive zero-turn evidence.
//
// The provider name does not start with "claude" or "codex" so
// `ProviderRecognizer::for_provider` routes to OpenAiCompat.
#[test]
fn one_shot_openai_compat_lagging_zero_turn_does_not_retry_or_mark_exhausted() {
    let fixture = Age153Fixture::new();
    let transcript = fixture
        .dir
        .path()
        .join("one-shot-oai-compat-first-zero-turn.jsonl");
    fs::write(&transcript, "").unwrap();
    let marker = fixture
        .dir
        .path()
        .join("one-shot-oai-compat-first-zero-turn.txt");
    let counter = fixture
        .dir
        .path()
        .join("one-shot-oai-compat-first-zero-turn-count.txt");
    let provider = fixture.write_script(
        "one-shot-oai-compat-first-zero-turn.sh",
        &zero_turn_then_assistant_turn_capture_body(&marker, &transcript, &counter),
    );
    write_capture_pool(
        &fixture,
        "age166-oai-compat-first-zero-turn",
        &[("openai-compat-age166-zt-a", &provider)],
        &[("openai-compat-age166-zt-a", &transcript)],
    );

    let output = fixture.run_one_shot_with_env(
        "age166-oai-compat-first-zero-turn",
        &[(FORCE_KIND, "MaybeQuotaExhausted")],
    );

    assert_no_terminal_marker_on_stdout(&output);
    let (_, stderr) = output_text(&output);
    assert_maybe_marker_without_zero_turn_evidence(&stderr);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(
        latest_invocation_terminal_reason(&fixture, "openai-compat-age166-zt-a").as_deref(),
        Some("maybe_quota_exhausted")
    );
    assert_eq!(fixture.exhausted_row_count("openai-compat-age166-zt-a"), 0);
    let sessions = provider_invocation_sessions(&fixture, "openai-compat-age166-zt-a");
    assert_eq!(sessions.len(), 1, "{sessions:?}");
    assert!(sessions[0].is_some(), "{sessions:?}");
    assert_eq!(line_count(&marker), 1);
}

// A lagging openai_compat session must not confirm quota exhaustion or migrate
// to a sibling account merely because its cached turn count is still zero.
#[test]
fn one_shot_openai_compat_lagging_zero_turn_does_not_confirm_or_migrate() {
    let fixture = Age153Fixture::new();
    let first_transcript = fixture
        .dir
        .path()
        .join("one-shot-oai-compat-second-a.jsonl");
    let sibling_transcript = fixture
        .dir
        .path()
        .join("one-shot-oai-compat-second-b.jsonl");
    fs::write(&first_transcript, "").unwrap();
    fs::write(&sibling_transcript, "").unwrap();
    let first_marker = fixture.dir.path().join("one-shot-oai-compat-second-a.txt");
    let sibling_marker = fixture.dir.path().join("one-shot-oai-compat-second-b.txt");
    let first = fixture.write_script(
        "one-shot-oai-compat-second-a.sh",
        &zero_turn_capture_body(&first_marker, 1),
    );
    let sibling = fixture.write_script(
        "one-shot-oai-compat-second-b.sh",
        &assistant_turn_stdout_capture_body(
            &sibling_marker,
            &sibling_transcript,
            "sibling openai-compat provider ran",
            23,
        ),
    );
    write_capture_pool(
        &fixture,
        "age166-oai-compat-second-zero-turn",
        &[
            ("openai-compat-age166-zt-a", &first),
            ("openai-compat-age166-zt-b", &sibling),
        ],
        &[
            ("openai-compat-age166-zt-a", &first_transcript),
            ("openai-compat-age166-zt-b", &sibling_transcript),
        ],
    );

    let output = fixture.run_one_shot_with_env(
        "age166-oai-compat-second-zero-turn",
        &[(FORCE_KIND, "MaybeQuotaExhausted")],
    );

    assert_no_terminal_marker_on_stdout(&output);
    let (_, stderr) = output_text(&output);
    assert_maybe_marker_without_zero_turn_evidence(&stderr);
    assert_eq!(fixture.exhausted_row_count("openai-compat-age166-zt-a"), 0);
    assert_eq!(
        failed_invocation_error_category(
            &fixture,
            "openai-compat-age166-zt-a",
            "maybe_quota_exhausted"
        )
        .as_deref(),
        None
    );
    let sessions = provider_invocation_sessions(&fixture, "openai-compat-age166-zt-a");
    assert_eq!(sessions.len(), 1, "{sessions:?}");
    assert!(sessions[0].is_some(), "{sessions:?}");
    assert_eq!(line_count(&first_marker), 1);
    assert_eq!(line_count(&sibling_marker), 0);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
}
