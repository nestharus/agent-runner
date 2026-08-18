//! ## Declared roles
//!
//! `accessor`, `formatter`, `mapper`, `orchestration`, `predicate`, `validator`

use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, SessionStorage,
    provider_implementation_ref::ProviderImplementationRef,
};
use oulipoly_runtime::observability::{
    InspectRef, LivenessStatus, MonitorNodeKind, MonitorStatus, ObservabilityRoot,
    ObservabilitySnapshotPort, ProductionObservabilitySnapshotService, SnapshotLimits,
};
use oulipoly_runtime::provider_registry::{ProviderRegistry, ProviderRegistryOptions};
use oulipoly_runtime::session_provider::SessionProviderIdentity;
use oulipoly_state::mailbox::{
    AgentBashCompleteEnqueue, CreateRuntimeGeneration, EnqueueResult, MailboxDb,
    RuntimeGenerationId, SessionRuntimeRunningUpdate, SessionRuntimeUpsert,
    WAKE_SWEEP_ABANDONED_ERROR, WakeClaimAcquireResult, WakeClaimRequest,
};
use oulipoly_state::pid_identity::{PidIdentityDb, PidIdentityRecord, ProcessIdentity};
use oulipoly_state::{InvocationStart, StateDb};
#[cfg(unix)]
use serde_json::Value;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::{Child, Command};
use std::sync::{Mutex, MutexGuard, OnceLock};

const ROOT_UUID: &str = "11111111-1111-4111-8111-111111111111";
const CHILD_UUID: &str = "22222222-2222-4222-8222-222222222222";
const SESSION_ID: &str = "session-observe";
#[cfg(target_os = "linux")]
const LIVE_CHILD_UUID: &str = "33333333-3333-4333-8333-333333333333";
#[cfg(target_os = "linux")]
const DEAD_CHILD_UUID: &str = "44444444-4444-4444-8444-444444444444";
#[cfg(target_os = "linux")]
const MISSING_CHILD_UUID: &str = "55555555-5555-4555-8555-555555555555";
#[cfg(target_os = "linux")]
const MISMATCHED_CHILD_UUID: &str = "66666666-6666-4666-8666-666666666666";
#[cfg(target_os = "linux")]
const UNRELATED_UUID: &str = "77777777-7777-4777-8777-777777777777";
#[cfg(target_os = "linux")]
const TERMINAL_DESCENDANT_COUNT: usize = 201;

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

#[derive(Debug, PartialEq, Eq)]
struct PhysicalFileSnapshot {
    directories: Vec<(PathBuf, std::time::SystemTime, bool)>,
    files: Vec<(PathBuf, Vec<u8>)>,
}

fn physical_file_snapshot(root: &Path) -> PhysicalFileSnapshot {
    fn collect(root: &Path, current: &Path, snapshot: &mut PhysicalFileSnapshot) {
        let metadata = std::fs::metadata(current).unwrap();
        snapshot.directories.push((
            current.strip_prefix(root).unwrap().to_path_buf(),
            metadata.modified().unwrap(),
            metadata.permissions().readonly(),
        ));
        for entry in std::fs::read_dir(current).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                collect(root, &path, snapshot);
            } else {
                snapshot.files.push((
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(path).unwrap(),
                ));
            }
        }
    }

    let mut snapshot = PhysicalFileSnapshot {
        directories: Vec::new(),
        files: Vec::new(),
    };
    collect(root, root, &mut snapshot);
    snapshot
        .directories
        .sort_by(|left, right| left.0.cmp(&right.0));
    snapshot.files.sort_by(|left, right| left.0.cmp(&right.0));
    snapshot
}

fn assert_physical_files_unchanged(before: &PhysicalFileSnapshot, after: &PhysicalFileSnapshot) {
    assert_eq!(after.directories, before.directories, "directories changed");
    assert_eq!(
        after.files.iter().map(|entry| &entry.0).collect::<Vec<_>>(),
        before
            .files
            .iter()
            .map(|entry| &entry.0)
            .collect::<Vec<_>>(),
        "physical file inventory changed"
    );
    for ((before_path, before_bytes), (after_path, after_bytes)) in
        before.files.iter().zip(&after.files)
    {
        assert_eq!(after_path, before_path);
        assert!(
            after_bytes == before_bytes,
            "physical bytes changed for {} (before={} after={})",
            before_path.display(),
            before_bytes.len(),
            after_bytes.len()
        );
    }
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

#[cfg(target_os = "linux")]
#[test]
fn overlay_counts_live_logical_child_after_terminal_history_and_fails_closed() {
    let scenario = overlay_logical_child_scenario();
    assert!(TERMINAL_DESCENDANT_COUNT > SnapshotLimits::default().max_invocation_nodes);
    assert_ne!(
        process_parent_pid(scenario.logical_child_process.pid()),
        scenario.root_process.pid(),
        "the logical child launcher must not be a physical child of the root process"
    );

    let snapshot = scenario
        .fixture
        .service()
        .snapshot(&scenario.fixture.root(), SnapshotLimits::default());

    assert_eq!(
        snapshot.summary.running_nodes, 4,
        "the overlay count must include the root and detached logical child invocation/process pairs: {:#?}",
        snapshot.nodes
    );
    assert_verified_running_process(&snapshot, ROOT_UUID, scenario.root_process.identity());
    assert_verified_running_process(
        &snapshot,
        LIVE_CHILD_UUID,
        scenario.logical_child_process.identity(),
    );
    assert_invocation_absent(&snapshot, DEAD_CHILD_UUID);
    assert_invocation_absent(&snapshot, MISSING_CHILD_UUID);
    assert_invocation_absent(&snapshot, MISMATCHED_CHILD_UUID);
    assert_invocation_absent(&snapshot, UNRELATED_UUID);
}

#[cfg(target_os = "linux")]
#[test]
fn delivered_wake_edge_keeps_live_workload_under_original_root() {
    let fixture = Fixture::new();
    let wake_process = TestProcess::spawn();
    let workload_process = TestProcess::spawn();
    let state = fixture.open_state();
    let root_id = seed_invocation(&state, ROOT_UUID, None);
    let owner_id = seed_invocation(&state, CHILD_UUID, Some(root_id));
    state
        .finalize_invocation(owner_id, true, 0, None, Some("completed"))
        .unwrap();
    let wake_id = seed_invocation(&state, LIVE_CHILD_UUID, None);
    state
        .finalize_invocation(wake_id, true, 0, None, Some("completed"))
        .unwrap();
    state
        .update_session_capture(root_id, Some(SESSION_ID), "stdout-json")
        .unwrap();
    drop(state);

    let pid = fixture.open_pid();
    record_identity(&pid, ROOT_UUID, Some(SESSION_ID), &current_identity());
    record_identity(
        &pid,
        LIVE_CHILD_UUID,
        Some(SESSION_ID),
        wake_process.identity(),
    );
    drop(pid);

    let handle = "ab_2_100_0000000000000003";
    write_agent_bash_meta(
        &fixture.agent_bash_root(),
        handle,
        &agent_bash_meta_with_workload_identity(
            handle,
            wake_process.identity(),
            workload_process.identity(),
        ),
        "wake workload running",
    );
    let mut mailbox = fixture.open_mailbox();
    let row = match mailbox
        .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
            owner_invocation_uuid: Some(CHILD_UUID),
            ..mailbox_input("wake-edge", SESSION_ID)
        })
        .unwrap()
    {
        EnqueueResult::Inserted(row) => row,
        result => panic!("unexpected enqueue result: {result:?}"),
    };
    mailbox
        .mark_delivered(SESSION_ID, &[row.seq], LIVE_CHILD_UUID)
        .unwrap();
    drop(mailbox);

    let snapshot = fixture
        .service()
        .snapshot(&fixture.root(), SnapshotLimits::default());

    let owner_node_id = format!("invocation:{CHILD_UUID}");
    let wake_node_id = format!("invocation:{LIVE_CHILD_UUID}");
    assert_eq!(
        node(&snapshot, &wake_node_id).parent_id.as_deref(),
        Some(owner_node_id.as_str())
    );
    let workload = node(&snapshot, &format!("agent-bash:{handle}"));
    assert_eq!(workload.status, MonitorStatus::Running);
    assert_eq!(workload.parent_id.as_deref(), Some(wake_node_id.as_str()));
}

