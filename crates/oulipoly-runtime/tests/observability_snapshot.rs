//! ## Declared roles
//!
//! `accessor`, `formatter`, `mapper`, `orchestration`, `predicate`, `validator`

use oulipoly_config::SessionStorage;
use oulipoly_runtime::observability::{
    InspectRef, LivenessStatus, MonitorNodeKind, MonitorStatus, ObservabilityRoot,
    ObservabilitySnapshotPort, ProductionObservabilitySnapshotService, SnapshotLimits,
};
use oulipoly_state::mailbox::{
    AgentBashCompleteEnqueue, MailboxDb, SessionRuntimeRunningUpdate, SessionRuntimeUpsert,
    WakeClaimAcquireResult, WakeClaimRequest,
};
use oulipoly_state::pid_identity::{PidIdentityDb, PidIdentityRecord, ProcessIdentity};
use oulipoly_state::{InvocationStart, StateDb};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

const ROOT_UUID: &str = "11111111-1111-4111-8111-111111111111";
const CHILD_UUID: &str = "22222222-2222-4222-8222-222222222222";
const SESSION_ID: &str = "session-observe";

struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    old_oulipoly_data_dir: Option<OsString>,
    old_xdg_state_home: Option<OsString>,
    old_xdg_data_home: Option<OsString>,
    old_xdg_config_home: Option<OsString>,
    old_home: Option<OsString>,
}

struct EnvSnapshot {
    old_oulipoly_data_dir: Option<OsString>,
    old_xdg_state_home: Option<OsString>,
    old_xdg_data_home: Option<OsString>,
    old_xdg_config_home: Option<OsString>,
    old_home: Option<OsString>,
}

impl EnvGuard {
    fn set(data_dir: &Path, xdg_state_home: &Path, home: &Path) -> Self {
        let lock = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let snapshot = capture_env_snapshot();
        apply_fixture_env(data_dir, xdg_state_home, home);
        env_guard_from_snapshot(lock, snapshot)
    }
}

fn capture_env_snapshot() -> EnvSnapshot {
    EnvSnapshot {
        old_oulipoly_data_dir: std::env::var_os("OULIPOLY_DATA_DIR"),
        old_xdg_state_home: std::env::var_os("XDG_STATE_HOME"),
        old_xdg_data_home: std::env::var_os("XDG_DATA_HOME"),
        old_xdg_config_home: std::env::var_os("XDG_CONFIG_HOME"),
        old_home: std::env::var_os("HOME"),
    }
}

fn apply_fixture_env(data_dir: &Path, xdg_state_home: &Path, home: &Path) {
    unsafe {
        std::env::set_var("OULIPOLY_DATA_DIR", data_dir);
        std::env::set_var("XDG_STATE_HOME", xdg_state_home);
        std::env::set_var("XDG_DATA_HOME", data_dir.join("xdg-data"));
        std::env::set_var("XDG_CONFIG_HOME", data_dir.join("xdg-config"));
        std::env::set_var("HOME", home);
    }
}

fn env_guard_from_snapshot(lock: MutexGuard<'static, ()>, snapshot: EnvSnapshot) -> EnvGuard {
    EnvGuard {
        _lock: lock,
        old_oulipoly_data_dir: snapshot.old_oulipoly_data_dir,
        old_xdg_state_home: snapshot.old_xdg_state_home,
        old_xdg_data_home: snapshot.old_xdg_data_home,
        old_xdg_config_home: snapshot.old_xdg_config_home,
        old_home: snapshot.old_home,
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        restore_env("OULIPOLY_DATA_DIR", self.old_oulipoly_data_dir.take());
        restore_env("XDG_STATE_HOME", self.old_xdg_state_home.take());
        restore_env("XDG_DATA_HOME", self.old_xdg_data_home.take());
        restore_env("XDG_CONFIG_HOME", self.old_xdg_config_home.take());
        restore_env("HOME", self.old_home.take());
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    _env: EnvGuard,
    data_dir: PathBuf,
    state_home: PathBuf,
}

struct FixturePaths {
    data_dir: PathBuf,
    state_home: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let paths = fixture_paths(dir.path());
        create_fixture_dirs(&paths);
        let env = EnvGuard::set(&paths.data_dir, &paths.state_home, &paths.home);
        Self {
            _dir: dir,
            _env: env,
            data_dir: paths.data_dir,
            state_home: paths.state_home,
        }
    }

    fn state_path(&self) -> PathBuf {
        self.data_dir.join("state.db")
    }

    fn sidecar_path(&self) -> PathBuf {
        self.data_dir.join("pid-identity.db")
    }

    fn agent_bash_root(&self) -> PathBuf {
        self.state_home.join("agent-bash")
    }

    fn open_state(&self) -> StateDb {
        StateDb::open(&self.state_path()).unwrap()
    }

    fn open_mailbox(&self) -> MailboxDb {
        MailboxDb::open(&self.sidecar_path()).unwrap()
    }

    fn open_pid(&self) -> PidIdentityDb {
        PidIdentityDb::open(&self.sidecar_path()).unwrap()
    }

    fn service(&self) -> ProductionObservabilitySnapshotService {
        ProductionObservabilitySnapshotService::default()
    }

    fn root(&self) -> ObservabilityRoot {
        ObservabilityRoot {
            invocation_uuid: Some(ROOT_UUID.to_string()),
            session_id: Some(SESSION_ID.to_string()),
            provider_name: Some("provider-a".to_string()),
            model_name: Some("model-a".to_string()),
        }
    }
}

