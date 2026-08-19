#![cfg(unix)]

use oulipoly_runtime::executor::cli::pty_broker::{
    inject_control_envelope, render_mailbox_notification_envelope,
};
use oulipoly_state::mailbox::{
    AgentBashCompleteEnqueue, BindRuntimeGenerationRunning, CreateRuntimeGeneration, EnqueueResult,
    MailboxDb, MailboxRow, RuntimeGenerationFence, RuntimeGenerationId, RuntimeLifecycleState,
    RuntimeTerminalReason, SessionGenerationProjection,
};
use oulipoly_state::pid_identity::{
    PidIdentityDb, PidIdentityRecord, ProcessIdentity, read_live_process_identity,
};
use oulipoly_state::{InvocationStart, ProviderSessionBinding, SessionTurnIngest, StateDb};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const INVOCATION_A: &str = "11111111-1111-4111-8111-111111111111";
const LIVE_INVOCATION: &str = "22222222-2222-4222-8222-222222222222";
const SESSION_A: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const REGISTRATION_AUTHORITY_FILE: &str = "completion-registration-authority";

struct Fixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    runtime_dir: PathBuf,
    state_home: PathBuf,
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
        let runtime_dir = dir.path().join("xdg-runtime");
        let state_home = dir.path().join("xdg-state");
        let home_dir = dir.path().join("home");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        fs::create_dir_all(&config_home).unwrap();
        fs::create_dir_all(&data_home).unwrap();
        fs::create_dir_all(&runtime_dir).unwrap();
        fs::create_dir_all(&state_home).unwrap();
        fs::create_dir_all(&home_dir).unwrap();
        fs::create_dir_all(&models_dir).unwrap();
        fs::set_permissions(&runtime_dir, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            dir,
            config_home,
            data_home,
            runtime_dir,
            state_home,
            home_dir,
            app_config_dir,
            models_dir,
        }
    }

    fn sidecar_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("pid-identity.db")
    }

    fn state_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("state.db")
    }

    fn registration_authority_path(&self) -> PathBuf {
        self.dir.path().join(REGISTRATION_AUTHORITY_FILE)
    }

    fn conn(&self) -> Connection {
        let _ = StateDb::open(&self.state_path()).unwrap();
        Connection::open(self.state_path()).unwrap()
    }

    fn run(&self, mut cmd: Command) -> Output {
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env("XDG_RUNTIME_DIR", &self.runtime_dir);
        cmd.env("XDG_STATE_HOME", &self.state_home);
        cmd.env("HOME", &self.home_dir);
        cmd.env_remove("OULIPOLY_DATA_DIR");
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd.output().unwrap()
    }

    fn run_notify(&self, handle: &str, metadata: Value) -> Output {
        let artifacts = self.write_notify_artifacts(handle, metadata, 0);
        let mut command = self.register_command(handle, &artifacts);
        command.env(
            oulipoly_state::COMPLETION_REGISTRATION_AUTHORITY_ENV,
            fs::read_to_string(self.registration_authority_path()).unwrap(),
        );
        let registration = self.run(command);
        assert!(registration.status.success(), "{registration:?}");
        self.run(self.notify_command(handle, &artifacts))
    }

    fn register_command(&self, handle: &str, artifacts: &NotifyArtifacts) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("notify")
            .arg("agent-bash-register")
            .arg("--handle")
            .arg(handle)
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
            .arg("--json");
        cmd
    }

    fn notify_command(&self, handle: &str, artifacts: &NotifyArtifacts) -> Command {
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
        cmd
    }

    fn base_repl_command(&self, model_name: &str, session_id: &str) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("repl")
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg("--resume")
            .arg(session_id)
            .arg(model_name);
        self.configure_env(&mut cmd);
        cmd.current_dir(self.dir.path());
        cmd
    }

    fn configure_env(&self, cmd: &mut Command) {
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env("XDG_RUNTIME_DIR", &self.runtime_dir);
        cmd.env("XDG_STATE_HOME", &self.state_home);
        cmd.env("HOME", &self.home_dir);
        cmd.env_remove("OULIPOLY_DATA_DIR");
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
    }

    fn write_notify_artifacts(&self, handle: &str, metadata: Value, rc: i32) -> NotifyArtifacts {
        let state_dir = self.dir.path().join(format!("notify-{handle}"));
        fs::create_dir_all(&state_dir).unwrap();
        let meta = state_dir.join("meta.json");
        let log = state_dir.join("log");
        let rc_path = state_dir.join("rc");
        fs::write(&meta, serde_json::to_string_pretty(&metadata).unwrap()).unwrap();
        fs::write(&log, format!("log for {handle}\n")).unwrap();
        fs::write(&rc_path, format!("{rc}\n")).unwrap();
        NotifyArtifacts {
            state_dir,
            meta,
            log,
            rc: rc_path,
        }
    }

    fn record_owner_identity(&self, identity: &ProcessIdentity) {
        let state = StateDb::open(&self.state_path()).unwrap();
        let started = state
            .start_invocation_with_completion_registration_authority(&InvocationStart {
                invocation_uuid: INVOCATION_A.to_string(),
                model_name: "fixture-model".to_string(),
                provider_name: "fixture-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        fs::write(
            self.registration_authority_path(),
            started
                .completion_registration_authority
                .process_environment_value(),
        )
        .unwrap();
        state
            .bind_invocation_provider_session_start(
                started.invocation_row_id,
                &ProviderSessionBinding {
                    provider_session_id: SESSION_A.to_string(),
                    capture_method: "fixture",
                    resume_input_id: None,
                    provider_session_resolved_account: None,
                },
            )
            .unwrap();
        let sidecar = PidIdentityDb::open(&self.sidecar_path()).unwrap();
        sidecar
            .record_identity(PidIdentityRecord {
                identity,
                os_pgid: None,
                invocation_uuid: INVOCATION_A,
                session_id: Some(SESSION_A),
                provider_name: Some("fixture-provider"),
                model_name: Some("fixture-model"),
                recorded_at: "2026-06-04T12:00:00Z",
            })
            .unwrap();
    }

    fn mark_live_pty_runtime(&self, identity: &ProcessIdentity, control_path: &Path) {
        let mut mailbox = MailboxDb::open(&self.sidecar_path()).unwrap();
        let generation_id = RuntimeGenerationId::parse(LIVE_INVOCATION).unwrap();
        mailbox
            .runtime_lifecycle()
            .create_runtime_generation(CreateRuntimeGeneration {
                generation_id: &generation_id,
                spawn_invocation_uuid: INVOCATION_A,
                session_id: Some(SESSION_A),
                runtime_mode: "pty_interactive",
                provider_name: "fixture-provider",
                model_name: Some("fixture-model"),
                pty_control_path: Some(&path_string(control_path)),
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();
        mailbox
            .runtime_lifecycle()
            .bind_runtime_generation_running(BindRuntimeGenerationRunning {
                fence: RuntimeGenerationFence {
                    generation_id: &generation_id,
                    spawn_invocation_uuid: INVOCATION_A,
                },
                spawned_os_pid: identity.os_pid,
                exact_process_identity: Some(identity),
                os_pgid: None,
            })
            .unwrap();
    }

    fn mailbox(&self) -> MailboxDb {
        MailboxDb::open(&self.sidecar_path()).unwrap()
    }

    fn seed_mailbox(&self, handle: &str) -> MailboxRow {
        let artifacts = self.write_notify_artifacts(handle, json!({"caller_chain": []}), 0);
        let mut db = self.mailbox();
        match db
            .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
                session_id: SESSION_A,
                handle,
                payload_json: r#"{"schema_version":1,"kind":"agent_bash_complete"}"#,
                owner_invocation_uuid: Some(INVOCATION_A),
                matched_os_pid: Some(9000),
                matched_os_boot_id: Some("boot-mailbox"),
                matched_os_pid_starttime_ticks: Some(1),
                matched_chain_index: Some(0),
                state_dir: &path_string(&artifacts.state_dir),
                meta_path: &path_string(&artifacts.meta),
                log_path: &path_string(&artifacts.log),
                rc_path: &path_string(&artifacts.rc),
                rc: 0,
            })
            .unwrap()
        {
            EnqueueResult::Inserted(row) => row,
            other => panic!("expected inserted mailbox row, got {other:?}"),
        }
    }

    fn socket_path(&self, name: &str) -> PathBuf {
        let dir = self.runtime_dir.join("oulipoly-agent-runner/pty");
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        dir.join(name)
    }

    fn notify_trace_path(&self) -> PathBuf {
        self.state_home
            .join("oulipoly-agent-runner")
            .join("notify-trace.log")
    }

    fn write_interactive_model(&self, model_name: &str, provider_name: &str, script: &Path) {
        fs::write(
            self.models_dir.join(format!("{model_name}.toml")),
            format!(
                r#"[[providers]]
name = "{provider_name}"
args = []
"#,
            ),
        )
        .unwrap();
        fs::write(
            self.app_config_dir.join("providers.toml"),
            format!(
                r#"[{provider_name}]
command = {}
args = []
interactive_args = []
prompt_mode = "arg"

[{provider_name}.resume]
kind = "flag"
flag = "--resume"
"#,
                toml_string(&path_string(script))
            ),
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

    fn ingest_turn(&self, provider: &str, session_id: &str, turn_id: &str, role: &str, body: &str) {
        let state = StateDb::open(&self.state_path()).unwrap();
        state
            .ingest_session_turns_batch(
                provider,
                &[SessionTurnIngest {
                    session_id: session_id.to_string(),
                    turn_id: turn_id.to_string(),
                    timestamp: chrono::Utc::now(),
                    role: role.to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: false,
                    body: Some(body.to_string()),
                }],
            )
            .unwrap();
    }

    fn assert_default_user_paths_untouched(&self) {
        assert!(
            !self
                .home_dir
                .join(".local/share/oulipoly-agent-runner")
                .exists()
        );
        assert!(!self.home_dir.join(".config/oulipoly-agent-runner").exists());
    }
}

#[test]
fn notify_control_ack_immediately_delivers_without_provider_observation() {
    let fixture = Fixture::new();
    let identity = current_identity();
    fixture.record_owner_identity(&identity);
    let socket = fixture.socket_path("ack.sock");
    let captured = Arc::new(Mutex::new(String::new()));
    let server = spawn_control_server(&socket, true, "$delivery_nonce", Arc::clone(&captured));
    fixture.mark_live_pty_runtime(&identity, &socket);

    let output = fixture.run_notify("h-live-ack", owner_metadata(SESSION_A, INVOCATION_A));
    server.join().unwrap();

    assert_success(&output);
    let value = stdout_json(&output);
    assert_eq!(value["status"], "triggered");
    assert_eq!(value["pty_delivery"]["status"], "acked");
    assert_eq!(value["pty_delivery"]["submitted"], true);
    assert_eq!(value["pty_delivery"]["delivered_seqs"], json!([1]));
    assert!(value["wake"].is_null());
    let payload = captured.lock().unwrap().clone();
    assert!(payload.contains("[OULIPOLY NOTIFICATIONS]"));
    assert!(payload.contains("handle: h-live-ack"));
    assert!(!payload.contains("log for h-live-ack"));
    let trace = fs::read_to_string(fixture.notify_trace_path()).unwrap();
    assert!(
        trace.contains("trigger=completion-event"),
        "trace was {trace}"
    );
    assert!(trace.contains("decision=inject"), "trace was {trace}");
    assert!(trace.contains("inject_status=acked"), "trace was {trace}");

    let rows = fixture.mailbox().list_mailbox(SESSION_A, true).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].delivered_at.is_some());
    assert_eq!(
        rows[0].delivered_by_invocation_uuid.as_deref(),
        Some(INVOCATION_A)
    );
    assert_eq!(rows[0].delivery_attempts, 1);
    assert!(
        fixture
            .mailbox()
            .list_pending(SESSION_A)
            .unwrap()
            .is_empty()
    );
    let attempt_id = delivery_attempt_id(&payload);
    let attempt = fixture
        .mailbox()
        .delivery_attempt_window(&attempt_id)
        .unwrap()
        .unwrap();
    assert!(attempt.acknowledged_at.is_some());
    let resolved_at: Option<String> = Connection::open(fixture.sidecar_path())
        .unwrap()
        .query_row(
            "SELECT resolved_at FROM mailbox_delivery_attempts WHERE attempt_id = ?1",
            params![attempt_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(resolved_at.is_some());
    let listeners = fixture
        .mailbox()
        .completion_event_listeners("h-live-ack")
        .unwrap();
    assert_eq!(listeners.len(), 1);
    assert!(listeners[0].acknowledged_at.is_some());
    assert_eq!(
        listeners[0].acknowledgement_reason.as_deref(),
        Some("injected")
    );
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn notify_live_pty_generic_ack_is_not_delivery_evidence() {
    let fixture = Fixture::new();
    let identity = current_identity();
    fixture.record_owner_identity(&identity);
    let socket = fixture.socket_path("generic-ack.sock");
    let captured = Arc::new(Mutex::new(String::new()));
    let server = spawn_control_server(&socket, true, "ok", Arc::clone(&captured));
    fixture.mark_live_pty_runtime(&identity, &socket);

    let output = fixture.run_notify("h-generic-ack", owner_metadata(SESSION_A, INVOCATION_A));
    server.join().unwrap();

    assert_success(&output);
    let value = stdout_json(&output);
    assert_eq!(value["pty_delivery"]["status"], "unconfirmed_ack");
    assert_eq!(value["pty_delivery"]["submitted"], false);
    assert_eq!(value["pty_delivery"]["delivered_seqs"], json!([]));
    assert_eq!(value["wake"]["status"], "busy");
    let rows = fixture.mailbox().list_mailbox(SESSION_A, true).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].delivered_at.is_none());
    assert_eq!(rows[0].delivery_attempts, 1);
    let generation = fixture
        .mailbox()
        .runtime_lifecycle_reader()
        .runtime_generation(&RuntimeGenerationId::parse(LIVE_INVOCATION).unwrap())
        .unwrap()
        .unwrap();
    assert!(generation.active_delivery_claim_id.is_none());
}

#[test]
fn notify_live_pty_nack_leaves_pending_and_reports_busy_wake() {
    let fixture = Fixture::new();
    let identity = current_identity();
    fixture.record_owner_identity(&identity);
    let socket = fixture.socket_path("nack.sock");
    let captured = Arc::new(Mutex::new(String::new()));
    let server = spawn_control_server(&socket, false, "broker_rejected", Arc::clone(&captured));
    fixture.mark_live_pty_runtime(&identity, &socket);

    let output = fixture.run_notify("h-nack", owner_metadata(SESSION_A, INVOCATION_A));
    server.join().unwrap();

    assert_success(&output);
    let value = stdout_json(&output);
    assert_eq!(value["pty_delivery"]["status"], "protocol_error");
    assert_eq!(value["pty_delivery"]["submitted"], false);
    assert_eq!(value["wake"]["status"], "busy");
    let trace = fs::read_to_string(fixture.notify_trace_path()).unwrap();
    assert!(
        trace.contains("decision=skip-protocol_error"),
        "trace was {trace}"
    );
    assert!(
        trace.contains("inject_status=protocol_error"),
        "trace was {trace}"
    );
    let rows = fixture.mailbox().list_mailbox(SESSION_A, true).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].delivered_at.is_none());
    assert_eq!(unresolved_delivery_attempt_count(&fixture), 0);
    assert!(captured.lock().unwrap().contains("h-nack"));
    let listeners = fixture
        .mailbox()
        .completion_event_listeners("h-nack")
        .unwrap();
    assert_eq!(listeners.len(), 1);
    assert!(listeners[0].acknowledged_at.is_none());
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn notify_stale_socket_cleans_runtime_and_does_not_report_busy() {
    let fixture = Fixture::new();
    let owner_identity = current_identity();
    fixture.record_owner_identity(&owner_identity);
    let stale_identity = ProcessIdentity {
        os_pid: 9_999_999,
        os_boot_id: "stale-boot".to_string(),
        os_pid_starttime_ticks: 1,
    };
    let stale_socket = fixture.socket_path("stale.sock");
    fs::write(&stale_socket, "stale").unwrap();
    fixture.mark_live_pty_runtime(&stale_identity, &stale_socket);

    let output = fixture.run_notify("h-stale", owner_metadata(SESSION_A, INVOCATION_A));

    assert_success(&output);
    let value = stdout_json(&output);
    assert_eq!(value["pty_delivery"]["status"], "stale_generation");
    assert_eq!(value["pty_delivery"]["submitted"], false);
    assert_eq!(value["wake"]["status"], "spawned");
    let projection = fixture
        .mailbox()
        .wake_session_reader()
        .legacy_runtime_projection(SESSION_A)
        .unwrap()
        .unwrap();
    assert_eq!(projection.run_state, "idle");
    assert!(projection.pty_control_path.is_none());
    assert!(!stale_socket.exists());
    assert_eq!(unresolved_delivery_attempt_count(&fixture), 0);
    let rows = fixture.mailbox().list_pending(SESSION_A).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].handle, "h-stale");
    assert_eq!(rows[0].delivery_attempts, 0);
    let trace = fs::read_to_string(fixture.notify_trace_path()).unwrap();
    assert!(
        trace.contains("decision=skip-connect_error"),
        "trace was {trace}"
    );
    assert!(
        trace.contains("inject_status=connect_error"),
        "trace was {trace}"
    );
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn fixture_interactive_session_agent_bash_completion_arrives_live() {
    let fixture = Fixture::new();
    let received_log = fixture.dir.path().join("received.log");
    let script = fixture_provider_waiting_for_notification(fixture.dir.path(), &received_log);
    fixture.write_interactive_model("fixture", "fixture-provider", &script);
    fixture.seed_active_chain(
        "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        "fixture-provider",
        SESSION_A,
        "fixture",
    );
    let pty = OuterPty::open(30, 100);
    let mut repl = spawn_repl_under_pty(&fixture, &pty, "fixture", SESSION_A);
    let startup = read_until(
        pty.master.as_raw_fd(),
        "READY_FOR_NOTIFY",
        Duration::from_secs(5),
    );
    assert!(
        startup.contains("READY_FOR_NOTIFY"),
        "startup was {startup:?}"
    );
    let invocation_uuid = wait_for_running_invocation(&fixture);
    let _child_identity = wait_for_child_identity(&fixture, &invocation_uuid);

    let output = fixture.run_notify("h-e2e-live", owner_metadata(SESSION_A, &invocation_uuid));
    assert_success(&output);
    let value = stdout_json(&output);
    assert_eq!(value["pty_delivery"]["status"], "acked");
    assert_eq!(value["pty_delivery"]["submitted"], true);
    assert!(value["wake"].is_null());

    let output = read_until(pty.master.as_raw_fd(), "GOT_NOTIFY", Duration::from_secs(5));
    assert!(output.contains("GOT_NOTIFY"), "output was {output:?}");
    assert!(
        repl.try_wait().unwrap().is_none(),
        "interactive process should still be live when notification arrives"
    );
    let received = fs::read_to_string(&received_log).unwrap();
    assert!(received.contains("[OULIPOLY NOTIFICATIONS]"));
    assert!(received.contains("handle: h-e2e-live"));
    assert!(!received.contains("log for h-e2e-live"));
    fixture.ingest_turn(
        "fixture-provider",
        SESSION_A,
        "live-notification-turn",
        "user",
        &format!("[OULIPOLY-DELIVERY {}]", delivery_attempt_id(&received)),
    );
    assert!(repl.wait().unwrap().success());
    let projection = fixture
        .mailbox()
        .wake_session_reader()
        .legacy_runtime_projection(SESSION_A)
        .unwrap()
        .unwrap();
    assert_eq!(projection.run_state, "idle");
    assert!(projection.pty_control_path.is_none());
    let history = fixture
        .mailbox()
        .runtime_lifecycle_reader()
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
fn observed_tui_delivers_live_notification_during_continuous_child_redraw() {
    let fixture = Fixture::new();
    let received_log = fixture.dir.path().join("continuous-redraw-received.log");
    let script = fixture_provider_with_continuous_redraw(fixture.dir.path(), &received_log);
    fixture.write_interactive_model("fixture-continuous-redraw", "fixture-provider", &script);
    fixture.seed_active_chain(
        "ffffffff-ffff-4fff-8fff-ffffffffffff",
        "fixture-provider",
        SESSION_A,
        "fixture-continuous-redraw",
    );
    let pty = OuterPty::open(30, 100);
    let mut repl = spawn_repl_under_pty(&fixture, &pty, "fixture-continuous-redraw", SESSION_A);
    let startup = read_until(
        pty.master.as_raw_fd(),
        "READY_FOR_NOTIFY",
        Duration::from_secs(5),
    );
    assert!(
        startup.contains("READY_FOR_NOTIFY"),
        "startup was {startup:?}"
    );
    let invocation_uuid = wait_for_running_invocation(&fixture);
    let _child_identity = wait_for_child_identity(&fixture, &invocation_uuid);

    let notify = fixture.run_notify(
        "h-continuous-redraw",
        owner_metadata(SESSION_A, &invocation_uuid),
    );
    let diagnostic = stdout_json(&notify);
    if diagnostic["pty_delivery"]["status"] != "acked" {
        let _ = repl.kill();
        let _ = repl.wait();
        panic!("continuous-redraw delivery was not accepted: {diagnostic}");
    }
    let output = read_until(pty.master.as_raw_fd(), "GOT_NOTIFY", Duration::from_secs(5));
    assert!(output.contains("GOT_NOTIFY"), "output was {output:?}");
    let received = fs::read_to_string(&received_log).unwrap();
    assert!(
        received.contains("handle: h-continuous-redraw"),
        "{received}"
    );
    fixture.ingest_turn(
        "fixture-provider",
        SESSION_A,
        "continuous-redraw-notification-turn",
        "user",
        &format!("[OULIPOLY-DELIVERY {}]", delivery_attempt_id(&received)),
    );
    assert!(repl.wait().unwrap().success());
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn resumed_repl_retries_pending_mailbox_after_replacing_stale_runtime() {
    let fixture = Fixture::new();
    let received_log = fixture.dir.path().join("resumed-pending-received.log");
    let script = fixture_provider_waiting_for_notification(fixture.dir.path(), &received_log);
    fixture.write_interactive_model("fixture-resumed-pending", "fixture-provider", &script);
    fixture.seed_active_chain(
        "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee",
        "fixture-provider",
        SESSION_A,
        "fixture-resumed-pending",
    );
    fixture.ingest_turn(
        "fixture-provider",
        SESSION_A,
        "pre-reboot-assistant-turn",
        "assistant",
        "durable work before reboot",
    );
    let stale_identity = ProcessIdentity {
        os_pid: 999_999_001,
        os_boot_id: "pre-reboot-boot-id".to_string(),
        os_pid_starttime_ticks: 1,
    };
    fixture.mark_live_pty_runtime(
        &stale_identity,
        &fixture.socket_path("pre-reboot-stale.sock"),
    );
    fixture.seed_mailbox("h-pre-reboot-pending");

    let pty = OuterPty::open(30, 100);
    let mut repl = spawn_repl_under_pty(&fixture, &pty, "fixture-resumed-pending", SESSION_A);
    let startup = read_until(
        pty.master.as_raw_fd(),
        "READY_FOR_NOTIFY",
        Duration::from_secs(5),
    );
    assert!(
        startup.contains("READY_FOR_NOTIFY"),
        "startup was {startup:?}"
    );

    let output = read_until(pty.master.as_raw_fd(), "GOT_NOTIFY", Duration::from_secs(6));
    if !output.contains("GOT_NOTIFY") {
        let _ = repl.kill();
        let _ = repl.wait();
        panic!("resumed REPL did not retry its pending mailbox: {output:?}");
    }
    let received = fs::read_to_string(&received_log).unwrap();
    assert!(
        received.contains("handle: h-pre-reboot-pending"),
        "{received}"
    );
    fixture.ingest_turn(
        "fixture-provider",
        SESSION_A,
        "resumed-pending-notification-turn",
        "user",
        &format!("[OULIPOLY-DELIVERY {}]", delivery_attempt_id(&received)),
    );
    assert!(repl.wait().unwrap().success());
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn live_broker_confirmation_contracts_overlapping_attempts() {
    let fixture = Fixture::new();
    let received_log = fixture.dir.path().join("overlap-received.log");
    let script = fixture_provider_waiting_for_notification(fixture.dir.path(), &received_log);
    fixture.write_interactive_model("fixture-overlap", "fixture-provider", &script);
    fixture.seed_active_chain(
        "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
        "fixture-provider",
        SESSION_A,
        "fixture-overlap",
    );
    let pty = OuterPty::open(30, 100);
    let mut repl = spawn_repl_under_pty(&fixture, &pty, "fixture-overlap", SESSION_A);
    let startup = read_until(
        pty.master.as_raw_fd(),
        "READY_FOR_NOTIFY",
        Duration::from_secs(5),
    );
    assert!(
        startup.contains("READY_FOR_NOTIFY"),
        "startup was {startup:?}"
    );
    let invocation_uuid = wait_for_running_invocation(&fixture);
    let control_path = running_control_path(&fixture);
    let rows = (1..=6)
        .map(|index| fixture.seed_mailbox(&format!("h-overlap-{index}")))
        .collect::<Vec<_>>();
    let first_seqs = rows[..3].iter().map(|row| row.seq).collect::<Vec<_>>();
    let all_seqs = rows.iter().map(|row| row.seq).collect::<Vec<_>>();
    let mut mailbox = fixture.mailbox();
    mailbox
        .register_delivery_attempt(
            "overlap-attempt-1",
            SESSION_A,
            &invocation_uuid,
            &first_seqs,
            3,
        )
        .unwrap();
    mailbox
        .register_delivery_attempt(
            "overlap-attempt-2",
            SESSION_A,
            &invocation_uuid,
            &all_seqs,
            0,
        )
        .unwrap();
    mailbox
        .record_delivery_attempt_transport_ack("overlap-attempt-1")
        .unwrap();
    fixture.ingest_turn(
        "fixture-provider",
        SESSION_A,
        "overlap-first-confirmation",
        "user",
        "[OULIPOLY-DELIVERY overlap-attempt-1]",
    );
    mailbox
        .confirm_delivery_attempt("overlap-attempt-1")
        .unwrap();
    drop(mailbox);

    let duplicate = render_mailbox_notification_envelope(&rows[..3], 3, "overlap-attempt-1");
    let duplicate_response = inject_control_envelope(&control_path, &duplicate).unwrap();
    assert!(duplicate_response.ack, "{duplicate_response:?}");
    let duplicate_output = read_until(
        pty.master.as_raw_fd(),
        "GOT_NOTIFY",
        Duration::from_millis(250),
    );
    assert!(
        !duplicate_output.contains("GOT_NOTIFY"),
        "resolved duplicate was injected: {duplicate_output:?}"
    );

    let overlap = render_mailbox_notification_envelope(&rows, 0, "overlap-attempt-2");
    let overlap_response = inject_control_envelope(&control_path, &overlap).unwrap();
    assert!(overlap_response.ack, "{overlap_response:?}");
    let output = read_until(pty.master.as_raw_fd(), "GOT_NOTIFY", Duration::from_secs(5));
    assert!(output.contains("GOT_NOTIFY"), "output was {output:?}");
    fixture.ingest_turn(
        "fixture-provider",
        SESSION_A,
        "overlap-second-confirmation",
        "user",
        "[OULIPOLY-DELIVERY overlap-attempt-2]",
    );
    assert!(repl.wait().unwrap().success());

    let received = fs::read_to_string(&received_log).unwrap();
    for index in 1..=3 {
        assert!(
            !received.contains(&format!("handle: h-overlap-{index}")),
            "{received}"
        );
    }
    for index in 4..=6 {
        assert!(
            received.contains(&format!("handle: h-overlap-{index}")),
            "{received}"
        );
    }
    assert!(
        fixture
            .mailbox()
            .list_pending(SESSION_A)
            .unwrap()
            .is_empty()
    );
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn live_broker_rejects_attempt_resolved_before_socket_acceptance() {
    let fixture = Fixture::new();
    let received_log = fixture.dir.path().join("resolved-request-received.log");
    let script = fixture_provider_waiting_for_notification(fixture.dir.path(), &received_log);
    fixture.write_interactive_model("fixture-resolved-request", "fixture-provider", &script);
    fixture.seed_active_chain(
        "abababab-abab-4bab-8bab-abababababab",
        "fixture-provider",
        SESSION_A,
        "fixture-resolved-request",
    );
    let pty = OuterPty::open(30, 100);
    let mut repl = spawn_repl_under_pty(&fixture, &pty, "fixture-resolved-request", SESSION_A);
    let startup = read_until(
        pty.master.as_raw_fd(),
        "READY_FOR_NOTIFY",
        Duration::from_secs(5),
    );
    assert!(
        startup.contains("READY_FOR_NOTIFY"),
        "startup was {startup:?}"
    );
    let invocation_uuid = wait_for_running_invocation(&fixture);
    let control_path = running_control_path(&fixture);
    let row = fixture.seed_mailbox("h-resolved-request");
    let mut mailbox = fixture.mailbox();
    mailbox
        .register_delivery_attempt(
            "resolved-before-accept",
            SESSION_A,
            &invocation_uuid,
            &[row.seq],
            0,
        )
        .unwrap();
    mailbox
        .resolve_unacknowledged_delivery_attempt("resolved-before-accept")
        .unwrap();
    drop(mailbox);

    let stale = render_mailbox_notification_envelope(
        std::slice::from_ref(&row),
        0,
        "resolved-before-accept",
    );
    let stale_response = inject_control_envelope(&control_path, &stale).unwrap();
    assert!(!stale_response.ack, "{stale_response:?}");
    assert_eq!(stale_response.message, "mailbox_delivery_stale");
    let stale_output = read_until(
        pty.master.as_raw_fd(),
        "GOT_NOTIFY",
        Duration::from_millis(250),
    );
    assert!(!stale_output.contains("GOT_NOTIFY"), "{stale_output:?}");

    fixture
        .mailbox()
        .register_delivery_attempt(
            "fresh-after-stale",
            SESSION_A,
            &invocation_uuid,
            &[row.seq],
            0,
        )
        .unwrap();
    let fresh = render_mailbox_notification_envelope(&[row], 0, "fresh-after-stale");
    let fresh_response = inject_control_envelope(&control_path, &fresh).unwrap();
    assert!(fresh_response.ack, "{fresh_response:?}");
    let fresh_output = read_until(pty.master.as_raw_fd(), "GOT_NOTIFY", Duration::from_secs(5));
    assert!(fresh_output.contains("GOT_NOTIFY"), "{fresh_output:?}");
    fixture.ingest_turn(
        "fixture-provider",
        SESSION_A,
        "fresh-after-stale-turn",
        "user",
        "[OULIPOLY-DELIVERY fresh-after-stale]",
    );
    assert!(repl.wait().unwrap().success());
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn live_broker_contracts_registered_overlap_after_each_injection() {
    let fixture = Fixture::new();
    let received_log = fixture.dir.path().join("concurrent-overlap-received.log");
    let script = fixture_provider_waiting_for_two_notifications(fixture.dir.path(), &received_log);
    fixture.write_interactive_model("fixture-concurrent-overlap", "fixture-provider", &script);
    fixture.seed_active_chain(
        "ffffffff-ffff-4fff-8fff-ffffffffffff",
        "fixture-provider",
        SESSION_A,
        "fixture-concurrent-overlap",
    );
    let pty = OuterPty::open(30, 100);
    let mut repl = spawn_repl_under_pty(&fixture, &pty, "fixture-concurrent-overlap", SESSION_A);
    let startup = read_until(
        pty.master.as_raw_fd(),
        "READY_FOR_NOTIFY",
        Duration::from_secs(5),
    );
    assert!(
        startup.contains("READY_FOR_NOTIFY"),
        "startup was {startup:?}"
    );
    let invocation_uuid = wait_for_running_invocation(&fixture);
    let control_path = running_control_path(&fixture);
    let rows = (1..=6)
        .map(|index| fixture.seed_mailbox(&format!("h-concurrent-overlap-{index}")))
        .collect::<Vec<_>>();
    let first_seqs = rows[..3].iter().map(|row| row.seq).collect::<Vec<_>>();
    let all_seqs = rows.iter().map(|row| row.seq).collect::<Vec<_>>();
    let mut mailbox = fixture.mailbox();
    mailbox
        .register_delivery_attempt(
            "concurrent-overlap-attempt-1",
            SESSION_A,
            &invocation_uuid,
            &first_seqs,
            3,
        )
        .unwrap();
    mailbox
        .register_delivery_attempt(
            "concurrent-overlap-attempt-2",
            SESSION_A,
            &invocation_uuid,
            &all_seqs,
            0,
        )
        .unwrap();
    drop(mailbox);

    let first = render_mailbox_notification_envelope(&rows[..3], 3, "concurrent-overlap-attempt-1");
    let first_response = inject_control_envelope(&control_path, &first).unwrap();
    assert!(first_response.ack, "{first_response:?}");
    wait_for_file_contains(
        &received_log,
        "[OULIPOLY-DELIVERY concurrent-overlap-attempt-1]",
        Duration::from_secs(5),
    );

    let overlapping =
        render_mailbox_notification_envelope(&rows, 0, "concurrent-overlap-attempt-2");
    let second_response = inject_control_envelope(&control_path, &overlapping).unwrap();
    assert!(second_response.ack, "{second_response:?}");
    let second_received = wait_for_file_contains(
        &received_log,
        "handle: h-concurrent-overlap-6",
        Duration::from_secs(5),
    );
    assert!(
        second_received.contains("handle: h-concurrent-overlap-6"),
        "{second_received}"
    );
    let second_window = fixture
        .mailbox()
        .delivery_attempt_window("concurrent-overlap-attempt-2")
        .unwrap()
        .unwrap();
    assert!(second_window.acknowledged_at.is_some());
    assert!(second_window.resolved_at.is_some());
    assert!(repl.wait().unwrap().success());

    let received = fs::read_to_string(&received_log).unwrap();
    for index in 1..=6 {
        assert_eq!(
            received
                .matches(&format!("handle: h-concurrent-overlap-{index}"))
                .count(),
            1,
            "{received}"
        );
    }
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn live_broker_transport_ack_completes_delivery_when_response_is_unread() {
    let fixture = Fixture::new();
    let received_log = fixture.dir.path().join("lost-ack-received.log");
    let script = fixture_provider_waiting_for_notification(fixture.dir.path(), &received_log);
    fixture.write_interactive_model("fixture-lost-ack", "fixture-provider", &script);
    fixture.seed_active_chain(
        "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
        "fixture-provider",
        SESSION_A,
        "fixture-lost-ack",
    );
    let pty = OuterPty::open(30, 100);
    let mut repl = spawn_repl_under_pty(&fixture, &pty, "fixture-lost-ack", SESSION_A);
    let startup = read_until(
        pty.master.as_raw_fd(),
        "READY_FOR_NOTIFY",
        Duration::from_secs(5),
    );
    assert!(
        startup.contains("READY_FOR_NOTIFY"),
        "startup was {startup:?}"
    );
    let invocation_uuid = wait_for_running_invocation(&fixture);
    let control_path = running_control_path(&fixture);
    let row = fixture.seed_mailbox("h-lost-ack");
    let mut mailbox = fixture.mailbox();
    mailbox
        .register_delivery_attempt(
            "lost-ack-attempt",
            SESSION_A,
            &invocation_uuid,
            &[row.seq],
            0,
        )
        .unwrap();
    drop(mailbox);
    let envelope = render_mailbox_notification_envelope(&[row], 0, "lost-ack-attempt");

    let mut stream = UnixStream::connect(&control_path).unwrap();
    write_inject_request(&mut stream, envelope.as_bytes());
    drop(stream);

    let output = read_until(pty.master.as_raw_fd(), "GOT_NOTIFY", Duration::from_secs(5));
    assert!(output.contains("GOT_NOTIFY"), "output was {output:?}");
    let rows = fixture.mailbox().list_mailbox(SESSION_A, true).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].delivered_at.is_some());
    assert_eq!(
        rows[0].delivered_by_invocation_uuid.as_deref(),
        Some(invocation_uuid.as_str())
    );
    assert_eq!(rows[0].delivery_attempts, 1);
    let attempt = fixture
        .mailbox()
        .delivery_attempt_window("lost-ack-attempt")
        .unwrap()
        .unwrap();
    assert!(attempt.acknowledged_at.is_some());
    assert!(attempt.resolved_at.is_some());
    assert!(repl.wait().unwrap().success());
    assert!(
        fs::read_to_string(&received_log)
            .unwrap()
            .contains("handle: h-lost-ack")
    );
    fixture.assert_default_user_paths_untouched();
}

fn spawn_control_server(
    path: &Path,
    ack: bool,
    message: &'static str,
    captured: Arc<Mutex<String>>,
) -> thread::JoinHandle<()> {
    let listener = UnixListener::bind(path).unwrap();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let payload = read_inject_payload(&mut stream);
        let response_message = if message == "$delivery_nonce" {
            format!("delivery_ack:{}", delivery_nonce_from_payload(&payload))
        } else {
            message.to_string()
        };
        *captured.lock().unwrap() = payload;
        write_response(&mut stream, ack, &response_message);
    })
}

fn delivery_nonce_from_payload(payload: &str) -> &str {
    let start = payload.rfind("[OULIPOLY-DELIVERY ").unwrap() + "[OULIPOLY-DELIVERY ".len();
    let tail = &payload[start..];
    &tail[..tail.find(']').unwrap()]
}

struct OuterPty {
    master: File,
    slave: File,
}

impl OuterPty {
    fn open(rows: u16, cols: u16) -> Self {
        let mut master_fd = -1;
        let mut slave_fd = -1;
        let winsize = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let rc = unsafe {
            libc::openpty(
                &mut master_fd,
                &mut slave_fd,
                std::ptr::null_mut(),
                std::ptr::null(),
                &winsize,
            )
        };
        assert_eq!(rc, 0, "openpty failed: {}", io::Error::last_os_error());
        Self {
            master: unsafe { File::from_raw_fd(master_fd) },
            slave: unsafe { File::from_raw_fd(slave_fd) },
        }
    }
}

fn spawn_repl_under_pty(
    fixture: &Fixture,
    pty: &OuterPty,
    model_name: &str,
    session_id: &str,
) -> Child {
    let stdin = pty.slave.try_clone().unwrap();
    let stdout = pty.slave.try_clone().unwrap();
    let stderr = pty.slave.try_clone().unwrap();
    let slave_fd = pty.slave.as_raw_fd();
    let master_fd = pty.master.as_raw_fd();
    let mut cmd = fixture.base_repl_command(model_name, session_id);
    cmd.stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    unsafe {
        cmd.pre_exec(move || {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(slave_fd, libc::TIOCSCTTY, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            if libc::tcsetpgrp(slave_fd, libc::getpid()) == -1 {
                return Err(io::Error::last_os_error());
            }
            if master_fd > 2 {
                libc::close(master_fd);
            }
            Ok(())
        });
    }
    cmd.spawn().unwrap()
}

fn fixture_provider_waiting_for_notification(dir: &Path, received_log: &Path) -> PathBuf {
    let path = dir.join("fixture-live-provider.sh");
    fs::write(
        &path,
        format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
: > {received}
printf '%s' "${{OULIPOLY_COMPLETION_REGISTRATION_AUTHORITY-}}" > {authority}
test -t 0
test -t 1
test -t 2
printf '\033[?2004hREADY_FOR_NOTIFY\n'
while IFS= read -r line; do
  line="${{line//$'\033[200~'/}}"
  line="${{line//$'\033[201~'/}}"
  printf '%s\n' "$line" >> {received}
  if [ "$line" = "[END OULIPOLY NOTIFICATIONS]" ]; then
    printf 'GOT_NOTIFY\n'
    sleep 1
    exit 0
  fi
done
"#,
            received = shell_single_quote(&path_string(received_log)),
            authority = shell_single_quote(&path_string(&dir.join(REGISTRATION_AUTHORITY_FILE))),
        ),
    )
    .unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    path
}

fn fixture_provider_with_continuous_redraw(dir: &Path, received_log: &Path) -> PathBuf {
    let path = dir.join("fixture-continuous-redraw-provider.sh");
    fs::write(
        &path,
        format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
: > {received}
printf '%s' "${{OULIPOLY_COMPLETION_REGISTRATION_AUTHORITY-}}" > {authority}
test -t 0
test -t 1
test -t 2
printf '\033[?2004hREADY_FOR_NOTIFY\n'
while true; do
  if IFS= read -r -t 0.05 line; then
    line="${{line//$'\033[200~'/}}"
    line="${{line//$'\033[201~'/}}"
    printf '%s\n' "$line" >> {received}
    if [ "$line" = "[END OULIPOLY NOTIFICATIONS]" ]; then
      printf 'GOT_NOTIFY\n'
      sleep 1
      exit 0
    fi
  else
    printf '\033[2;1H'
  fi
done
"#,
            received = shell_single_quote(&path_string(received_log)),
            authority = shell_single_quote(&path_string(&dir.join(REGISTRATION_AUTHORITY_FILE))),
        ),
    )
    .unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    path
}

fn fixture_provider_waiting_for_two_notifications(dir: &Path, received_log: &Path) -> PathBuf {
    let path = dir.join("fixture-two-notifications-provider.sh");
    fs::write(
        &path,
        format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
: > {received}
test -t 0
test -t 1
test -t 2
printf '\033[?2004hREADY_FOR_NOTIFY\n'
count=0
while IFS= read -r line; do
  line="${{line//$'\033[200~'/}}"
  line="${{line//$'\033[201~'/}}"
  printf '%s\n' "$line" >> {received}
  if [ "$line" = "[END OULIPOLY NOTIFICATIONS]" ]; then
    count=$((count + 1))
    printf 'GOT_NOTIFY_%s\n' "$count"
    if [ "$count" -eq 2 ]; then
      sleep 1
      exit 0
    fi
  fi
done
"#,
            received = shell_single_quote(&path_string(received_log))
        ),
    )
    .unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    path
}

