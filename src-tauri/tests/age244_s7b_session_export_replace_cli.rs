#![cfg(unix)]

mod fixtures;

use base64::Engine;
use fixtures::initiative_06_export::{ExportFixture, SESSION_A};
use fixtures::initiative_06_import_replace::{
    CHAIN_A as REPLACE_CHAIN_A, ImportReplaceFixture, MODEL, canonical_jsonl,
};
use oulipoly_runtime::session_replace::ReplaceReceipt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const EXTERNAL_MODEL: &str = "provider-a-model";
const EXTERNAL_PROVIDER: &str = "provider-a-account";

#[test]
fn export_cli_external_provider_model_reaches_session_export_dispatch() {
    let fixture = ExportFixture::new();
    let record_path = fixture.root().join("provider-records.jsonl");
    let canonical_path = fixture.root().join("external-canonical.jsonl");
    fs::write(&canonical_path, external_canonical_jsonl()).unwrap();
    let provider_path = write_cli_session_provider(
        fixture.root(),
        "export_success",
        &record_path,
        &canonical_path,
        &fixture.root().join("unused-native.jsonl"),
    );
    fixture.write_external_model(EXTERNAL_MODEL, EXTERNAL_PROVIDER, &provider_path);
    fixture.write_provider(
        EXTERNAL_PROVIDER,
        fixtures::initiative_06_export::StorageKind::None,
        false,
        None,
    );
    fixture.set_provider_authority(EXTERNAL_PROVIDER, &provider_path);
    fixture.seed_active_chain(
        fixtures::initiative_06_export::CHAIN_A,
        EXTERNAL_PROVIDER,
        SESSION_A,
        EXTERNAL_MODEL,
        "2026-05-01T00:00:00Z",
    );

    let output = fixture.run_export(SESSION_A, &[]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    assert_eq!(output.stdout, fs::read(&canonical_path).unwrap());
    let records = provider_records(&record_path);
    assert_subcommand_count(&records, "describe", 1);
    assert_subcommand_count(&records, "session.export", 1);
    let request = request_for(&records, "session.export");
    assert_eq!(request["params"]["session_id"], SESSION_A);
    assert_eq!(request["params"]["provider_name"], EXTERNAL_PROVIDER);
    assert_eq!(request["params"]["model_name"], EXTERNAL_MODEL);
}

#[test]
fn import_replace_cli_external_provider_model_reaches_session_replace_dispatch_without_builtin_apply()
 {
    let prepared = external_replace_fixture("replace_provider_error");
    let input = canonical_jsonl(
        &prepared.session_id,
        EXTERNAL_PROVIDER,
        &prepared.jsonl_path,
        "external-cli",
    );

    let output = prepared
        .fixture
        .run_import_replace(&prepared.session_id, &input, &[]);

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("provider_replace_failed"), "{stderr}");
    let records = provider_records(&prepared.record_path);
    assert_subcommand_count(&records, "describe", 1);
    assert_subcommand_count(&records, "session.replace", 1);
    let request = request_for(&records, "session.replace");
    assert_eq!(request["params"]["session_id"], prepared.session_id);
    assert_eq!(request["params"]["provider_name"], EXTERNAL_PROVIDER);
    assert_eq!(request["params"]["model_name"], MODEL);
    let transcript = fs::read_to_string(&prepared.jsonl_path).unwrap();
    assert!(!transcript.contains("external-cli user"), "{transcript}");
}

#[test]
fn import_replace_cli_provider_owned_success_uses_receipt_evidence_without_local_native_transcript()
{
    let prepared = external_replace_fixture("replace_provider_owned_success");
    let input = canonical_jsonl(
        &prepared.session_id,
        EXTERNAL_PROVIDER,
        &prepared.jsonl_path,
        "external-cli-owned",
    );
    let canonical_hash = sha256_hex(input.as_bytes());
    fs::remove_file(&prepared.jsonl_path).unwrap();

    let output = prepared.fixture.run_import_replace(
        &prepared.session_id,
        &input,
        &["--preimage-sha256", &provider_owned_preimage_sha256()],
    );

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let receipt: ReplaceReceipt = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt.preimage_sha256, provider_owned_preimage_sha256());
    assert_eq!(receipt.postimage_sha256, canonical_hash);
    assert_eq!(
        receipt.jsonl_path,
        PathBuf::from(provider_owned_source_id())
    );
    assert!(!prepared.jsonl_path.exists());
    let records = provider_records(&prepared.record_path);
    assert_subcommand_count(&records, "describe", 1);
    assert_subcommand_count(&records, "session.replace", 1);
    assert_provider_owned_request_shape(request_for(&records, "session.replace"), input.as_bytes());
    assert_provider_request_excludes_host_apply_authority(request_for(&records, "session.replace"));
}