fn fixture_paths(root: &Path) -> FixturePaths {
    FixturePaths {
        data_dir: root.join("data"),
        state_home: root.join("state"),
        home: root.join("home"),
    }
}

fn create_fixture_dirs(paths: &FixturePaths) {
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    std::fs::create_dir_all(&paths.state_home).unwrap();
    std::fs::create_dir_all(&paths.home).unwrap();
}

#[test]
fn provider_invocation_process_liveness_distinguishes_verified_live_and_dead() {
    let fixture = Fixture::new();
    let state = fixture.open_state();
    let root_id = seed_invocation(&state, ROOT_UUID, None);
    seed_invocation(&state, CHILD_UUID, Some(root_id));
    state
        .update_session_capture(root_id, Some(SESSION_ID), "stdout-json")
        .unwrap();
    drop(state);
    let pid = fixture.open_pid();
    let live_identity = current_identity();
    record_identity(&pid, ROOT_UUID, Some(SESSION_ID), &live_identity);
    record_identity(&pid, CHILD_UUID, Some(SESSION_ID), &dead_identity());
    drop(pid);

    let snapshot = fixture
        .service()
        .snapshot(&fixture.root(), full_snapshot_limits());

    let live = node(
        &snapshot,
        &format!("process:{ROOT_UUID}:{}", live_identity.os_pid),
    );
    assert_eq!(live.kind, MonitorNodeKind::ProviderProcess);
    assert_eq!(live.status, MonitorStatus::Running);
    assert_eq!(live.liveness, LivenessStatus::VerifiedLive);
    let dead = node(
        &snapshot,
        "process:22222222-2222-4222-8222-222222222222:999999999",
    );
    assert_eq!(dead.liveness, LivenessStatus::Dead);
    assert_eq!(dead.status, MonitorStatus::Stale);
}

#[test]
fn running_invocation_with_dead_pid_is_reconciled_to_stale_not_running() {
    let fixture = Fixture::new();
    let state = fixture.open_state();
    let root_id = seed_invocation(&state, ROOT_UUID, None);
    seed_invocation(&state, CHILD_UUID, Some(root_id));
    state
        .update_session_capture(root_id, Some(SESSION_ID), "stdout-json")
        .unwrap();
    drop(state);
    let pid = fixture.open_pid();
    record_identity(&pid, ROOT_UUID, Some(SESSION_ID), &current_identity());
    record_identity(&pid, CHILD_UUID, Some(SESSION_ID), &dead_identity());
    drop(pid);

    let snapshot = fixture
        .service()
        .snapshot(&fixture.root(), full_snapshot_limits());

    // The live-backed running invocation stays Running; the running-status
    // invocation whose process is dead is reconciled to Stale (it died without
    // finalizing) and must NOT be presented as running.
    let root_inv = node(&snapshot, &format!("invocation:{ROOT_UUID}"));
    assert_eq!(root_inv.status, MonitorStatus::Running);
    let child_inv = node(&snapshot, &format!("invocation:{CHILD_UUID}"));
    assert_eq!(child_inv.status, MonitorStatus::Stale);
    assert!(
        !snapshot
            .nodes
            .iter()
            .any(|node| node.id == format!("invocation:{CHILD_UUID}")
                && node.status == MonitorStatus::Running),
        "dead-pid running invocation must not be counted as running"
    );
}

