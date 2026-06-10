#![cfg(unix)]

//! ## Declared roles
//!
//! Roles: orchestration, formatter, mapper, accessor, parser, validator,
//! predicate, filter.
//!
//! TEST: proactive wake and wake-reclaim end-to-end fixtures — fake CLI script
//! formatters, model/config mappers, state and mailbox accessors, JSON/record
//! parsers, liveness and delivery predicates/filters, wake-claim and durable
//! mailbox validators, and test orchestration.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/tests/wu_d_proactive_wake_integration.rs
//!     role: adapter
//!     Translates:
//!       - runtime-cli-dispatch-contract
//!       - wake-claim-sidecar-contract
//!       - pid-identity-sidecar-contract
//!       - mailbox-delivery-contract
//!       - test-fixture-process-contract
//! intrinsic_surface_declarations:
//!   - component: src-tauri/tests/wu_d_proactive_wake_integration.rs
//!     role: intrinsic-surface
//!     Domain: proactive wake and wake-reclaim regression suite
//!     Owns:
//!       - isolated config/data fixture materialization
//!       - fake provider script generation and executable setup
//!       - auto-wake command invocation and environment isolation
//!       - sidecar wake-claim fixture construction
//!       - mailbox delivery, busy suppression, and consumed suppression assertions
//! ```

use chrono::{DateTime, Utc};
use oulipoly_state::mailbox::{
    AgentBashCompleteEnqueue, MailboxDb, SessionRuntimeUpsert, WAKE_SWEEP_ABANDONED_ERROR,
    WakeClaimAcquireResult, WakeClaimRequest,
};
use oulipoly_state::pid_identity::{
    PidIdentityDb, PidIdentityRecord, ProcessIdentity, read_live_process_identity,
};
use oulipoly_state::{SessionTurnIngest, StateDb};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

const MODEL: &str = "wu-d-fixture-model";
const PROVIDER: &str = "wu-d-fixture-provider";
const SESSION: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const CAPTURED_OPENCODE_SESSION: &str = "ses_capturemidturn";
const INVOCATION: &str = "11111111-1111-4111-8111-111111111111";

