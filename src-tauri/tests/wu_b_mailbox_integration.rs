#![cfg(unix)]

use chrono::{DateTime, Utc};
use oulipoly_state::mailbox::{AgentBashCompleteEnqueue, EnqueueResult, MailboxDb, MailboxRow};
use oulipoly_state::pid_identity::{PidIdentityDb, PidIdentityRecord, ProcessIdentity};
use oulipoly_state::{
    CompositeInvocationId, InvocationStart, ProviderSessionBinding, SessionTurnIngest, StateDb,
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const INVOCATION_A: &str = "11111111-1111-4111-8111-111111111111";
const INVOCATION_B: &str = "22222222-2222-4222-8222-222222222222";
const SESSION_A: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const SESSION_B: &str = "6169694d-de0f-40d1-890c-6e28e55bab28";
const SESSION_OTHER: &str = "7169694d-de0f-40d1-890c-6e28e55bab29";
const CHAIN_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

struct Fixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    home_dir: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
}

struct NotifyArtifacts {
    state_dir: PathBuf,
    meta: PathBuf,
    log: PathBuf,
    rc: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("xdg-config");
        let data_home = dir.path().join("xdg-data");
        let home_dir = dir.path().join("home");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        fs::create_dir_all(&home_dir).unwrap();
        Self {
            dir,
            config_home,
            data_home,
            home_dir,
            app_config_dir,
            models_dir,
        }
    }

    fn state_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("state.db")
    }

    fn sidecar_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("pid-identity.db")
    }

    fn open_state(&self) -> StateDb {
        StateDb::open(&self.state_path()).unwrap()
    }

    fn conn(&self) -> Connection {
        let _ = self.open_state();
        Connection::open(self.state_path()).unwrap()
    }

    fn run(&self, mut cmd: Command) -> Output {
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env("HOME", &self.home_dir);
        cmd.env_remove("OULIPOLY_DATA_DIR");
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd.env_remove("OULIPOLY_AUTO_WAKE");
        cmd.env_remove("OULIPOLY_AUTO_WAKE_SESSION_ID");
        cmd.env_remove("OULIPOLY_AUTO_WAKE_TOKEN");
        cmd.env_remove("OULIPOLY_AUTO_WAKE_COUNT");
        cmd.env_remove("OULIPOLY_AUTO_WAKE_MAX");
        cmd.env_remove("OULIPOLY_AUTO_WAKE_RETRY_BASE_MS");
        cmd.output().unwrap()
    }

    fn run_notify(&self, handle: &str, metadata: Value) -> Output {
        let artifacts = self.write_notify_artifacts(handle, metadata, 0);
        self.run_notify_artifacts(handle, &artifacts)
    }

    fn run_notify_artifacts(&self, handle: &str, artifacts: &NotifyArtifacts) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("notify")
            .arg("agent-bash-complete")
            .arg("--caller-ppid")
            .arg(std::process::id().to_string())
            .arg("--handle")
            .arg(handle)
            .arg("--state-dir")
            .arg(&artifacts.state_dir)
            .arg("--meta")
            .arg(&artifacts.meta)
            .arg("--log")
            .arg(&artifacts.log)
            .arg("--rc")
            .arg(&artifacts.rc)
            .arg("--json");
        self.run(cmd)
    }

    fn run_mailbox_list(&self, session_id: &str, all: bool) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("mailbox")
            .arg("list")
            .arg("--session-id")
            .arg(session_id)
            .arg("--json");
        if all {
            cmd.arg("--all");
        }
        self.run(cmd)
    }

    fn base_resume_command(&self, model_name: &str, session_id: &str) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("resume")
            .arg("-m")
            .arg(model_name)
            .arg("--session-id")
            .arg(session_id)
            .arg("--models-dir")
            .arg(&self.models_dir);
        cmd.current_dir(self.dir.path());
        cmd
    }

    fn base_chain_resume_command(&self, model_name: &str, chain_id: &str) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("resume")
            .arg("-m")
            .arg(model_name)
            .arg(chain_id)
            .arg("--models-dir")
            .arg(&self.models_dir);
        cmd.current_dir(self.dir.path());
        cmd
    }

    fn write_notify_artifacts(&self, handle: &str, metadata: Value, rc: i32) -> NotifyArtifacts {
        let artifacts = notify_artifact_paths(self.dir.path(), handle);
        fs::create_dir_all(&artifacts.state_dir).unwrap();
        fs::write(&artifacts.meta, notify_metadata_content(&metadata)).unwrap();
        fs::write(&artifacts.log, notify_log_content(handle)).unwrap();
        fs::write(&artifacts.rc, notify_rc_content(rc)).unwrap();
        artifacts
    }

    fn record_identity(
        &self,
        identity: &ProcessIdentity,
        invocation_uuid: &str,
        session_id: Option<&str>,
    ) {
        let sidecar = PidIdentityDb::open(&self.sidecar_path()).unwrap();
        sidecar
            .record_identity(PidIdentityRecord {
                identity,
                os_pgid: None,
                invocation_uuid,
                session_id,
                provider_name: Some("fixture-provider"),
                model_name: Some("fixture-model"),
                recorded_at: "2026-06-04T12:00:00Z",
            })
            .unwrap();
    }

    fn seed_state_invocation_with_provider_session(
        &self,
        invocation_uuid: &str,
        provider_session_id: &str,
    ) {
        let db = self.open_state();
        let id = db
            .start_invocation(&InvocationStart {
                invocation_uuid: invocation_uuid.to_string(),
                model_name: "fixture-model".to_string(),
                provider_name: "fixture-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        db.bind_invocation_provider_session_start(
            id,
            &ProviderSessionBinding {
                provider_session_id: provider_session_id.to_string(),
                capture_method: "fixture",
                resume_input_id: None,
                provider_session_resolved_account: None,
            },
        )
        .unwrap();
    }

    fn mailbox_rows(&self, session_id: &str, all: bool) -> Vec<MailboxRow> {
        let db = MailboxDb::open(&self.sidecar_path()).unwrap();
        db.list_mailbox(session_id, all).unwrap()
    }

    fn seed_mailbox(&self, session_id: &str, handle: &str, rc: i32) -> MailboxRow {
        let state_dir = self.dir.path().join(format!("mailbox-{handle}"));
        fs::create_dir_all(&state_dir).unwrap();
        let meta = state_dir.join("meta.json");
        let log = state_dir.join("log");
        let rc_path = state_dir.join("rc");
        fs::write(&meta, r#"{"caller_chain":[]}"#).unwrap();
        fs::write(&log, "retained log\n").unwrap();
        fs::write(&rc_path, format!("{rc}\n")).unwrap();
        let mut db = MailboxDb::open(&self.sidecar_path()).unwrap();
        inserted_row(db.enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
            session_id,
            handle,
            payload_json: r#"{"schema_version":1,"kind":"agent_bash_complete"}"#,
            owner_invocation_uuid: Some(INVOCATION_A),
            matched_os_pid: Some(9000),
            matched_os_boot_id: Some("boot-mailbox"),
            matched_os_pid_starttime_ticks: Some(1),
            matched_chain_index: Some(0),
            state_dir: &path_string(&state_dir),
            meta_path: &path_string(&meta),
            log_path: &path_string(&log),
            rc_path: &path_string(&rc_path),
            rc,
        }))
    }

    fn write_script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        fs::write(
            &path,
            format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
        )
        .unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    fn write_single_provider_model(&self, model_name: &str, provider: &str, script: &Path) {
        fs::create_dir_all(&self.models_dir).unwrap();
        fs::write(
            self.models_dir.join(format!("{model_name}.toml")),
            format!(
                r#"[[providers]]
name = "{provider}"
args = ["one-shot-only"]
"#,
            ),
        )
        .unwrap();
        fs::write(
            self.app_config_dir.join("providers.toml"),
            format!(
                r#"[{provider}]
command = {}
args = []
interactive_args = ["launch"]
prompt_mode = "arg"

[{provider}.resume]
kind = "flag"
flag = "--resume"
"#,
                toml_string(&path_string(script))
            ),
        )
        .unwrap();
    }

    fn write_sessions_config(&self, provider: &str, turn_script: &Path) {
        fs::create_dir_all(&self.app_config_dir).unwrap();
        fs::write(
            self.app_config_dir.join("sessions.toml"),
            format!(
                r#"[{provider}]
turn_script = {}
"#,
                toml_string(&path_string(turn_script))
            ),
        )
        .unwrap();
    }

    fn seed_session_turn(&self, provider: &str, session_id: &str) {
        let db = self.open_state();
        db.ingest_session_turns_batch(
            provider,
            &[SessionTurnIngest {
                session_id: session_id.to_string(),
                turn_id: format!("turn-{session_id}"),
                timestamp: ts("2026-04-17T08:00:00Z"),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: None,
            }],
        )
        .unwrap();
    }

    fn seed_active_chain(&self, chain_id: &str, provider: &str, session_id: &str, model: &str) {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', ?2)",
            params![chain_id, model],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, '2026-04-17T08:00:00Z', 'initial')",
            params![chain_id, provider, session_id],
        )
        .unwrap();
    }

    fn assert_default_user_paths_untouched(&self) {
        assert!(
            !self
                .home_dir
                .join(".local/share/oulipoly-agent-runner")
                .exists(),
            "runner must use isolated XDG_DATA_HOME, not HOME/.local/share"
        );
        assert!(
            !self.home_dir.join(".config/oulipoly-agent-runner").exists(),
            "runner must use isolated XDG_CONFIG_HOME, not HOME/.config"
        );
    }
}