#[test]
fn active_session_nodes_point_inspect_at_live_transcript_when_resolvable() {
    let fixture = Fixture::new();
    let state = fixture.open_state();
    let root_id = seed_invocation(&state, ROOT_UUID, None);
    state
        .update_session_capture(root_id, Some(SESSION_ID), "stdout-json")
        .unwrap();
    drop(state);
    let pid = fixture.open_pid();
    record_identity(&pid, ROOT_UUID, Some(SESSION_ID), &current_identity());
    drop(pid);

    // A ClaudeCode-storage transcript whose filename stem is the session id is
    // what the storage locator resolves the active session to.
    let projects_dir = fixture.data_dir.join("transcript-projects");
    let project_dir = projects_dir.join("-home-nes-proj");
    std::fs::create_dir_all(&project_dir).unwrap();
    std::fs::write(
        project_dir.join(format!("{SESSION_ID}.jsonl")),
        "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hi\"}]}}\n",
    )
    .unwrap();

    let service =
        ProductionObservabilitySnapshotService::for_session(Some(SessionStorage::ClaudeCode {
            projects_dir,
        }));
    let snapshot = service.snapshot(&fixture.root(), SnapshotLimits::default());

    let session = node(&snapshot, "session:session-observe");
    assert!(
        matches!(
            session.inspect_ref,
            Some(InspectRef::SessionTranscript { .. })
        ),
        "session node should stream the live transcript, got {:?}",
        session.inspect_ref
    );
    let root_inv = node(&snapshot, &format!("invocation:{ROOT_UUID}"));
    assert!(
        matches!(
            root_inv.inspect_ref,
            Some(InspectRef::SessionTranscript { .. })
        ),
        "root invocation node should stream the live transcript, got {:?}",
        root_inv.inspect_ref
    );
    let process = node(
        &snapshot,
        &format!("process:{ROOT_UUID}:{}", current_identity().os_pid),
    );
    assert!(
        matches!(
            process.inspect_ref,
            Some(InspectRef::SessionTranscript { .. })
        ),
        "root process node should stream the live transcript, got {:?}",
        process.inspect_ref
    );
}

#[test]
fn stale_runtime_snapshot_emits_diagnostic_without_mutating_runtime_row() {
    let fixture = Fixture::new();
    let state = fixture.open_state();
    let root_id = seed_invocation(&state, ROOT_UUID, None);
    state
        .update_session_capture(root_id, Some(SESSION_ID), "stdout-json")
        .unwrap();
    drop(state);
    let mut stale = current_identity();
    stale.os_pid_starttime_ticks += 1;
    let mut mailbox = fixture.open_mailbox();
    mailbox
        .mark_session_running(SessionRuntimeRunningUpdate {
            session_id: SESSION_ID,
            mode: "pty_interactive",
            invocation_uuid: ROOT_UUID,
            provider_name: Some("provider-a"),
            model_name: Some("model-a"),
            identity: &stale,
            pty_control_path: Some("/tmp/oulipoly-observe.sock"),
            turn_start_max_mailbox_seq: None,
            models_dir: None,
            effective_cwd: Some("/tmp/work"),
        })
        .unwrap();
    drop(mailbox);
    let before = runtime_row_bytes(&fixture.sidecar_path(), SESSION_ID);

    let snapshot = fixture
        .service()
        .snapshot(&fixture.root(), full_snapshot_limits());

    assert!(has_diagnostic(&snapshot, "stale-runtime"));
    let session = node(&snapshot, "session:session-observe");
    assert_eq!(session.status, MonitorStatus::Stale);
    assert_eq!(session.liveness, LivenessStatus::PidReused);
    assert_eq!(
        runtime_row_bytes(&fixture.sidecar_path(), SESSION_ID),
        before
    );
}