struct Fixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    home_dir: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
    work_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("xdg-config");
        let data_home = dir.path().join("xdg-data");
        let home_dir = dir.path().join("home");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        let work_dir = dir.path().join("work");
        fs::create_dir_all(&models_dir).unwrap();
        fs::create_dir_all(&home_dir).unwrap();
        fs::create_dir_all(&work_dir).unwrap();
        Self {
            dir,
            config_home,
            data_home,
            home_dir,
            app_config_dir,
            models_dir,
            work_dir,
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

    fn pinned_data_dir(&self) -> PathBuf {
        self.data_home.join("oulipoly-agent-runner")
    }

    fn run(&self, mut cmd: Command) -> Output {
        cmd.env("XDG_CONFIG_HOME", &self.config_home)
            .env("XDG_DATA_HOME", &self.data_home)
            .env("HOME", &self.home_dir)
            .env("AGENT_BASH_AGENT_RUNNER_BIN", runner_bin())
            .env("WU_D_WORK_DIR", &self.work_dir)
            .env_remove("OULIPOLY_DATA_DIR")
            .env_remove("OULIPOLY_AUTO_WAKE")
            .env_remove("OULIPOLY_AUTO_WAKE_SESSION_ID")
            .env_remove("OULIPOLY_AUTO_WAKE_TOKEN")
            .env_remove("OULIPOLY_AUTO_WAKE_COUNT")
            .env_remove("OULIPOLY_PARENT_INVOCATION")
            .current_dir(self.dir.path());
        cmd.output().unwrap()
    }

    fn run_agent(&self, prompt: &str) -> Output {
        let mut cmd = Command::new(runner_bin());
        cmd.arg("-m")
            .arg(MODEL)
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg(prompt);
        self.run(cmd)
    }

    fn run_resume(&self) -> Output {
        let mut cmd = Command::new(runner_bin());
        cmd.arg("resume")
            .arg("-m")
            .arg(MODEL)
            .arg("--session-id")
            .arg(SESSION)
            .arg("--models-dir")
            .arg(&self.models_dir);
        self.run(cmd)
    }

    fn run_mailbox_list(&self, session_id: &str) -> Output {
        let mut cmd = Command::new(runner_bin());
        cmd.arg("mailbox")
            .arg("list")
            .arg("--session-id")
            .arg(session_id)
            .arg("--json");
        self.run(cmd)
    }

    fn write_provider(&self, body: &str) {
        let script = self.write_script("provider.sh", body);
        fs::write(
            self.models_dir.join(format!("{MODEL}.toml")),
            format!(
                r#"[[providers]]
name = "{PROVIDER}"
args = []
"#,
            ),
        )
        .unwrap();
        fs::write(
            self.app_config_dir.join("providers.toml"),
            format!(
                r#"[{PROVIDER}]
command = {}
args = []
interactive_args = ["interactive"]
prompt_mode = "arg"

[{PROVIDER}.session_capture]
kind = "forced_flag_verified"
flag = "--session-id"

[{PROVIDER}.resume]
kind = "flag"
flag = "--resume"
"#,
                toml_string(&path_string(&script))
            ),
        )
        .unwrap();
    }

    fn write_opencode_provider(&self, body: &str) {
        let script = self.write_script("opencode.sh", body);
        fs::write(
            self.models_dir.join(format!("{MODEL}.toml")),
            r#"[[providers]]
name = "opencode"
args = []
"#,
        )
        .unwrap();
        fs::write(
            self.app_config_dir.join("providers.toml"),
            format!(
                r#"[opencode]
command = {}
args = []
interactive_args = ["interactive"]
prompt_mode = "arg"

[opencode.resume]
kind = "flag"
flag = "--session"
"#,
                toml_string(&path_string(&script))
            ),
        )
        .unwrap();
    }

    fn write_opencode_capture_provider(&self, body: &str) {
        let script = self.write_script("opencode-capture.sh", body);
        fs::write(
            self.models_dir.join(format!("{MODEL}.toml")),
            r#"[[providers]]
name = "opencode"
args = []
"#,
        )
        .unwrap();
        fs::write(
            self.app_config_dir.join("providers.toml"),
            format!(
                r#"[opencode]
command = {}
args = []
interactive_args = ["interactive"]
prompt_mode = "arg"

[opencode.session_capture]
kind = "stdout_json_event"
json_args = ["--json"]
event_type = "step_start"
event_id_path = "sessionID"

[opencode.resume]
kind = "flag"
flag = "--session"
"#,
                toml_string(&path_string(&script))
            ),
        )
        .unwrap();
    }

    fn pid_identity_session_id_for_provider(&self, provider_name: &str) -> String {
        self.sidecar_conn()
            .query_row(
                "SELECT session_id
                 FROM pid_identity
                 WHERE provider_name = ?1
                 ORDER BY recorded_at DESC, os_pid DESC
                 LIMIT 1",
                rusqlite::params![provider_name],
                |row| row.get(0),
            )
            .unwrap()
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

    fn mailbox(&self) -> MailboxDb {
        MailboxDb::open(&self.sidecar_path()).unwrap()
    }

    fn sidecar_conn(&self) -> Connection {
        Connection::open(self.sidecar_path()).unwrap()
    }

    fn state(&self) -> StateDb {
        StateDb::open(&self.state_path()).unwrap()
    }

    fn seed_session_turn(&self) {
        self.seed_session_turn_for(PROVIDER, SESSION, "turn-a");
    }

    fn seed_session_turn_for(&self, provider_name: &str, session_id: &str, turn_id: &str) {
        let db = self.state();
        db.ingest_session_turns_batch(
            provider_name,
            &[SessionTurnIngest {
                session_id: session_id.to_string(),
                turn_id: turn_id.to_string(),
                timestamp: ts("2026-06-04T12:00:00Z"),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: None,
            }],
        )
        .unwrap();
    }

    fn seed_consumed_notification_turn(&self, handle: &str) {
        self.state()
            .ingest_session_turns_batch(
                PROVIDER,
                &[SessionTurnIngest {
                    session_id: SESSION.to_string(),
                    turn_id: format!("turn-consumed-{handle}"),
                    timestamp: ts("2026-06-04T12:01:00Z"),
                    role: "user".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: false,
                    body: Some(format!("[OULIPOLY NOTIFICATIONS]\nhandle: {handle}\n")),
                }],
            )
            .unwrap();
    }

    fn seed_idle_runtime(&self) {
        self.seed_idle_runtime_for(SESSION, PROVIDER, MODEL);
    }

    fn seed_idle_runtime_for(&self, session_id: &str, provider_name: &str, model_name: &str) {
        let mut db = MailboxDb::open(&self.sidecar_path()).unwrap();
        let models_dir = path_string(&self.models_dir);
        db.upsert_session_runtime(SessionRuntimeUpsert {
            session_id,
            mode: "headless",
            invocation_uuid: None,
            provider_name: Some(provider_name),
            model_name: Some(model_name),
            pty_control_path: None,
            models_dir: Some(&models_dir),
            effective_cwd: None,
        })
        .unwrap();
    }

    fn seed_idle_runtime_without_models_dir(&self, session_id: &str) {
        let mut db = MailboxDb::open(&self.sidecar_path()).unwrap();
        db.upsert_session_runtime(SessionRuntimeUpsert {
            session_id,
            mode: "headless",
            invocation_uuid: None,
            provider_name: Some(PROVIDER),
            model_name: Some(MODEL),
            pty_control_path: None,
            models_dir: None,
            effective_cwd: None,
        })
        .unwrap();
    }

    fn seed_active_chain_for(
        &self,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
        model_name: &str,
    ) {
        let _ = StateDb::open(&self.state_path()).unwrap();
        let conn = Connection::open(self.state_path()).unwrap();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, '2026-06-04T12:00:00Z', '2026-06-04T12:00:00Z', ?2)",
            rusqlite::params![chain_id, model_name],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, '2026-06-04T12:00:00Z', 'initial')",
            rusqlite::params![chain_id, provider_name, session_id],
        )
        .unwrap();
    }

    fn seed_mailbox(&self, session_id: &str, handle: &str) {
        self.seed_mailbox_for(session_id, handle, Some(INVOCATION));
    }

    fn seed_mailbox_for(
        &self,
        session_id: &str,
        handle: &str,
        owner_invocation_uuid: Option<&str>,
    ) {
        let state_dir = self.work_dir.join(format!("seed-{handle}"));
        fs::create_dir_all(&state_dir).unwrap();
        let meta = state_dir.join("meta.json");
        let log = state_dir.join("log");
        let rc = state_dir.join("rc");
        fs::write(&meta, r#"{"caller_chain":[]}"#).unwrap();
        fs::write(&log, format!("log {handle}\n")).unwrap();
        fs::write(&rc, "0\n").unwrap();
        let mut db = MailboxDb::open(&self.sidecar_path()).unwrap();
        db.enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
            session_id,
            handle,
            payload_json: r#"{"schema_version":1,"kind":"agent_bash_complete"}"#,
            owner_invocation_uuid,
            matched_os_pid: Some(9_300),
            matched_os_boot_id: Some("boot-seeded"),
            matched_os_pid_starttime_ticks: Some(456),
            matched_chain_index: Some(0),
            state_dir: &path_string(&state_dir),
            meta_path: &path_string(&meta),
            log_path: &path_string(&log),
            rc_path: &path_string(&rc),
            rc: 0,
        })
        .unwrap();
    }

    fn age_mailbox_for(&self, session_id: &str, seconds_old: i64) {
        let enqueued_at = (Utc::now() - chrono::Duration::seconds(seconds_old)).to_rfc3339();
        self.sidecar_conn()
            .execute(
                "UPDATE mailbox SET enqueued_at = ?2 WHERE session_id = ?1",
                rusqlite::params![session_id, enqueued_at],
            )
            .unwrap();
    }

    fn mark_mailbox_unconfirmed_twice(&self, session_id: &str, handle: &str) {
        let mut db = self.mailbox();
        let row = db
            .list_mailbox(session_id, true)
            .unwrap()
            .into_iter()
            .find(|row| row.handle == handle)
            .unwrap_or_else(|| panic!("missing seeded mailbox row {handle}"));
        db.mark_delivery_failed(session_id, &[row.seq], "mailbox_delivery_unconfirmed")
            .unwrap();
        db.mark_delivery_failed(session_id, &[row.seq], "mailbox_delivery_unconfirmed")
            .unwrap();
    }

    fn record_identity(&self, identity: &ProcessIdentity) {
        self.record_identity_for(identity, SESSION, PROVIDER, MODEL);
    }

    fn record_identity_for(
        &self,
        identity: &ProcessIdentity,
        session_id: &str,
        provider_name: &str,
        model_name: &str,
    ) {
        let sidecar = PidIdentityDb::open(&self.sidecar_path()).unwrap();
        sidecar
            .record_identity(PidIdentityRecord {
                identity,
                os_pgid: None,
                invocation_uuid: INVOCATION,
                session_id: Some(session_id),
                provider_name: Some(provider_name),
                model_name: Some(model_name),
                recorded_at: "2026-06-04T12:00:00Z",
            })
            .unwrap();
    }

    fn prompt_file(&self, name: &str) -> PathBuf {
        self.work_dir.join(name)
    }

    fn assert_xdg_isolated(&self) {
        assert!(
            !self
                .home_dir
                .join(".local/share/oulipoly-agent-runner")
                .exists(),
            "state must stay in isolated XDG_DATA_HOME"
        );
        assert!(
            !self.home_dir.join(".config/oulipoly-agent-runner").exists(),
            "config must stay in isolated XDG_CONFIG_HOME"
        );
    }
}

