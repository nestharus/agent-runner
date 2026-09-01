#![cfg(unix)]
//! ## Declared roles
//!
//! `accessor`, `formatter`, `parser`, `mapper`, `validator`, `orchestration`

mod provider_authority_fixture;

use chrono::{TimeZone, Utc};
use oulipoly_agent_messenger::{ReturnedArtifactRef, ReturnedArtifactSource, StoreAddress};
use oulipoly_state::{
    InvocationStart, InvocationStatus, MINIMUM_SUPPORTED_SCHEMA_VERSION, StateDb,
    StateReadConnection,
};
use rusqlite::{Connection, params};
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use uuid::Uuid;

struct Fixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    models_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        fs::create_dir_all(&models_dir).expect("models dir");
        Self {
            dir,
            config_home,
            data_home,
            models_dir,
        }
    }

    fn db_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("state.db")
    }

    fn open_db(&self) -> StateDb {
        StateDb::open(&self.db_path()).expect("open db")
    }

    fn write_script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        fs::write(
            &path,
            format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
        )
        .expect("write script");
        let mut perms = fs::metadata(&path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).expect("chmod script");
        path
    }

    fn write_one_shot_model(&self, script_path: &Path) {
        fs::write(
            self.models_dir.join("fixture.toml"),
            r#"[[providers]]
name = "fixture-provider"
args = []
interactive_args = ["launch"]
"#,
        )
        .expect("write model");
        fs::write(
            self.config_home
                .join("oulipoly-agent-runner")
                .join("providers.toml"),
            provider_authority_fixture::with_explicit_provider_authority(&format!(
                r#"[fixture-provider]
command = "{}"
args = []
interactive_args = ["launch"]
prompt_mode = "arg"

[fixture-provider.resume]
kind = "flag"
flag = "--resume"
"#,
                script_path.display()
            )),
        )
        .expect("write providers");
    }

    fn run_one_shot(&self) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("--models-dir")
            .arg(&self.models_dir)
            .arg("--model")
            .arg("fixture")
            .arg("prompt");
        self.apply_env(&mut cmd);
        cmd.output().expect("run one-shot")
    }

    fn run_headless_resume(&self, session_id: &str) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("-m")
            .arg("fixture")
            .arg("--resume")
            .arg(session_id)
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg("continuation prompt");
        self.apply_env(&mut cmd);
        cmd.current_dir(self.dir.path());
        cmd.output().expect("run resume")
    }

    fn run_repl_with_stale_channel(&self, stale_channel: &Path) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("repl")
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg("fixture");
        self.apply_env(&mut cmd);
        cmd.env("OULIPOLY_RETURN_CHANNEL", stale_channel);
        cmd.output().expect("run repl")
    }

    fn apply_env(&self, cmd: &mut Command) {
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env(
            "OULIPOLY_DATA_DIR",
            self.data_home.join("oulipoly-agent-runner"),
        );
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd.env_remove("OULIPOLY_RETURN_CHANNEL");
    }

    fn seed_session_turn(&self, provider: &str, session_id: &str) {
        let db = self.open_db();
        db.ingest_session_turn(
            provider,
            session_id,
            "turn-1",
            &Utc.with_ymd_and_hms(2026, 4, 17, 8, 0, 0).unwrap(),
            "assistant",
            "fixture.jsonl",
        )
        .expect("seed turn");
    }
}

fn parse_invocation(stderr: &str) -> String {
    let line = stderr
        .lines()
        .find_map(|line| line.strip_prefix("OULIPOLY_INVOCATION="))
        .unwrap_or_else(|| panic!("missing invocation marker in stderr: {stderr}"));
    serde_json::from_str::<Value>(line)
        .expect("invocation JSON")
        .get("id")
        .and_then(Value::as_str)
        .expect("invocation id")
        .to_string()
}