#[test]
fn pending_mailbox_without_claim_is_reported_as_stuck() {
    let fixture = Fixture::new();
    seed_root_session(&fixture);
    let mut mailbox = fixture.open_mailbox();
    mailbox
        .upsert_session_runtime(SessionRuntimeUpsert {
            session_id: SESSION_ID,
            mode: "pty_interactive",
            invocation_uuid: Some(ROOT_UUID),
            provider_name: Some("provider-a"),
            model_name: Some("model-a"),
            pty_control_path: Some("/tmp/oulipoly-observe.sock"),
            models_dir: None,
            effective_cwd: None,
        })
        .unwrap();
    mailbox
        .enqueue_agent_bash_complete(&mailbox_input("handle-pending", SESSION_ID))
        .unwrap();
    drop(mailbox);

    let snapshot = fixture
        .service()
        .snapshot(&fixture.root(), SnapshotLimits::default());

    assert!(has_diagnostic(&snapshot, "stuck:pending-no-claim"));
    let row = snapshot
        .nodes
        .iter()
        .find(|node| node.id == "mailbox:session-observe:1")
        .unwrap();
    assert_eq!(row.kind, MonitorNodeKind::MailboxNotification);
    assert_eq!(row.status, MonitorStatus::Pending);
}

#[test]
fn wake_claim_with_dead_pid_is_reported_as_claim_dead() {
    let fixture = Fixture::new();
    seed_root_session(&fixture);
    let mut mailbox = fixture.open_mailbox();
    mailbox
        .enqueue_agent_bash_complete(&mailbox_input("handle-pending", SESSION_ID))
        .unwrap();
    let claim = mailbox
        .try_acquire_wake_claim(WakeClaimRequest {
            session_id: SESSION_ID,
            claim_token: "claim-a",
            reason: "notify_idle",
            auto_wake_count: 1,
            wake_invocation_uuid: Some("wake-invocation"),
            stale_after_seconds: 600,
        })
        .unwrap();
    assert!(matches!(claim, WakeClaimAcquireResult::Acquired(_)));
    assert!(
        mailbox
            .record_wake_claim_pid(SESSION_ID, "claim-a", 999_999_999)
            .unwrap()
    );
    drop(mailbox);

    let snapshot = fixture
        .service()
        .snapshot(&fixture.root(), full_snapshot_limits());

    assert!(has_diagnostic(&snapshot, "stuck:claim-dead"));
    let wake = node(&snapshot, "wake:session-observe:claim-a");
    assert_eq!(wake.kind, MonitorNodeKind::WakeClaim);
    assert_eq!(wake.liveness, LivenessStatus::Dead);
}

#[test]
fn missing_runtime_with_pending_mailbox_reports_wake_needed() {
    let fixture = Fixture::new();
    seed_root_session(&fixture);
    let mut mailbox = fixture.open_mailbox();
    mailbox
        .enqueue_agent_bash_complete(&mailbox_input("handle-pending", SESSION_ID))
        .unwrap();
    drop(mailbox);

    let snapshot = fixture
        .service()
        .snapshot(&fixture.root(), SnapshotLimits::default());

    assert!(has_diagnostic(&snapshot, "wake-needed:no-runtime"));
}

#[test]
fn default_snapshot_hides_terminal_nodes_and_explicit_full_snapshot_keeps_them() {
    let fixture = Fixture::new();
    seed_root_session(&fixture);
    let pid = fixture.open_pid();
    let owner = current_identity();
    record_identity(&pid, ROOT_UUID, Some(SESSION_ID), &owner);
    drop(pid);
    write_agent_bash_meta(
        &fixture.agent_bash_root(),
        "done-workload",
        &agent_bash_meta("done-workload", "DONE", &owner, Some(778), Some(0)),
        "done tail",
    );

    assert!(!SnapshotLimits::default().include_terminal);
    let live_only = fixture
        .service()
        .snapshot(&fixture.root(), SnapshotLimits::default());
    let full = fixture
        .service()
        .snapshot(&fixture.root(), full_snapshot_limits());

    assert!(find_node(&live_only, "agent-bash:done-workload").is_none());
    assert_eq!(
        node(&full, "agent-bash:done-workload").status,
        MonitorStatus::Succeeded
    );
    assert!(full.nodes.len() > live_only.nodes.len());
}