#[test]
fn idle_wake_delivers() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script(
        r#"( sleep 0.3; notify_handle h-idle 0 ) >/dev/null 2>&1 &"#,
        "",
        "resumed-input.txt",
    ));

    let output = fixture.run_agent("dispatch background work");
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let prompt = wait_for_file(&fixture.prompt_file("resumed-input.txt"));
    assert!(prompt.starts_with("[OULIPOLY NOTIFICATIONS]"), "{prompt}");
    assert!(prompt.contains("kind: agent_bash_complete"), "{prompt}");
    assert!(prompt.contains("handle: h-idle"), "{prompt}");
    assert!(prompt.contains("rc: 0"), "{prompt}");
    let session_id = wait_for_mailbox_session(&fixture);
    wait_until("mailbox delivered", || {
        let db = fixture.mailbox();
        let rows = db.list_mailbox(&session_id, true).unwrap();
        rows.len() == 1
            && rows[0].delivered_at.is_some()
            && db.wake_claim(&session_id).unwrap().is_none()
    });
    let mailbox = fixture.mailbox();
    assert!(mailbox.list_pending(&session_id).unwrap().is_empty());
    assert_eq!(
        mailbox
            .session_runtime(&session_id)
            .unwrap()
            .unwrap()
            .run_state,
        "idle"
    );
    assert!(mailbox.wake_claim(&session_id).unwrap().is_none());
    fixture.assert_xdg_isolated();
}

#[test]
fn busy_then_turn_end_delivers() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script(
        r#"( sleep 0.1; notify_handle h-a 0 ) >/dev/null 2>&1 &
( sleep 0.2; notify_handle h-b 0 ) >/dev/null 2>&1 &
( sleep 0.3; notify_handle h-c 0 ) >/dev/null 2>&1 &
sleep 1"#,
        "",
        "busy-resumed-input.txt",
    ));

    let output = fixture.run_agent("dispatch busy work");
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let prompt = wait_for_file(&fixture.prompt_file("busy-resumed-input.txt"));
    assert!(prompt.starts_with("[OULIPOLY NOTIFICATIONS]"), "{prompt}");
    let pos_a = prompt.find("handle: h-a").unwrap();
    let pos_b = prompt.find("handle: h-b").unwrap();
    let pos_c = prompt.find("handle: h-c").unwrap();
    assert!(pos_a < pos_b && pos_b < pos_c, "{prompt}");
    let session_id = wait_for_mailbox_session(&fixture);
    wait_until("busy rows delivered", || {
        let db = fixture.mailbox();
        let rows = db.list_mailbox(&session_id, true).unwrap();
        rows.len() == 3
            && rows.iter().all(|row| row.delivered_at.is_some())
            && db.wake_claim(&session_id).unwrap().is_none()
    });
    assert!(fixture.mailbox().wake_claim(&session_id).unwrap().is_none());
    fixture.assert_xdg_isolated();
}

#[test]
fn no_undelivered_no_wake_and_loop_terminates() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "no-pending-resume.txt"));

    let output = fixture.run_agent("no pending");
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let session_id = wait_for_runtime_session(&fixture);
    wait_until("runtime idle", || {
        fixture
            .mailbox()
            .session_runtime(&session_id)
            .unwrap()
            .is_some_and(|row| row.run_state == "idle")
    });
    let mailbox = fixture.mailbox();
    assert!(mailbox.list_pending(&session_id).unwrap().is_empty());
    assert!(mailbox.wake_claim(&session_id).unwrap().is_none());
    assert!(!fixture.prompt_file("no-pending-resume.txt").exists());
    fixture.assert_xdg_isolated();
}