fn assert_resume_success_result(stdout: &[u8]) {
    let stdout = String::from_utf8_lossy(stdout);
    assert!(
        stdout.starts_with("resume stdout\nOULIPOLY_RESULT="),
        "{stdout}"
    );
    let raw = stdout
        .strip_prefix("resume stdout\nOULIPOLY_RESULT=")
        .unwrap()
        .trim();
    let result: serde_json::Value = serde_json::from_str(raw).unwrap();
    let mut keys = result
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    assert_eq!(
        keys,
        [
            "error_category",
            "exit_code",
            "finished_at",
            "id",
            "status",
            "success",
            "terminal_reason"
        ]
    );
    assert_eq!(result["status"], "succeeded");
    assert_eq!(result["success"], true);
    assert_eq!(result["exit_code"], 0);
    assert!(result["error_category"].is_null());
    assert!(result["terminal_reason"].is_null());
}

fn invocation_count(db: &StateDb) -> i64 {
    db.connection()
        .query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
        .unwrap()
}

fn receipt_json(invocation_uuid: Uuid, name: &str, version: u64) -> String {
    let reference = returned_ref(invocation_uuid, name, version);
    serde_json::to_string(&reference).expect("receipt JSON")
}

fn receipt_emit_script(name: &str, version: u64) -> String {
    format!(
        r#"invocation_id=$(printf '%s' "${{OULIPOLY_PARENT_INVOCATION:?missing}}" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
printf '{{"version_id":"store://return/%s/{name}/{version}","name":"{name}","store_address":{{"workflow_run_id":"return:%s","artifact_name":"{name}","version":{version}}},"sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","content_len":5,"format_hint":"text/markdown","verdict_line":"APPROVED: ready","source":{{"kind":"inline_bytes"}},"producer_invocation_uuid":"%s","returned_at":"2026-05-07T12:00:00Z"}}\n' "$invocation_id" "$invocation_id" "$invocation_id" >> "${{OULIPOLY_RETURN_CHANNEL:?missing}}""#
    )
}

fn returned_ref(invocation_uuid: Uuid, name: &str, version: u64) -> ReturnedArtifactRef {
    ReturnedArtifactRef {
        version_id: format!("store://return/{invocation_uuid}/{name}/{version}"),
        name: name.to_string(),
        store_address: StoreAddress {
            workflow_run_id: format!("return:{invocation_uuid}"),
            artifact_name: name.to_string(),
            version,
        },
        sha256: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        content_len: 5,
        format_hint: Some("text/markdown".to_string()),
        verdict_line: Some("APPROVED: ready".to_string()),
        source: ReturnedArtifactSource::InlineBytes,
        producer_invocation_uuid: invocation_uuid,
        returned_at: Utc.with_ymd_and_hms(2026, 5, 7, 12, 0, 0).unwrap(),
    }
}

fn returned_rows(
    conn: StateReadConnection<'_>,
    invocation_row_id: i64,
) -> Vec<(i64, String, String, String)> {
    let mut stmt = conn
        .prepare(
            "SELECT ordinal, version_id, name, sha256
             FROM invocation_returned_artifacts
             WHERE invocation_id = ?1
             ORDER BY ordinal",
        )
        .expect("prepare returned rows");
    stmt.query_map(params![invocation_row_id], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    })
    .expect("query returned rows")
    .collect::<Result<Vec<_>, _>>()
    .expect("parse returned rows")
}

// proposal § Test-Intent Track row: one-shot run_with_balancing records returns
// contract § Top-level orchestration contract one-shot
// named risk: Top-Level Orchestration HIGH - one-shot could finalize success but drop returned artifacts or mutate provider stdout
// selected level: runtime_integration
#[test]
fn one_shot_records_returned_artifacts_before_finalization_and_preserves_stdout() {
    let fixture = Fixture::new();
    let script = fixture.write_script(
        "returning-provider.sh",
        &format!(
            r#"{}
printf 'provider stdout'"#,
            receipt_emit_script("proposal.md", 1)
        ),
    );
    fixture.write_one_shot_model(&script);

    let output = fixture.run_one_shot();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    // run_with_balancing now appends a single-line `OULIPOLY_RESULT={...}` envelope
    // after the provider stdout; the spirit of "preserves stdout" is now "preserves
    // the provider-stdout PREFIX."
    assert!(
        output.stdout.starts_with(b"provider stdout"),
        "{:?}",
        output.stdout
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("OULIPOLY_RESULT="),
        "{:?}",
        output.stdout
    );
    let invocation_id = parse_invocation(&String::from_utf8_lossy(&output.stderr));
    let db = fixture.open_db();
    let row = db.get_invocation_by_uuid(&invocation_id).unwrap().unwrap();
    assert_eq!(row.status, InvocationStatus::Succeeded);
    assert_eq!(row.exit_code, Some(0));
    let rows = returned_rows(db.connection(), row.id);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, 0);
    assert_eq!(rows[0].2, "proposal.md");
}

