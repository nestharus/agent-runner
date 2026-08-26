#![cfg(unix)]

use chrono::{DateTime, Utc};
use oulipoly_state::mailbox::{
    AgentBashCompleteEnqueue, EnqueueResult, InboxTarget, InboxTargetKind, MailboxDb, MailboxRow,
    RuntimeLifecycleState, RuntimeTerminalReason, SessionRuntimeUpsert, SubmittedInputEnqueue,
};
use oulipoly_state::pid_identity::{
    PidIdentityDb, PidIdentityRecord, ProcessIdentity, read_live_process_identity,
};
use oulipoly_state::{
    COMPLETION_REGISTRATION_AUTHORITY_ENV, CompletionRegistrationAuthority, CompositeInvocationId,
    InvocationStart, ProviderSessionBinding, SessionTurnIngest, StateDb,
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const INVOCATION_A: &str = "11111111-1111-4111-8111-111111111111";
const INVOCATION_B: &str = "22222222-2222-4222-8222-222222222222";
const SESSION_A: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const SESSION_B: &str = "6169694d-de0f-40d1-890c-6e28e55bab28";
const CHAIN_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

struct Fixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    home_dir: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
    completion_authorities:
        Mutex<std::collections::HashMap<String, CompletionRegistrationAuthority>>,
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
            completion_authorities: Mutex::new(std::collections::HashMap::new()),
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
        cmd.env_remove("OULIPOLY_AUTO_WAKE_RETRY_BASE_MS");
        cmd.output().unwrap()
    }

    fn register_and_notify(&self, handle: &str, metadata: Value) -> Output {
        let artifacts = self.write_notify_artifacts(handle, metadata, 0);
        let registration = self.run_register_artifacts(handle, "async", &artifacts);
        assert!(registration.status.success(), "{registration:?}");
        self.run_notify_artifacts(handle, &artifacts)
    }

    fn run_register_artifacts(
        &self,
        handle: &str,
        delivery_mode: &str,
        artifacts: &NotifyArtifacts,
    ) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("notify")
            .arg("agent-bash-register")
            .arg("--handle")
            .arg(handle)
            .arg("--delivery-mode")
            .arg(delivery_mode)
            .arg("--state-dir")
            .arg(&artifacts.state_dir)
            .arg("--meta")
            .arg(&artifacts.meta)
            .arg("--log")
            .arg(&artifacts.log)
            .arg("--rc")
            .arg(&artifacts.rc)
            .arg("--json");
        let metadata: Value = serde_json::from_slice(&fs::read(&artifacts.meta).unwrap()).unwrap();
        if let Some(invocation_uuid) = metadata
            .get("owner_invocation_uuid")
            .and_then(Value::as_str)
            && let Some(authority) = self
                .completion_authorities
                .lock()
                .unwrap()
                .get(invocation_uuid)
        {
            cmd.env(
                COMPLETION_REGISTRATION_AUTHORITY_ENV,
                authority.process_environment_value(),
            );
        }
        self.run(cmd)
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

    fn run_activate(&self, handle: &str) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("notify")
            .arg("agent-bash-activate")
            .arg("--handle")
            .arg(handle)
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

    fn run_mailbox(&self, args: &[String]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("mailbox").args(args);
        self.run(cmd)
    }

    fn run_mailbox_compact(&self, limit: usize, apply: bool) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("mailbox")
            .arg("compact-delivered")
            .arg("--limit")
            .arg(limit.to_string())
            .arg("--json");
        if apply {
            cmd.arg("--apply");
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

    fn seed_state_invocation_with_provider_session(
        &self,
        invocation_uuid: &str,
        provider_session_id: &str,
    ) {
        let db = self.open_state();
        let start = db
            .start_invocation_with_completion_registration_authority(&InvocationStart {
                invocation_uuid: invocation_uuid.to_string(),
                model_name: "fixture-model".to_string(),
                provider_name: "fixture-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        let id = start.invocation_row_id;
        self.remember_completion_authority(
            invocation_uuid,
            start.completion_registration_authority,
        );
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

    fn seed_running_invocation_with_live_session(
        &self,
        invocation_uuid: &str,
        session_id: &str,
    ) -> ProcessIdentity {
        let state = self.open_state();
        let start = state
            .start_invocation_with_completion_registration_authority(&InvocationStart {
                invocation_uuid: invocation_uuid.to_string(),
                model_name: "fixture-model".to_string(),
                provider_name: "fixture-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        self.remember_completion_authority(
            invocation_uuid,
            start.completion_registration_authority,
        );
        let identity = read_live_process_identity(i64::from(std::process::id()))
            .unwrap()
            .expect("test process identity");
        PidIdentityDb::open(&self.sidecar_path())
            .unwrap()
            .record_identity(PidIdentityRecord {
                identity: &identity,
                os_pgid: None,
                invocation_uuid,
                session_id: Some(session_id),
                provider_name: Some("fixture-provider"),
                model_name: Some("fixture-model"),
                recorded_at: "2026-08-07T12:00:00Z",
            })
            .unwrap();
        identity
    }

    fn remember_completion_authority(
        &self,
        invocation_uuid: &str,
        authority: CompletionRegistrationAuthority,
    ) {
        self.completion_authorities
            .lock()
            .unwrap()
            .insert(invocation_uuid.to_string(), authority);
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

    fn seed_submitted_input(
        &self,
        submission_token: &str,
        target_kind: InboxTargetKind,
        target_id: &str,
        payload: &[u8],
    ) -> MailboxRow {
        let mut db = MailboxDb::open(&self.sidecar_path()).unwrap();
        inserted_row(db.enqueue_submitted_input(&SubmittedInputEnqueue {
            submission_token,
            target: InboxTarget {
                kind: target_kind,
                id: target_id,
            },
            input: payload,
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

    fn write_confirming_resume_model(
        &self,
        model_name: &str,
        prompt_dump: &Path,
        session_id: &str,
    ) {
        let turns = self.dir.path().join(format!("{model_name}-turns.jsonl"));
        let turn_script = self.write_script(
            &format!("{model_name}-turns.sh"),
            &format!(
                "if [ -f {} ]; then cat {}; fi",
                shell_path(&turns),
                shell_path(&turns)
            ),
        );
        let provider_script = self.write_script(
            &format!("{model_name}-provider.sh"),
            &write_user_turn_script(prompt_dump, &turns, session_id, None, 0),
        );
        self.write_single_provider_model(model_name, "fixture-provider", &provider_script);
        self.write_sessions_config("fixture-provider", &turn_script);
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

    fn replace_active_chain_segment(&self, chain_id: &str, provider: &str, session_id: &str) {
        let conn = self.conn();
        conn.execute(
            "UPDATE session_chain_segments
             SET ended_at = '2026-04-17T08:01:00Z'
             WHERE chain_id = ?1 AND ended_at IS NULL",
            params![chain_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, '2026-04-17T08:02:00Z', 'manual')",
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
fn completion_registration_binds_the_explicit_owner_without_pid_lineage() {
    let fixture = Fixture::new();
    fixture.seed_state_invocation_with_provider_session(INVOCATION_A, SESSION_A);
    let artifacts = fixture.write_notify_artifacts(
        "h-explicit-owner",
        owner_metadata(SESSION_A, INVOCATION_A),
        0,
    );

    let registration = fixture.run_register_artifacts("h-explicit-owner", "async", &artifacts);
    let expected_generation = MailboxDb::open(&fixture.sidecar_path())
        .unwrap()
        .sidecar_generation()
        .unwrap();
    let completion = fixture.run_notify_artifacts("h-explicit-owner", &artifacts);

    assert!(registration.status.success(), "{registration:?}");
    let registered = stdout_json(&registration);
    assert_eq!(registered["status"], "registered");
    assert_eq!(registered["owner_session_id"], SESSION_A);
    assert_eq!(registered["owner_invocation_uuid"], INVOCATION_A);
    let obligations = fixture
        .open_state()
        .completion_obligations_for_invocation(INVOCATION_A)
        .unwrap();
    assert_eq!(obligations.len(), 1);
    assert_eq!(obligations[0].event_id, "h-explicit-owner");
    assert_eq!(obligations[0].owner_invocation_uuid, INVOCATION_A);
    assert_eq!(obligations[0].owner_session_id, SESSION_A);
    assert_eq!(
        obligations[0].expected_sidecar_generation,
        expected_generation
    );
    assert!(completion.status.success(), "{completion:?}");
    let completed = stdout_json(&completion);
    assert_eq!(completed["status"], "triggered");
    assert_eq!(completed["session_source"], "completion_event_listener");
    assert_eq!(
        fixture.mailbox_rows(SESSION_A, false)[0].handle,
        "h-explicit-owner"
    );
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn completion_registration_rejects_foreign_cli_without_invocation_capability() {
    let fixture = Fixture::new();
    fixture.seed_state_invocation_with_provider_session(INVOCATION_A, SESSION_A);
    let artifacts = fixture.write_notify_artifacts(
        "h-foreign-no-authority",
        owner_metadata(SESSION_A, INVOCATION_A),
        0,
    );
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
    cmd.arg("notify")
        .arg("agent-bash-register")
        .arg("--handle")
        .arg("h-foreign-no-authority")
        .arg("--delivery-mode")
        .arg("async")
        .arg("--state-dir")
        .arg(&artifacts.state_dir)
        .arg("--meta")
        .arg(&artifacts.meta)
        .arg("--log")
        .arg(&artifacts.log)
        .arg("--rc")
        .arg(&artifacts.rc)
        .arg("--json")
        .env_remove(COMPLETION_REGISTRATION_AUTHORITY_ENV);

    let registration = fixture.run(cmd);

    assert_eq!(registration.status.code(), Some(74), "{registration:?}");
    let response = stdout_json(&registration);
    assert!(
        response["message"]
            .as_str()
            .unwrap()
            .contains("caller-bound invocation authority"),
        "{response}"
    );
    assert!(
        fixture
            .open_state()
            .completion_obligations_for_invocation(INVOCATION_A)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn completion_registration_accepts_running_owner_from_exact_live_pid_sidecar() {
    let fixture = Fixture::new();
    let identity = fixture.seed_running_invocation_with_live_session(INVOCATION_A, SESSION_A);
    let artifacts = fixture.write_notify_artifacts(
        "h-running-owner",
        running_owner_metadata(SESSION_A, INVOCATION_A, &identity),
        0,
    );

    let registration = fixture.run_register_artifacts("h-running-owner", "async", &artifacts);

    assert!(registration.status.success(), "{registration:?}");
    let registered = stdout_json(&registration);
    assert_eq!(registered["owner_session_id"], SESSION_A);
    assert_eq!(registered["owner_invocation_uuid"], INVOCATION_A);
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn completion_registration_waits_for_live_session_binding() {
    let fixture = Fixture::new();
    let state = fixture.open_state();
    let invocation_start = state
        .start_invocation_with_completion_registration_authority(&InvocationStart {
            invocation_uuid: INVOCATION_A.to_string(),
            model_name: "fixture-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    let invocation_row_id = invocation_start.invocation_row_id;
    let artifacts = fixture.write_notify_artifacts(
        "h-live-binding-owner",
        owner_metadata(SESSION_A, INVOCATION_A),
        0,
    );
    let socket_path = fixture.dir.path().join("live-binding.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    listener.set_nonblocking(true).unwrap();
    let state_path = fixture.state_path();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "runner never connected");
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(err) => panic!("live-binding fixture accept failed: {err}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        let report: Value = serde_json::from_str(&line).unwrap();
        StateDb::open(&state_path)
            .unwrap()
            .bind_invocation_provider_session_start(
                invocation_row_id,
                &ProviderSessionBinding {
                    provider_session_id: SESSION_A.to_string(),
                    capture_method: "provider_live_report",
                    resume_input_id: None,
                    provider_session_resolved_account: Some("fixture-provider".to_string()),
                },
            )
            .unwrap();
        writeln!(
            stream,
            "{}",
            json!({
                "ok": true,
                "agent_runner_chain_id": null,
                "agent_runner_invocation_id": INVOCATION_A,
                "id": INVOCATION_A,
                "provider_name": "fixture-provider",
                "provider_session_id": SESSION_A,
                "resume_input_id": null,
                "session_id": SESSION_A,
                "error": null,
            })
        )
        .unwrap();
        assert_eq!(report["invocation_uuid"], INVOCATION_A);
        assert_eq!(report["provider_session_id"], SESSION_A);
        assert_eq!(report["token"], "fixture-token");
    });

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
    cmd.arg("notify")
        .arg("agent-bash-register")
        .arg("--handle")
        .arg("h-live-binding-owner")
        .arg("--delivery-mode")
        .arg("async")
        .arg("--state-dir")
        .arg(&artifacts.state_dir)
        .arg("--meta")
        .arg(&artifacts.meta)
        .arg("--log")
        .arg(&artifacts.log)
        .arg("--rc")
        .arg(&artifacts.rc)
        .arg("--json")
        .env(
            COMPLETION_REGISTRATION_AUTHORITY_ENV,
            invocation_start
                .completion_registration_authority
                .process_environment_value(),
        )
        .env("OULIPOLY_LIVE_SESSION_BIND_SOCKET", &socket_path)
        .env("OULIPOLY_LIVE_SESSION_BIND_TOKEN", "fixture-token");
    let registration = fixture.run(cmd);
    server.join().unwrap();

    assert!(registration.status.success(), "{registration:?}");
    let registered = stdout_json(&registration);
    assert_eq!(registered["owner_session_id"], SESSION_A);
    assert_eq!(registered["owner_invocation_uuid"], INVOCATION_A);
    let rebound = fixture
        .open_state()
        .get_invocation_by_uuid(INVOCATION_A)
        .unwrap()
        .unwrap();
    assert_eq!(rebound.provider_session_id.as_deref(), Some(SESSION_A));
}

#[test]
fn completion_registration_rejects_mismatched_running_owner_sidecar() {
    let fixture = Fixture::new();
    let identity = fixture.seed_running_invocation_with_live_session(INVOCATION_A, SESSION_A);
    let artifacts = fixture.write_notify_artifacts(
        "h-running-wrong-session",
        running_owner_metadata(SESSION_B, INVOCATION_A, &identity),
        0,
    );

    let registration =
        fixture.run_register_artifacts("h-running-wrong-session", "async", &artifacts);

    assert_eq!(registration.status.code(), Some(74), "{registration:?}");
    assert!(
        stdout_json(&registration)["message"]
            .as_str()
            .unwrap()
            .contains("is not bound")
    );
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn completion_registration_rejects_stale_running_owner_identity() {
    let fixture = Fixture::new();
    let mut identity = fixture.seed_running_invocation_with_live_session(INVOCATION_A, SESSION_A);
    identity.os_pid_starttime_ticks += 1;
    PidIdentityDb::open(&fixture.sidecar_path())
        .unwrap()
        .record_identity(PidIdentityRecord {
            identity: &identity,
            os_pgid: None,
            invocation_uuid: INVOCATION_A,
            session_id: Some(SESSION_A),
            provider_name: Some("fixture-provider"),
            model_name: Some("fixture-model"),
            recorded_at: "2026-08-07T12:00:00Z",
        })
        .unwrap();
    let artifacts = fixture.write_notify_artifacts(
        "h-running-stale-owner",
        running_owner_metadata(SESSION_A, INVOCATION_A, &identity),
        0,
    );

    let registration = fixture.run_register_artifacts("h-running-stale-owner", "async", &artifacts);

    assert_eq!(registration.status.code(), Some(74), "{registration:?}");
    assert!(
        stdout_json(&registration)["message"]
            .as_str()
            .unwrap()
            .contains("is not bound")
    );
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn completion_response_reports_delivery_for_every_listener_session() {
    let fixture = Fixture::new();
    fixture.seed_state_invocation_with_provider_session(INVOCATION_A, SESSION_A);
    fixture.seed_state_invocation_with_provider_session(INVOCATION_B, SESSION_B);
    let artifacts = fixture.write_notify_artifacts(
        "h-multi-listener",
        owner_metadata(SESSION_A, INVOCATION_A),
        0,
    );
    let first = fixture.run_register_artifacts("h-multi-listener", "async", &artifacts);
    fs::write(
        &artifacts.meta,
        serde_json::to_string(&owner_metadata(SESSION_B, INVOCATION_B)).unwrap(),
    )
    .unwrap();
    let second = fixture.run_register_artifacts("h-multi-listener", "async", &artifacts);

    assert!(first.status.success(), "{first:?}");
    assert!(second.status.success(), "{second:?}");
    let completion = fixture.run_notify_artifacts("h-multi-listener", &artifacts);

    assert!(completion.status.success(), "{completion:?}");
    let completed = stdout_json(&completion);
    assert_eq!(completed["pty_deliveries"].as_array().unwrap().len(), 2);
    assert_eq!(completed["wake"]["status"], "spawned");
    assert!(
        completed["pty_deliveries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|diagnostic| diagnostic["status"] == "no_runtime"
                && diagnostic["submitted"] == false)
    );
    assert_eq!(fixture.mailbox_rows(SESSION_A, false).len(), 1);
    assert_eq!(fixture.mailbox_rows(SESSION_B, false).len(), 1);
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn completion_for_headless_runtime_is_not_submitted_to_pty() {
    let fixture = Fixture::new();
    fixture.seed_state_invocation_with_provider_session(INVOCATION_A, SESSION_A);
    MailboxDb::open(&fixture.sidecar_path())
        .unwrap()
        .upsert_session_runtime(SessionRuntimeUpsert {
            session_id: SESSION_A,
            mode: "headless",
            invocation_uuid: Some(INVOCATION_A),
            provider_name: None,
            model_name: None,
            pty_control_path: None,
            models_dir: None,
            effective_cwd: None,
        })
        .unwrap();

    let completion = fixture.register_and_notify(
        "h-headless-not-pty",
        owner_metadata(SESSION_A, INVOCATION_A),
    );

    assert!(completion.status.success(), "{completion:?}");
    let completed = stdout_json(&completion);
    assert_eq!(completed["pty_delivery"]["status"], "not_pty");
    assert_eq!(completed["pty_delivery"]["submitted"], false);
    assert_eq!(completed["wake"]["status"], "spawned");
    let rows = fixture.mailbox_rows(SESSION_A, false);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].handle, "h-headless-not-pty");
    assert!(rows[0].delivered_at.is_none());
    assert_eq!(rows[0].delivery_attempts, 0);
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn completion_registration_rejects_an_unbound_session() {
    let fixture = Fixture::new();
    fixture.seed_state_invocation_with_provider_session(INVOCATION_A, SESSION_A);
    let artifacts = fixture.write_notify_artifacts(
        "h-wrong-session",
        owner_metadata(SESSION_B, INVOCATION_A),
        0,
    );

    let output = fixture.run_register_artifacts("h-wrong-session", "async", &artifacts);

    assert_eq!(output.status.code(), Some(74), "{output:?}");
    let json = stdout_json(&output);
    assert_eq!(json["status"], "notification_event_error");
    assert!(json["message"].as_str().unwrap().contains("is not bound"));
    assert!(fixture.mailbox_rows(SESSION_A, true).is_empty());
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn completion_trigger_requires_prior_registration() {
    let fixture = Fixture::new();
    let artifacts = fixture.write_notify_artifacts(
        "h-unregistered",
        owner_metadata(SESSION_A, INVOCATION_A),
        0,
    );

    let output = fixture.run_notify_artifacts("h-unregistered", &artifacts);

    assert_eq!(output.status.code(), Some(74), "{output:?}");
    let json = stdout_json(&output);
    assert_eq!(json["status"], "notification_event_error");
    assert!(
        json["message"]
            .as_str()
            .unwrap()
            .contains("is not registered")
    );
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn completion_registration_requires_both_owner_fields() {
    let fixture = Fixture::new();
    let artifacts = fixture.write_notify_artifacts(
        "h-partial-owner",
        json!({"owner_session_id": SESSION_A}),
        0,
    );

    let output = fixture.run_register_artifacts("h-partial-owner", "async", &artifacts);

    assert_eq!(output.status.code(), Some(74), "{output:?}");
    let json = stdout_json(&output);
    assert_eq!(json["status"], "notification_event_error");
    assert!(json["message"].as_str().unwrap().contains("both required"));
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn completion_registration_rejects_ownerless_metadata() {
    let fixture = Fixture::new();
    let artifacts = fixture.write_notify_artifacts("h-ownerless", json!({}), 0);

    let output = fixture.run_register_artifacts("h-ownerless", "async", &artifacts);

    assert_eq!(output.status.code(), Some(74), "{output:?}");
    let json = stdout_json(&output);
    assert_eq!(json["status"], "notification_event_error");
    assert!(json["message"].as_str().unwrap().contains("both required"));
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn completion_registration_rejects_non_object_metadata() {
    let fixture = Fixture::new();
    let artifacts = fixture.write_notify_artifacts("h-invalid-meta", json!([]), 0);

    let output = fixture.run_register_artifacts("h-invalid-meta", "async", &artifacts);

    assert_eq!(output.status.code(), Some(74), "{output:?}");
    let json = stdout_json(&output);
    assert_eq!(json["status"], "notification_event_error");
    assert_eq!(json["message"], "meta.json must contain a JSON object");
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn synchronous_completion_materializes_only_after_activation() {
    let fixture = Fixture::new();
    fixture.seed_state_invocation_with_provider_session(INVOCATION_A, SESSION_A);
    let artifacts = fixture.write_notify_artifacts(
        "h-sync-activate",
        owner_metadata(SESSION_A, INVOCATION_A),
        0,
    );
    let registration = fixture.run_register_artifacts("h-sync-activate", "sync", &artifacts);
    let completion = fixture.run_notify_artifacts("h-sync-activate", &artifacts);

    assert!(registration.status.success(), "{registration:?}");
    assert!(completion.status.success(), "{completion:?}");
    assert!(fixture.mailbox_rows(SESSION_A, true).is_empty());

    let activation = fixture.run_activate("h-sync-activate");

    assert!(activation.status.success(), "{activation:?}");
    let activated = stdout_json(&activation);
    assert_eq!(activated["status"], "activated");
    assert_eq!(activated["listener_count"], 1);
    let rows = fixture.mailbox_rows(SESSION_A, true);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].handle, "h-sync-activate");
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn completion_trigger_is_idempotent_for_the_registered_handle() {
    let fixture = Fixture::new();
    fixture.seed_state_invocation_with_provider_session(INVOCATION_A, SESSION_A);
    let artifacts =
        fixture.write_notify_artifacts("h-idem", owner_metadata(SESSION_A, INVOCATION_A), 0);
    let registration = fixture.run_register_artifacts("h-idem", "async", &artifacts);
    assert!(registration.status.success(), "{registration:?}");

    let first = fixture.run_notify_artifacts("h-idem", &artifacts);
    let second = fixture.run_notify_artifacts("h-idem", &artifacts);

    assert!(first.status.success(), "{first:?}");
    assert!(second.status.success(), "{second:?}");
    let first_json = stdout_json(&first);
    let second_json = stdout_json(&second);
    assert_eq!(first_json["status"], "triggered");
    assert_eq!(second_json["status"], "already_triggered");
    assert_eq!(second_json["seq"], first_json["seq"]);
    let rows = fixture.mailbox_rows(SESSION_A, true);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].seq, first_json["seq"].as_i64().unwrap());
    assert_eq!(
        first_json["payload_file_path"],
        rows[0].payload_file_path.as_deref().unwrap()
    );
    assert_eq!(
        first_json["payload_sha256"],
        rows[0].payload_sha256.as_deref().unwrap()
    );
    assert_eq!(
        first_json["payload_byte_len"],
        rows[0].payload_byte_len.unwrap()
    );
    let payload_path = Path::new(rows[0].payload_file_path.as_deref().unwrap());
    let retained_payload = fs::read_to_string(payload_path).unwrap();
    assert_ne!(retained_payload, rows[0].payload_json);
    assert_eq!(
        serde_json::from_str::<Value>(&retained_payload).unwrap()["handle"],
        "h-idem"
    );
    assert_eq!(
        serde_json::from_str::<Value>(&retained_payload).unwrap()["meta"]["spooler_extra"],
        "preserve-me"
    );
    assert_eq!(
        rows[0].payload_sha256.as_deref(),
        Some(sha256_hex(retained_payload.as_bytes()).as_str())
    );
    assert_eq!(
        rows[0].payload_byte_len,
        Some(retained_payload.len() as i64)
    );
    assert_eq!(
        rows[0].payload_retention_policy.as_deref(),
        Some("until_terminal_disposition")
    );
    assert_eq!(
        fs::metadata(payload_path).unwrap().permissions().mode() & 0o222,
        0
    );
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn published_payload_without_metadata_commit_is_not_accepted() {
    let fixture = Fixture::new();
    let payload = r#"{"schema_version":1,"kind":"agent_bash_complete","body":"durable"}"#;
    let sidecar_path = fixture.sidecar_path();
    let mut db = MailboxDb::open(&sidecar_path).unwrap();
    Connection::open(&sidecar_path)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER reject_mailbox_acceptance
             BEFORE INSERT ON mailbox
             BEGIN
                 SELECT RAISE(ABORT, 'forced metadata failure');
             END;",
        )
        .unwrap();
    let input = AgentBashCompleteEnqueue {
        session_id: SESSION_A,
        handle: "h-metadata-failure",
        payload_json: payload,
        owner_invocation_uuid: Some(INVOCATION_A),
        matched_os_pid: Some(9000),
        matched_os_boot_id: Some("boot-mailbox"),
        matched_os_pid_starttime_ticks: Some(1),
        matched_chain_index: Some(0),
        state_dir: "/tmp/state",
        meta_path: "/tmp/state/meta.json",
        log_path: "/tmp/state/log",
        rc_path: "/tmp/state/rc",
        rc: 0,
    };

    let error = db.enqueue_agent_bash_complete(&input).unwrap_err();

    assert!(error.contains("forced metadata failure"), "{error}");
    assert!(db.list_mailbox(SESSION_A, true).unwrap().is_empty());
    assert!(expected_payload_path(&sidecar_path, payload.as_bytes()).exists());
}

#[test]
fn notify_ordering_by_seq() {
    let fixture = Fixture::new();
    fixture.seed_state_invocation_with_provider_session(INVOCATION_A, SESSION_A);
    for handle in ["h-a", "h-b", "h-c"] {
        let output = fixture.register_and_notify(handle, owner_metadata(SESSION_A, INVOCATION_A));
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
fn mailbox_compact_delivered_is_dry_run_by_default_and_hydrates_list_output() {
    let fixture = Fixture::new();
    let row = fixture.seed_mailbox(SESSION_A, "h-compact", 0);
    let original_payload = fs::read_to_string(row.payload_file_path.as_deref().unwrap()).unwrap();
    let sidecar_path = fixture.sidecar_path();
    let mut db = MailboxDb::open(&sidecar_path).unwrap();
    Connection::open(&sidecar_path)
        .unwrap()
        .execute(
            "UPDATE mailbox
             SET payload_json = ?2, payload_compacted_at = NULL
             WHERE seq = ?1",
            params![row.seq, &original_payload],
        )
        .unwrap();
    db.mark_delivered(SESSION_A, &[row.seq], INVOCATION_A)
        .unwrap();
    drop(db);

    let dry_run = fixture.run_mailbox_compact(1, false);
    assert!(dry_run.status.success(), "{dry_run:?}");
    let dry_run_json = stdout_json(&dry_run);
    assert_eq!(dry_run_json["applied"], false);
    assert_eq!(dry_run_json["before"]["eligible_rows"], 1);
    assert!(dry_run_json["report"].is_null());
    assert_eq!(
        Connection::open(&sidecar_path)
            .unwrap()
            .query_row(
                "SELECT payload_compacted_at FROM mailbox WHERE seq = ?1",
                params![row.seq],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap(),
        None
    );

    let apply = fixture.run_mailbox_compact(1, true);
    assert!(apply.status.success(), "{apply:?}");
    let apply_json = stdout_json(&apply);
    assert_eq!(apply_json["applied"], true);
    assert_eq!(apply_json["report"]["compacted_rows"], 1);
    assert_eq!(apply_json["after"]["eligible_rows"], 0);

    let listed = fixture.run_mailbox_list(SESSION_A, true);
    assert!(listed.status.success(), "{listed:?}");
    assert_eq!(
        stdout_json(&listed)["rows"][0]["payload_json"],
        original_payload
    );
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn mailbox_isolation() {
    let fixture = Fixture::new();
    fixture.seed_state_invocation_with_provider_session(INVOCATION_A, SESSION_A);
    fixture.seed_state_invocation_with_provider_session(INVOCATION_B, SESSION_B);
    assert!(
        fixture
            .register_and_notify("h-a", owner_metadata(SESSION_A, INVOCATION_A))
            .status
            .success()
    );
    assert!(
        fixture
            .register_and_notify("h-b", owner_metadata(SESSION_B, INVOCATION_B))
            .status
            .success()
    );

    let output = fixture.run_mailbox_list(SESSION_A, false);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(row_handles(&stdout_json(&output)), vec!["h-a"]);
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn mailbox_recovery_commands_search_show_and_ack_bounded_range() {
    let fixture = Fixture::new();
    let first = fixture.seed_mailbox(SESSION_A, "h-recovery-a", 0);
    let second = fixture.seed_mailbox(SESSION_A, "h-recovery-b", 7);
    let third = fixture.seed_mailbox(SESSION_A, "h-recovery-c", 0);

    let search = fixture.run_mailbox(&[
        "search".to_string(),
        "--session-id".to_string(),
        SESSION_A.to_string(),
        "recovery-b".to_string(),
        "--json".to_string(),
    ]);
    assert!(search.status.success(), "{search:?}");
    assert_eq!(row_handles(&stdout_json(&search)), vec!["h-recovery-b"]);

    let show = fixture.run_mailbox(&[
        "show".to_string(),
        "--session-id".to_string(),
        SESSION_A.to_string(),
        "--seq".to_string(),
        second.seq.to_string(),
        "--include-artifacts".to_string(),
        "--max-bytes".to_string(),
        "5".to_string(),
        "--json".to_string(),
    ]);
    assert!(show.status.success(), "{show:?}");
    let shown = stdout_json(&show);
    assert_eq!(shown["row"]["handle"], "h-recovery-b");
    for artifact in ["meta", "log", "rc"] {
        assert!(shown["artifacts"][artifact].as_str().unwrap().len() <= 5 + "\n[truncated]".len());
    }
    assert!(
        shown["artifacts"]["meta"]
            .as_str()
            .unwrap()
            .ends_with("[truncated]")
    );

    let ack = fixture.run_mailbox(&[
        "ack".to_string(),
        "--session-id".to_string(),
        SESSION_A.to_string(),
        "--from-seq".to_string(),
        first.seq.to_string(),
        "--to-seq".to_string(),
        second.seq.to_string(),
        "--delivered-by".to_string(),
        "recovery-test".to_string(),
        "--json".to_string(),
    ]);
    assert!(ack.status.success(), "{ack:?}");
    let acknowledged = stdout_json(&ack);
    assert_eq!(acknowledged["acknowledged_count"], 2);
    assert_eq!(acknowledged["remaining_pending"], 1);
    let pending = fixture.mailbox_rows(SESSION_A, false);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].seq, third.seq);
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn mailbox_pause_suppresses_notify_delivery_and_wake_until_resume() {
    let fixture = Fixture::new();
    fixture.seed_state_invocation_with_provider_session(INVOCATION_A, SESSION_A);
    let pause = fixture.run_mailbox(&[
        "pause".to_string(),
        "--session-id".to_string(),
        SESSION_A.to_string(),
        "--json".to_string(),
    ]);
    assert!(pause.status.success(), "{pause:?}");
    assert_eq!(stdout_json(&pause)["paused"], true);

    let notify = fixture.register_and_notify("h-paused", owner_metadata(SESSION_A, INVOCATION_A));

    assert!(notify.status.success(), "{notify:?}");
    let notified = stdout_json(&notify);
    assert_eq!(notified["pty_delivery"]["status"], "paused");
    assert_eq!(notified["pty_delivery"]["submitted"], false);
    assert!(notified["wake"].is_null());
    let paused_status = fixture.run_mailbox(&[
        "status".to_string(),
        "--session-id".to_string(),
        SESSION_A.to_string(),
        "--json".to_string(),
    ]);
    let paused = stdout_json(&paused_status);
    assert_eq!(paused["pending_count"], 1);
    assert_eq!(paused["deliverable_count"], 0);

    let resume = fixture.run_mailbox(&[
        "resume".to_string(),
        "--session-id".to_string(),
        SESSION_A.to_string(),
        "--json".to_string(),
    ]);
    assert!(resume.status.success(), "{resume:?}");
    let resumed = stdout_json(&resume);
    assert_eq!(resumed["paused"], false);
    assert_eq!(resumed["pending_count"], 1);
    assert_eq!(resumed["deliverable_count"], 1, "{resumed}");
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn resume_with_pending_mailbox_prepends_notifications() {
    let fixture = Fixture::new();
    let prompt_dump = fixture.dir.path().join("prompt.txt");
    fixture.write_confirming_resume_model("fixture-model", &prompt_dump, SESSION_A);
    fixture.seed_session_turn("fixture-provider", SESSION_A);
    fixture.seed_mailbox(SESSION_A, "h-1", 0);
    fixture.seed_mailbox(SESSION_A, "h-2", 7);

    let mut cmd = fixture.base_resume_command("fixture-model", SESSION_A);
    cmd.arg("--prompt").arg("continue");
    let output = fixture.run(cmd);

    assert_unconfirmed_resume(&output, SESSION_A);
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

    assert_unconfirmed_resume(&output, SESSION_A);
    assert_eq!(fs::read_to_string(&prompt_dump).unwrap(), "continue");
    assert!(fixture.mailbox_rows(SESSION_A, false).is_empty());
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn tokenized_session_resume_persists_input_before_shared_delivery() {
    let fixture = Fixture::new();
    let prompt_dump = fixture.dir.path().join("prompt.txt");
    let script = fixture.write_script(
        "resume-tokenized-session.sh",
        &dump_last_arg_script(&prompt_dump, None, 0),
    );
    fixture.write_single_provider_model("fixture-model", "fixture-provider", &script);
    fixture.seed_session_turn("fixture-provider", SESSION_A);

    let mut cmd = fixture.base_resume_command("fixture-model", SESSION_A);
    cmd.arg("--prompt")
        .arg("durable session input")
        .arg("--submission-token")
        .arg("logical-session-submit-1");
    let output = fixture.run(cmd);

    assert_unconfirmed_resume(&output, SESSION_A);
    let prompt = fs::read_to_string(&prompt_dump).unwrap();
    assert!(prompt.starts_with("[OULIPOLY INBOX]"), "{prompt}");
    assert!(prompt.contains("durable session input"), "{prompt}");
    let rows = fixture.mailbox_rows(SESSION_A, true);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, "input");
    assert_eq!(
        rows[0].submission_token.as_deref(),
        Some("logical-session-submit-1")
    );
    assert_eq!(rows[0].target_kind.as_deref(), Some("session"));
    assert_eq!(rows[0].target_id.as_deref(), Some(SESSION_A));
    assert_eq!(
        fs::read(rows[0].payload_file_path.as_deref().unwrap()).unwrap(),
        b"durable session input"
    );
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn chain_input_remains_reachable_after_active_segment_reselection() {
    let fixture = Fixture::new();
    let prompt_dump = fixture.dir.path().join("prompt.txt");
    let script = fixture.write_script(
        "resume-chain-reselected-input.sh",
        &dump_last_arg_script(&prompt_dump, None, 0),
    );
    fixture.write_single_provider_model("fixture-model", "fixture-provider", &script);
    fixture.seed_active_chain(CHAIN_ID, "fixture-provider", SESSION_A, "fixture-model");
    let accepted = fixture.seed_submitted_input(
        "logical-chain-submit-1",
        InboxTargetKind::Chain,
        CHAIN_ID,
        b"durable chain input",
    );
    fixture.replace_active_chain_segment(CHAIN_ID, "fixture-provider", SESSION_B);
    fixture.seed_session_turn("fixture-provider", SESSION_B);

    let output = fixture.run(fixture.base_chain_resume_command("fixture-model", CHAIN_ID));

    assert_unconfirmed_resume(&output, SESSION_B);
    let prompt = fs::read_to_string(&prompt_dump).unwrap();
    assert!(prompt.starts_with("[OULIPOLY INBOX]"), "{prompt}");
    assert!(prompt.contains("durable chain input"), "{prompt}");
    let db = MailboxDb::open(&fixture.sidecar_path()).unwrap();
    let row = db
        .list_pending_for_delivery(SESSION_B, Some(CHAIN_ID))
        .unwrap();
    assert!(row.is_empty());
    let stored = Connection::open(fixture.sidecar_path())
        .unwrap()
        .query_row(
            "SELECT delivered_at, target_kind, target_id FROM mailbox WHERE seq = ?1",
            params![accepted.seq],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .unwrap();
    assert!(stored.0.is_some());
    assert_eq!(stored.1.as_deref(), Some("chain"));
    assert_eq!(stored.2.as_deref(), Some(CHAIN_ID));
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

    assert_unconfirmed_resume(&output, SESSION_A);
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
    fixture.write_confirming_resume_model("fixture-model", &prompt_dump, SESSION_A);
    fixture.seed_session_turn("fixture-provider", SESSION_A);
    fixture.seed_mailbox(SESSION_A, "h-only", 0);

    let output = fixture.run(fixture.base_resume_command("fixture-model", SESSION_A));

    assert_unconfirmed_resume(&output, SESSION_A);
    let prompt = fs::read_to_string(&prompt_dump).unwrap();
    assert!(prompt.starts_with("[OULIPOLY NOTIFICATIONS]"), "{prompt}");
    assert!(prompt.contains("handle: h-only"), "{prompt}");
    assert!(!prompt.contains("[USER RESUME PAYLOAD]"), "{prompt}");
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn resume_marks_delivered_after_exact_turn_confirmation() {
    let fixture = Fixture::new();
    let prompt_dump = fixture.dir.path().join("prompt.txt");
    fixture.write_confirming_resume_model("fixture-model", &prompt_dump, SESSION_A);
    fixture.seed_session_turn("fixture-provider", SESSION_A);
    let row = fixture.seed_mailbox(SESSION_A, "h-deliver", 0);

    let mut cmd = fixture.base_resume_command("fixture-model", SESSION_A);
    cmd.arg("--prompt").arg("continue");
    let output = fixture.run(cmd);

    let invocation = assert_unconfirmed_resume(&output, SESSION_A);
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
    assert!(
        Path::new(delivered.payload_file_path.as_deref().unwrap()).exists(),
        "confirmed delivery must not remove payload before governed cleanup"
    );
    let history = MailboxDb::open(&fixture.sidecar_path())
        .unwrap()
        .runtime_generation_history(SESSION_A)
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].lifecycle_state, RuntimeLifecycleState::Exited);
    assert_eq!(
        history[0].terminal_reason,
        Some(RuntimeTerminalReason::OrderlyCompletion)
    );
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn resume_fails_closed_when_immutable_payload_is_missing() {
    let fixture = Fixture::new();
    let prompt_dump = fixture.dir.path().join("prompt.txt");
    let script = fixture.write_script(
        "resume-missing-payload.sh",
        &dump_last_arg_script(&prompt_dump, None, 0),
    );
    fixture.write_single_provider_model("fixture-model", "fixture-provider", &script);
    fixture.seed_session_turn("fixture-provider", SESSION_A);
    let row = fixture.seed_mailbox(SESSION_A, "h-missing-payload", 0);
    fs::remove_file(row.payload_file_path.as_deref().unwrap()).unwrap();

    let output = fixture.run(fixture.base_resume_command("fixture-model", SESSION_A));

    assert!(!output.status.success(), "{output:?}");
    assert!(
        !prompt_dump.exists(),
        "provider must not receive an unverified payload"
    );
    let pending = fixture.mailbox_rows(SESSION_A, true).remove(0);
    assert!(pending.delivered_at.is_none());
    assert_eq!(pending.delivery_attempts, 0);
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

    let invocation = assert_unconfirmed_resume(&output, SESSION_A);
    let prompt = fs::read_to_string(&prompt_dump).unwrap();
    assert!(prompt.starts_with("[OULIPOLY NOTIFICATIONS]"), "{prompt}");
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
fn resume_typed_physical_zero_failure_keeps_selected_mailbox_outside_age270_seam() {
    let fixture = Fixture::new();
    let prompt_dump = fixture.dir.path().join("prompt.txt");
    let turns = fixture.dir.path().join("turns.jsonl");
    let turn_script = fixture.write_script(
        "turns-typed-zero.sh",
        &format!(
            "if [ -f {} ]; then cat {}; fi",
            shell_path(&turns),
            shell_path(&turns)
        ),
    );
    let script = fixture.write_script(
        "resume-typed-zero.sh",
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
    let selected = fixture.seed_mailbox(SESSION_A, "h-typed-zero", 0);
    let mut cmd = fixture.base_resume_command("fixture-model", SESSION_A);
    cmd.arg("--prompt").arg("continue");
    cmd.env_remove("OULIPOLY_AGE153_FORCE_TERMINAL_SIGNAL_NONE");
    cmd.env(
        "OULIPOLY_AGE153_FORCE_TERMINAL_SIGNAL_KIND",
        "ProlongedSilence",
    );
    let output = fixture.run(cmd);
    let prompt = fs::read_to_string(&prompt_dump).unwrap();
    assert!(prompt.starts_with("[OULIPOLY NOTIFICATIONS]"));
    assert!(prompt.contains("handle: h-typed-zero"));
    let invocation = parse_invocation(&String::from_utf8_lossy(&output.stderr));
    let result = assert_failure_result(
        &output,
        &invocation,
        "fixture-provider",
        SESSION_A,
        "bounded_silence",
    );
    assert!(
        result["agent_runner_chain_id"].as_str().is_some(),
        "{result}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("resume_completion_unconfirmed"));
    assert!(!stderr.contains("incomplete_tool_boundary"));
    let markers = stderr
        .lines()
        .filter_map(|line| line.strip_prefix("OULIPOLY_TERMINAL_SIGNAL="))
        .collect::<Vec<_>>();
    assert_eq!(markers.len(), 1, "{stderr}");
    let marker: Value = serde_json::from_str(markers[0]).unwrap();
    assert_eq!(marker["kind"], "ProlongedSilence");
    assert_eq!(marker["invocation_id"], invocation.id);
    assert_eq!(marker["session_id"], SESSION_A);
    assert_failed_invocation(&fixture, &invocation.id, SESSION_A, "bounded_silence");
    let row = fixture
        .mailbox_rows(SESSION_A, true)
        .into_iter()
        .find(|row| row.seq == selected.seq)
        .unwrap();
    assert!(row.delivered_at.is_none());
    assert!(row.delivered_by_invocation_uuid.is_none());
    assert_eq!(row.delivery_attempts, 1);
    assert_eq!(row.delivery_error.as_deref(), Some("bounded_silence"));
    let mailbox = MailboxDb::open(&fixture.sidecar_path()).unwrap();
    let runtime = mailbox.session_runtime(SESSION_A).unwrap().unwrap();
    assert_eq!(runtime.run_state, "idle");
    assert!(runtime.running_invocation_uuid.is_none());
    assert!(runtime.running_os_pid.is_none());
    assert_eq!(runtime.last_exit_code, Some(1));
    assert!(mailbox.wake_claim(SESSION_A).unwrap().is_none());
    assert_eq!(invocation_count(&fixture), 1);
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
    fixture.write_confirming_resume_model("fixture-model", &prompt_dump, SESSION_A);
    fixture.seed_session_turn("fixture-provider", SESSION_A);
    for index in 1..=21 {
        fixture.seed_mailbox(SESSION_A, &format!("h-{index:02}"), 0);
    }

    let mut cmd = fixture.base_resume_command("fixture-model", SESSION_A);
    cmd.arg("--prompt").arg("continue");
    let output = fixture.run(cmd);

    assert_unconfirmed_resume(&output, SESSION_A);
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
    fixture.write_confirming_resume_model("fixture-model", &prompt_dump, SESSION_B);
    fixture.seed_active_chain(CHAIN_ID, "fixture-provider", SESSION_B, "fixture-model");
    fixture.seed_mailbox(SESSION_B, "h-active", 0);

    let mut cmd = fixture.base_chain_resume_command("fixture-model", CHAIN_ID);
    cmd.arg("--prompt").arg("continue");
    let output = fixture.run(cmd);

    assert_unconfirmed_resume(&output, SESSION_B);
    let prompt = fs::read_to_string(&prompt_dump).unwrap();
    assert!(prompt.contains("handle: h-active"), "{prompt}");
    assert!(fixture.mailbox_rows(CHAIN_ID, false).is_empty());
    assert!(fixture.mailbox_rows(SESSION_B, false).is_empty());
    fixture.assert_default_user_paths_untouched();
}

fn owner_metadata(session_id: &str, invocation_uuid: &str) -> Value {
    json!({
        "owner_session_id": session_id,
        "owner_invocation_uuid": invocation_uuid,
        "spooler_extra": "preserve-me",
    })
}

fn running_owner_metadata(
    session_id: &str,
    invocation_uuid: &str,
    identity: &ProcessIdentity,
) -> Value {
    json!({
        "owner_session_id": session_id,
        "owner_invocation_uuid": invocation_uuid,
        "caller_chain": [{
            "pid": identity.os_pid,
            "boot_id": identity.os_boot_id,
            "starttime_ticks": identity.os_pid_starttime_ticks,
        }],
    })
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

fn assert_failure_result(
    output: &Output,
    invocation: &CompositeInvocationId,
    provider_name: &str,
    provider_session_id: &str,
    terminal_reason: &str,
) -> Value {
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("OULIPOLY_RESULT="))
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 1, "{stdout}");
    let result: Value = serde_json::from_str(lines[0]).unwrap();
    let mut keys = result
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "agent_runner_chain_id",
            "agent_runner_invocation_id",
            "error_category",
            "exit_code",
            "finished_at",
            "id",
            "provider_name",
            "provider_session_id",
            "status",
            "success",
            "terminal_reason",
        ]
    );
    assert_eq!(result["status"], "failed");
    assert_eq!(result["success"], false);
    assert_eq!(result["exit_code"], 0);
    assert_eq!(result["error_category"], terminal_reason);
    assert_eq!(result["terminal_reason"], terminal_reason);
    assert_eq!(result["id"], invocation.id);
    assert_eq!(result["agent_runner_invocation_id"], invocation.id);
    assert_eq!(result["provider_name"], provider_name);
    assert_eq!(result["provider_session_id"], provider_session_id);
    assert!(result["finished_at"].as_str().is_some());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .lines()
            .any(|line| line == terminal_reason),
        "{output:?}"
    );
    result
}

fn assert_unconfirmed_resume(output: &Output, provider_session_id: &str) -> CompositeInvocationId {
    let invocation = parse_invocation(&String::from_utf8_lossy(&output.stderr));
    assert_failure_result(
        output,
        &invocation,
        "fixture-provider",
        provider_session_id,
        "resume_completion_unconfirmed",
    );
    invocation
}

fn assert_failed_invocation(
    fixture: &Fixture,
    invocation_id: &str,
    provider_session_id: &str,
    terminal_reason: &str,
) {
    let observed = fixture
        .conn()
        .query_row(
            "SELECT status, success, exit_code, error_category, terminal_reason,
                    provider_name, provider_session_id, finished_at
             FROM invocations
             WHERE invocation_uuid = ?1",
            params![invocation_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(observed.0, "failed");
    assert_eq!(observed.1, 0);
    assert_eq!(observed.2, 0);
    assert_eq!(observed.3.as_deref(), Some(terminal_reason));
    assert_eq!(observed.4.as_deref(), Some(terminal_reason));
    assert_eq!(observed.5.as_deref(), Some("fixture-provider"));
    assert_eq!(observed.6.as_deref(), Some(provider_session_id));
    assert!(observed.7.is_some());
}

fn invocation_count(fixture: &Fixture) -> i64 {
    fixture
        .conn()
        .query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
        .unwrap()
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

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn expected_payload_path(sidecar_path: &Path, bytes: &[u8]) -> PathBuf {
    let sha256 = sha256_hex(bytes);
    sidecar_path
        .parent()
        .unwrap()
        .join("inbox-payloads")
        .join("v1")
        .join("sha256")
        .join(&sha256[..2])
        .join(sha256)
}