#[test]
fn auto_wake_cap_stops_self_replicating_session() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script(
        r#"( sleep 0.3; notify_handle h-start 0 ) >/dev/null 2>&1 &"#,
        r#"count="${OULIPOLY_AUTO_WAKE_COUNT:-0}"
notify_handle "h-auto-${count}" 0"#,
        "resumed-${OULIPOLY_AUTO_WAKE_COUNT:-0}.txt",
    ));
    let mut cmd = Command::new(runner_bin());
    cmd.arg("-m")
        .arg(MODEL)
        .arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("self replicate")
        .env("OULIPOLY_AUTO_WAKE_MAX", "2");
    let output = fixture.run(cmd);
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let first = wait_for_file(&fixture.prompt_file("resumed-1.txt"));
    let second = wait_for_file(&fixture.prompt_file("resumed-2.txt"));
    let session_id = wait_for_mailbox_session(&fixture);
    assert!(first.contains("handle: h-start"), "{first}");
    assert!(second.contains("handle: h-auto-1"), "{second}");
    wait_until("cap leaves pending", || {
        let db = fixture.mailbox();
        let pending = db.list_pending(&session_id).unwrap();
        pending.len() == 1
            && pending[0].handle == "h-auto-2"
            && db.wake_claim(&session_id).unwrap().is_none()
            && db
                .session_runtime(&session_id)
                .unwrap()
                .is_some_and(|row| row.auto_wake_count == 2)
    });
    fixture.assert_xdg_isolated();
}

#[test]
fn concurrent_notify_single_flight() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script(
        "",
        r#"printf 'wake\n' >> "$work/concurrent-wake-launches.log"
sleep 0.2"#,
        "concurrent-resume.txt",
    ));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    let identity = identity(9_200, "boot-concurrent", 123);
    fixture.record_identity(&identity);

    let child_a = notify_command(&fixture, "h-concurrent-a", &identity)
        .spawn()
        .unwrap();
    let child_b = notify_command(&fixture, "h-concurrent-b", &identity)
        .spawn()
        .unwrap();
    let output_a = child_a.wait_with_output().unwrap();
    let output_b = child_b.wait_with_output().unwrap();
    assert_notify_success(&output_a);
    assert_notify_success(&output_b);
    assert_single_wake_claim_won(&[notify_wake(&output_a), notify_wake(&output_b)]);

    let prompt = wait_for_file(&fixture.prompt_file("concurrent-resume.txt"));
    assert!(prompt.contains("handle: h-concurrent-"), "{prompt}");
    wait_until("concurrent rows delivered", || {
        let db = fixture.mailbox();
        let rows = db.list_mailbox(SESSION, true).unwrap();
        rows.len() == 2
            && rows.iter().all(|row| row.delivered_at.is_some())
            && db.wake_claim(SESSION).unwrap().is_none()
    });
    assert_single_wake_child_launch(&wait_for_file(
        &fixture.prompt_file("concurrent-wake-launches.log"),
    ));
    assert!(fixture.mailbox().wake_claim(SESSION).unwrap().is_none());
    fixture.assert_xdg_isolated();
}

#[test]
fn manual_resume_race_is_safe() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "manual-race.txt"));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-manual-race");
    let mut db = fixture.mailbox();
    assert!(matches!(
        db.try_acquire_wake_claim(WakeClaimRequest {
            session_id: SESSION,
            claim_token: "manual-race-token",
            reason: "notify_idle",
            auto_wake_count: 1,
            wake_invocation_uuid: None,
            stale_after_seconds: 600,
        })
        .unwrap(),
        WakeClaimAcquireResult::Acquired(_)
    ));
    drop(db);

    let output = fixture.run_resume();
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let prompt = wait_for_file(&fixture.prompt_file("manual-race.txt"));
    assert!(prompt.contains("handle: h-manual-race"), "{prompt}");
    wait_until("manual race delivered", || {
        let db = fixture.mailbox();
        let rows = db.list_mailbox(SESSION, true).unwrap();
        rows.len() == 1
            && rows[0].delivered_at.is_some()
            && db.wake_claim(SESSION).unwrap().is_none()
    });
    fixture.assert_xdg_isolated();
}

#[test]
fn opencode_notify_idle_wakes_resume_with_ses_session() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_opencode_provider(&provider_script(
        "",
        r#"if [ "$resume" != "ses_fixture" ]; then
  printf 'expected --session ses_fixture, got %s\n' "$resume" >&2
  exit 66
fi"#,
        "opencode-resumed.txt",
    ));
    fixture.seed_active_chain_for(
        "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        "opencode",
        "ses_fixture",
        MODEL,
    );
    fixture.seed_idle_runtime_for("ses_fixture", "opencode", MODEL);
    let identity = identity(9_400, "boot-opencode", 789);
    fixture.record_identity_for(&identity, "ses_fixture", "opencode", MODEL);

    let output = notify_command(&fixture, "h-opencode", &identity)
        .output()
        .unwrap();

    assert_notify_success(&output);
    let prompt = wait_for_file(&fixture.prompt_file("opencode-resumed.txt"));
    assert!(prompt.contains("handle: h-opencode"), "{prompt}");
    wait_until("opencode mailbox delivered", || {
        let db = fixture.mailbox();
        let rows = db.list_mailbox("ses_fixture", true).unwrap();
        rows.len() == 1
            && rows[0].delivered_at.is_some()
            && db.wake_claim("ses_fixture").unwrap().is_none()
    });
    assert!(
        fixture
            .mailbox()
            .list_pending("ses_fixture")
            .unwrap()
            .is_empty()
    );
    fixture.assert_xdg_isolated();
}