#[test]
fn active_session_nodes_point_inspect_at_live_transcript_when_resolvable() {
    let fixture = Fixture::new();
    #[cfg(unix)]
    let unrelated_record_path = prepare_unrelated_registry_records(&fixture);
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
    let transcript_path = project_dir.join(format!("{SESSION_ID}.jsonl"));
    std::fs::write(
        &transcript_path,
        "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hi\"}]}}\n",
    )
    .unwrap();

    let service =
        ProductionObservabilitySnapshotService::for_session(Some(SessionStorage::ClaudeCode {
            projects_dir,
        }));
    let snapshot = service.snapshot(&fixture.root(), SnapshotLimits::default());

    let session = node(&snapshot, "session:session-observe");
    assert_session_transcript_path(session, &transcript_path);
    let root_inv = node(&snapshot, &format!("invocation:{ROOT_UUID}"));
    assert_session_transcript_path(root_inv, &transcript_path);
    let process = node(
        &snapshot,
        &format!("process:{ROOT_UUID}:{}", current_identity().os_pid),
    );
    assert_session_transcript_path(process, &transcript_path);
    #[cfg(unix)]
    assert_record_file_empty(&unrelated_record_path);
}

#[test]
fn active_session_nodes_do_not_attach_transcript_inspect_ref_without_local_transcript() {
    let fixture = Fixture::new();
    #[cfg(unix)]
    let unrelated_record_path = prepare_unrelated_registry_records(&fixture);
    let state = fixture.open_state();
    let root_id = seed_invocation(&state, ROOT_UUID, None);
    state
        .update_session_capture(root_id, Some(SESSION_ID), "stdout-json")
        .unwrap();
    drop(state);
    let pid = fixture.open_pid();
    let identity = current_identity();
    record_identity(&pid, ROOT_UUID, Some(SESSION_ID), &identity);
    drop(pid);

    let service = ProductionObservabilitySnapshotService::for_session(None);
    let snapshot = service.snapshot(&fixture.root(), SnapshotLimits::default());

    assert_eq!(node(&snapshot, "session:session-observe").inspect_ref, None);
    let invocation = node(&snapshot, &format!("invocation:{ROOT_UUID}"));
    assert!(matches!(
        invocation.inspect_ref,
        Some(InspectRef::InvocationStatus { .. })
    ));
    assert!(!matches!(
        invocation.inspect_ref,
        Some(InspectRef::SessionTranscript { .. })
    ));
    let process = node(
        &snapshot,
        &format!("process:{ROOT_UUID}:{}", identity.os_pid),
    );
    assert!(!matches!(
        process.inspect_ref,
        Some(InspectRef::SessionTranscript { .. })
    ));
    #[cfg(unix)]
    assert_record_file_empty(&unrelated_record_path);
}

#[cfg(unix)]
#[test]
fn provider_inspect_source_attaches_external_transcript_and_request_metadata() {
    assert_observability_service_has_provider_inspect_source_wiring();

    let fixture = Fixture::new();
    let identity = seed_running_root(&fixture);
    let record_path = fixture.data_dir.join("provider-records.jsonl");
    let external_path = fixture.data_dir.join("external-inspect-session.jsonl");
    std::fs::write(&external_path, "{\"type\":\"assistant\"}\n").unwrap();
    let provider_path = write_external_locate_provider(&fixture, &record_path, &external_path);
    let registry = external_registry(&provider_path);
    let effective_cwd = fixture.data_dir.join("provider-work");
    std::fs::create_dir_all(&effective_cwd).unwrap();
    let limits = SnapshotLimits::default();
    let service = ProductionObservabilitySnapshotService::for_provider_inspect(
        registry,
        external_identity(),
        SESSION_ID.to_string(),
        Some(effective_cwd.clone()),
    );

    let snapshot = service.snapshot(&fixture.root(), limits);

    let expected_path = external_path.canonicalize().unwrap();
    assert_session_transcript_ref(
        node(&snapshot, "session:session-observe"),
        &expected_path,
        limits.transcript_tail_bytes,
        Some("canonical-transcript-v1"),
        Some("provider-a"),
    );
    assert_session_transcript_ref(
        node(&snapshot, &format!("invocation:{ROOT_UUID}")),
        &expected_path,
        limits.transcript_tail_bytes,
        Some("canonical-transcript-v1"),
        Some("provider-a"),
    );
    assert_session_transcript_ref(
        node(
            &snapshot,
            &format!("process:{ROOT_UUID}:{}", identity.os_pid),
        ),
        &expected_path,
        limits.transcript_tail_bytes,
        Some("canonical-transcript-v1"),
        Some("provider-a"),
    );
    assert_eq!(
        recorded_subcommand_count(&record_path, "session.locate_transcript"),
        1
    );
    assert_provider_inspect_request(&record_path, &effective_cwd, limits.transcript_tail_bytes);
}