#[test]
fn agent_bash_scan_is_bounded_filters_unrelated_and_degrades_corrupt_meta() {
    let fixture = Fixture::new();
    seed_root_session(&fixture);
    let pid = fixture.open_pid();
    let owner = current_identity();
    let unrelated = ProcessIdentity {
        os_pid: 888_888_888,
        os_boot_id: "boot-other".to_string(),
        os_pid_starttime_ticks: 88,
    };
    record_identity(&pid, ROOT_UUID, Some(SESSION_ID), &owner);
    record_identity(&pid, "other-invocation", Some("other-session"), &unrelated);
    drop(pid);
    let root = fixture.agent_bash_root();
    let corrupt_dir = write_agent_bash_meta(&root, "zz-corrupt", "not-json", "corrupt log");
    set_dir_mtime(&corrupt_dir, 60);
    let running_dir = write_agent_bash_meta(
        &root,
        "yy-running",
        &agent_bash_meta("yy-running", "RUNNING", &owner, Some(777), None),
        "running tail",
    );
    set_dir_mtime(&running_dir, 50);
    let done_dir = write_agent_bash_meta(
        &root,
        "xx-done",
        &agent_bash_meta("xx-done", "DONE", &owner, Some(778), Some(0)),
        "done tail",
    );
    set_dir_mtime(&done_dir, 40);
    let error_dir = write_agent_bash_meta(
        &root,
        "ww-error",
        &agent_bash_meta("ww-error", "ERROR", &owner, Some(779), Some(2)),
        "error tail",
    );
    set_dir_mtime(&error_dir, 30);
    let old_dir = write_agent_bash_meta(
        &root,
        "vv-old",
        &agent_bash_meta("vv-old", "RUNNING", &owner, Some(780), None),
        "old tail",
    );
    set_dir_mtime(&old_dir, 10);
    let other_dir = write_agent_bash_meta(
        &root,
        "uu-other",
        &agent_bash_meta("uu-other", "RUNNING", &unrelated, Some(781), None),
        "other tail",
    );
    set_dir_mtime(&other_dir, 20);

    let snapshot = fixture.service().snapshot(
        &fixture.root(),
        SnapshotLimits {
            include_terminal: true,
            agent_bash_scan_dirs: 4,
            log_tail_bytes: 64,
            ..SnapshotLimits::default()
        },
    );

    assert!(has_diagnostic(&snapshot, "agent-bash:meta-corrupt"));
    assert_eq!(
        node(&snapshot, "agent-bash:yy-running").status,
        MonitorStatus::Running
    );
    assert_eq!(
        node(&snapshot, "agent-bash:xx-done").status,
        MonitorStatus::Succeeded
    );
    assert_eq!(
        node(&snapshot, "agent-bash:ww-error").status,
        MonitorStatus::Error
    );
    assert!(
        snapshot
            .nodes
            .iter()
            .all(|node| node.id != "agent-bash:vv-old")
    );
    assert!(
        snapshot
            .nodes
            .iter()
            .all(|node| node.id != "agent-bash:uu-other")
    );
    assert_eq!(
        node(&snapshot, "agent-bash:yy-running")
            .last_output_excerpt
            .as_deref(),
        Some("running tail")
    );
}

#[test]
fn agent_bash_mailbox_referenced_state_dir_is_included_even_outside_scan_limit() {
    let fixture = Fixture::new();
    seed_root_session(&fixture);
    let pid = fixture.open_pid();
    let owner = current_identity();
    record_identity(&pid, ROOT_UUID, Some(SESSION_ID), &owner);
    drop(pid);
    let root = fixture.agent_bash_root();
    write_agent_bash_meta(
        &root,
        "zz-newer",
        &agent_bash_meta("zz-newer", "RUNNING", &owner, Some(700), None),
        "newer tail",
    );
    let old_dir = write_agent_bash_meta(
        &root,
        "aa-mailbox-old",
        &agent_bash_meta("aa-mailbox-old", "DONE", &owner, Some(701), Some(0)),
        "mailbox tail",
    );
    let mut mailbox = fixture.open_mailbox();
    mailbox
        .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
            state_dir: old_dir.to_str().unwrap(),
            meta_path: old_dir.join("meta.json").to_str().unwrap(),
            log_path: old_dir.join("log").to_str().unwrap(),
            rc_path: old_dir.join("rc").to_str().unwrap(),
            ..mailbox_input("aa-mailbox-old", SESSION_ID)
        })
        .unwrap();
    drop(mailbox);

    let snapshot = fixture.service().snapshot(
        &fixture.root(),
        SnapshotLimits {
            include_terminal: true,
            agent_bash_scan_dirs: 1,
            log_tail_bytes: 64,
            ..SnapshotLimits::default()
        },
    );

    assert_eq!(
        node(&snapshot, "agent-bash:aa-mailbox-old").status,
        MonitorStatus::Succeeded
    );
}