#[test]
fn notify_resolves_nearest_ancestor_sidecar_session() {
    let fixture = Fixture::new();
    let nearest = identity(9001, "boot-a", 11);
    let older = identity(9002, "boot-b", 22);
    fixture.record_identity(&older, INVOCATION_A, Some(SESSION_A));
    fixture.record_identity(&nearest, INVOCATION_B, Some(SESSION_B));

    let output = fixture.run_notify("h-nearest", caller_chain(&[&nearest, &older]));

    assert!(output.status.success(), "{output:?}");
    let json = stdout_json(&output);
    assert_eq!(json["status"], "enqueued");
    assert_eq!(json["owner_session_id"], SESSION_B);
    assert_eq!(json["owner_invocation_uuid"], INVOCATION_B);
    assert_eq!(json["matched_chain_index"], 0);
    assert_eq!(
        fixture.mailbox_rows(SESSION_B, false)[0].handle,
        "h-nearest"
    );
    assert!(fixture.mailbox_rows(SESSION_A, false).is_empty());
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn notify_resolves_from_state_when_sidecar_session_null() {
    let fixture = Fixture::new();
    let caller = identity(9010, "boot-state", 33);
    fixture.seed_state_invocation_with_provider_session(INVOCATION_A, SESSION_A);
    fixture.record_identity(&caller, INVOCATION_A, None);

    let output = fixture.run_notify("h-state", caller_chain(&[&caller]));

    assert!(output.status.success(), "{output:?}");
    let json = stdout_json(&output);
    assert_eq!(json["owner_session_id"], SESSION_A);
    assert_eq!(json["session_source"], "state_db_invocation_join");
    assert_eq!(fixture.mailbox_rows(SESSION_A, false)[0].handle, "h-state");
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn notify_works_after_caller_dead() {
    let fixture = Fixture::new();
    let dead = identity(999_999_001, "dead-boot", 44);
    fixture.record_identity(&dead, INVOCATION_A, Some(SESSION_A));

    let output = fixture.run_notify("h-dead", caller_chain(&[&dead]));

    assert!(output.status.success(), "{output:?}");
    let json = stdout_json(&output);
    assert_eq!(json["matched_pid"], dead.os_pid);
    assert_eq!(json["owner_session_id"], SESSION_A);
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn notify_rejects_reuse_mismatch() {
    let fixture = Fixture::new();
    let recorded = identity(9020, "boot-reuse", 55);
    let reused = identity(9020, "boot-reuse", 56);
    fixture.record_identity(&recorded, INVOCATION_A, Some(SESSION_A));

    let output = fixture.run_notify("h-reuse", caller_chain(&[&reused]));

    assert!(output.status.success(), "{output:?}");
    let json = stdout_json(&output);
    assert_eq!(json["status"], "no_owner");
    assert!(fixture.mailbox_rows(SESSION_A, false).is_empty());
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn notify_no_owner_valid_chain_returns_zero() {
    let fixture = Fixture::new();
    let caller = identity(9030, "boot-none", 66);

    let output = fixture.run_notify("h-none", caller_chain(&[&caller]));

    assert!(output.status.success(), "{output:?}");
    let json = stdout_json(&output);
    assert_eq!(json["status"], "no_owner");
    assert_eq!(json["enqueued"], false);
    assert!(fixture.mailbox_rows(SESSION_A, false).is_empty());
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn notify_missing_chain_is_usage_error() {
    let fixture = Fixture::new();
    for (handle, metadata) in [
        ("h-missing", json!({"extra": true})),
        ("h-empty", json!({"caller_chain": []})),
        ("h-malformed", json!({"caller_chain": [{"pid": "not-int"}]})),
    ] {
        let output = fixture.run_notify(handle, metadata);
        assert_eq!(output.status.code(), Some(64), "{output:?}");
        let json = stdout_json(&output);
        assert_eq!(json["status"], "malformed_metadata");
    }
    assert!(fixture.mailbox_rows(SESSION_A, false).is_empty());
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn notify_idempotent_retried_handle() {
    let fixture = Fixture::new();
    let caller = identity(9040, "boot-idem", 77);
    fixture.record_identity(&caller, INVOCATION_A, Some(SESSION_A));
    let artifacts = fixture.write_notify_artifacts("h-idem", caller_chain(&[&caller]), 0);

    let first = fixture.run_notify_artifacts("h-idem", &artifacts);
    let second = fixture.run_notify_artifacts("h-idem", &artifacts);

    assert!(first.status.success(), "{first:?}");
    assert!(second.status.success(), "{second:?}");
    let first_json = stdout_json(&first);
    let second_json = stdout_json(&second);
    assert_eq!(second_json["status"], "already_enqueued");
    assert_eq!(second_json["seq"], first_json["seq"]);
    let rows = fixture.mailbox_rows(SESSION_A, true);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].seq, first_json["seq"].as_i64().unwrap());
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn notify_handle_conflict_different_session() {
    let fixture = Fixture::new();
    let caller = identity(9050, "boot-conflict", 88);
    fixture.seed_mailbox(SESSION_OTHER, "h-conflict", 0);
    fixture.record_identity(&caller, INVOCATION_A, Some(SESSION_A));

    let output = fixture.run_notify("h-conflict", caller_chain(&[&caller]));

    assert_eq!(output.status.code(), Some(73), "{output:?}");
    let json = stdout_json(&output);
    assert_eq!(json["status"], "idempotency_conflict");
    assert_eq!(fixture.mailbox_rows(SESSION_OTHER, true).len(), 1);
    assert!(fixture.mailbox_rows(SESSION_A, true).is_empty());
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn notify_ordering_by_seq() {
    let fixture = Fixture::new();
    let caller = identity(9060, "boot-order", 99);
    fixture.record_identity(&caller, INVOCATION_A, Some(SESSION_A));
    for handle in ["h-a", "h-b", "h-c"] {
        let output = fixture.run_notify(handle, caller_chain(&[&caller]));
        assert!(output.status.success(), "{output:?}");
    }

    let output = fixture.run_mailbox_list(SESSION_A, false);

    assert!(output.status.success(), "{output:?}");
    let json = stdout_json(&output);
    let handles = row_handles(&json);
    assert_eq!(handles, vec!["h-a", "h-b", "h-c"]);
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn mailbox_isolation() {
    let fixture = Fixture::new();
    let caller_a = identity(9070, "boot-iso-a", 100);
    let caller_b = identity(9071, "boot-iso-b", 101);
    fixture.record_identity(&caller_a, INVOCATION_A, Some(SESSION_A));
    fixture.record_identity(&caller_b, INVOCATION_B, Some(SESSION_B));
    assert!(
        fixture
            .run_notify("h-a", caller_chain(&[&caller_a]))
            .status
            .success()
    );
    assert!(
        fixture
            .run_notify("h-b", caller_chain(&[&caller_b]))
            .status
            .success()
    );

    let output = fixture.run_mailbox_list(SESSION_A, false);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(row_handles(&stdout_json(&output)), vec!["h-a"]);
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn resume_with_pending_mailbox_prepends_notifications() {
    let fixture = Fixture::new();
    let prompt_dump = fixture.dir.path().join("prompt.txt");
    let script = fixture.write_script(
        "resume-prepend.sh",
        &dump_last_arg_script(&prompt_dump, None, 0),
    );
    fixture.write_single_provider_model("fixture-model", "fixture-provider", &script);
    fixture.seed_session_turn("fixture-provider", SESSION_A);
    fixture.seed_mailbox(SESSION_A, "h-1", 0);
    fixture.seed_mailbox(SESSION_A, "h-2", 7);

    let mut cmd = fixture.base_resume_command("fixture-model", SESSION_A);
    cmd.arg("--prompt").arg("continue");
    let output = fixture.run(cmd);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(prompt_dump.exists(), "{output:?}");
    let prompt = fs::read_to_string(&prompt_dump).unwrap();
    assert!(prompt.starts_with("[OULIPOLY NOTIFICATIONS]"), "{prompt}");
    assert!(prompt.find("handle: h-1").unwrap() < prompt.find("handle: h-2").unwrap());
    assert!(
        prompt.contains("[USER RESUME PAYLOAD]\ncontinue"),
        "{prompt}"
    );
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn resume_without_mailbox_preserves_payload() {
    let fixture = Fixture::new();
    let prompt_dump = fixture.dir.path().join("prompt.txt");
    let script = fixture.write_script(
        "resume-no-mailbox.sh",
        &dump_last_arg_script(&prompt_dump, None, 0),
    );
    fixture.write_single_provider_model("fixture-model", "fixture-provider", &script);
    fixture.seed_session_turn("fixture-provider", SESSION_A);

    let mut cmd = fixture.base_resume_command("fixture-model", SESSION_A);
    cmd.arg("--prompt").arg("continue");
    let output = fixture.run(cmd);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(fs::read_to_string(&prompt_dump).unwrap(), "continue");
    assert!(fixture.mailbox_rows(SESSION_A, false).is_empty());
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn resume_without_mailbox_and_without_prompt_preserves_native_resume() {
    let fixture = Fixture::new();
    let argv_dump = fixture.dir.path().join("argv.txt");
    let script = fixture.write_script(
        "resume-no-prompt.sh",
        &format!("printf '%s\\n' \"$@\" > {}; exit 0", shell_path(&argv_dump)),
    );
    fixture.write_single_provider_model("fixture-model", "fixture-provider", &script);
    fixture.seed_session_turn("fixture-provider", SESSION_A);

    let output = fixture.run(fixture.base_resume_command("fixture-model", SESSION_A));

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        fs::read_to_string(&argv_dump).unwrap(),
        format!("one-shot-only\n--resume\n{SESSION_A}\n")
    );
    assert!(fixture.mailbox_rows(SESSION_A, false).is_empty());
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn resume_with_only_mailbox_sends_notification_prompt() {
    let fixture = Fixture::new();
    let prompt_dump = fixture.dir.path().join("prompt.txt");
    let script = fixture.write_script(
        "resume-only-mailbox.sh",
        &dump_last_arg_script(&prompt_dump, None, 0),
    );
    fixture.write_single_provider_model("fixture-model", "fixture-provider", &script);
    fixture.seed_session_turn("fixture-provider", SESSION_A);
    fixture.seed_mailbox(SESSION_A, "h-only", 0);

    let output = fixture.run(fixture.base_resume_command("fixture-model", SESSION_A));

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let prompt = fs::read_to_string(&prompt_dump).unwrap();
    assert!(prompt.starts_with("[OULIPOLY NOTIFICATIONS]"), "{prompt}");
    assert!(prompt.contains("handle: h-only"), "{prompt}");
    assert!(!prompt.contains("[USER RESUME PAYLOAD]"), "{prompt}");
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn resume_marks_delivered_after_success() {
    let fixture = Fixture::new();
    let prompt_dump = fixture.dir.path().join("prompt.txt");
    let script = fixture.write_script(
        "resume-deliver.sh",
        &dump_last_arg_script(&prompt_dump, None, 0),
    );
    fixture.write_single_provider_model("fixture-model", "fixture-provider", &script);
    fixture.seed_session_turn("fixture-provider", SESSION_A);
    let row = fixture.seed_mailbox(SESSION_A, "h-deliver", 0);

    let mut cmd = fixture.base_resume_command("fixture-model", SESSION_A);
    cmd.arg("--prompt").arg("continue");
    let output = fixture.run(cmd);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let invocation = parse_invocation(&String::from_utf8_lossy(&output.stderr));
    let rows = fixture.mailbox_rows(SESSION_A, true);
    let delivered = rows
        .iter()
        .find(|candidate| candidate.seq == row.seq)
        .unwrap();
    assert!(delivered.delivered_at.is_some());
    assert_eq!(
        delivered.delivered_by_invocation_uuid.as_deref(),
        Some(invocation.id.as_str())
    );
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn resume_marks_delivered_from_exact_ingested_user_turn_without_assistant_delta() {
    let fixture = Fixture::new();
    let prompt_dump = fixture.dir.path().join("prompt.txt");
    let turns = fixture.dir.path().join("turns.jsonl");
    let turn_script = fixture.write_script(
        "turns-exact.sh",
        &format!(
            "if [ -f {} ]; then cat {}; fi",
            shell_path(&turns),
            shell_path(&turns)
        ),
    );
    let script = fixture.write_script(
        "resume-exact-user-turn.sh",
        &write_user_turn_script(&prompt_dump, &turns, SESSION_A, None, 0),
    );
    fixture.write_single_provider_model("fixture-model", "fixture-provider", &script);
    fixture.write_sessions_config("fixture-provider", &turn_script);
    fixture.seed_session_turn("fixture-provider", SESSION_A);
    let row = fixture.seed_mailbox(SESSION_A, "h-exact-user", 0);

    let mut cmd = fixture.base_resume_command("fixture-model", SESSION_A);
    cmd.arg("--prompt").arg("continue");
    let output = fixture.run(cmd);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let prompt = fs::read_to_string(&prompt_dump).unwrap();
    assert!(prompt.starts_with("[OULIPOLY NOTIFICATIONS]"), "{prompt}");
    let invocation = parse_invocation(&String::from_utf8_lossy(&output.stderr));
    let rows = fixture.mailbox_rows(SESSION_A, true);
    let delivered = rows
        .iter()
        .find(|candidate| candidate.seq == row.seq)
        .unwrap();
    assert!(delivered.delivered_at.is_some());
    assert_eq!(
        delivered.delivered_by_invocation_uuid.as_deref(),
        Some(invocation.id.as_str())
    );
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn resume_rejects_different_ingested_user_turn_without_assistant_delta() {
    let fixture = Fixture::new();
    let prompt_dump = fixture.dir.path().join("prompt.txt");
    let turns = fixture.dir.path().join("turns.jsonl");
    let turn_script = fixture.write_script(
        "turns-different.sh",
        &format!(
            "if [ -f {} ]; then cat {}; fi",
            shell_path(&turns),
            shell_path(&turns)
        ),
    );
    let script = fixture.write_script(
        "resume-different-user-turn.sh",
        &write_user_turn_script(
            &prompt_dump,
            &turns,
            SESSION_A,
            Some("different payload"),
            0,
        ),
    );
    fixture.write_single_provider_model("fixture-model", "fixture-provider", &script);
    fixture.write_sessions_config("fixture-provider", &turn_script);
    fixture.seed_session_turn("fixture-provider", SESSION_A);
    let row = fixture.seed_mailbox(SESSION_A, "h-different-user", 0);

    let mut cmd = fixture.base_resume_command("fixture-model", SESSION_A);
    cmd.arg("--prompt").arg("continue");
    let output = fixture.run(cmd);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(prompt_dump.exists(), "{output:?}");
    let prompt = fs::read_to_string(&prompt_dump).unwrap();
    assert!(prompt.starts_with("[OULIPOLY NOTIFICATIONS]"), "{prompt}");
    let rows = fixture.mailbox_rows(SESSION_A, true);
    let pending = rows
        .iter()
        .find(|candidate| candidate.seq == row.seq)
        .unwrap();
    assert!(pending.delivered_at.is_none());
    assert_eq!(pending.delivery_attempts, 1);
    assert_eq!(
        pending.delivery_error.as_deref(),
        Some("mailbox_delivery_unconfirmed")
    );
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn resume_failure_leaves_pending() {
    let fixture = Fixture::new();
    let prompt_dump = fixture.dir.path().join("prompt.txt");
    let script = fixture.write_script(
        "resume-fail.sh",
        &dump_last_arg_script(&prompt_dump, None, 42),
    );
    fixture.write_single_provider_model("fixture-model", "fixture-provider", &script);
    fixture.seed_session_turn("fixture-provider", SESSION_A);
    fixture.seed_mailbox(SESSION_A, "h-fail", 0);

    let mut cmd = fixture.base_resume_command("fixture-model", SESSION_A);
    cmd.arg("--prompt").arg("continue");
    let output = fixture.run(cmd);

    assert!(!output.status.success(), "{output:?}");
    let rows = fixture.mailbox_rows(SESSION_A, true);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].delivered_at.is_none());
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn resume_drains_in_order_and_respects_batch_cap() {
    let fixture = Fixture::new();
    let prompt_dump = fixture.dir.path().join("prompt.txt");
    let script = fixture.write_script(
        "resume-batch.sh",
        &dump_last_arg_script(&prompt_dump, None, 0),
    );
    fixture.write_single_provider_model("fixture-model", "fixture-provider", &script);
    fixture.seed_session_turn("fixture-provider", SESSION_A);
    for index in 1..=21 {
        fixture.seed_mailbox(SESSION_A, &format!("h-{index:02}"), 0);
    }

    let mut cmd = fixture.base_resume_command("fixture-model", SESSION_A);
    cmd.arg("--prompt").arg("continue");
    let output = fixture.run(cmd);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let prompt = fs::read_to_string(&prompt_dump).unwrap();
    for index in 1..=20 {
        assert!(
            prompt.contains(&format!("handle: h-{index:02}")),
            "{prompt}"
        );
    }
    assert!(!prompt.contains("handle: h-21"), "{prompt}");
    assert!(
        prompt.contains("1 additional notification(s) remain queued"),
        "{prompt}"
    );
    let all = fixture.mailbox_rows(SESSION_A, true);
    assert_eq!(
        all.iter().filter(|row| row.delivered_at.is_some()).count(),
        20
    );
    assert_eq!(fixture.mailbox_rows(SESSION_A, false)[0].handle, "h-21");
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn resume_uses_resolved_active_session_id() {
    let fixture = Fixture::new();
    let prompt_dump = fixture.dir.path().join("prompt.txt");
    let script = fixture.write_script(
        "resume-chain.sh",
        &dump_last_arg_script(&prompt_dump, None, 0),
    );
    fixture.write_single_provider_model("fixture-model", "fixture-provider", &script);
    fixture.seed_active_chain(CHAIN_ID, "fixture-provider", SESSION_B, "fixture-model");
    fixture.seed_mailbox(SESSION_B, "h-active", 0);

    let mut cmd = fixture.base_chain_resume_command("fixture-model", CHAIN_ID);
    cmd.arg("--prompt").arg("continue");
    let output = fixture.run(cmd);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let prompt = fs::read_to_string(&prompt_dump).unwrap();
    assert!(prompt.contains("handle: h-active"), "{prompt}");
    assert!(fixture.mailbox_rows(CHAIN_ID, false).is_empty());
    assert!(fixture.mailbox_rows(SESSION_B, false).is_empty());
    fixture.assert_default_user_paths_untouched();
}

fn identity(os_pid: i64, os_boot_id: &str, os_pid_starttime_ticks: i64) -> ProcessIdentity {
    ProcessIdentity {
        os_pid,
        os_boot_id: os_boot_id.to_string(),
        os_pid_starttime_ticks,
    }
}

fn caller_chain(identities: &[&ProcessIdentity]) -> Value {
    caller_chain_payload(caller_chain_entries(identities))
}

fn caller_chain_entries(identities: &[&ProcessIdentity]) -> Vec<Value> {
    identities
        .iter()
        .map(|identity| process_identity_json(identity))
        .collect()
}

fn process_identity_json(identity: &ProcessIdentity) -> Value {
    json!({
        "pid": identity.os_pid,
        "boot_id": identity.os_boot_id,
        "starttime_ticks": identity.os_pid_starttime_ticks,
    })
}

fn caller_chain_payload(caller_chain: Vec<Value>) -> Value {
    json!({"caller_chain": caller_chain, "spooler_extra": "preserve-me"})
}

fn stdout_json(output: &Output) -> Value {
    parse_stdout_json(&output.stdout)
        .unwrap_or_else(|err| panic!("{}", stdout_json_diagnostic(&err, output)))
}

fn parse_stdout_json(stdout: &[u8]) -> serde_json::Result<Value> {
    serde_json::from_slice(stdout)
}

fn stdout_json_diagnostic(err: &serde_json::Error, output: &Output) -> String {
    format!(
        "failed to parse stdout as JSON: {err}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn row_handles(json: &Value) -> Vec<String> {
    json["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["handle"].as_str().unwrap().to_string())
        .collect()
}

fn inserted_row(result: Result<EnqueueResult, String>) -> MailboxRow {
    let result = unwrap_enqueue_result(result);
    assert_inserted_enqueue_result(&result);
    extract_inserted_mailbox_row(result)
}

fn unwrap_enqueue_result(result: Result<EnqueueResult, String>) -> EnqueueResult {
    result.unwrap()
}

fn assert_inserted_enqueue_result(result: &EnqueueResult) {
    if !matches!(result, EnqueueResult::Inserted(_)) {
        panic!("expected inserted mailbox row, got {result:?}");
    }
}

fn extract_inserted_mailbox_row(result: EnqueueResult) -> MailboxRow {
    match result {
        EnqueueResult::Inserted(row) => row,
        other => unreachable!("validated inserted mailbox row, got {other:?}"),
    }
}

fn notify_artifact_paths(root: &Path, handle: &str) -> NotifyArtifacts {
    let state_dir = root.join(format!("notify-{handle}"));
    NotifyArtifacts {
        meta: state_dir.join("meta.json"),
        log: state_dir.join("log"),
        rc: state_dir.join("rc"),
        state_dir,
    }
}

fn notify_metadata_content(metadata: &Value) -> String {
    serde_json::to_string_pretty(metadata).unwrap()
}

fn notify_log_content(handle: &str) -> String {
    format!("log for {handle}\n")
}

fn notify_rc_content(rc: i32) -> String {
    format!("{rc}\n")
}

fn dump_last_arg_script(prompt_dump: &Path, argv_dump: Option<&Path>, exit_code: i32) -> String {
    let argv_line = argv_dump
        .map(|path| format!("printf '%s\\n' \"$@\" > {}\n", shell_path(path)))
        .unwrap_or_default();
    format!(
        r#"{argv_line}last="${{@: -1}}"
printf '%s' "$last" > {}
exit {exit_code}"#,
        shell_path(prompt_dump)
    )
}

fn write_user_turn_script(
    prompt_dump: &Path,
    turns: &Path,
    session_id: &str,
    replacement_text: Option<&str>,
    exit_code: i32,
) -> String {
    let text_assignment = replacement_text
        .map(|text| format!("prompt={}\n", shell_path(Path::new(text))))
        .unwrap_or_default();
    format!(
        r#"last="${{@: -1}}"
printf '%s' "$last" > {}
prompt="$last"
{text_assignment}python3 - "$prompt" {} <<'PY'
import json
import sys

prompt = sys.argv[1]
turns = sys.argv[2]
record = {{
    "session_id": "{session_id}",
    "turn_id": "user-delivery-proof",
    "timestamp": "2026-04-17T08:00:01Z",
    "role": "user",
    "body": [{{"type": "text", "text": prompt}}],
}}
with open(turns, "w", encoding="utf-8") as out:
    out.write(json.dumps(record, separators=(",", ":")) + "\n")
PY
exit {exit_code}"#,
        shell_path(prompt_dump),
        shell_path(turns),
    )
}

fn parse_invocation(stderr: &str) -> CompositeInvocationId {
    let line = stderr
        .lines()
        .find(|line| line.starts_with("OULIPOLY_INVOCATION="))
        .unwrap_or_else(|| panic!("stderr missing invocation marker: {stderr}"));
    CompositeInvocationId::parse_env_value(line.strip_prefix("OULIPOLY_INVOCATION=").unwrap())
        .unwrap()
}

fn ts(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn toml_string(value: &str) -> String {
    format!("{value:?}")
}

fn shell_path(path: &Path) -> String {
    format!("'{}'", path_string(path).replace('\'', "'\\''"))
}