#[test]
fn opencode_mid_turn_notify_resolves_capture_time_sidecar_owner() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    let event = json!({
        "type": "step_start",
        "sessionID": CAPTURED_OPENCODE_SESSION,
    });
    fixture.write_opencode_capture_provider(&provider_script(
        &format!(
            r#"printf '%s\n' '{}'
sleep 0.2
notify_handle h-capture-midturn 0
sleep 0.2"#,
            event
        ),
        r#"if [ "$resume" != "ses_capturemidturn" ]; then
  printf 'expected --session ses_capturemidturn, got %s\n' "$resume" >&2
  exit 66
fi"#,
        "opencode-capture-resumed.txt",
    ));
    fixture.seed_active_chain_for(
        "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
        "opencode",
        CAPTURED_OPENCODE_SESSION,
        MODEL,
    );

    let output = fixture.run_agent("dispatch capture race");
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let notify_json = wait_for_file(&fixture.work_dir.join("h-capture-midturn/notify.json"));
    let notify: Value = serde_json::from_str(&notify_json).unwrap();
    assert_eq!(
        notify.get("status").and_then(Value::as_str),
        Some("enqueued")
    );
    assert_eq!(notify.get("enqueued").and_then(Value::as_bool), Some(true));
    assert_eq!(
        notify.get("owner_session_id").and_then(Value::as_str),
        Some(CAPTURED_OPENCODE_SESSION)
    );
    assert_eq!(
        notify.get("session_source").and_then(Value::as_str),
        Some("sidecar_session_id")
    );
    assert_eq!(
        notify
            .get("wake")
            .and_then(|wake| wake.get("status"))
            .and_then(Value::as_str),
        Some("busy")
    );

    assert_eq!(
        fixture.pid_identity_session_id_for_provider("opencode"),
        CAPTURED_OPENCODE_SESSION
    );
    let prompt = wait_for_file(&fixture.prompt_file("opencode-capture-resumed.txt"));
    assert!(prompt.contains("handle: h-capture-midturn"), "{prompt}");
    wait_until("captured opencode mailbox delivered", || {
        let db = fixture.mailbox();
        let rows = db.list_mailbox(CAPTURED_OPENCODE_SESSION, true).unwrap();
        rows.len() == 1
            && rows[0].handle == "h-capture-midturn"
            && rows[0].delivered_at.is_some()
            && rows[0].owner_invocation_uuid.is_some()
            && rows[0].matched_os_pid.is_some()
            && rows[0].matched_chain_index == Some(0)
            && db.wake_claim(CAPTURED_OPENCODE_SESSION).unwrap().is_none()
    });
    assert_eq!(
        fixture
            .mailbox()
            .session_runtime(CAPTURED_OPENCODE_SESSION)
            .unwrap()
            .unwrap()
            .run_state,
        "idle"
    );
    fixture.assert_xdg_isolated();
}

#[test]
fn batch_cap_followup_wake() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script(
        "",
        "",
        "batch-${OULIPOLY_AUTO_WAKE_COUNT:-manual}.txt",
    ));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    for index in 0..25 {
        fixture.seed_mailbox(SESSION, &format!("h-batch-{index:02}"));
    }

    let output = fixture.run_resume();
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let first = wait_for_file(&fixture.prompt_file("batch-manual.txt"));
    let second = wait_for_file(&fixture.prompt_file("batch-1.txt"));
    assert!(
        first.contains("5 additional notification(s) remain queued"),
        "{first}"
    );
    assert!(second.contains("handle: h-batch-20"), "{second}");
    wait_until("batch rows delivered", || {
        let db = fixture.mailbox();
        let rows = db.list_mailbox(SESSION, true).unwrap();
        rows.len() == 25
            && rows.iter().all(|row| row.delivered_at.is_some())
            && db.list_pending(SESSION).unwrap().is_empty()
            && db.wake_claim(SESSION).unwrap().is_none()
    });
    fixture.assert_xdg_isolated();
}

#[test]
fn wake_sweep_reclaims_dead_claim_and_delivers_pending_mailbox() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "sweep-reclaimed.txt"));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-sweep-reclaim");
    seed_dead_wake_claim(&fixture, "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", 601);

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);

    let prompt = wait_for_file(&fixture.prompt_file("sweep-reclaimed.txt"));
    assert!(prompt.contains("handle: h-sweep-reclaim"), "{prompt}");
    wait_until("sweep reclaimed dead claim and delivered mailbox", || {
        let db = fixture.mailbox();
        let rows = db.list_mailbox(SESSION, true).unwrap();
        rows.len() == 1
            && rows[0].delivered_at.is_some()
            && rows[0].delivery_error.is_none()
            && db.wake_claim(SESSION).unwrap().is_none()
    });
    fixture.assert_xdg_isolated();
}

#[test]
fn wake_sweep_does_not_resurrect_abandoned_transient_session() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "abandoned-transient-resumed.txt"));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-abandoned-transient");

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);
    std::thread::sleep(Duration::from_millis(300));

    assert!(
        !fixture
            .prompt_file("abandoned-transient-resumed.txt")
            .exists()
    );
    let rows = fixture.mailbox().list_pending(SESSION).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].handle, "h-abandoned-transient");
    assert!(rows[0].delivery_error.is_none());
    assert!(fixture.mailbox().wake_claim(SESSION).unwrap().is_none());
    fixture.assert_xdg_isolated();
}

#[test]
fn wake_sweep_delivers_resumable_session_missing_models_dir() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "missing-models-dir-resumed.txt"));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime_without_models_dir(SESSION);
    fixture.seed_mailbox_for(SESSION, "h-missing-models-dir", None);
    seed_dead_wake_claim(&fixture, "dddddddd-dddd-4ddd-8ddd-dddddddddddd", 601);

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);

    let prompt = wait_for_file(&fixture.prompt_file("missing-models-dir-resumed.txt"));
    assert!(prompt.contains("handle: h-missing-models-dir"), "{prompt}");
    wait_until("missing models_dir wake delivered", || {
        let db = fixture.mailbox();
        let rows = db.list_mailbox(SESSION, true).unwrap();
        rows.len() == 1
            && rows[0].delivered_at.is_some()
            && rows[0].delivery_error.is_none()
            && db.wake_claim(SESSION).unwrap().is_none()
    });
    fixture.assert_xdg_isolated();
}

