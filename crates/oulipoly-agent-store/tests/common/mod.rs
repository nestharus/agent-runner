#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use oulipoly_agent_store::{ArtifactKey, PutRequest, Store};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

pub struct TempDb {
    _dir: TempDir,
    path: PathBuf,
}

impl TempDb {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("agent-store.sqlite");
        Self { _dir: dir, path }
    }

    pub fn path(&self) -> &Path {
        &self.path
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

pub fn key(run: &str, artifact: &str) -> ArtifactKey {
    ArtifactKey {
        workflow_run_id: run.to_string(),
        artifact_name: artifact.to_string(),
    }
}

pub fn put_request(key: ArtifactKey, content: impl Into<Vec<u8>>) -> PutRequest {
    PutRequest {
        key,
        producer_invocation_uuid: None,
        format_hint: None,
        verdict_line: None,
        predecessor_version: None,
        content: content.into(),
    }
}

pub fn list_filter(
    workflow_run_id: Option<&str>,
    artifact_name: Option<&str>,
    include_tombstoned: bool,
) -> oulipoly_agent_store::ListFilter {
    oulipoly_agent_store::ListFilter {
        workflow_run_id: workflow_run_id.map(str::to_string),
        artifact_name: artifact_name.map(str::to_string),
        include_tombstoned,
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
}

pub fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

pub fn fixture_bytes(name: &str) -> Vec<u8> {
    fs::read(fixture_path(name)).expect("fixture bytes")
}

pub fn agent_store_cmd() -> Command {
    Command::new(env!("CARGO_BIN_EXE_agent-store"))
}

pub fn run_agent_store(args: &[&str]) -> Output {
    agent_store_cmd()
        .args(args)
        .output()
        .expect("run agent-store")
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

pub fn expect_meta_json_shape(value: &Value) {
    assert!(value.get("workflow_run_id").is_some());
    assert!(value.get("artifact_name").is_some());
    assert!(value.get("version").and_then(Value::as_u64).is_some());
    assert!(
        value
            .get("sha256")
            .and_then(Value::as_str)
            .is_some_and(is_lower_hex_sha256)
    );
    assert!(value.get("content_len").and_then(Value::as_u64).is_some());
    assert!(value.get("producer_invocation_uuid").is_some());
    assert!(value.get("format_hint").is_some());
    assert!(value.get("verdict_line").is_some());
    assert!(value.get("predecessor_version").is_some());
    assert!(
        value
            .get("created_at")
            .and_then(Value::as_str)
            .is_some_and(is_rfc3339)
    );
    assert!(value.get("tombstone").is_some());
    assert!(value.get("content").is_none());
}

pub fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

pub fn is_rfc3339(value: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(value).is_ok()
}