#[test]
fn import_replace_cli_provider_owned_protocol_failure_does_not_use_builtin_apply() {
    let prepared = external_replace_fixture("replace_missing_operation_id");
    let input = canonical_jsonl(
        &prepared.session_id,
        EXTERNAL_PROVIDER,
        &prepared.jsonl_path,
        "external-cli-failure",
    );
    let before = prepared.fixture.mutation_snapshot(
        &prepared.jsonl_path,
        EXTERNAL_PROVIDER,
        &prepared.session_id,
    );

    let output = prepared
        .fixture
        .run_import_replace(&prepared.session_id, &input, &[]);

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("missing_operation_id"), "{stderr}");
    let after = prepared.fixture.mutation_snapshot(
        &prepared.jsonl_path,
        EXTERNAL_PROVIDER,
        &prepared.session_id,
    );
    assert_eq!(after.transcript_bytes, before.transcript_bytes);
    assert_eq!(after.turn_rows, before.turn_rows);
    assert_eq!(after.journal_files.len(), 1, "{after:?}");
    let journal: Value =
        serde_json::from_slice(&fs::read(&after.journal_files[0]).unwrap()).unwrap();
    assert_eq!(journal["operation_id"], provider_owned_operation_id());
    assert_eq!(journal["recovery_id"], provider_owned_recovery_id());
    assert!(
        journal["failure_context"]
            .as_str()
            .is_some_and(|context| context.contains("missing_operation_id")),
        "{journal}"
    );
    let records = provider_records(&prepared.record_path);
    assert_subcommand_count(&records, "session.replace", 1);
    assert_provider_request_excludes_host_apply_authority(request_for(&records, "session.replace"));
}

#[test]
fn export_cli_external_provider_describe_failure_does_not_fall_back_to_builtin_storage() {
    let prepared = external_export_with_builtin_fallback_fixture("describe_error");

    let output = prepared.fixture.run_export(&prepared.session_id, &[]);

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "external provider failure must not emit builtin fallback bytes: {output:?}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("provider_describe_failed") || stderr.contains("external provider"),
        "{stderr}"
    );
    let records = provider_records(&prepared.record_path);
    assert_subcommand_count(&records, "describe", 1);
    assert_subcommand_count(&records, "session.export", 0);
}

struct ExternalReplaceFixture {
    fixture: ImportReplaceFixture,
    session_id: String,
    jsonl_path: PathBuf,
    record_path: PathBuf,
}

struct ExternalExportFallbackFixture {
    fixture: ExportFixture,
    session_id: String,
    record_path: PathBuf,
}

fn external_replace_fixture(mode: &str) -> ExternalReplaceFixture {
    let fixture = ImportReplaceFixture::new();
    let jsonl_path = fixture.stage_jsonl(
        "native-transcript.jsonl",
        &format!(
            "{}\n{}\n",
            native_line(SESSION_A, "old-turn-1", "user", "old user", 0),
            native_line(SESSION_A, "old-turn-2", "assistant", "old assistant", 1)
        ),
    );
    let record_path = fixture.root().join("provider-records.jsonl");
    let provider_path = write_cli_session_provider(
        fixture.root(),
        mode,
        &record_path,
        &fixture.root().join("unused-canonical.jsonl"),
        &jsonl_path,
    );
    fixture.write_external_model(MODEL, EXTERNAL_PROVIDER, &provider_path);
    let workspace_root = fixture.root().join("workspace");
    fs::create_dir_all(&workspace_root).unwrap();
    let cwd_script = fixture.write_script(
        "native-cwd.sh",
        &format!(
            "printf '%s\\n' {}\n",
            shell_single_quoted(&json!({"found": true, "cwd": workspace_root}).to_string())
        ),
    );
    let transcript_script = fixture.write_script(
        "native-transcript.sh",
        &format!(
            "printf '%s\\n' {}\n",
            shell_single_quoted(&jsonl_path.display().to_string())
        ),
    );
    fixture.write_provider_with_script_storage(
        EXTERNAL_PROVIDER,
        &native_storage_kind(),
        &cwd_script,
        &transcript_script,
    );
    fixture.set_provider_authority(EXTERNAL_PROVIDER, &provider_path);
    fixture.write_sessions_with_locator_path(EXTERNAL_PROVIDER, &jsonl_path);
    fixture.seed_active_chain(
        REPLACE_CHAIN_A,
        EXTERNAL_PROVIDER,
        SESSION_A,
        MODEL,
        "2026-04-17T08:00:00Z",
    );
    fixture.seed_turns_with_metadata(EXTERNAL_PROVIDER, SESSION_A, &jsonl_path);
    ExternalReplaceFixture {
        fixture,
        session_id: SESSION_A.to_string(),
        jsonl_path,
        record_path,
    }
}