// proposal § Test-Intent Track row: returned-artifact persistence failure
// contract § Top-level orchestration contract one-shot terminal outcome
// named risk: Top-Level Orchestration HIGH - a run could report success after dropping returned artifact refs
// selected level: runtime_integration
#[test]
fn one_shot_returned_artifact_persist_failure_marks_invocation_failed() {
    let fixture = Fixture::new();
    let receipt = receipt_json(Uuid::new_v4(), "wrong-invocation.md", 1).replace('\'', r#"'\''"#);
    let script = fixture.write_script(
        "bad-returning-provider.sh",
        &format!(
            r#"printf '%s\n' '{receipt}' >> "${{OULIPOLY_RETURN_CHANNEL:?missing}}"
printf 'provider stdout'"#
        ),
    );
    fixture.write_one_shot_model(&script);

    let output = fixture.run_one_shot();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let invocation_id = parse_invocation(&String::from_utf8_lossy(&output.stderr));
    let db = fixture.open_db();
    let row = db.get_invocation_by_uuid(&invocation_id).unwrap().unwrap();
    assert_eq!(row.status, InvocationStatus::Failed);
    assert_eq!(row.success, Some(false));
    assert_eq!(row.exit_code, Some(1));
    assert_eq!(row.error_category.as_deref(), Some("returned_artifacts"));
    assert_eq!(
        row.terminal_reason.as_deref(),
        Some("returned_artifacts_persist_failed")
    );
    assert!(returned_rows(db.connection(), row.id).is_empty());
}

// proposal § Test-Intent Track row: headless run_resume records returns
// contract § Top-level orchestration contract run_resume
// named risk: Top-Level Orchestration HIGH - resume constructor/finalizer could drop returned artifacts while preserving resume status
// selected level: runtime_integration
#[test]
fn headless_resume_records_returned_artifacts_before_finalization() {
    let fixture = Fixture::new();
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    fixture.seed_session_turn("fixture-provider", session_id);
    let script = fixture.write_script(
        "resume-returning-provider.sh",
        &format!(
            r#"{}
printf 'resume stdout'"#,
            receipt_emit_script("resume.md", 1)
        ),
    );
    fixture.write_one_shot_model(&script);

    let output = fixture.run_headless_resume(session_id);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_resume_success_result(&output.stdout);
    let invocation_id = parse_invocation(&String::from_utf8_lossy(&output.stderr));
    let db = fixture.open_db();
    let row = db.get_invocation_by_uuid(&invocation_id).unwrap().unwrap();
    assert_eq!(row.session_capture_method.as_deref(), Some("resumed"));
    assert_eq!(returned_rows(db.connection(), row.id).len(), 1);
}

// proposal § Test-Intent Track row: REPL does not bind returns
// contract § Top-level orchestration contract run_interactive
// named risk: Top-Level Orchestration HIGH - REPL could inherit stale return channels or persist unsupported v1 returns
// selected level: runtime_integration
#[test]
fn repl_does_not_record_returns_and_scrubs_stale_channel() {
    let fixture = Fixture::new();
    let observed = fixture.dir.path().join("observed-return-channel.txt");
    let stale = fixture.dir.path().join("stale.jsonl");
    let script = fixture.write_script(
        "repl-provider.sh",
        &format!(
            r#"printf '%s' "${{OULIPOLY_RETURN_CHANNEL-}}" > "{}"
printf 'interactive stdout'"#,
            observed.display()
        ),
    );
    fixture.write_one_shot_model(&script);

    let output = fixture.run_repl_with_stale_channel(&stale);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(fs::read_to_string(observed).expect("observed"), "");
    let invocation_id = parse_invocation(&String::from_utf8_lossy(&output.stderr));
    let db = fixture.open_db();
    let row = db.get_invocation_by_uuid(&invocation_id).unwrap().unwrap();
    assert!(returned_rows(db.connection(), row.id).is_empty());
}

// proposal § Test-Intent Track row: failed child after successful return
// contract § Expected observable signals row failed child after successful return
// named risk: Top-Level Orchestration HIGH - returned artifacts could be treated as success proof or lost on failure
// selected level: runtime_integration
#[test]
fn failed_child_after_successful_return_persists_returns_and_reports_failure() {
    let fixture = Fixture::new();
    let script = fixture.write_script(
        "return-then-fail.sh",
        &format!(
            r#"{}
exit 7"#,
            receipt_emit_script("diagnostic.md", 1)
        ),
    );
    fixture.write_one_shot_model(&script);

    let output = fixture.run_one_shot();

    assert_eq!(output.status.code(), Some(7), "{output:?}");
    let invocation_id = parse_invocation(&String::from_utf8_lossy(&output.stderr));
    let db = fixture.open_db();
    let row = db.get_invocation_by_uuid(&invocation_id).unwrap().unwrap();
    assert_eq!(row.status, InvocationStatus::Failed);
    assert_eq!(row.success, Some(false));
    assert_eq!(row.exit_code, Some(7));
    assert_eq!(returned_rows(db.connection(), row.id).len(), 1);
}

// proposal § Test-Intent Track row: state DB fresh and incremental migration
// contract § Expected observable signals row state DB migration
// named risk: State DB HIGH - additive returned-artifact table could be missing on fresh or old schemas
// selected level: runtime_integration
#[test]
fn state_db_fresh_and_incremental_open_create_returned_artifacts_table_without_losing_rows() {
    let fixture = Fixture::new();
    let fresh = fixture.open_db();
    let exists: i64 = fresh
        .connection()
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'invocation_returned_artifacts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(exists, 1);

    let legacy_path = fixture.dir.path().join("legacy-state.db");
    let conn = Connection::open(&legacy_path).unwrap();
    conn.execute_batch(
        &format!(
            "PRAGMA user_version = {MINIMUM_SUPPORTED_SCHEMA_VERSION};
        CREATE TABLE invocations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            invocation_uuid TEXT NOT NULL UNIQUE,
            model_name TEXT NOT NULL,
            provider_name TEXT,
            provider_index INTEGER NOT NULL,
            parent_invocation_id INTEGER REFERENCES invocations(id),
            status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'legacy')),
            success INTEGER,
            exit_code INTEGER,
            error_category TEXT,
            terminal_reason TEXT,
            session_id TEXT,
            session_capture_method TEXT,
            resume_acceptance_status TEXT,
            resume_acceptance_evidence TEXT,
            created_at TEXT NOT NULL,
            finished_at TEXT
        );
        INSERT INTO invocations (
            invocation_uuid, model_name, provider_name, provider_index, status, created_at
        ) VALUES (
            '11111111-1111-1111-1111-111111111111', 'fixture', 'fixture-provider', 0, 'running', '2026-05-07T12:00:00Z'
        );"
        ),
    )
    .unwrap();
    let legacy_read_only = StateDb::open_read_only(&legacy_path).unwrap();
    assert_eq!(
        legacy_read_only.list_returned_artifacts(1).unwrap(),
        Vec::new()
    );
    drop(legacy_read_only);
    drop(conn);

    let corrupt_path = fixture.dir.path().join("corrupt-returned-artifacts.db");
    let corrupt_conn = Connection::open(&corrupt_path).unwrap();
    corrupt_conn
        .execute_batch(
            "CREATE TABLE invocations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                invocation_uuid TEXT NOT NULL UNIQUE
            );
            INSERT INTO invocations (invocation_uuid)
            VALUES ('33333333-3333-3333-3333-333333333333');
            CREATE VIEW invocation_returned_artifacts AS SELECT 1 AS id;",
        )
        .unwrap();
    drop(corrupt_conn);
    let corrupt_read_only = StateDb::open_read_only(&corrupt_path).unwrap();
    let err = corrupt_read_only
        .list_returned_artifacts(1)
        .expect_err("non-table returned-artifacts object fails");
    assert!(
        err.contains("object type=view"),
        "unexpected corrupt schema error: {err}"
    );

    let migrated = StateDb::open(&legacy_path).unwrap();
    let row = migrated
        .get_invocation_by_uuid("11111111-1111-1111-1111-111111111111")
        .unwrap()
        .unwrap();
    assert_eq!(
        migrated.list_returned_artifacts(row.id).unwrap(),
        Vec::new()
    );
}