#[cfg(unix)]
#[test]
fn provider_inspect_failure_does_not_attach_local_transcript() {
    let fixture = Fixture::new();
    let identity = seed_running_root(&fixture);
    let local = write_local_transcript_tree(&fixture, SESSION_ID);
    let local_probe =
        ProductionObservabilitySnapshotService::for_session(Some(SessionStorage::ClaudeCode {
            projects_dir: local.projects_dir.clone(),
        }));
    let local_snapshot = local_probe.snapshot(&fixture.root(), SnapshotLimits::default());
    assert_session_transcript_path(
        node(&local_snapshot, "session:session-observe"),
        &local.transcript_path,
    );
    let record_path = fixture.data_dir.join("provider-failure-records.jsonl");
    let external_path = fixture.data_dir.join("external-failure-session.jsonl");
    let provider_path = write_external_locate_provider_with_behavior(
        &fixture,
        &record_path,
        &external_path,
        ExternalLocateProviderBehavior::Failing,
    );
    let registry = external_registry(&provider_path);
    let service = ProductionObservabilitySnapshotService::for_provider_inspect(
        registry,
        external_identity(),
        SESSION_ID.to_string(),
        Some(fixture.data_dir.join("provider-work")),
    );

    let snapshot = service.snapshot(&fixture.root(), SnapshotLimits::default());

    assert_no_session_transcript_ref(node(&snapshot, "session:session-observe"));
    assert_no_session_transcript_ref(node(&snapshot, &format!("invocation:{ROOT_UUID}")));
    assert_no_session_transcript_ref(node(
        &snapshot,
        &format!("process:{ROOT_UUID}:{}", identity.os_pid),
    ));
    assert_no_attached_transcript_path(&snapshot, &local.transcript_path);
    assert_eq!(
        recorded_subcommand_count(&record_path, "session.locate_transcript"),
        1
    );
}