fn external_export_with_builtin_fallback_fixture(mode: &str) -> ExternalExportFallbackFixture {
    let fixture = ExportFixture::new();
    let fallback_path = fixture.root().join("builtin-fallback.jsonl");
    fs::write(
        &fallback_path,
        format!(
            "{}\n",
            json!({
                "sessionId": SESSION_A,
                "type": "user",
                "uuid": "fallback-turn",
                "timestamp": "2026-05-01T00:00:00Z",
                "message": "builtin fallback must not be emitted",
            })
        ),
    )
    .unwrap();
    let record_path = fixture.root().join("provider-records.jsonl");
    let provider_path = write_cli_session_provider(
        fixture.root(),
        mode,
        &record_path,
        &fixture.root().join("unused-canonical.jsonl"),
        &fixture.root().join("unused-native.jsonl"),
    );
    fixture.write_external_model(EXTERNAL_MODEL, EXTERNAL_PROVIDER, &provider_path);
    fixture.write_provider(
        EXTERNAL_PROVIDER,
        fixtures::initiative_06_export::StorageKind::None,
        true,
        None,
    );
    fixture.set_provider_authority(EXTERNAL_PROVIDER, &provider_path);
    fixture.write_sessions_with_locator_path(EXTERNAL_PROVIDER, &fallback_path);
    fixture.seed_active_chain(
        fixtures::initiative_06_export::CHAIN_A,
        EXTERNAL_PROVIDER,
        SESSION_A,
        EXTERNAL_MODEL,
        "2026-05-01T00:00:00Z",
    );
    ExternalExportFallbackFixture {
        fixture,
        session_id: SESSION_A.to_string(),
        record_path,
    }
}

fn external_canonical_jsonl() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "session_id": SESSION_A,
        "provider_name": EXTERNAL_PROVIDER,
        "turn_id": "external-export-turn-1",
        "role": "user",
        "timestamp": "2026-05-01T00:00:00Z",
        "content": [{"type": "text", "text": "external export user"}],
        "source": {
            "storage_type": "other",
            "jsonl_path": "/provider/external/transcript.jsonl",
            "line": 1,
            "byte_start": 0,
            "byte_end": 10,
            "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
        },
        "unsupported_record": false,
    }))
    .unwrap()
    .into_iter()
    .chain(std::iter::once(b'\n'))
    .collect()
}

fn write_cli_session_provider(
    dir: &Path,
    mode: &str,
    record_path: &Path,
    canonical_path: &Path,
    transcript_path: &Path,
) -> PathBuf {
    fs::write(record_path, "").unwrap();
    let script = dir.join(format!("session-provider-{mode}.py"));
    fs::write(
        &script,
        cli_session_provider_script(mode, record_path, canonical_path, transcript_path),
    )
    .unwrap();
    let mut perms = fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).unwrap();
    script
}

fn cli_session_provider_script(
    mode: &str,
    record_path: &Path,
    canonical_path: &Path,
    transcript_path: &Path,
) -> String {
    format!(
        r#"#!/usr/bin/env python3
import base64
import hashlib
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
        "request_id": request.get("request_id", "request-age244-cli"),
        "ok": True,
        "result": result,
    }}

def error(code):
    return {{
        "contract": request.get("contract", CONTRACT),
        "request_id": request.get("request_id", "request-age244-cli"),
        "ok": False,
        "error": {{"category": "failed", "code": code, "message": code, "retryable": False}},
    }}

def describe():
    if MODE == "describe_error":
        return error("provider_describe_failed")
    return envelope({{
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
    }})

