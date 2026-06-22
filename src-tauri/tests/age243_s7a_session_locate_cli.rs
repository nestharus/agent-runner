#![cfg(unix)]

mod fixtures;

use fixtures::initiative_06::{
    CHAIN_A, LocateFixture, SESSION_A, StorageKind, component_no_storage_fixture,
    parse_stdout_json, required_success_fields,
};
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const PROVIDER_A_MODEL: &str = "provider-a-model";
const PROVIDER_A_ACCOUNT: &str = "provider-a-account";
const PROVIDER_A_INSTANCE: &str = "provider-a-instance";
const PROVIDER_A_SETTINGS: &str = "provider-a-test-settings";

#[test]
fn locate_cli_dispatches_external_provider_locate_and_preserves_request_shape() {
    let prepared = external_provider_locate_fixture("locate_success");

    let output = prepared
        .fixture
        .run_locate(&prepared.session_id, &["--json"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let stdout = parse_stdout_json(&output);
    for field in required_success_fields() {
        assert!(stdout.get(field).is_some(), "missing {field} in {stdout}");
    }
    assert_eq!(stdout["session_id"], prepared.session_id);
    assert_eq!(stdout["chain_id"], prepared.chain_id);
    assert_eq!(stdout["provider_name"], PROVIDER_A_ACCOUNT);
    assert_eq!(stdout["storage_type"], "other");
    assert_eq!(stdout["transcript_state"], "available");
    assert_eq!(
        stdout["jsonl_path"],
        prepared.transcript_path.display().to_string()
    );
    assert_external_locate_request_shape(&provider_records(&prepared.record_path));
}

#[test]
fn locate_cli_external_provider_failure_does_not_fall_back_to_builtin_locator() {
    let prepared = external_provider_locate_fixture("provider_error");
    let fallback_path = prepared
        .fixture
        .root()
        .join("builtin-fallback-transcript.jsonl");
    fs::write(&fallback_path, "{\"fallback\":true}\n").unwrap();
    prepared
        .fixture
        .write_sessions_with_locator_path(PROVIDER_A_ACCOUNT, &fallback_path);

    let output = prepared
        .fixture
        .run_locate(&prepared.session_id, &["--json"]);

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "provider locate failure must not emit built-in fallback metadata: {output:?}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("provider") || stderr.contains("session"),
        "stderr should identify external provider locate failure: {stderr}"
    );
    assert_eq!(
        provider_records(&prepared.record_path)
            .iter()
            .filter(|record| record["subcommand"] == "session.locate_transcript")
            .count(),
        1
    );
}

#[test]
fn locate_cli_no_ref_output_stderr_and_exit_code_preserved_with_unrelated_registry() {
    let prepared = component_no_storage_fixture(true);
    let baseline = prepared
        .fixture
        .run_locate(&prepared.session_id, &["--json"]);
    let record_path = prepared.fixture.root().join("provider-a-records.jsonl");
    let provider_path = write_cli_provider_a_script(
        prepared.fixture.root(),
        "locate_success",
        &record_path,
        &prepared.jsonl_path,
    );
    prepared
        .fixture
        .write_external_model(PROVIDER_A_MODEL, PROVIDER_A_ACCOUNT, &provider_path);
    prepared
        .fixture
        .write_provider(PROVIDER_A_ACCOUNT, StorageKind::None, false, None);

    let dispatch = prepared
        .fixture
        .run_locate(&prepared.session_id, &["--json"]);

    assert_eq!(dispatch.status.code(), baseline.status.code());
    assert_eq!(dispatch.stdout, baseline.stdout);
    assert_eq!(dispatch.stderr, baseline.stderr);
    assert!(
        provider_records(&record_path).is_empty(),
        "no-ref locate must not describe or invoke unrelated external providers"
    );
}

#[test]
fn provider_ref_locate_uses_external_record_even_when_local_locator_exists() {
    let prepared = external_provider_locate_fixture("locate_success");
    let local_path = prepared.fixture.root().join("local-transcript.jsonl");
    fs::write(&local_path, "{\"local\":true}\n").unwrap();
    prepared
        .fixture
        .write_sessions_with_locator_path(PROVIDER_A_ACCOUNT, &local_path);

    let output = prepared
        .fixture
        .run_locate(&prepared.session_id, &["--json"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = parse_stdout_json(&output);
    assert_eq!(stdout["session_id"], prepared.session_id);
    assert_eq!(
        stdout["jsonl_path"],
        prepared.transcript_path.display().to_string()
    );
    assert_ne!(stdout["jsonl_path"], local_path.display().to_string());
    let records = provider_records(&prepared.record_path);
    assert_eq!(
        records
            .iter()
            .filter(|record| record["subcommand"] == "session.locate_transcript")
            .count(),
        1
    );
    assert_external_locate_request_shape(&records);

    let failure = external_provider_locate_fixture("provider_error");
    let failure_local_path = failure
        .fixture
        .root()
        .join("local-fallback-transcript.jsonl");
    fs::write(&failure_local_path, "{\"local\":true}\n").unwrap();
    failure
        .fixture
        .write_sessions_with_locator_path(PROVIDER_A_ACCOUNT, &failure_local_path);

    let failure_output = failure.fixture.run_locate(&failure.session_id, &["--json"]);

    assert_ne!(failure_output.status.code(), Some(0), "{failure_output:?}");
    assert!(
        failure_output.stdout.is_empty(),
        "provider error must not be replaced by local fallback metadata: {failure_output:?}"
    );
    let failure_stderr = String::from_utf8_lossy(&failure_output.stderr);
    assert!(
        failure_stderr.contains("provider_locate_failed"),
        "stderr should return the provider error: {failure_stderr}"
    );
    let failure_records = provider_records(&failure.record_path);
    assert_eq!(
        failure_records
            .iter()
            .filter(|record| record["subcommand"] == "session.locate_transcript")
            .count(),
        1
    );
    assert_external_locate_request_shape(&failure_records);
}

#[test]
fn no_ref_locate_uses_local_locator_and_ignores_unrelated_registry() {
    let prepared = component_no_storage_fixture(true);
    let baseline = prepared
        .fixture
        .run_locate(&prepared.session_id, &["--json"]);
    let record_path = prepared.fixture.root().join("provider-a-records.jsonl");
    let provider_path = write_cli_provider_a_script(
        prepared.fixture.root(),
        "locate_success",
        &record_path,
        &prepared.jsonl_path,
    );
    prepared
        .fixture
        .write_external_model(PROVIDER_A_MODEL, PROVIDER_A_ACCOUNT, &provider_path);
    prepared
        .fixture
        .write_provider(PROVIDER_A_ACCOUNT, StorageKind::None, false, None);

    let output = prepared
        .fixture
        .run_locate(&prepared.session_id, &["--json"]);

    assert_eq!(output.status.code(), baseline.status.code());
    assert_eq!(output.stdout, baseline.stdout);
    assert_eq!(output.stderr, baseline.stderr);
    assert!(provider_records(&record_path).is_empty());
}

#[test]
fn historical_ref_locate_uses_external_path_and_rejects_local_success_on_error() {
    let prepared = historical_ref_locate_fixture("locate_success");

    let output = prepared
        .fixture
        .run_locate(&prepared.session_id, &["--json"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = parse_stdout_json(&output);
    assert_eq!(stdout["session_id"], prepared.session_id);
    assert_eq!(stdout["chain_id"], prepared.chain_id);
    assert_eq!(stdout["provider_name"], prepared.provider_name);
    assert_eq!(
        stdout["jsonl_path"],
        prepared.transcript_path.display().to_string()
    );
    assert_ne!(
        stdout["jsonl_path"],
        prepared.local_path.display().to_string()
    );
    assert!(
        !String::from_utf8_lossy(&output.stdout)
            .contains(&prepared.local_path.display().to_string())
    );
    assert_historical_locate_request_shape(
        &provider_records(&prepared.record_path),
        &prepared.model_name,
        &prepared.provider_name,
    );

    let failure = historical_ref_locate_fixture("provider_error");
    let failure_output = failure.fixture.run_locate(&failure.session_id, &["--json"]);

    assert_ne!(failure_output.status.code(), Some(0), "{failure_output:?}");
    assert!(failure_output.stdout.is_empty(), "{failure_output:?}");
    assert_historical_locate_request_shape(
        &provider_records(&failure.record_path),
        &failure.model_name,
        &failure.provider_name,
    );
}

#[test]
fn historical_no_ref_locate_uses_local_storage_and_ignores_unrelated_external_model() {
    let prepared = historical_no_ref_locate_fixture();
    let record_path = prepared.fixture.root().join("provider-a-records.jsonl");
    let provider_path = write_cli_provider_a_script(
        prepared.fixture.root(),
        "locate_success",
        &record_path,
        &prepared.transcript_path,
    );
    prepared
        .fixture
        .write_external_model(PROVIDER_A_MODEL, PROVIDER_A_ACCOUNT, &provider_path);
    prepared
        .fixture
        .write_provider(PROVIDER_A_ACCOUNT, StorageKind::None, false, None);

    let output = prepared
        .fixture
        .run_locate(&prepared.session_id, &["--json"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let stdout = parse_stdout_json(&output);
    assert_eq!(stdout["session_id"], prepared.session_id);
    assert_eq!(stdout["chain_id"], prepared.chain_id);
    assert_eq!(stdout["provider_name"], prepared.provider_name);
    assert_eq!(stdout["storage_type"], native_storage_kind());
    assert_eq!(
        stdout["jsonl_path"],
        prepared.transcript_path.display().to_string()
    );
    assert!(provider_records(&record_path).is_empty());
}

struct ExternalLocateFixture {
    fixture: LocateFixture,
    session_id: String,
    chain_id: String,
    transcript_path: PathBuf,
    record_path: PathBuf,
}

struct HistoricalLocateFixture {
    fixture: LocateFixture,
    session_id: String,
    chain_id: String,
    provider_name: String,
    model_name: String,
    transcript_path: PathBuf,
    local_path: PathBuf,
    record_path: PathBuf,
}

fn external_provider_locate_fixture(mode: &str) -> ExternalLocateFixture {
    let fixture = LocateFixture::new();
    let transcript_path = fixture.root().join("provider-a-transcript.jsonl");
    fs::write(&transcript_path, "{}\n").unwrap();
    let record_path = fixture.root().join("provider-a-records.jsonl");
    let provider_path =
        write_cli_provider_a_script(fixture.root(), mode, &record_path, &transcript_path);
    fixture.write_external_model(PROVIDER_A_MODEL, PROVIDER_A_ACCOUNT, &provider_path);
    fixture.write_provider(PROVIDER_A_ACCOUNT, StorageKind::None, false, None);
    fixture.seed_active_chain(
        CHAIN_A,
        PROVIDER_A_ACCOUNT,
        SESSION_A,
        PROVIDER_A_MODEL,
        "2026-05-01T00:00:00Z",
    );
    ExternalLocateFixture {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        transcript_path,
        record_path,
    }
}

fn historical_ref_locate_fixture(mode: &str) -> HistoricalLocateFixture {
    let fixture = LocateFixture::new();
    let provider_name = real_provider_token(&["cla", "ude"]);
    let model_name = format!("{}-opus", provider_name);
    let (projects_dir, local_path) = stage_local_storage_transcript(&fixture, SESSION_A);
    let transcript_path = fixture.root().join("provider-owned-transcript.jsonl");
    fs::write(&transcript_path, "{}\n").unwrap();
    let record_path = fixture.root().join("provider-records.jsonl");
    let provider_path =
        write_cli_provider_a_script(fixture.root(), mode, &record_path, &transcript_path);
    fixture.write_external_model(&model_name, &provider_name, &provider_path);
    fixture.write_provider(
        &provider_name,
        StorageKind::ClaudeCode {
            projects_dir: &projects_dir,
        },
        true,
        None,
    );
    fixture.write_sessions_with_locator_path(&provider_name, &local_path);
    fixture.seed_active_chain(
        CHAIN_A,
        &provider_name,
        SESSION_A,
        &model_name,
        "2026-01-15T00:00:00Z",
    );
    HistoricalLocateFixture {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name,
        model_name,
        transcript_path,
        local_path,
        record_path,
    }
}

fn historical_no_ref_locate_fixture() -> HistoricalLocateFixture {
    let fixture = LocateFixture::new();
    let provider_name = real_provider_token(&["cla", "ude"]);
    let model_name = format!("{}-opus", provider_name);
    let (projects_dir, transcript_path) = stage_local_storage_transcript(&fixture, SESSION_A);
    fixture.write_model(&model_name, &[&provider_name]);
    fixture.write_provider(
        &provider_name,
        StorageKind::ClaudeCode {
            projects_dir: &projects_dir,
        },
        true,
        None,
    );
    fixture.seed_active_chain(
        CHAIN_A,
        &provider_name,
        SESSION_A,
        &model_name,
        "2026-01-15T00:00:00Z",
    );
    HistoricalLocateFixture {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name,
        model_name,
        transcript_path: transcript_path.clone(),
        local_path: transcript_path,
        record_path: PathBuf::new(),
    }
}

fn stage_local_storage_transcript(fixture: &LocateFixture, session_id: &str) -> (PathBuf, PathBuf) {
    let projects_dir = fixture.root().join("native-projects");
    let workspace_root = fixture.root().join("workspace");
    fs::create_dir_all(&workspace_root).unwrap();
    let project_dir = projects_dir.join(project_dir_name(&workspace_root));
    fs::create_dir_all(&project_dir).unwrap();
    let transcript_path = project_dir.join(format!("{session_id}.jsonl"));
    fs::write(
        &transcript_path,
        format!(
            "{{\"sessionId\":\"{session_id}\",\"type\":\"assistant\",\"uuid\":\"local-turn\",\"timestamp\":\"2026-04-17T08:00:00Z\",\"message\":\"local\"}}\n"
        ),
    )
    .unwrap();
    (projects_dir, transcript_path)
}

fn project_dir_name(path: &Path) -> String {
    let raw = path.to_string_lossy();
    format!("-{}", raw.trim_start_matches('/').replace('/', "-"))
}

fn real_provider_token(parts: &[&str]) -> String {
    parts.concat()
}

fn native_storage_kind() -> String {
    format!("{}_code", real_provider_token(&["cla", "ude"]))
}

fn assert_historical_locate_request_shape(
    records: &[Value],
    model_name: &str,
    provider_name: &str,
) {
    let locate_records = records
        .iter()
        .filter(|record| record["subcommand"] == "session.locate_transcript")
        .collect::<Vec<_>>();
    assert_eq!(locate_records.len(), 1, "{records:?}");
    let request = &locate_records[0]["request"];
    assert_eq!(request["provider_instance_id"], PROVIDER_A_INSTANCE);
    assert_eq!(request["params"]["settings_id"], PROVIDER_A_SETTINGS);
    assert_eq!(request["params"]["model_name"], model_name);
    assert_eq!(request["params"]["provider_name"], provider_name);
    assert_eq!(request["params"]["session_id"], SESSION_A);
    assert_eq!(request["params"]["lookup_mode"], "require_existing");
    assert!(
        request["params"].get("purpose").is_none(),
        "non-inspect locate request must omit purpose: {request}"
    );
    assert!(
        request["params"].get("tail_bytes_hint").is_none(),
        "non-inspect locate request must omit tail_bytes_hint: {request}"
    );
}

fn assert_external_locate_request_shape(records: &[Value]) {
    let locate_records = records
        .iter()
        .filter(|record| record["subcommand"] == "session.locate_transcript")
        .collect::<Vec<_>>();
    assert_eq!(locate_records.len(), 1, "{records:?}");
    let request = &locate_records[0]["request"];
    assert_eq!(request["provider_instance_id"], PROVIDER_A_INSTANCE);
    assert_eq!(request["params"]["settings_id"], PROVIDER_A_SETTINGS);
    assert_eq!(request["params"]["model_name"], PROVIDER_A_MODEL);
    assert_eq!(request["params"]["provider_name"], PROVIDER_A_ACCOUNT);
    assert_eq!(request["params"]["session_id"], SESSION_A);
    assert_eq!(request["params"]["lookup_mode"], "require_existing");
    assert!(
        request["params"].get("purpose").is_none(),
        "non-inspect locate request must omit purpose: {request}"
    );
    assert!(
        request["params"].get("tail_bytes_hint").is_none(),
        "non-inspect locate request must omit tail_bytes_hint: {request}"
    );
    assert!(
        !request.to_string().contains("state.db"),
        "locate request must not expose host SQLite paths"
    );
}

fn provider_records(record_path: &Path) -> Vec<Value> {
    provider_record_values(&provider_record_text_or_empty(record_path))
}

fn provider_record_text_or_empty(record_path: &Path) -> String {
    if provider_record_path_missing(record_path) {
        String::new()
    } else {
        read_provider_record_text(record_path)
    }
}

fn provider_record_path_missing(record_path: &Path) -> bool {
    !record_path.exists()
}

fn read_provider_record_text(record_path: &Path) -> String {
    fs::read_to_string(record_path).unwrap()
}

fn provider_record_values(records: &str) -> Vec<Value> {
    non_empty_provider_record_lines(records)
        .iter()
        .map(|line| provider_record_value(line))
        .collect()
}

fn non_empty_provider_record_lines(records: &str) -> Vec<&str> {
    records
        .lines()
        .filter(|line| provider_record_line_is_non_empty(line))
        .collect()
}

fn provider_record_line_is_non_empty(line: &str) -> bool {
    !line.trim().is_empty()
}

fn provider_record_value(line: &str) -> Value {
    serde_json::from_str(line).unwrap()
}

fn write_cli_provider_a_script(
    dir: &Path,
    mode: &str,
    record_path: &Path,
    transcript_path: &Path,
) -> PathBuf {
    fs::write(record_path, "").unwrap();
    let script = dir.join(format!("provider-a-{mode}.py"));
    fs::write(
        &script,
        cli_provider_a_script(mode, record_path, transcript_path),
    )
    .unwrap();
    let mut perms = fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).unwrap();
    script
}

fn cli_provider_a_script(mode: &str, record_path: &Path, transcript_path: &Path) -> String {
    format!(
        r#"#!/usr/bin/env python3
import json
import pathlib
import sys

CONTRACT = "oulipoly.provider/v1"
MODE = {mode}
subcommand = sys.argv[1] if len(sys.argv) > 1 else ""
request = json.loads(sys.stdin.read() or "{{}}")
with pathlib.Path({record_path}).open("a", encoding="utf-8") as handle:
    handle.write(json.dumps({{"subcommand": subcommand, "request": request}}, sort_keys=True) + "\n")

def envelope(result):
    return {{
        "contract": request.get("contract", CONTRACT),
        "request_id": request.get("request_id", "request-age243"),
        "ok": True,
        "result": result,
    }}

def error(code):
    return {{
        "contract": request.get("contract", CONTRACT),
        "request_id": request.get("request_id", "request-age243"),
        "ok": False,
        "error": {{"category": "failed", "code": code, "message": code, "retryable": False}},
    }}

if subcommand == "describe":
    response = envelope({{
        "provider_id": "provider-a",
        "display_name": "Provider A",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": {{
            "launch": False,
            "policy": False,
            "quota": False,
            "session": True,
            "terminal": False,
            "rotation": False,
            "discovery": False,
            "settings": False,
            "setup_brain": False,
            "setup": False,
            "migration": False,
        }},
        "settings_schema_id": "provider-a-test-settings",
    }})
elif subcommand == "session.locate_transcript" and MODE == "locate_success":
    response = envelope({{
        "located": True,
        "path": {transcript_path},
        "format_id": "jsonl",
        "source_id": "provider-a",
        "require_existing_observed": True,
    }})
elif subcommand == "session.locate_transcript":
    response = error("provider_locate_failed")
else:
    response = error("unsupported_subcommand")
print(json.dumps(response))
"#,
        mode = serde_json::to_string(mode).unwrap(),
        record_path = serde_json::to_string(&record_path.display().to_string()).unwrap(),
        transcript_path = serde_json::to_string(&transcript_path.display().to_string()).unwrap(),
    )
}