#[cfg(unix)]
#[test]
fn provider_inspect_missing_format_id_attaches_no_transcript_ref() {
    let fixture = Fixture::new();
    let identity = seed_running_root(&fixture);
    let record_path = fixture.data_dir.join("provider-no-format-records.jsonl");
    let external_path = fixture.data_dir.join("external-no-format-session.jsonl");
    std::fs::write(&external_path, "{\"type\":\"assistant\"}\n").unwrap();
    let provider_path = write_external_locate_provider_with_behavior(
        &fixture,
        &record_path,
        &external_path,
        ExternalLocateProviderBehavior::LocatedWithoutFormatId,
    );
    let registry = external_registry(&provider_path);
    let service = ProductionObservabilitySnapshotService::for_provider_inspect(
        registry,
        external_identity(),
        SESSION_ID.to_string(),
        Some(fixture.data_dir.join("provider-work")),
    );

    let snapshot = service.snapshot(&fixture.root(), SnapshotLimits::default());

    assert_no_session_transcript_ref(node(&snapshot, "session:session-observe"));
    assert_no_session_transcript_ref(node(&snapshot, &format!("invocation:{ROOT_UUID}")));
    assert_no_session_transcript_ref(node(
        &snapshot,
        &format!("process:{ROOT_UUID}:{}", identity.os_pid),
    ));
    assert_no_attached_transcript_path(&snapshot, &external_path);
    assert_no_attached_transcript_path(&snapshot, &external_path.canonicalize().unwrap());
    assert_eq!(
        recorded_subcommand_count(&record_path, "session.locate_transcript"),
        1
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
fn observability_sidecar_reads_preserve_physical_file_inventory_and_bytes() {
    let fixture = Fixture::new();
    seed_root_session(&fixture);
    let state = fixture.open_state();
    state.disable_wal_autocheckpoint_for_test().unwrap();
    let root_id = state.get_invocation_by_uuid(ROOT_UUID).unwrap().unwrap().id;
    seed_invocation(&state, CHILD_UUID, Some(root_id));
    let mut mailbox = fixture.open_mailbox();
    let row = match mailbox
        .enqueue_agent_bash_complete(&mailbox_input("handle-physical-read-only", SESSION_ID))
        .unwrap()
    {
        EnqueueResult::Inserted(row) => row,
        other => panic!("unexpected enqueue result: {other:?}"),
    };
    mailbox
        .register_delivery_attempt(
            "attempt-physical-read-only",
            SESSION_ID,
            ROOT_UUID,
            &[row.seq],
            0,
        )
        .unwrap();
    assert!(
        mailbox
            .record_delivery_attempt_transport_ack("attempt-physical-read-only")
            .unwrap()
    );
    let claim = mailbox
        .try_acquire_wake_claim(WakeClaimRequest {
            session_id: SESSION_ID,
            claim_token: "claim-physical-read-only",
            reason: "notify_idle",
            auto_wake_count: 1,
            wake_invocation_uuid: Some(ROOT_UUID),
            stale_after_seconds: 600,
        })
        .unwrap();
    assert!(matches!(claim, WakeClaimAcquireResult::Acquired(_)));
    let generation = RuntimeGenerationId::parse("88888888-8888-4888-8888-888888888888").unwrap();
    mailbox
        .create_runtime_generation(CreateRuntimeGeneration {
            generation_id: &generation,
            spawn_invocation_uuid: ROOT_UUID,
            session_id: Some(SESSION_ID),
            runtime_mode: "headless",
            provider_name: "provider-a",
            model_name: Some("model-a"),
            pty_control_path: None,
            models_dir: Some("/models/observability-read-only"),
            effective_cwd: Some("/work/observability-read-only"),
        })
        .unwrap();
    mailbox
        .upsert_session_runtime(SessionRuntimeUpsert {
            session_id: SESSION_ID,
            mode: "headless",
            invocation_uuid: Some(ROOT_UUID),
            provider_name: Some("provider-a"),
            model_name: Some("model-a"),
            pty_control_path: None,
            models_dir: Some("/models/observability-read-only"),
            effective_cwd: Some("/work/observability-read-only"),
            selected_auto_wake_max: Some(5),
        })
        .unwrap();
    let before = physical_file_snapshot(&fixture.data_dir);

    let snapshot = fixture
        .service()
        .snapshot(&fixture.root(), full_snapshot_limits());

    assert!(find_node(&snapshot, "mailbox:session-observe:1").is_some());
    assert!(find_node(&snapshot, "wake:session-observe:claim-physical-read-only").is_some());
    assert!(find_node(&snapshot, &format!("invocation:{ROOT_UUID}")).is_some());
    assert!(find_node(&snapshot, &format!("invocation:{CHILD_UUID}")).is_some());
    let session = node(&snapshot, "session:session-observe");
    assert_eq!(session.status, MonitorStatus::Running);
    assert_eq!(session.label, "session provider-a/model-a");
    assert_physical_files_unchanged(&before, &physical_file_snapshot(&fixture.data_dir));
    drop(mailbox);
    drop(state);
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
            selected_auto_wake_max: None,
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
fn abandoned_mailbox_is_failed_and_does_not_require_wake() {
    let fixture = Fixture::new();
    seed_root_session(&fixture);
    let mut mailbox = fixture.open_mailbox();
    mailbox
        .enqueue_agent_bash_complete(&mailbox_input("handle-abandoned", SESSION_ID))
        .unwrap();
    assert_eq!(
        mailbox
            .mark_pending_abandoned(SESSION_ID, WAKE_SWEEP_ABANDONED_ERROR, 1)
            .unwrap(),
        1
    );
    drop(mailbox);

    let live = fixture
        .service()
        .snapshot(&fixture.root(), SnapshotLimits::default());
    assert_eq!(live.summary.pending_mailbox_count, 0);
    assert!(!has_diagnostic(&live, "wake-needed:no-runtime"));
    assert!(find_node(&live, "mailbox:session-observe:1").is_none());

    let full = fixture
        .service()
        .snapshot(&fixture.root(), full_snapshot_limits());
    let row = node(&full, "mailbox:session-observe:1");
    assert_eq!(row.status, MonitorStatus::Failed);
    assert_eq!(
        row.mailbox.as_ref().unwrap().delivery_error.as_deref(),
        Some(WAKE_SWEEP_ABANDONED_ERROR)
    );
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
        MonitorStatus::Stale
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
    assert_eq!(snapshot.summary.running_agent_bash_count, 0);
}

#[test]
fn agent_bash_scan_uses_workload_invocation_marker_when_caller_chain_has_no_owner() {
    let fixture = Fixture::new();
    seed_root_session(&fixture);
    let unregistered_caller = dead_identity();
    let live_workload = current_identity();
    write_agent_bash_meta(
        &fixture.agent_bash_root(),
        "manual-launch",
        &agent_bash_meta_with_workload_identity(
            "manual-launch",
            &unregistered_caller,
            &live_workload,
        ),
        &format!(
            "OULIPOLY_INVOCATION={{\"source\":\"opencode\",\"id\":\"{ROOT_UUID}\"}}\nreview output"
        ),
    );

    let snapshot = fixture
        .service()
        .snapshot(&fixture.root(), SnapshotLimits::default());

    let workload = node(&snapshot, "agent-bash:manual-launch");
    assert_eq!(workload.status, MonitorStatus::Running);
    assert_eq!(workload.liveness, LivenessStatus::VerifiedLive);
    assert_eq!(
        workload.parent_id.as_deref(),
        Some(format!("invocation:{ROOT_UUID}").as_str())
    );
    assert!(!workload.label.contains("chain_index="));
}

#[test]
fn agent_bash_scan_rejects_workload_marker_outside_active_invocation_subtree() {
    let fixture = Fixture::new();
    seed_root_session(&fixture);
    let unregistered_caller = dead_identity();
    write_agent_bash_meta(
        &fixture.agent_bash_root(),
        "unrelated-manual-launch",
        &agent_bash_meta(
            "unrelated-manual-launch",
            "DONE",
            &unregistered_caller,
            Some(778),
            Some(0),
        ),
        &format!(
            "OULIPOLY_INVOCATION={{\"source\":\"opencode\",\"id\":\"{CHILD_UUID}\"}}\nreview output"
        ),
    );

    let snapshot = fixture
        .service()
        .snapshot(&fixture.root(), full_snapshot_limits());

    assert!(find_node(&snapshot, "agent-bash:unrelated-manual-launch").is_none());
}

#[test]
fn agent_bash_scan_rejects_legacy_shell_mangled_workload_marker() {
    let fixture = Fixture::new();
    seed_root_session(&fixture);
    let unregistered_caller = dead_identity();
    let live_workload = current_identity();
    write_agent_bash_meta(
        &fixture.agent_bash_root(),
        "legacy-marker-manual-launch",
        &agent_bash_meta_with_workload_identity(
            "legacy-marker-manual-launch",
            &unregistered_caller,
            &live_workload,
        ),
        &format!("OULIPOLY_INVOCATION={{source:'opencode',id:'{ROOT_UUID}'}}\nreview output"),
    );

    let snapshot = fixture
        .service()
        .snapshot(&fixture.root(), SnapshotLimits::default());

    assert!(find_node(&snapshot, "agent-bash:legacy-marker-manual-launch").is_none());
}

#[test]
fn agent_bash_running_status_is_reconciled_against_exact_workload_identity() {
    let fixture = Fixture::new();
    seed_root_session(&fixture);
    let pid = fixture.open_pid();
    let owner = current_identity();
    record_identity(&pid, ROOT_UUID, Some(SESSION_ID), &owner);
    drop(pid);
    let root = fixture.agent_bash_root();
    let live = current_identity();
    let dead = dead_identity();
    let mut reused = current_identity();
    reused.os_pid_starttime_ticks += 1;
    write_agent_bash_meta(
        &root,
        "live-running",
        &agent_bash_meta_with_workload_identity("live-running", &owner, &live),
        "live tail",
    );
    write_agent_bash_meta(
        &root,
        "legacy-live-running",
        &agent_bash_meta(
            "legacy-live-running",
            "RUNNING",
            &owner,
            Some(live.os_pid),
            None,
        ),
        "legacy live tail",
    );
    write_agent_bash_meta(
        &root,
        "dead-running",
        &agent_bash_meta_with_workload_identity("dead-running", &owner, &dead),
        "dead tail",
    );
    write_agent_bash_meta(
        &root,
        "reused-running",
        &agent_bash_meta_with_workload_identity("reused-running", &owner, &reused),
        "reused tail",
    );
    write_agent_bash_meta(
        &root,
        "missing-pid-running",
        &agent_bash_meta("missing-pid-running", "RUNNING", &owner, None, None),
        "missing tail",
    );
    let mut missing_starttime = agent_bash_meta_json(
        "missing-starttime",
        "RUNNING",
        &owner,
        Some(live.os_pid),
        None,
    );
    missing_starttime["process_boot_id"] = live.os_boot_id.clone().into();
    write_agent_bash_meta(
        &root,
        "missing-starttime",
        &missing_starttime.to_string(),
        "missing starttime tail",
    );
    let mut empty_boot_id =
        agent_bash_meta_json("empty-boot-id", "RUNNING", &owner, Some(live.os_pid), None);
    empty_boot_id["process_boot_id"] = "".into();
    empty_boot_id["workload_pid_starttime_ticks"] = live.os_pid_starttime_ticks.into();
    write_agent_bash_meta(
        &root,
        "empty-boot-id",
        &empty_boot_id.to_string(),
        "empty boot ID tail",
    );
    let mut wrong_starttime = agent_bash_meta_json(
        "wrong-starttime",
        "RUNNING",
        &owner,
        Some(live.os_pid),
        None,
    );
    wrong_starttime["process_boot_id"] = live.os_boot_id.clone().into();
    wrong_starttime["workload_pid_starttime_ticks"] = "not-an-integer".into();
    write_agent_bash_meta(
        &root,
        "wrong-starttime",
        &wrong_starttime.to_string(),
        "wrong starttime tail",
    );

    let snapshot = fixture
        .service()
        .snapshot(&fixture.root(), full_snapshot_limits());

    let live_node = node(&snapshot, "agent-bash:live-running");
    assert_eq!(live_node.status, MonitorStatus::Running);
    assert_eq!(live_node.liveness, LivenessStatus::VerifiedLive);
    let legacy_live_node = node(&snapshot, "agent-bash:legacy-live-running");
    assert_eq!(legacy_live_node.status, MonitorStatus::Running);
    assert_eq!(legacy_live_node.liveness, LivenessStatus::UnverifiedLive);
    let dead_node = node(&snapshot, "agent-bash:dead-running");
    assert_eq!(dead_node.status, MonitorStatus::Stale);
    assert_eq!(dead_node.liveness, LivenessStatus::Dead);
    let reused_node = node(&snapshot, "agent-bash:reused-running");
    assert_eq!(reused_node.status, MonitorStatus::Stale);
    assert_eq!(reused_node.liveness, LivenessStatus::PidReused);
    let missing_node = node(&snapshot, "agent-bash:missing-pid-running");
    assert_eq!(missing_node.status, MonitorStatus::Unknown);
    assert_eq!(missing_node.liveness, LivenessStatus::NotApplicable);
    for handle in ["missing-starttime", "empty-boot-id", "wrong-starttime"] {
        let malformed_node = node(&snapshot, &format!("agent-bash:{handle}"));
        assert_eq!(malformed_node.status, MonitorStatus::Unknown);
        assert_eq!(malformed_node.liveness, LivenessStatus::Unknown);
    }
    assert_eq!(snapshot.summary.running_agent_bash_count, 2);
}

#[test]
fn agent_bash_scan_orders_canonical_handles_without_directory_mtime() {
    let fixture = Fixture::new();
    seed_root_session(&fixture);
    let pid = fixture.open_pid();
    let owner = current_identity();
    record_identity(&pid, ROOT_UUID, Some(SESSION_ID), &owner);
    drop(pid);
    let root = fixture.agent_bash_root();
    let older_handle = "ab_1_100_0000000000000001";
    let newer_handle = "ab_2_100_0000000000000002";
    let older_dir = write_agent_bash_meta(
        &root,
        older_handle,
        &agent_bash_meta(older_handle, "RUNNING", &owner, Some(700), None),
        "older",
    );
    let newer_dir = write_agent_bash_meta(
        &root,
        newer_handle,
        &agent_bash_meta(newer_handle, "RUNNING", &owner, Some(701), None),
        "newer",
    );
    set_dir_mtime(&older_dir, 60);
    set_dir_mtime(&newer_dir, 10);

    let snapshot = fixture.service().snapshot(
        &fixture.root(),
        SnapshotLimits {
            include_terminal: true,
            agent_bash_scan_dirs: 1,
            ..SnapshotLimits::default()
        },
    );

    assert!(
        snapshot
            .nodes
            .iter()
            .any(|node| node.id == format!("agent-bash:{newer_handle}"))
    );
    assert!(
        snapshot
            .nodes
            .iter()
            .all(|node| node.id != format!("agent-bash:{older_handle}"))
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
            matched_chain_index: Some(7),
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
    assert!(
        node(&snapshot, "agent-bash:aa-mailbox-old")
            .label
            .contains("chain_index=7")
    );
}

#[test]
fn agent_bash_mailbox_completion_rc_overrides_nonterminal_metadata() {
    let fixture = Fixture::new();
    seed_root_session(&fixture);
    let pid = fixture.open_pid();
    let owner = current_identity();
    record_identity(&pid, ROOT_UUID, Some(SESSION_ID), &owner);
    drop(pid);
    let state_dir = write_agent_bash_meta(
        &fixture.agent_bash_root(),
        "completed-with-running-meta",
        &agent_bash_meta_with_workload_identity(
            "completed-with-running-meta",
            &owner,
            &dead_identity(),
        ),
        "completed tail",
    );
    let mut mailbox = fixture.open_mailbox();
    mailbox
        .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
            state_dir: state_dir.to_str().unwrap(),
            meta_path: state_dir.join("meta.json").to_str().unwrap(),
            log_path: state_dir.join("log").to_str().unwrap(),
            rc_path: state_dir.join("rc").to_str().unwrap(),
            ..mailbox_input("completed-with-running-meta", SESSION_ID)
        })
        .unwrap();
    drop(mailbox);

    let snapshot = fixture
        .service()
        .snapshot(&fixture.root(), full_snapshot_limits());
    let completed = node(&snapshot, "agent-bash:completed-with-running-meta");
    assert_eq!(completed.status, MonitorStatus::Succeeded);
    assert_eq!(completed.liveness, LivenessStatus::Dead);
    assert_eq!(snapshot.summary.running_agent_bash_count, 0);
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

fn seed_running_root(fixture: &Fixture) -> ProcessIdentity {
    seed_root_session(fixture);
    let pid = fixture.open_pid();
    let identity = current_identity();
    record_identity(&pid, ROOT_UUID, Some(SESSION_ID), &identity);
    drop(pid);
    identity
}

struct LocalTranscriptTree {
    projects_dir: PathBuf,
    transcript_path: PathBuf,
}

fn write_local_transcript_tree(fixture: &Fixture, session_id: &str) -> LocalTranscriptTree {
    let projects_dir = fixture.data_dir.join("native-projects");
    let workspace_root = fixture.data_dir.join("workspace");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let project_dir = projects_dir.join(project_dir_name(&workspace_root));
    std::fs::create_dir_all(&project_dir).unwrap();
    let transcript_path = project_dir.join(format!("{session_id}.jsonl"));
    std::fs::write(
        &transcript_path,
        "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hi\"}]}}\n",
    )
    .unwrap();
    LocalTranscriptTree {
        projects_dir,
        transcript_path,
    }
}

fn project_dir_name(path: &Path) -> String {
    let raw = path.to_string_lossy();
    format!("-{}", raw.trim_start_matches('/').replace('/', "-"))
}

fn assert_observability_service_has_provider_inspect_source_wiring() {
    let source = std::fs::read_to_string(observability_service_source()).unwrap();
    assert!(
        source.contains("pub fn for_session(session_storage: Option<SessionStorage>) -> Self"),
        "snapshot construction must keep the local session-storage constructor"
    );
    let forbidden_fallback = ["local", "fallback", "storage"].join("_");
    assert!(
        !source.contains(&forbidden_fallback),
        "provider-inspect source must not carry local fallback storage"
    );
    assert_clean_provider_inspect_constructor(&source);
    assert!(
        source.contains("ProviderRegistry"),
        "provider-inspect source must be registry-backed"
    );
    assert!(
        source.contains("SessionProviderIdentity"),
        "provider-inspect source must carry the provider identity envelope"
    );
    assert_provider_inspect_resolver_uses_provider_locate_only(
        &provider_inspect_transcript_source(),
    );
}

fn observability_service_source() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/observability/service.rs")
}

fn observability_transcript_source() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/observability/transcript_source.rs")
}

fn assert_clean_provider_inspect_constructor(source: &str) {
    let signature = source_function_signature(source, "pub fn for_provider_inspect(");
    let signature = compact_whitespace(signature);
    let accepted = [
        "pub fn for_provider_inspect( registry: ProviderRegistry, identity: SessionProviderIdentity, active_session_id: String, effective_cwd: Option<PathBuf>, ) -> Self",
        "pub fn for_provider_inspect( registry: ProviderRegistry, identity: SessionProviderIdentity, active_session_id: String, effective_cwd: Option<PathBuf> ) -> Self",
    ];
    assert!(
        accepted.contains(&signature.as_str()),
        "provider-inspect constructor must keep the clean C2 shape, got `{signature}`"
    );
}

fn assert_provider_inspect_resolver_uses_provider_locate_only(source: &str) {
    assert!(
        source.contains("session_provider::locate_transcript"),
        "provider-inspect source must use session_provider::locate_transcript"
    );
    for forbidden in [
        "SessionTranscriptResolver",
        "resolve_jsonl_path_for_provider_with_mode",
        "resolve_jsonl_path_for_provider(",
        "locate_transcript_path(",
        "SessionStorage",
        "SessionsConfig",
        "std::fs::read",
        "read_to_string(",
        "File::open",
        "BufReader",
    ] {
        assert!(
            !source.contains(forbidden),
            "provider-inspect source must not consult local transcript resolver/reader `{forbidden}`"
        );
    }
}

fn provider_inspect_transcript_source() -> String {
    for path in provider_inspect_dedicated_source_paths() {
        if path.exists() {
            let source = std::fs::read_to_string(path).unwrap();
            return production_source(&source).to_string();
        }
    }

    provider_inspect_source_from_transcript_module().unwrap_or_else(|| {
        panic!(
            "provider-inspect transcript source must be scoped to a dedicated source path or ProviderInspectTranscriptResolver impl"
        )
    })
}

fn provider_inspect_dedicated_source_paths() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    vec![
        root.join("src/observability/provider_inspect_transcript_source.rs"),
        root.join("src/observability/transcript_source/provider_inspect.rs"),
        root.join("src/observability/transcript_source_provider_inspect.rs"),
    ]
}