// proposal § Test-Intent Track row: state DB records multiple returns with ordinals
// contract § State DB binding contract
// named risk: State DB HIGH - returned-artifact rows could reorder, duplicate, or mutate invocation final status
// selected level: runtime_integration
#[test]
fn state_db_records_multiple_returns_with_ordinals_without_changing_final_status() {
    let fixture = Fixture::new();
    let db = fixture.open_db();
    let before_invocation_count = invocation_count(&db);
    let invocation_uuid = Uuid::new_v4();
    let row_id = db
        .start_invocation(&InvocationStart {
            invocation_uuid: invocation_uuid.to_string(),
            model_name: "fixture".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    db.finalize_invocation(row_id, false, 7, Some("fixture"), Some("exit_nonzero"))
        .unwrap();
    let producer = invocation_uuid;

    db.record_returned_artifacts(
        row_id,
        &[
            returned_ref(producer, "first.md", 1),
            returned_ref(producer, "second.md", 1),
        ],
    )
    .expect("record returns");
    db.record_returned_artifacts(
        row_id,
        &[
            returned_ref(producer, "first.md", 1),
            returned_ref(producer, "second.md", 1),
        ],
    )
    .expect("record returns retry");

    let rows = db.list_returned_artifacts(row_id).expect("list returns");
    assert_eq!(
        rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
        vec!["first.md", "second.md"]
    );
    let mut mismatched = returned_ref(Uuid::new_v4(), "mismatch.md", 1);
    mismatched.producer_invocation_uuid = producer;
    let err = db
        .record_returned_artifacts(row_id, &[mismatched])
        .expect_err("mismatched producer id fails");
    assert!(
        err.contains("producer UUID mismatch"),
        "unexpected error: {err}"
    );
    let other_invocation_uuid = Uuid::new_v4();
    let other_row_id = db
        .start_invocation(&InvocationStart {
            invocation_uuid: other_invocation_uuid.to_string(),
            model_name: "fixture".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    let err = db
        .record_returned_artifacts(other_row_id, &[returned_ref(producer, "wrong-row.md", 1)])
        .expect_err("wrong invocation row fails");
    assert!(
        err.contains("belongs to"),
        "unexpected wrong-row error: {err}"
    );
    let mut wrong_version_id = returned_ref(producer, "wrong-version.md", 1);
    wrong_version_id.version_id = format!("store://return/{producer}/other-name.md/1");
    let err = db
        .record_returned_artifacts(row_id, &[wrong_version_id])
        .expect_err("wrong version_id fails");
    assert!(
        err.contains("version_id mismatch"),
        "unexpected version_id error: {err}"
    );
    let err = db
        .record_returned_artifacts(
            row_id,
            &[returned_ref(producer, "huge-version.md", u64::MAX)],
        )
        .expect_err("oversized version fails");
    assert!(
        err.contains("version exceeds SQLite INTEGER range"),
        "unexpected oversized-version error: {err}"
    );
    let mut huge_content_len = returned_ref(producer, "huge-content.md", 1);
    huge_content_len.content_len = u64::MAX;
    let err = db
        .record_returned_artifacts(row_id, &[huge_content_len])
        .expect_err("oversized content_len fails");
    assert!(
        err.contains("content_len exceeds SQLite INTEGER range"),
        "unexpected oversized-content error: {err}"
    );
    let rows_after_error = db
        .list_returned_artifacts(row_id)
        .expect("list after error");
    assert_eq!(
        rows_after_error
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        vec!["first.md", "second.md"]
    );
    let invocation = db
        .get_invocation_by_uuid(
            &db.connection()
                .query_row(
                    "SELECT invocation_uuid FROM invocations WHERE id = ?1",
                    params![row_id],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(invocation.status, InvocationStatus::Failed);
    assert_eq!(invocation.exit_code, Some(7));
    assert_eq!(invocation_count(&db), before_invocation_count + 2);
}

// proposal § Test-Intent Track row: trace JSON returned_artifacts projection
// contract § Trace projection contract
// named risk: Trace JSON HIGH - machine trace consumers could miss returned_artifact refs or fail on legacy empty rows
// selected level: runtime_integration
#[test]
fn trace_json_includes_returned_artifacts_and_legacy_missing_defaults_to_empty() {
    let fixture = Fixture::new();
    let db = fixture.open_db();
    let before_invocation_count = invocation_count(&db);
    let invocation_uuid = "22222222-2222-2222-2222-222222222222";
    let row_id = db
        .start_invocation(&InvocationStart {
            invocation_uuid: invocation_uuid.to_string(),
            model_name: "fixture".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    db.finalize_invocation(row_id, true, 0, None, None).unwrap();
    db.record_returned_artifacts(
        row_id,
        &[returned_ref(
            Uuid::parse_str(invocation_uuid).unwrap(),
            "proposal.md",
            1,
        )],
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
    cmd.arg("trace").arg(invocation_uuid).arg("--json");
    fixture.apply_env(&mut cmd);
    let output = cmd.output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    let returned = json["root"]["invocation"]["returned_artifacts"]
        .as_array()
        .expect("returned_artifacts array");
    assert_eq!(returned.len(), 1);
    assert_eq!(returned[0]["name"], "proposal.md");
    let after = fixture.open_db();
    assert_eq!(invocation_count(&after), before_invocation_count + 1);
    assert_eq!(returned[0]["verdict_line"], "APPROVED: ready");

    #[derive(serde::Deserialize)]
    struct LegacyInvocation {
        #[serde(default)]
        returned_artifacts: Vec<ReturnedArtifactRef>,
    }
    let legacy: LegacyInvocation = serde_json::from_value(serde_json::json!({})).unwrap();
    assert!(legacy.returned_artifacts.is_empty());
}

// proposal § Test-Intent Track row: Tauri IPC unchanged
// contract § Tauri IPC contract
// named risk: Tauri IPC HIGH - TestModelResult could accidentally expose returned_artifacts into frontend IPC
// selected level: runtime_integration
#[test]
fn test_model_ipc_result_source_shape_has_no_returned_artifacts_field() {
    let source_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/test_model/mapper.rs");
    let source = fs::read_to_string(&source_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", source_path.display()));
    let struct_start = source
        .find("struct TestModelResult")
        .expect("TestModelResult");
    let brace_start = source[struct_start..]
        .find('{')
        .map(|offset| struct_start + offset)
        .expect("TestModelResult opening brace");
    let mut depth = 0;
    let mut brace_end = None;
    for (offset, ch) in source[brace_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    brace_end = Some(brace_start + offset);
                    break;
                }
            }
            _ => {}
        }
    }
    let struct_body = &source[struct_start..=brace_end.expect("TestModelResult closing brace")];

    for field in [
        "success:",
        "exit_code:",
        "stdout_preview:",
        "stdout_preview_truncated:",
        "stdout_bytes:",
        "stdout_sha256:",
        "stdout_content_type:",
        "stderr_preview:",
        "stderr_preview_truncated:",
        "stderr_bytes:",
        "stderr_sha256:",
        "stderr_content_type:",
        "output_artifact_token:",
    ] {
        assert!(
            struct_body.contains(field),
            "missing existing field {field}"
        );
    }
    assert!(
        !struct_body.contains("returned_artifacts"),
        "TestModelResult must not add returned_artifacts"
    );
}