fn restore_env(name: &str, value: Option<OsString>) {
    match value {
        Some(value) => unsafe {
            std::env::set_var(name, value);
        },
        None => unsafe {
            std::env::remove_var(name);
        },
    }
}

fn full_snapshot_limits() -> SnapshotLimits {
    SnapshotLimits {
        include_terminal: true,
        ..SnapshotLimits::default()
    }
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn seed_root_session(fixture: &Fixture) {
    let state = fixture.open_state();
    let root_id = seed_invocation(&state, ROOT_UUID, None);
    state
        .update_session_capture(root_id, Some(SESSION_ID), "stdout-json")
        .unwrap();
}

fn seed_invocation(db: &StateDb, uuid: &str, parent: Option<i64>) -> i64 {
    db.start_invocation(&InvocationStart {
        invocation_uuid: uuid.to_string(),
        model_name: "model-a".to_string(),
        provider_name: "provider-a".to_string(),
        provider_index: 0,
        parent_invocation_id: parent,
    })
    .unwrap()
}

fn current_identity() -> ProcessIdentity {
    expect_live_identity(read_current_identity().unwrap())
}

fn read_current_identity() -> Result<Option<ProcessIdentity>, String> {
    oulipoly_state::pid_identity::read_live_process_identity(i64::from(std::process::id()))
}

fn expect_live_identity(identity: Option<ProcessIdentity>) -> ProcessIdentity {
    identity.unwrap()
}

fn dead_identity() -> ProcessIdentity {
    ProcessIdentity {
        os_pid: 999_999_999,
        os_boot_id: "boot-dead".to_string(),
        os_pid_starttime_ticks: 99,
    }
}

fn record_identity(
    db: &PidIdentityDb,
    invocation_uuid: &str,
    session_id: Option<&str>,
    identity: &ProcessIdentity,
) {
    db.record_identity(pid_identity_record(invocation_uuid, session_id, identity))
        .unwrap();
}

fn pid_identity_record<'a>(
    invocation_uuid: &'a str,
    session_id: Option<&'a str>,
    identity: &'a ProcessIdentity,
) -> PidIdentityRecord<'a> {
    PidIdentityRecord {
        identity,
        os_pgid: Some(identity.os_pid),
        invocation_uuid,
        session_id,
        provider_name: Some("provider-a"),
        model_name: Some("model-a"),
        recorded_at: "2026-06-08T12:00:00Z",
    }
}

fn mailbox_input<'a>(handle: &'a str, session_id: &'a str) -> AgentBashCompleteEnqueue<'a> {
    AgentBashCompleteEnqueue {
        session_id,
        handle,
        payload_json: r#"{"schema_version":1,"kind":"agent_bash_complete"}"#,
        owner_invocation_uuid: Some(ROOT_UUID),
        matched_os_pid: Some(42),
        matched_os_boot_id: Some("boot-a"),
        matched_os_pid_starttime_ticks: Some(7),
        matched_chain_index: Some(0),
        state_dir: "/tmp/state",
        meta_path: "/tmp/state/meta.json",
        log_path: "/tmp/state/log",
        rc_path: "/tmp/state/rc",
        rc: 0,
    }
}

fn runtime_row_bytes(path: &Path, session_id: &str) -> Vec<u8> {
    runtime_row_text(path, session_id).into_bytes()
}

fn runtime_row_text(path: &Path, session_id: &str) -> String {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    query_runtime_row_text(&conn, session_id)
}

fn query_runtime_row_text(conn: &rusqlite::Connection, session_id: &str) -> String {
    conn.query_row(runtime_row_text_query(), [session_id], |row| row.get(0))
        .unwrap()
}

fn runtime_row_text_query() -> &'static str {
    "SELECT COALESCE(session_id, '') || x'1f' || COALESCE(mode, '') || x'1f' ||
            COALESCE(invocation_uuid, '') || x'1f' || COALESCE(provider_name, '') || x'1f' ||
            COALESCE(model_name, '') || x'1f' || COALESCE(pty_control_path, '') || x'1f' ||
            COALESCE(updated_at, '') || x'1f' || COALESCE(run_state, '') || x'1f' ||
            COALESCE(running_invocation_uuid, '') || x'1f' || COALESCE(running_os_pid, '') || x'1f' ||
            COALESCE(running_os_boot_id, '') || x'1f' || COALESCE(running_os_pid_starttime_ticks, '') || x'1f' ||
            COALESCE(turn_started_at, '') || x'1f' || COALESCE(turn_ended_at, '') || x'1f' ||
            COALESCE(turn_start_max_mailbox_seq, '') || x'1f' || COALESCE(last_exit_code, '') || x'1f' ||
            COALESCE(models_dir, '') || x'1f' || COALESCE(effective_cwd, '')
     FROM session_runtime WHERE session_id = ?1"
}

