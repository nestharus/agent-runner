#![allow(dead_code)]

use chrono::{TimeZone, Utc};
use oulipoly_agent_messenger::{
    ReturnName, ReturnRequest, ReturnSource, ReturnedArtifact, ReturnedArtifactRef,
    ReturnedArtifactSource, StoreAddress,
};
use oulipoly_agent_scratchpad::{
    InvocationScope, ReadRequest, Scratchpad, ScratchpadName, WriteRequest,
};
use oulipoly_agent_store::Store;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;
use uuid::Uuid;

pub struct TempDb {
    _dir: TempDir,
    path: PathBuf,
}

impl TempDb {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("agent-messenger.sqlite");
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

pub fn open_scratchpad(db: &TempDb) -> Scratchpad {
    Scratchpad::open(db.path()).expect("scratchpad open")
}

pub fn return_name(value: &str) -> ReturnName {
    ReturnName::new(value).expect("valid return name")
}

pub fn scratchpad_name(value: &str) -> ScratchpadName {
    ScratchpadName::new(value).expect("valid scratchpad name")
}

pub fn scope(invocation_uuid: Uuid) -> InvocationScope {
    InvocationScope { invocation_uuid }
}

pub fn write_scratchpad(
    db: &TempDb,
    invocation_uuid: Uuid,
    name: &str,
    content: Vec<u8>,
    format_hint: Option<&str>,
    verdict_line: Option<&str>,
) {
    open_scratchpad(db)
        .write(WriteRequest {
            scope: scope(invocation_uuid),
            name: scratchpad_name(name),
            content,
            format_hint: format_hint.map(str::to_string),
            verdict_line: verdict_line.map(str::to_string),
            predecessor_version: None,
        })
        .expect("write scratchpad fixture");
}

pub fn read_scratchpad(db: &TempDb, invocation_uuid: Uuid, name: &str) -> Vec<u8> {
    open_scratchpad(db)
        .read(ReadRequest {
            scope: scope(invocation_uuid),
            name: scratchpad_name(name),
            version: None,
        })
        .expect("read scratchpad fixture")
        .content
}

pub fn inline_request(
    db: &TempDb,
    invocation_uuid: Uuid,
    name: &str,
    bytes: Vec<u8>,
) -> ReturnRequest {
    ReturnRequest {
        db_path: db.path().to_path_buf(),
        invocation_uuid,
        name: return_name(name),
        source: ReturnSource::InlineBytes(bytes),
        format_hint: None,
        verdict_line: None,
        return_channel: None,
    }
}

pub fn scratchpad_request(
    db: &TempDb,
    invocation_uuid: Uuid,
    return_name: &str,
    source_name: &str,
) -> ReturnRequest {
    ReturnRequest {
        db_path: db.path().to_path_buf(),
        invocation_uuid,
        name: self::return_name(return_name),
        source: ReturnSource::Scratchpad {
            name: scratchpad_name(source_name),
            version: None,
        },
        format_hint: None,
        verdict_line: None,
        return_channel: None,
    }
}

pub fn text_bytes() -> Vec<u8> {
    b"# Returned\nhello\n".to_vec()
}

pub fn replacement_text_bytes() -> Vec<u8> {
    b"# Returned\nsecond\n".to_vec()
}

pub fn binary_bytes() -> Vec<u8> {
    vec![0, 159, 146, 150, b'\n', 0, 255, b'Z']
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
}

pub fn artifact_ref(invocation_uuid: Uuid, name: &str, version: u64) -> ReturnedArtifactRef {
    ReturnedArtifactRef {
        version_id: format!("store://return/{invocation_uuid}/{name}/{version}"),
        name: name.to_string(),
        store_address: StoreAddress {
            workflow_run_id: format!("return:{invocation_uuid}"),
            artifact_name: name.to_string(),
            version,
        },
        sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
        content_len: 8,
        format_hint: Some("text/markdown".to_string()),
        verdict_line: Some("APPROVED: ready".to_string()),
        source: ReturnedArtifactSource::InlineBytes,
        producer_invocation_uuid: invocation_uuid,
        returned_at: Utc.with_ymd_and_hms(2026, 5, 7, 12, 0, 0).unwrap(),
    }
}

pub fn artifact_receipt(invocation_uuid: Uuid, name: &str, version: u64) -> ReturnedArtifact {
    let reference = artifact_ref(invocation_uuid, name, version);
    ReturnedArtifact {
        schema_version: 1,
        version_id: reference.version_id,
        name: reference.name,
        store_address: reference.store_address,
        sha256: reference.sha256,
        content_len: reference.content_len,
        format_hint: reference.format_hint,
        verdict_line: reference.verdict_line,
        source: reference.source,
        producer_invocation_uuid: reference.producer_invocation_uuid,
        returned_at: reference.returned_at,
    }
}

pub fn assert_no_private_scratchpad_leak(value: &impl serde::Serialize) {
    let rendered = serde_json::to_string(value).expect("serialize receipt");
    assert!(
        !rendered.contains("scratchpad:"),
        "caller-bound receipt leaked private scratchpad address: {rendered}"
    );
}

pub fn parent_invocation_env(invocation_uuid: Uuid) -> String {
    format!(r#"{{"id":"{invocation_uuid}","source":"test"}}"#)
}

pub fn write_file(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).expect("write fixture file");
}

pub fn agent_messenger_cmd() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-messenger"));
    command.env_remove("OULIPOLY_PARENT_INVOCATION");
    command.env_remove("OULIPOLY_RETURN_CHANNEL");
    command
}

pub fn run_agent_messenger(args: &[&str]) -> Output {
    agent_messenger_cmd()
        .args(args)
        .output()
        .expect("run agent-messenger")
}

pub fn run_agent_messenger_with_env(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut command = agent_messenger_cmd();
    command.args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().expect("run agent-messenger")
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