#[test]
fn wake_sweep_does_not_disturb_live_identity_matched_claim() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "live-claim-not-disturbed.txt"));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-live-claim");
    seed_live_wake_claim(&fixture, "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);
    std::thread::sleep(Duration::from_millis(300));

    assert!(!fixture.prompt_file("live-claim-not-disturbed.txt").exists());
    let claim = fixture.mailbox().wake_claim(SESSION).unwrap().unwrap();
    assert_eq!(claim.claim_token, "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
    assert_eq!(fixture.mailbox().list_pending(SESSION).unwrap().len(), 1);
    fixture.assert_xdg_isolated();
}

#[test]
fn wake_sweep_does_not_rewake_consumed_pending_mailbox() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "consumed-not-rewoken.txt"));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-consumed");
    fixture.seed_consumed_notification_turn("h-consumed");
    seed_dead_wake_claim(&fixture, "cccccccc-cccc-4ccc-8ccc-cccccccccccc", 601);

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);
    std::thread::sleep(Duration::from_millis(300));

    assert!(!fixture.prompt_file("consumed-not-rewoken.txt").exists());
    assert_eq!(fixture.mailbox().list_pending(SESSION).unwrap().len(), 1);
    fixture.assert_xdg_isolated();
}

#[test]
fn wake_sweep_does_not_rewake_twice_unconfirmed_pending_mailbox() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script(
        "",
        "",
        "twice-unconfirmed-not-rewoken.txt",
    ));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-unconfirmed");
    fixture.mark_mailbox_unconfirmed_twice(SESSION, "h-unconfirmed");
    seed_dead_wake_claim(&fixture, "dddddddd-dddd-4ddd-8ddd-dddddddddddd", 601);

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);
    std::thread::sleep(Duration::from_millis(300));

    assert!(
        !fixture
            .prompt_file("twice-unconfirmed-not-rewoken.txt")
            .exists()
    );
    let rows = fixture.mailbox().list_pending(SESSION).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].handle, "h-unconfirmed");
    assert_eq!(rows[0].delivery_attempts, 2);
    fixture.assert_xdg_isolated();
}

#[test]
fn wake_sweep_skips_twice_unconfirmed_rows_and_delivers_newer_pending_mailbox() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "newer-after-unconfirmed.txt"));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-unconfirmed-old");
    fixture.seed_mailbox(SESSION, "h-newer");
    fixture.mark_mailbox_unconfirmed_twice(SESSION, "h-unconfirmed-old");
    seed_dead_wake_claim(&fixture, "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee", 601);

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);

    let prompt = wait_for_file(&fixture.prompt_file("newer-after-unconfirmed.txt"));
    assert!(!prompt.contains("handle: h-unconfirmed-old"), "{prompt}");
    assert!(prompt.contains("handle: h-newer"), "{prompt}");
    wait_until(
        "newer mailbox delivered while exhausted row remains pending",
        || {
            let rows = fixture.mailbox().list_mailbox(SESSION, true).unwrap();
            let old = rows.iter().find(|row| row.handle == "h-unconfirmed-old");
            let newer = rows.iter().find(|row| row.handle == "h-newer");
            old.is_some_and(|row| row.delivered_at.is_none() && row.delivery_attempts == 2)
                && newer.is_some_and(|row| row.delivered_at.is_some())
        },
    );
    fixture.assert_xdg_isolated();
}

#[test]
fn wake_sweep_backlog_recovers_recent_leak_and_reaps_dead_owner_debris() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script(
        "",
        r#"printf '%s' "$last" > "$work/backlog-$resume.txt""#,
        "backlog-any.txt",
    ));

    let dead_sessions = (0..16)
        .map(|index| format!("00000000-0000-4000-8000-{index:012x}"))
        .collect::<Vec<_>>();
    for (index, session_id) in dead_sessions.iter().enumerate() {
        let handle = format!("h-dead-owner-{index}");
        fixture.seed_mailbox_for(session_id, &handle, None);
        fixture.age_mailbox_for(session_id, 86_400);
        seed_dead_wake_claim_for(
            &fixture,
            session_id,
            &format!("dead0000-0000-4000-8000-{index:012x}"),
            601,
        );
    }

    let idle_session = "11111111-1111-4111-8111-000000000001";
    fixture.seed_active_chain_for(
        "22222222-2222-4222-8222-000000000001",
        PROVIDER,
        idle_session,
        MODEL,
    );
    fixture.seed_session_turn_for(PROVIDER, idle_session, "turn-idle-backlog");
    fixture.seed_idle_runtime_for(idle_session, PROVIDER, MODEL);
    fixture.seed_mailbox(idle_session, "h-idle-resumable-backlog");
    fixture.age_mailbox_for(idle_session, 3_600);
    seed_dead_wake_claim_for(
        &fixture,
        idle_session,
        "eeee0000-0000-4000-8000-000000000001",
        601,
    );

    let recent_session = "11111111-1111-4111-8111-000000000002";
    fixture.seed_active_chain_for(
        "22222222-2222-4222-8222-000000000002",
        PROVIDER,
        recent_session,
        MODEL,
    );
    fixture.seed_session_turn_for(PROVIDER, recent_session, "turn-recent-backlog");
    fixture.seed_idle_runtime_for(recent_session, PROVIDER, MODEL);
    fixture.seed_mailbox(recent_session, "h-recent-leak-backlog");
    seed_dead_wake_claim_for(
        &fixture,
        recent_session,
        "eeee0000-0000-4000-8000-000000000002",
        601,
    );

    let output = fixture.run_mailbox_list(recent_session);
    assert_success(&output);

    let idle_prompt = wait_for_file(&fixture.prompt_file(&format!("backlog-{idle_session}.txt")));
    assert!(
        idle_prompt.contains("handle: h-idle-resumable-backlog"),
        "{idle_prompt}"
    );
    let recent_prompt =
        wait_for_file(&fixture.prompt_file(&format!("backlog-{recent_session}.txt")));
    assert!(
        recent_prompt.contains("handle: h-recent-leak-backlog"),
        "{recent_prompt}"
    );

    wait_until(
        "backlog recoverable sessions delivered and debris reaped",
        || {
            let db = fixture.mailbox();
            let idle_rows = db.list_mailbox(idle_session, true).unwrap();
            let recent_rows = db.list_mailbox(recent_session, true).unwrap();
            let recovered = idle_rows.len() == 1
                && idle_rows[0].delivered_at.is_some()
                && recent_rows.len() == 1
                && recent_rows[0].delivered_at.is_some();
            let debris_reaped = dead_sessions.iter().all(|session_id| {
                let rows = db.list_mailbox(session_id, true).unwrap();
                rows.len() == 1
                    && rows[0].delivered_at.is_none()
                    && rows[0].delivery_error.as_deref() == Some(WAKE_SWEEP_ABANDONED_ERROR)
                    && db.wake_claim(session_id).unwrap().is_none()
            });
            recovered && debris_reaped
        },
    );
    for session_id in dead_sessions {
        assert!(
            !fixture
                .prompt_file(&format!("backlog-{session_id}.txt"))
                .exists(),
            "dead-owner debris must not be re-woken: {session_id}"
        );
    }
    fixture.assert_xdg_isolated();
}