def export_result():
    data = pathlib.Path({canonical_path}).read_bytes()
    return envelope({{
        "canonical_format": "oulipoly.canonical_transcript/v1",
        "data_base64": base64.b64encode(data).decode("ascii"),
        "turn_count": len([line for line in data.splitlines() if line.strip()]),
        "sha256": hashlib.sha256(data).hexdigest(),
    }})

def native_bytes_from_canonical(data):
    lines = []
    for raw_line in data.decode("utf-8").splitlines():
        record = json.loads(raw_line)
        lines.append(json.dumps({{
            "sessionId": record["session_id"],
            "type": record["role"],
            "uuid": record["turn_id"],
            "timestamp": record["timestamp"],
            "message": {{
                "role": record["role"],
                "content": record["content"],
            }},
        }}, separators=(",", ":")))
    return ("\n".join(lines) + "\n").encode("utf-8")

def canonical_bytes_from_native(native_data, path):
    records = []
    offset = 0
    line_no = 1
    for raw_line in native_data.splitlines():
        start = offset
        end = start + len(raw_line)
        value = json.loads(raw_line.decode("utf-8"))
        records.append({{
            "session_id": value["sessionId"],
            "provider_name": "{provider_name}",
            "turn_id": value["uuid"],
            "role": value["type"],
            "timestamp": value["timestamp"],
            "content": value["message"]["content"],
            "source": {{
                "storage_type": "{native_storage_kind}",
                "jsonl_path": path,
                "line": line_no,
                "byte_start": start,
                "byte_end": end,
                "sha256": hashlib.sha256(raw_line).hexdigest(),
            }},
            "unsupported_record": False,
        }})
        offset = end + 1
        line_no += 1
    return ("".join(json.dumps(record, separators=(",", ":")) + "\n" for record in records)).encode("utf-8")

def replace_result():
    params = request.get("params", {{}})
    if MODE == "replace_provider_error":
        return error("provider_replace_failed")
    transcript_param = params.get("canonical_transcript", {{}})
    data = base64.b64decode(transcript_param.get("data_base64") or params.get("data_base64", ""))
    canonical_hash = hashlib.sha256(data).hexdigest()
    operation_id = params.get("operation_id", "{operation_id}")
    recovery_id = "{recovery_id}"
    provider_preimage = "{preimage_sha256}"
    source_id = "{source_id}"
    provider_plan = {{
        "schema_version": 2,
        "operation": "session.replace",
        "replace_protocol": "{replace_protocol}",
        "operation_id": operation_id,
        "recovery_id": recovery_id,
        "session_id": params.get("session_id"),
        "provider_name": params.get("provider_name"),
        "canonical_format": "oulipoly.canonical_transcript/v1",
        "input_sha256": canonical_hash,
        "postimage_sha256": canonical_hash,
        "preimage_sha256_observed": provider_preimage,
        "turn_count": len([line for line in data.splitlines() if line.strip()]),
        "db_apply": "replace_session_turns_from_canonical_v1",
        "source_id": source_id,
        "last_turn_id": "external-cli-owned-turn-2",
        "last_used_at": "2026-04-17T09:00:01Z",
    }}
    provider_owned_result = {{
        "changed": True,
        "operation_id": operation_id,
        "recovery_id": recovery_id,
        "operation_state": "atomic_committed",
        "preimage_sha256_observed": provider_preimage,
        "postimage_sha256": canonical_hash,
        "canonical_postimage": {{
            "format_id": "oulipoly.canonical_transcript/v1",
            "sha256": canonical_hash,
            "turn_count": len([line for line in data.splitlines() if line.strip()]),
            "source_id": source_id,
            "data_base64": base64.b64encode(data).decode("ascii"),
        }},
        "artifacts": [],
        "host_state_plan": provider_plan,
    }}
    if MODE == "replace_provider_owned_success":
        return envelope(provider_owned_result)
    if MODE == "replace_missing_operation_id":
        broken = dict(provider_owned_result)
        broken.pop("operation_id", None)
        return envelope(broken)
    native_data = native_bytes_from_canonical(data)
    transcript = pathlib.Path({transcript_path})
    transcript.write_bytes(native_data)
    postimage = hashlib.sha256(canonical_bytes_from_native(native_data, str(transcript))).hexdigest()
    records_hash = hashlib.sha256(data).hexdigest()
    artifact = {{"kind": "file", "path": str(transcript), "sha256": postimage}}
    plan = {{
        "schema_version": 1,
        "operation": "session.replace",
        "session_id": params.get("session_id"),
        "provider_name": params.get("provider_name"),
        "canonical_format": "oulipoly.canonical_transcript/v1",
        "turn_count": len([line for line in data.splitlines() if line.strip()]),
        "records_sha256": records_hash,
        "postimage_sha256": postimage,
        "artifacts": [artifact],
    }}
    return envelope({{
        "changed": True,
        "postimage_sha256": postimage,
        "artifacts": [artifact],
        "host_state_plan": plan,
    }})