fn wait_for_file_contains(path: &Path, expected: &str, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let contents = fs::read_to_string(path).unwrap_or_default();
        if contents.contains(expected) {
            return contents;
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!(
        "timed out waiting for {expected:?} at {}: {:?}",
        path.display(),
        fs::read_to_string(path)
    );
}

fn read_until(fd: RawFd, needle: &str, timeout: Duration) -> String {
    let start = Instant::now();
    let mut output = Vec::new();
    let mut buffer = [0_u8; 4096];
    while start.elapsed() < timeout {
        if poll_readable(fd, Duration::from_millis(50)).unwrap() {
            let n = read_fd(fd, &mut buffer).unwrap();
            if n == 0 {
                break;
            }
            output.extend_from_slice(&buffer[..n]);
            let rendered = String::from_utf8_lossy(&output);
            if rendered.contains(needle) {
                return rendered.into_owned();
            }
        }
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn poll_readable(fd: RawFd, timeout: Duration) -> io::Result<bool> {
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let rc = unsafe { libc::poll(&mut pollfd, 1, timeout.as_millis() as i32) };
    if rc == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(rc > 0 && pollfd.revents & libc::POLLIN != 0)
}

fn read_fd(fd: RawFd, buffer: &mut [u8]) -> io::Result<usize> {
    let rc = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
    if rc == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(rc as usize)
}

fn wait_for_running_invocation(fixture: &Fixture) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(db) = MailboxDb::open(&fixture.sidecar_path())
            && let Ok(SessionGenerationProjection::One(generation)) = db
                .runtime_lifecycle_reader()
                .session_generation_projection(SESSION_A)
        {
            return generation.spawn_invocation_uuid;
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for running invocation");
}

fn running_control_path(fixture: &Fixture) -> String {
    let projection = fixture
        .mailbox()
        .runtime_lifecycle_reader()
        .session_generation_projection(SESSION_A)
        .unwrap();
    let SessionGenerationProjection::One(generation) = projection else {
        panic!("expected one running runtime generation");
    };
    generation.pty_control_path.expect("PTY control path")
}

fn wait_for_child_identity(fixture: &Fixture, invocation_uuid: &str) -> ProcessIdentity {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(sidecar) = PidIdentityDb::open(&fixture.sidecar_path()) {
            let rows = sidecar.lookup_by_invocation_uuid(invocation_uuid).unwrap();
            if let Some(row) = rows.first() {
                return row.identity();
            }
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for child identity for {invocation_uuid}");
}

fn read_inject_payload(stream: &mut UnixStream) -> String {
    let mut header = [0_u8; 12];
    stream.read_exact(&mut header).unwrap();
    assert_eq!(&header[..4], b"OPTY");
    assert_eq!(header[4], 1);
    assert_eq!(header[5], 1);
    let length = u32::from_be_bytes([header[8], header[9], header[10], header[11]]) as usize;
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload).unwrap();
    String::from_utf8(payload).unwrap()
}

fn write_response(stream: &mut UnixStream, ack: bool, message: &str) {
    let mut header = [0_u8; 12];
    header[..4].copy_from_slice(b"OPTY");
    header[4] = 1;
    header[5] = if ack { 0 } else { 1 };
    header[8..12].copy_from_slice(&(message.len() as u32).to_be_bytes());
    stream.write_all(&header).unwrap();
    stream.write_all(message.as_bytes()).unwrap();
}

fn write_inject_request(stream: &mut UnixStream, payload: &[u8]) {
    let mut header = [0_u8; 12];
    header[..4].copy_from_slice(b"OPTY");
    header[4] = 1;
    header[5] = 1;
    header[8..12].copy_from_slice(&(payload.len() as u32).to_be_bytes());
    stream.write_all(&header).unwrap();
    stream.write_all(payload).unwrap();
}

fn delivery_attempt_id(payload: &str) -> String {
    let marker = "[OULIPOLY-DELIVERY ";
    let start = payload.rfind(marker).expect("delivery marker") + marker.len();
    let tail = &payload[start..];
    tail[..tail.find(']').expect("delivery marker suffix")].to_string()
}

fn unresolved_delivery_attempt_count(fixture: &Fixture) -> i64 {
    Connection::open(fixture.sidecar_path())
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM mailbox_delivery_attempts WHERE resolved_at IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn owner_metadata(session_id: &str, invocation_uuid: &str) -> Value {
    json!({
        "owner_session_id": session_id,
        "owner_invocation_uuid": invocation_uuid,
    })
}

fn current_identity() -> ProcessIdentity {
    read_live_process_identity(i64::from(std::process::id()))
        .unwrap()
        .unwrap()
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "failed to parse stdout JSON: {err}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn toml_string(value: &str) -> String {
    serde_json::to_string(value).unwrap()
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