fn notify_command(fixture: &Fixture, handle: &str, identity: &ProcessIdentity) -> Command {
    let state_dir = fixture.work_dir.join(format!("concurrent-{handle}"));
    fs::create_dir_all(&state_dir).unwrap();
    let meta = state_dir.join("meta.json");
    let log = state_dir.join("log");
    let rc = state_dir.join("rc");
    fs::write(
        &meta,
        serde_json::to_string(&caller_chain(identity)).unwrap(),
    )
    .unwrap();
    fs::write(&log, format!("log {handle}\n")).unwrap();
    fs::write(&rc, "0\n").unwrap();
    let mut cmd = Command::new(runner_bin());
    cmd.arg("notify")
        .arg("agent-bash-complete")
        .arg("--caller-ppid")
        .arg(std::process::id().to_string())
        .arg("--handle")
        .arg(handle)
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("--meta")
        .arg(&meta)
        .arg("--log")
        .arg(&log)
        .arg("--rc")
        .arg(&rc)
        .arg("--json")
        .env("XDG_CONFIG_HOME", &fixture.config_home)
        .env("XDG_DATA_HOME", &fixture.data_home)
        .env("HOME", &fixture.home_dir)
        .env("AGENT_BASH_AGENT_RUNNER_BIN", runner_bin())
        .env("WU_D_WORK_DIR", &fixture.work_dir)
        .env_remove("OULIPOLY_DATA_DIR")
        .env_remove("OULIPOLY_PARENT_INVOCATION")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(fixture.dir.path());
    cmd
}

fn seed_dead_wake_claim(fixture: &Fixture, claim_token: &str, seconds_old: i64) {
    seed_dead_wake_claim_for(fixture, SESSION, claim_token, seconds_old);
}

fn seed_dead_wake_claim_for(
    fixture: &Fixture,
    session_id: &str,
    claim_token: &str,
    seconds_old: i64,
) {
    acquire_seed_wake_claim_for(fixture, session_id, claim_token);
    fixture
        .mailbox()
        .record_wake_claim_pid(session_id, claim_token, 999_999_999)
        .unwrap();
    age_wake_claim_for(fixture, session_id, seconds_old);
}

fn seed_live_wake_claim(fixture: &Fixture, claim_token: &str) {
    acquire_seed_wake_claim(fixture, claim_token);
    let identity = current_process_identity();
    PidIdentityDb::open(&fixture.sidecar_path())
        .unwrap()
        .record_identity(PidIdentityRecord {
            identity: &identity,
            os_pgid: None,
            invocation_uuid: claim_token,
            session_id: Some(SESSION),
            provider_name: Some(PROVIDER),
            model_name: Some(MODEL),
            recorded_at: "2026-06-04T12:02:00Z",
        })
        .unwrap();
    fixture
        .mailbox()
        .record_wake_claim_pid(SESSION, claim_token, identity.os_pid)
        .unwrap();
}

fn acquire_seed_wake_claim(fixture: &Fixture, claim_token: &str) {
    acquire_seed_wake_claim_for(fixture, SESSION, claim_token);
}

fn acquire_seed_wake_claim_for(fixture: &Fixture, session_id: &str, claim_token: &str) {
    let mut db = fixture.mailbox();
    assert!(matches!(
        db.try_acquire_wake_claim(WakeClaimRequest {
            session_id,
            claim_token,
            reason: "notify_idle",
            auto_wake_count: 1,
            wake_invocation_uuid: None,
            stale_after_seconds: 600,
        })
        .unwrap(),
        WakeClaimAcquireResult::Acquired(_)
    ));
}

fn age_wake_claim_for(fixture: &Fixture, session_id: &str, seconds_old: i64) {
    let claimed_at = (Utc::now() - chrono::Duration::seconds(seconds_old)).to_rfc3339();
    fixture
        .sidecar_conn()
        .execute(
            "UPDATE session_wake_claim SET claimed_at = ?2 WHERE session_id = ?1",
            rusqlite::params![session_id, claimed_at],
        )
        .unwrap();
}

fn current_process_identity() -> ProcessIdentity {
    read_live_process_identity(i64::from(std::process::id()))
        .unwrap()
        .expect("test process should have a live identity")
}

#[test]
fn provider_shadow_xdg_notify_uses_pinned_data_dir_and_wakes() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script(
        r#"if [ -z "${OULIPOLY_DATA_DIR:-}" ]; then
  printf 'missing OULIPOLY_DATA_DIR\n' >&2
  exit 65
fi
export XDG_DATA_HOME="$work/shadow-xdg"
( sleep 0.3; notify_handle h-shadow-xdg 0 ) >/dev/null 2>&1 &"#,
        r#"printf '%s\n' "${OULIPOLY_DATA_DIR:-}" > "$work/shadow-resumed-data-dir.txt""#,
        "shadow-resumed-input.txt",
    ));

    let output = fixture.run_agent("dispatch from shadowed provider");
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let prompt = wait_for_file(&fixture.prompt_file("shadow-resumed-input.txt"));
    assert!(prompt.contains("handle: h-shadow-xdg"), "{prompt}");
    let resumed_data_dir = wait_for_file(&fixture.prompt_file("shadow-resumed-data-dir.txt"));
    let expected_data_dir = fixture.pinned_data_dir();
    let expected_data_dir = expected_data_dir.to_string_lossy();
    assert_eq!(resumed_data_dir.trim_end(), expected_data_dir.as_ref());
    let session_id = wait_for_mailbox_session(&fixture);
    wait_until("shadow-xdg mailbox delivered", || {
        let db = fixture.mailbox();
        let rows = db.list_mailbox(&session_id, true).unwrap();
        rows.len() == 1
            && rows[0].handle == "h-shadow-xdg"
            && rows[0].delivered_at.is_some()
            && db.wake_claim(&session_id).unwrap().is_none()
    });
    assert!(
        !fixture
            .work_dir
            .join("shadow-xdg")
            .join("oulipoly-agent-runner")
            .exists(),
        "shadow XDG_DATA_HOME must not receive agent-runner state"
    );
    fixture.assert_xdg_isolated();
}