fn provider_inspect_source_from_transcript_module() -> Option<String> {
    let source = std::fs::read_to_string(observability_transcript_source()).unwrap();
    let source = production_source(&source);
    let resolver = extract_source_item(source, "struct ProviderInspectTranscriptResolver")?;
    let implementation = extract_source_item(source, "impl ProviderInspectTranscriptResolver")?;
    Some(format!("{resolver}\n{implementation}"))
}

fn source_function_signature<'a>(source: &'a str, marker: &str) -> &'a str {
    let start = source
        .find(marker)
        .unwrap_or_else(|| panic!("missing source marker `{marker}`"));
    let suffix = &source[start..];
    let end = suffix
        .find('{')
        .or_else(|| suffix.find(';'))
        .unwrap_or_else(|| panic!("missing function signature end for `{marker}`"));
    &suffix[..end]
}

fn compact_whitespace(source: &str) -> String {
    source.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn production_source(source: &str) -> &str {
    source.split("\n#[cfg(test)]").next().unwrap_or(source)
}

fn extract_source_item<'a>(source: &'a str, marker: &str) -> Option<&'a str> {
    let start = source.find(marker)?;
    let suffix = &source[start..];
    let brace = suffix.find('{');
    let semicolon = suffix.find(';');
    match (semicolon, brace) {
        (Some(semicolon), Some(brace)) if semicolon < brace => {
            return Some(&source[start..start + semicolon + 1]);
        }
        (Some(semicolon), None) => {
            return Some(&source[start..start + semicolon + 1]);
        }
        _ => {}
    }
    let brace = start + brace?;
    let end = matching_brace_end(source, brace)?;
    Some(&source[start..end])
}