if subcommand == "describe":
    response = describe()
elif subcommand == "session.export":
    response = export_result()
elif subcommand == "session.replace":
    response = replace_result()
else:
    response = error("unsupported_subcommand")
print(json.dumps(response))
"#,
        mode = serde_json::to_string(mode).unwrap(),
        record_path = serde_json::to_string(&record_path.display().to_string()).unwrap(),
        canonical_path = serde_json::to_string(&canonical_path.display().to_string()).unwrap(),
        transcript_path = serde_json::to_string(&transcript_path.display().to_string()).unwrap(),
        provider_name = EXTERNAL_PROVIDER,
        native_storage_kind = native_storage_kind(),
        replace_protocol = provider_owned_replace_protocol(),
        operation_id = provider_owned_operation_id(),
        recovery_id = provider_owned_recovery_id(),
        preimage_sha256 = provider_owned_preimage_sha256(),
        source_id = provider_owned_source_id(),
    )
}

fn native_line(session_id: &str, turn_id: &str, role: &str, message: &str, offset: i64) -> String {
    json!({
        "sessionId": session_id,
        "type": role,
        "uuid": turn_id,
        "timestamp": format!("2026-04-17T08:00:{offset:02}Z"),
        "message": message,
    })
    .to_string()
}

fn native_storage_kind() -> String {
    ["clau", "de_code"].concat()
}

fn shell_single_quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn provider_records(record_path: &Path) -> Vec<Value> {
    fs::read_to_string(record_path)
        .unwrap()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn assert_subcommand_count(records: &[Value], subcommand: &str, expected: usize) {
    assert_eq!(
        records
            .iter()
            .filter(|record| record["subcommand"] == subcommand)
            .count(),
        expected,
        "{records:?}"
    );
}

fn request_for<'a>(records: &'a [Value], subcommand: &str) -> &'a Value {
    &records
        .iter()
        .find(|record| record["subcommand"] == subcommand)
        .unwrap()["request"]
}

fn assert_provider_owned_request_shape(request: &Value, canonical_bytes: &[u8]) {
    let params = &request["params"];
    assert_eq!(
        params["replace_protocol"],
        provider_owned_replace_protocol()
    );
    assert_eq!(params["operation_id"], provider_owned_operation_id());
    assert!(
        params.get("operation_mode").is_none(),
        "initial provider-owned replace must not be a recovery request: {params}"
    );
    assert!(
        params.get("recovery_action").is_none(),
        "initial provider-owned replace must not carry recovery action: {params}"
    );
    assert_eq!(params["canonical_transcript"]["kind"], "bytes");
    assert_eq!(
        params["canonical_transcript"]["data_base64"],
        base64::engine::general_purpose::STANDARD.encode(canonical_bytes)
    );
    assert_eq!(
        params["canonical_transcript"]["sha256"],
        sha256_hex(canonical_bytes)
    );
    assert_eq!(
        params["preimage_sha256_expected"],
        provider_owned_preimage_sha256()
    );
    assert!(
        params.get("preimage_sha256").is_none(),
        "host-observed preimage must not be sent: {params}"
    );
}

fn assert_provider_request_excludes_host_apply_authority(request: &Value) {
    let text = request.to_string();
    assert!(
        !text.contains("state.db"),
        "request exposes SQLite path: {text}"
    );
    assert!(
        !text.contains("journal") && !text.contains("transaction") && !text.contains("sql"),
        "request exposes host mutation authority: {text}"
    );
}

fn provider_owned_replace_protocol() -> &'static str {
    "oulipoly.provider_owned_replace/v1"
}

fn provider_owned_operation_id() -> String {
    "55555555-5555-4555-8555-555555555555".to_string()
}

fn provider_owned_recovery_id() -> String {
    "66666666-6666-4666-8666-666666666666".to_string()
}

fn provider_owned_preimage_sha256() -> String {
    "1".repeat(64)
}

fn provider_owned_source_id() -> String {
    "provider-a-owned-canonical-source".to_string()
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}