fn assert_notify_success(output: &Output) {
    assert_success(output);
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn notify_wake(output: &Output) -> Value {
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    response.get("wake").cloned().unwrap_or(Value::Null)
}

fn assert_single_wake_claim_won(wakes: &[Value]) {
    let statuses = wakes
        .iter()
        .filter_map(|wake| wake.get("status").and_then(Value::as_str))
        .collect::<Vec<_>>();
    let spawned_count = statuses
        .iter()
        .filter(|status| **status == "spawned")
        .count();
    assert_eq!(spawned_count, 1, "wake statuses: {statuses:?}");

    let mut claim_tokens = wakes
        .iter()
        .filter_map(|wake| wake.get("claim_token").and_then(Value::as_str))
        .collect::<Vec<_>>();
    claim_tokens.sort_unstable();
    claim_tokens.dedup();
    assert_eq!(claim_tokens.len(), 1, "wake diagnostics: {wakes:?}");
}

fn assert_single_wake_child_launch(log: &str) {
    let launches = log.lines().filter(|line| *line == "wake").count();
    assert_eq!(launches, 1, "wake launch log: {log:?}");
}

fn integration_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn integration_test_guard() -> MutexGuard<'static, ()> {
    integration_test_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn provider_script(on_initial: &str, on_resume: &str, prompt_file: &str) -> String {
    let prompt_file = prompt_file.replace('"', "\\\"");
    format!(
        r#"runner="${{AGENT_BASH_AGENT_RUNNER_BIN:?missing}}"
work="${{WU_D_WORK_DIR:?missing}}"
session=""
resume=""
for ((i=1; i <= $#; i++)); do
  arg="${{!i}}"
  if [ "$arg" = "--session-id" ]; then
    j=$((i + 1))
    session="${{!j}}"
  fi
  if [ "$arg" = "--resume" ]; then
    j=$((i + 1))
    resume="${{!j}}"
  fi
  if [ "$arg" = "--session" ]; then
    j=$((i + 1))
    resume="${{!j}}"
  fi
done
last="${{@: -1}}"
provider_pid="$$"
boot_id="$(< /proc/sys/kernel/random/boot_id)"
stat_line="$(< "/proc/${{provider_pid}}/stat")"
after=
after="${{stat_line##*) }}"
read -r -a stat_fields <<< "$after"
start_ticks="${{stat_fields[19]}}"
notify_handle() {{
  handle="$1"
  rc_value="$2"
  state="$work/$handle"
  mkdir -p "$state"
  printf '{{"caller_chain":[{{"pid":%s,"boot_id":"%s","starttime_ticks":%s}}]}}\n' "$provider_pid" "$boot_id" "$start_ticks" > "$state/meta.json"
  printf 'log for %s\n' "$handle" > "$state/log"
  printf '%s\n' "$rc_value" > "$state/rc"
  "$runner" notify agent-bash-complete \
    --caller-ppid "$provider_pid" \
    --handle "$handle" \
    --state-dir "$state" \
    --meta "$state/meta.json" \
    --log "$state/log" \
    --rc "$state/rc" \
    --json > "$state/notify.json" 2> "$state/notify.err" || true
}}
if [ -n "$resume" ]; then
  target="$work/{prompt_file}"
  mkdir -p "$(dirname "$target")"
  printf '%s' "$last" > "$target"
  {on_resume}
  exit 0
fi
{on_initial}
exit 0
"#
    )
}

fn wait_for_file(path: &Path) -> String {
    wait_until(&format!("{} exists", path.display()), || path.exists());
    fs::read_to_string(path).unwrap()
}

fn wait_for_mailbox_session(fixture: &Fixture) -> String {
    wait_for_sidecar_session(fixture, "mailbox")
}

fn wait_for_runtime_session(fixture: &Fixture) -> String {
    wait_for_sidecar_session(fixture, "session_runtime")
}

fn wait_for_sidecar_session(fixture: &Fixture, table: &str) -> String {
    let mut found = None;
    wait_until(&format!("{table} session id"), || {
        found = sidecar_session_id(fixture, table);
        found.is_some()
    });
    found.unwrap()
}

fn sidecar_session_id(fixture: &Fixture, table: &str) -> Option<String> {
    let conn = fixture.sidecar_conn();
    conn.query_row(
        &format!("SELECT session_id FROM {table} ORDER BY session_id LIMIT 1"),
        [],
        |row| row.get(0),
    )
    .ok()
}

fn wait_until(label: &str, mut predicate: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if predicate() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {label}");
}

fn caller_chain(identity: &ProcessIdentity) -> Value {
    json!({
        "caller_chain": [{
            "pid": identity.os_pid,
            "boot_id": identity.os_boot_id,
            "starttime_ticks": identity.os_pid_starttime_ticks,
        }]
    })
}

fn identity(os_pid: i64, os_boot_id: &str, os_pid_starttime_ticks: i64) -> ProcessIdentity {
    ProcessIdentity {
        os_pid,
        os_boot_id: os_boot_id.to_string(),
        os_pid_starttime_ticks,
    }
}

fn runner_bin() -> &'static str {
    env!("CARGO_BIN_EXE_oulipoly-agent-runner")
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
    serde_json::to_string(value).unwrap()
}