fn matching_brace_end(source: &str, open_brace: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, byte) in source.as_bytes()[open_brace..].iter().enumerate() {
        match *byte {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(open_brace + offset + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn assert_session_transcript_path(
    node: &oulipoly_runtime::observability::MonitorNode,
    expected_path: &Path,
) {
    assert_session_transcript_ref(
        node,
        expected_path,
        SnapshotLimits::default().transcript_tail_bytes,
        None,
        None,
    );
}

fn assert_session_transcript_ref(
    node: &oulipoly_runtime::observability::MonitorNode,
    expected_path: &Path,
    expected_tail_bytes: usize,
    expected_format_id: Option<&str>,
    expected_source_id: Option<&str>,
) {
    match &node.inspect_ref {
        Some(InspectRef::SessionTranscript {
            path,
            max_tail_bytes,
            format_id,
            source_id,
        }) => {
            let expected = expected_path.to_string_lossy().into_owned();
            assert_eq!(path, &expected);
            assert_eq!(*max_tail_bytes, expected_tail_bytes);
            assert_eq!(format_id.as_deref(), expected_format_id);
            assert_eq!(source_id.as_deref(), expected_source_id);
        }
        other => panic!("expected session transcript inspect ref, got {other:?}"),
    }
}

fn assert_no_session_transcript_ref(node: &oulipoly_runtime::observability::MonitorNode) {
    assert!(
        !matches!(
            &node.inspect_ref,
            Some(InspectRef::SessionTranscript { .. })
        ),
        "expected no session transcript inspect ref, got {:?}",
        node.inspect_ref
    );
}

fn assert_no_attached_transcript_path(
    snapshot: &oulipoly_runtime::observability::MonitorSnapshot,
    forbidden_path: &Path,
) {
    let forbidden = forbidden_path.to_string_lossy();
    assert!(
        !snapshot.nodes.iter().any(|node| matches!(
            &node.inspect_ref,
            Some(InspectRef::SessionTranscript { path, .. }) if path == forbidden.as_ref()
        )),
        "snapshot must not attach forbidden transcript path {}",
        forbidden_path.display()
    );
}

#[cfg(unix)]
#[derive(Clone, Copy)]
enum ExternalLocateProviderBehavior {
    Located,
    LocatedWithoutFormatId,
    Failing,
}

#[cfg(unix)]
fn prepare_unrelated_registry_records(fixture: &Fixture) -> PathBuf {
    let record_path = fixture.data_dir.join("unrelated-provider-records.jsonl");
    let external_path = fixture.data_dir.join("unrelated-session.jsonl");
    std::fs::write(&external_path, "{\"type\":\"assistant\"}\n").unwrap();
    let provider_path = write_external_locate_provider(fixture, &record_path, &external_path);
    let _registry = external_registry(&provider_path);
    assert_record_file_empty(&record_path);
    record_path
}

#[cfg(unix)]
fn write_external_locate_provider(
    fixture: &Fixture,
    record_path: &Path,
    transcript_path: &Path,
) -> PathBuf {
    write_external_locate_provider_with_behavior(
        fixture,
        record_path,
        transcript_path,
        ExternalLocateProviderBehavior::Located,
    )
}

#[cfg(unix)]
fn write_external_locate_provider_with_behavior(
    fixture: &Fixture,
    record_path: &Path,
    transcript_path: &Path,
    behavior: ExternalLocateProviderBehavior,
) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let provider_path = fixture.data_dir.join("external-locate-provider.py");
    std::fs::write(record_path, "").unwrap();
    std::fs::write(
        &provider_path,
        external_locate_provider_body(record_path, transcript_path, behavior),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&provider_path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&provider_path, permissions).unwrap();
    provider_path
}

#[cfg(unix)]
fn external_locate_provider_body(
    record_path: &Path,
    transcript_path: &Path,
    behavior: ExternalLocateProviderBehavior,
) -> String {
    let locate_ok = !matches!(behavior, ExternalLocateProviderBehavior::Failing);
    let format_id = match behavior {
        ExternalLocateProviderBehavior::Located => r#""canonical-transcript-v1""#,
        ExternalLocateProviderBehavior::LocatedWithoutFormatId
        | ExternalLocateProviderBehavior::Failing => "None",
    };
    format!(
        r#"#!/usr/bin/env python3
import json
import pathlib
import sys

CONTRACT = "oulipoly.provider/v1"
LOCATE_OK = {locate_ok}
FORMAT_ID = {format_id}
subcommand = sys.argv[1] if len(sys.argv) > 1 else ""
request = json.loads(sys.stdin.read() or "{{}}")
with pathlib.Path({record_path}).open("a", encoding="utf-8") as handle:
    handle.write(json.dumps({{"subcommand": subcommand, "request": request}}, sort_keys=True) + "\n")

def envelope(result):
    return {{
        "contract": request.get("contract", CONTRACT),
        "request_id": request.get("request_id", "request-observe"),
        "ok": True,
        "result": result,
    }}

if subcommand == "describe":
    response = envelope({{
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
        "settings_schema_id": "provider-a-test-settings",
    }})
elif subcommand == "session.locate_transcript":
    if LOCATE_OK:
        result = {{
            "located": True,
            "path": {transcript_path},
            "source_id": "provider-a",
            "require_existing_observed": True,
        }}
        if FORMAT_ID is not None:
            result["format_id"] = FORMAT_ID
        response = envelope(result)
    else:
        response = {{
            "contract": request.get("contract", CONTRACT),
            "request_id": request.get("request_id", "request-observe"),
            "ok": False,
            "error": {{
                "category": "failed",
                "code": "inspect_locate_failed",
                "message": "inspect_locate_failed",
                "retryable": False,
            }},
        }}
else:
    response = {{
        "contract": request.get("contract", CONTRACT),
        "request_id": request.get("request_id", "request-observe"),
        "ok": False,
        "error": {{
            "category": "failed",
            "code": "unsupported_subcommand",
            "message": "unsupported_subcommand",
            "retryable": False,
        }},
    }}
print(json.dumps(response))
"#,
        record_path = serde_json::to_string(&record_path.display().to_string()).unwrap(),
        transcript_path = serde_json::to_string(&transcript_path.display().to_string()).unwrap(),
        locate_ok = if locate_ok { "True" } else { "False" },
        format_id = format_id,
    )
}

#[cfg(unix)]
fn external_registry(provider_path: &Path) -> ProviderRegistry {
    ProviderRegistry::from_model_configs(
        &[external_model_config(provider_path)],
        ProviderRegistryOptions::default(),
    )
    .expect("external registry")
}

#[cfg(unix)]
fn external_model_config(provider_path: &Path) -> ModelConfig {
    ModelConfig {
        name: "model-a".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::model_provider("provider-a", Vec::new())],
        inputs: Vec::new(),
        provider: Some(ProviderImplementationRef {
            path: Some(provider_path.display().to_string()),
            crate_name: None,
            version: None,
            binary: None,
            script: None,
        }),
    }
}

fn external_identity() -> SessionProviderIdentity {
    SessionProviderIdentity {
        model_name: "model-a".to_string(),
        provider_name: "provider-a".to_string(),
        provider_instance_id: Some("provider-a-instance".to_string()),
        settings_id: "provider-a-settings".to_string(),
    }
}

fn recorded_subcommand_count(record_path: &Path, subcommand: &str) -> usize {
    std::fs::read_to_string(record_path)
        .unwrap()
        .lines()
        .filter(|line| recorded_subcommand(line).as_deref() == Some(subcommand))
        .count()
}

fn recorded_subcommand(line: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    value.get("subcommand")?.as_str().map(str::to_string)
}

#[cfg(unix)]
fn assert_record_file_empty(record_path: &Path) {
    let contents = std::fs::read_to_string(record_path).unwrap();
    assert!(
        contents.trim().is_empty(),
        "provider registry should be unrelated, got records: {contents}"
    );
}

#[cfg(unix)]
fn recorded_request_for(record_path: &Path, subcommand: &str) -> Value {
    let records = recorded_requests_for(record_path, subcommand);
    assert_eq!(
        records.len(),
        1,
        "expected exactly one {subcommand} request record, got {records:?}"
    );
    records.into_iter().next().unwrap()
}

#[cfg(unix)]
fn recorded_requests_for(record_path: &Path, subcommand: &str) -> Vec<Value> {
    std::fs::read_to_string(record_path)
        .unwrap()
        .lines()
        .filter_map(|line| recorded_request(line, subcommand))
        .collect()
}

#[cfg(unix)]
fn recorded_request(line: &str, subcommand: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("subcommand")?.as_str()? == subcommand {
        value.get("request").cloned()
    } else {
        None
    }
}

#[cfg(unix)]
fn assert_provider_inspect_request(record_path: &Path, effective_cwd: &Path, tail_bytes: usize) {
    let request = recorded_request_for(record_path, "session.locate_transcript");
    assert_eq!(request["provider_instance_id"], "provider-a-instance");
    assert_eq!(
        request["host"]["working_directory"],
        Value::String(path_string(effective_cwd))
    );
    assert!(
        !request.to_string().contains("state.db"),
        "inspect request must not expose host SQLite paths: {request}"
    );

    let params = &request["params"];
    assert_eq!(params["settings_id"], "provider-a-settings");
    assert_eq!(params["model_name"], "model-a");
    assert_eq!(params["provider_name"], "provider-a");
    assert_eq!(params["session_id"], SESSION_ID);
    assert_eq!(params["lookup_mode"], "require_existing");
    assert_eq!(params["purpose"], "inspect");
    assert_eq!(params["tail_bytes_hint"], Value::from(tail_bytes as u64));
}

#[cfg(unix)]
fn path_string(path: &Path) -> String {
    path.display().to_string()
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

#[cfg(target_os = "linux")]
struct TestProcess {
    child: Child,
    identity: ProcessIdentity,
}

#[cfg(target_os = "linux")]
struct OverlayLogicalChildScenario {
    fixture: Fixture,
    root_process: TestProcess,
    logical_child_process: TestProcess,
    _mismatched_process: TestProcess,
    _unrelated_process: TestProcess,
}

#[cfg(target_os = "linux")]
fn overlay_logical_child_scenario() -> OverlayLogicalChildScenario {
    let fixture = Fixture::new();
    let root_process = TestProcess::spawn();
    let logical_child_process = TestProcess::spawn();
    let mismatched_process = TestProcess::spawn();
    let unrelated_process = TestProcess::spawn();
    seed_overlay_logical_child_state(&fixture);
    seed_overlay_logical_child_identities(
        &fixture,
        &root_process,
        &logical_child_process,
        &mismatched_process,
        &unrelated_process,
    );
    OverlayLogicalChildScenario {
        fixture,
        root_process,
        logical_child_process,
        _mismatched_process: mismatched_process,
        _unrelated_process: unrelated_process,
    }
}

#[cfg(target_os = "linux")]
fn seed_overlay_logical_child_state(fixture: &Fixture) {
    let state = fixture.open_state();
    let root_id = seed_invocation(&state, ROOT_UUID, None);
    state
        .update_session_capture(root_id, Some(SESSION_ID), "stdout-json")
        .unwrap();
    for index in 0..TERMINAL_DESCENDANT_COUNT {
        let uuid = format!("10000000-0000-4000-8000-{index:012}");
        let row_id = seed_invocation(&state, &uuid, Some(root_id));
        state
            .finalize_invocation(row_id, true, 0, None, Some("completed"))
            .unwrap();
    }
    seed_invocation(&state, LIVE_CHILD_UUID, Some(root_id));
    seed_invocation(&state, DEAD_CHILD_UUID, Some(root_id));
    seed_invocation(&state, MISSING_CHILD_UUID, Some(root_id));
    seed_invocation(&state, MISMATCHED_CHILD_UUID, Some(root_id));
    seed_invocation(&state, UNRELATED_UUID, None);
}

#[cfg(target_os = "linux")]
fn seed_overlay_logical_child_identities(
    fixture: &Fixture,
    root_process: &TestProcess,
    logical_child_process: &TestProcess,
    mismatched_process: &TestProcess,
    unrelated_process: &TestProcess,
) {
    let pid = fixture.open_pid();
    record_identity(&pid, ROOT_UUID, Some(SESSION_ID), root_process.identity());
    record_identity(
        &pid,
        LIVE_CHILD_UUID,
        Some(SESSION_ID),
        logical_child_process.identity(),
    );
    record_identity(&pid, DEAD_CHILD_UUID, Some(SESSION_ID), &dead_identity());
    let mut mismatched_identity = mismatched_process.identity().clone();
    mismatched_identity.os_pid_starttime_ticks += 1;
    record_identity(
        &pid,
        MISMATCHED_CHILD_UUID,
        Some(SESSION_ID),
        &mismatched_identity,
    );
    record_identity(
        &pid,
        UNRELATED_UUID,
        Some(SESSION_ID),
        unrelated_process.identity(),
    );
}

#[cfg(target_os = "linux")]
impl TestProcess {
    fn spawn() -> Self {
        let child = Command::new("sleep").arg("30").spawn().unwrap();
        let identity =
            oulipoly_state::pid_identity::read_live_process_identity(i64::from(child.id()))
                .unwrap()
                .unwrap();
        Self { child, identity }
    }

    fn pid(&self) -> i64 {
        self.identity.os_pid
    }

    fn identity(&self) -> &ProcessIdentity {
        &self.identity
    }
}

#[cfg(target_os = "linux")]
impl Drop for TestProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(target_os = "linux")]
fn process_parent_pid(pid: i64) -> i64 {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    stat.rsplit_once(") ")
        .unwrap()
        .1
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap()
}

#[cfg(target_os = "linux")]
fn assert_verified_running_process(
    snapshot: &oulipoly_runtime::observability::MonitorSnapshot,
    invocation_uuid: &str,
    identity: &ProcessIdentity,
) {
    let process = node(
        snapshot,
        &format!("process:{invocation_uuid}:{}", identity.os_pid),
    );
    assert_eq!(process.kind, MonitorNodeKind::ProviderProcess);
    assert_eq!(process.status, MonitorStatus::Running);
    assert_eq!(process.liveness, LivenessStatus::VerifiedLive);
}

#[cfg(target_os = "linux")]
fn assert_invocation_absent(
    snapshot: &oulipoly_runtime::observability::MonitorSnapshot,
    invocation_uuid: &str,
) {
    assert!(
        find_node(snapshot, &format!("invocation:{invocation_uuid}")).is_none(),
        "invocation {invocation_uuid} must not be visible: {:#?}",
        snapshot.nodes
    );
    assert!(
        !snapshot
            .nodes
            .iter()
            .any(|node| node.id.starts_with(&format!("process:{invocation_uuid}:"))),
        "invocation {invocation_uuid} must not contribute a process node: {:#?}",
        snapshot.nodes
    );
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

fn agent_bash_meta_with_workload_identity(
    handle: &str,
    owner: &ProcessIdentity,
    workload: &ProcessIdentity,
) -> String {
    let mut meta = agent_bash_meta_json(handle, "RUNNING", owner, Some(workload.os_pid), None);
    meta["workload_pid_starttime_ticks"] = workload.os_pid_starttime_ticks.into();
    meta["process_boot_id"] = workload.os_boot_id.clone().into();
    meta.to_string()
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