fn write_agent_bash_meta(root: &Path, handle: &str, meta: &str, log: &str) -> PathBuf {
    let state_dir = agent_bash_state_dir(root, handle);
    write_agent_bash_meta_files(&state_dir, meta, log);
    state_dir
}

fn agent_bash_state_dir(root: &Path, handle: &str) -> PathBuf {
    root.join(handle)
}

fn write_agent_bash_meta_files(state_dir: &Path, meta: &str, log: &str) {
    std::fs::create_dir_all(state_dir).unwrap();
    std::fs::write(state_dir.join("meta.json"), meta).unwrap();
    std::fs::write(state_dir.join("log"), log).unwrap();
    std::fs::write(state_dir.join("rc"), "0").unwrap();
}

#[cfg(unix)]
fn set_dir_mtime(path: &Path, seconds: i64) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes()).unwrap();
    let times = dir_mtime_times(seconds);
    set_dir_mtime_raw(c_path.as_ptr(), times.as_ptr(), path);
}

#[cfg(unix)]
fn dir_mtime_times(seconds: i64) -> [libc::timespec; 2] {
    [
        libc::timespec {
            tv_sec: seconds,
            tv_nsec: 0,
        },
        libc::timespec {
            tv_sec: seconds,
            tv_nsec: 0,
        },
    ]
}

#[cfg(unix)]
fn set_dir_mtime_raw(path_ptr: *const libc::c_char, times_ptr: *const libc::timespec, path: &Path) {
    let rc = unsafe { libc::utimensat(libc::AT_FDCWD, path_ptr, times_ptr, 0) };
    assert_eq!(rc, 0, "failed to set mtime for {}", path.display());
}

#[cfg(not(unix))]
fn set_dir_mtime(_path: &Path, _seconds: i64) {}

fn agent_bash_meta(
    handle: &str,
    state: &str,
    identity: &ProcessIdentity,
    workload_pid: Option<i64>,
    rc: Option<i32>,
) -> String {
    agent_bash_meta_json(handle, state, identity, workload_pid, rc).to_string()
}

fn agent_bash_meta_json(
    handle: &str,
    state: &str,
    identity: &ProcessIdentity,
    workload_pid: Option<i64>,
    rc: Option<i32>,
) -> serde_json::Value {
    serde_json::json!({
        "handle": handle,
        "state": state,
        "caller_chain": [{
            "pid": identity.os_pid,
            "boot_id": identity.os_boot_id,
            "starttime_ticks": identity.os_pid_starttime_ticks,
        }],
        "supervisor_pid": workload_pid.map(|pid| pid + 10),
        "workload_pid": workload_pid,
        "workload_pgid": workload_pid,
        "argv": ["bash", "-lc", "echo hi"],
        "cwd": "/tmp/work",
        "ready_at": "2026-06-08T12:00:00Z",
        "completed_at": rc.map(|_| "2026-06-08T12:01:00Z"),
        "rc": rc,
        "log_path": null,
    })
}

fn has_diagnostic(snapshot: &oulipoly_runtime::observability::MonitorSnapshot, code: &str) -> bool {
    snapshot
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == code)
}

fn node<'a>(
    snapshot: &'a oulipoly_runtime::observability::MonitorSnapshot,
    id: &str,
) -> &'a oulipoly_runtime::observability::MonitorNode {
    expect_node(find_node(snapshot, id), snapshot, id)
}

fn find_node<'a>(
    snapshot: &'a oulipoly_runtime::observability::MonitorSnapshot,
    id: &str,
) -> Option<&'a oulipoly_runtime::observability::MonitorNode> {
    snapshot.nodes.iter().find(|node| node.id == id)
}

fn expect_node<'a>(
    node: Option<&'a oulipoly_runtime::observability::MonitorNode>,
    snapshot: &oulipoly_runtime::observability::MonitorSnapshot,
    id: &str,
) -> &'a oulipoly_runtime::observability::MonitorNode {
    node.unwrap_or_else(|| panic!("missing node {id}; nodes: {:#?}", snapshot.nodes))
}
