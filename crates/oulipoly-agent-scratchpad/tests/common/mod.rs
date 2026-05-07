#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use chrono::{DateTime, Utc};
use oulipoly_agent_scratchpad::{
    CanonicalAddress, DeleteRequest, DeleteSelector, GcRequest, GcSelector, InvocationScope,
    ListRequest, PublishRequest, ReadRequest, Scratchpad, ScratchpadAddress, ScratchpadName,
    WriteRequest,
};
use oulipoly_agent_store::{ArtifactKey, ListFilter, PutReceipt, PutRequest, Store};
use rusqlite::params;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use uuid::Uuid;

pub struct TempDb {
    _dir: TempDir,
    path: PathBuf,
}

impl TempDb {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("agent-scratchpad.sqlite");
        Self { _dir: dir, path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn path_arg(&self) -> String {
        self.path.to_str().expect("utf8 db path").to_string()
    }

    pub fn output_path(&self, name: &str) -> PathBuf {
        self.path.parent().expect("temp db has parent").join(name)
    }
}

pub fn init_temp_store() -> (TempDb, Store) {
    let db = TempDb::new();
    let store = Store::init(db.path()).expect("store init");
    (db, store)
}

pub fn open_store(db: &TempDb) -> Store {
    Store::open(db.path()).expect("store open")
}

pub fn open_scratchpad(db: &TempDb) -> Scratchpad {
    Scratchpad::open(db.path()).expect("scratchpad open")
}

pub fn scope(invocation_uuid: Uuid) -> InvocationScope {
    InvocationScope { invocation_uuid }
}

pub fn name(value: &str) -> ScratchpadName {
    ScratchpadName::new(value).expect("valid scratchpad name")
}

pub fn name_result(
    value: &str,
) -> Result<ScratchpadName, oulipoly_agent_scratchpad::ScratchpadError> {
    ScratchpadName::new(value)
}

pub fn address(invocation_uuid: Uuid, artifact_name: &str) -> ScratchpadAddress {
    ScratchpadAddress {
        invocation_uuid,
        name: name(artifact_name),
    }
}

pub fn canonical(workflow_run_id: &str, artifact_name: &str) -> CanonicalAddress {
    CanonicalAddress {
        workflow_run_id: workflow_run_id.to_string(),
        artifact_name: artifact_name.to_string(),
    }
}

pub fn write_request(invocation_uuid: Uuid, artifact_name: &str, content: Vec<u8>) -> WriteRequest {
    WriteRequest {
        scope: scope(invocation_uuid),
        name: name(artifact_name),
        content,
        format_hint: None,
        verdict_line: None,
        predecessor_version: None,
    }
}

pub fn read_request(
    invocation_uuid: Uuid,
    artifact_name: &str,
    version: Option<u64>,
) -> ReadRequest {
    ReadRequest {
        scope: scope(invocation_uuid),
        name: name(artifact_name),
        version,
    }
}

pub fn list_request(
    invocation_uuid: Uuid,
    artifact_name: Option<&str>,
    include_tombstoned: bool,
) -> ListRequest {
    ListRequest {
        scope: scope(invocation_uuid),
        name: artifact_name.map(name),
        include_tombstoned,
    }
}

pub fn delete_request(
    invocation_uuid: Uuid,
    artifact_name: &str,
    selector: DeleteSelector,
) -> DeleteRequest {
    DeleteRequest {
        scope: scope(invocation_uuid),
        name: name(artifact_name),
        selector,
        actor: None,
        reason: None,
    }
}

pub fn publish_request(
    invocation_uuid: Uuid,
    artifact_name: &str,
    destination_run: &str,
    destination_name: &str,
) -> PublishRequest {
    PublishRequest {
        source: address(invocation_uuid, artifact_name),
        source_version: None,
        destination: canonical(destination_run, destination_name),
        format_hint: None,
        verdict_line: None,
        predecessor_version: None,
    }
}

pub fn gc_invocation_request(invocation_uuid: Uuid, dry_run: bool) -> GcRequest {
    GcRequest {
        selector: GcSelector::Invocation(invocation_uuid),
        dry_run,
        actor: None,
        reason: None,
    }
}

pub fn gc_expired_request(cutoff: DateTime<Utc>, dry_run: bool) -> GcRequest {
    GcRequest {
        selector: GcSelector::ExpiredBefore(cutoff),
        dry_run,
        actor: None,
        reason: None,
    }
}

pub fn scratchpad_workflow(invocation_uuid: Uuid) -> String {
    format!("scratchpad:{invocation_uuid}")
}

pub fn store_key(workflow_run_id: &str, artifact_name: &str) -> ArtifactKey {
    ArtifactKey {
        workflow_run_id: workflow_run_id.to_string(),
        artifact_name: artifact_name.to_string(),
    }
}

pub fn put_scratchpad_row(
    store: &Store,
    invocation_uuid: Uuid,
    artifact_name: &str,
    content: Vec<u8>,
) -> PutReceipt {
    store
        .put(PutRequest {
            key: store_key(&scratchpad_workflow(invocation_uuid), artifact_name),
            producer_invocation_uuid: Some(invocation_uuid),
            format_hint: None,
            verdict_line: None,
            predecessor_version: None,
            content,
        })
        .expect("put scratchpad row")
}

pub fn put_scratchpad_row_with_created_at(
    db: &TempDb,
    store: &Store,
    invocation_uuid: Uuid,
    artifact_name: &str,
    content: Vec<u8>,
    created_at: DateTime<Utc>,
) -> PutReceipt {
    let receipt = put_scratchpad_row(store, invocation_uuid, artifact_name, content);
    let connection = rusqlite::Connection::open(db.path()).expect("open temp db for timestamp");
    connection
        .execute(
            "UPDATE artifacts SET created_at = ?1 \
             WHERE workflow_run_id = ?2 AND artifact_name = ?3 AND version = ?4",
            params![
                created_at.to_rfc3339(),
                receipt.key.workflow_run_id,
                receipt.key.artifact_name,
                receipt.version as i64
            ],
        )
        .expect("update controlled created_at");
    receipt
}

pub fn put_canonical_row(store: &Store, workflow_run_id: &str, artifact_name: &str) -> PutReceipt {
    store
        .put(PutRequest {
            key: store_key(workflow_run_id, artifact_name),
            producer_invocation_uuid: None,
            format_hint: Some("text/plain".to_string()),
            verdict_line: Some("canonical remains".to_string()),
            predecessor_version: None,
            content: b"canonical bytes".to_vec(),
        })
        .expect("put canonical row")
}

pub fn assert_no_canonical_rows_tombstoned(store: &Store) {
    let rows = store
        .list(ListFilter {
            workflow_run_id: None,
            artifact_name: None,
            include_tombstoned: true,
        })
        .expect("list all rows");
    for row in rows
        .iter()
        .filter(|row| !row.key.workflow_run_id.starts_with("scratchpad:"))
    {
        assert!(
            row.tombstone.is_none(),
            "canonical row was tombstoned: {:?}",
            row.key
        );
    }
}

pub fn text_bytes() -> Vec<u8> {
    b"# Scratchpad\nprivate notes\n".to_vec()
}

pub fn replacement_text_bytes() -> Vec<u8> {
    b"# Scratchpad\nupdated notes\n".to_vec()
}

pub fn binary_bytes() -> Vec<u8> {
    vec![0, 159, 146, 150, b'\n', 0, 255, b'Z']
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
}

pub fn agent_scratchpad_cmd() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-scratchpad"));
    command.env_remove("OULIPOLY_PARENT_INVOCATION");
    command.env_remove("OULIPOLY_INVOCATION");
    command
}

pub fn run_agent_scratchpad(args: &[&str]) -> Output {
    agent_scratchpad_cmd()
        .args(args)
        .output()
        .expect("run agent-scratchpad")
}

pub fn run_agent_scratchpad_with_env(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut command = agent_scratchpad_cmd();
    command.args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().expect("run agent-scratchpad")
}

pub fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "expected success, got status {:?}\nstderr:\n{}\nstdout:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
}

pub fn assert_exit_code(output: &Output, expected: i32) {
    assert_eq!(
        output.status.code(),
        Some(expected),
        "stderr:\n{}\nstdout:\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
}

pub fn stdout_json(output: &Output) -> Value {
    assert_success(output);
    serde_json::from_slice(&output.stdout).expect("stdout is JSON")
}

pub fn stderr_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

pub fn write_file(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).expect("write fixture file");
}

pub fn parent_invocation_env(invocation_uuid: Uuid) -> String {
    format!(r#"{{"id":"{invocation_uuid}","source":"test"}}"#)
}

pub fn expect_json_string(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("missing string field {field}: {value}"))
        .to_string()
}
