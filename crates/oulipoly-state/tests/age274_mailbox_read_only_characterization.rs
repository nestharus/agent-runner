//! ## Declared roles
//!
//! Roles: accessor, validator.
//!
//! TEST: physical read-only mailbox sidecar characterization for AGE-274.

use chrono::Utc;
use oulipoly_state::mailbox::{
    AgentBashCompleteEnqueue, CreateRuntimeGeneration, EnqueueResult, MailboxDb,
    MailboxDeliveryAttemptDisposition, RuntimeGenerationId, SessionMetadataUpsert,
    WakeClaimAcquireResult, WakeClaimRequest,
};
use oulipoly_state::pid_identity::{PidIdentityDb, PidIdentityRecord, ProcessIdentity};
use oulipoly_state::{CURRENT_SCHEMA_VERSION, InvocationStart, SessionTurnIngest, StateDb};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

const SESSION: &str = "age274-read-only-session";
const INVOCATION: &str = "11111111-1111-4111-8111-111111111111";
const ATTEMPT: &str = "age274-historical-attempt";
const CLAIM: &str = "age274-historical-claim";
const GENERATION: &str = "22222222-2222-4222-8222-222222222222";

struct Fixture {
    _dir: tempfile::TempDir,
    sidecar_path: PathBuf,
}

impl Fixture {
    fn seeded() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let sidecar_path = dir.path().join("data").join("pid-identity.db");
        let state_dir = dir.path().join("agent-bash").join("h-read-only");
        std::fs::create_dir_all(&state_dir).unwrap();
        let meta = state_dir.join("meta.json");
        let log = state_dir.join("log");
        let rc = state_dir.join("rc");
        std::fs::write(&meta, r#"{"caller_chain":[]}"#).unwrap();
        std::fs::write(&log, "historical log\n").unwrap();
        std::fs::write(&rc, "0\n").unwrap();
        let mut mailbox = MailboxDb::open(&sidecar_path).unwrap();
        let row = match mailbox
            .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
                session_id: SESSION,
                handle: "h-read-only",
                payload_json: r#"{"schema_version":1,"kind":"agent_bash_complete"}"#,
                owner_invocation_uuid: Some(INVOCATION),
                matched_os_pid: Some(9_300),
                matched_os_boot_id: Some("boot-read-only"),
                matched_os_pid_starttime_ticks: Some(456),
                matched_chain_index: Some(0),
                state_dir: &path_string(&state_dir),
                meta_path: &path_string(&meta),
                log_path: &path_string(&log),
                rc_path: &path_string(&rc),
                rc: 0,
            })
            .unwrap()
        {
            EnqueueResult::Inserted(row) => row,
            other => panic!("unexpected enqueue result: {other:?}"),
        };
        mailbox
            .register_delivery_attempt(ATTEMPT, SESSION, INVOCATION, &[row.seq], 0)
            .unwrap();
        assert!(
            mailbox
                .record_delivery_attempt_transport_ack(ATTEMPT)
                .unwrap()
        );
        let claim = mailbox
            .wake_sessions()
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: SESSION,
                claim_token: CLAIM,
                reason: "notify_idle",
                auto_wake_count: 1,
                wake_invocation_uuid: Some(INVOCATION),
                stale_after_seconds: 600,
            })
            .unwrap();
        assert!(matches!(claim, WakeClaimAcquireResult::Acquired(_)));
        let generation = RuntimeGenerationId::parse(GENERATION).unwrap();
        mailbox
            .runtime_lifecycle()
            .create_runtime_generation(CreateRuntimeGeneration {
                generation_id: &generation,
                spawn_invocation_uuid: INVOCATION,
                session_id: Some(SESSION),
                runtime_mode: "headless",
                provider_name: "provider-read-only",
                model_name: Some("model-read-only"),
                pty_control_path: None,
                models_dir: Some("/models/read-only"),
                effective_cwd: Some("/work/read-only"),
            })
            .unwrap();
        mailbox
            .wake_sessions()
            .upsert_session_metadata(SessionMetadataUpsert {
                session_id: SESSION,
                mode: "headless",
                invocation_uuid: Some(INVOCATION),
                provider_name: Some("provider-read-only"),
                model_name: Some("model-read-only"),
                models_dir: Some("/models/read-only"),
                effective_cwd: Some("/work/read-only"),
                selected_auto_wake_max: Some(5),
            })
            .unwrap();
        drop(mailbox);
        Self {
            _dir: dir,
            sidecar_path,
        }
    }
}

#[test]
fn mailbox_open_read_only_preserves_files_and_recovers_claim_and_attempt_history() {
    let fixture = Fixture::seeded();
    let parent = fixture.sidecar_path.parent().unwrap();
    let wal = path_with_suffix(&fixture.sidecar_path, "-wal");
    let shm = path_with_suffix(&fixture.sidecar_path, "-shm");
    assert!(
        !wal.exists(),
        "closed fixture must begin without a WAL sidecar"
    );
    assert!(
        !shm.exists(),
        "closed fixture must begin without an SHM sidecar"
    );
    let before = physical_snapshot(parent);

    let mailbox = MailboxDb::open_read_only(&fixture.sidecar_path).unwrap();
    let rows = mailbox.list_mailbox(SESSION, true).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].handle, "h-read-only");
    assert_eq!(rows[0].delivery_attempts, 0);
    let claim = mailbox
        .wake_session_reader()
        .wake_claim(SESSION)
        .unwrap()
        .unwrap();
    assert_eq!(claim.claim_token, CLAIM);
    assert_eq!(claim.min_pending_seq_at_claim, Some(rows[0].seq));
    assert_eq!(claim.max_pending_seq_at_claim, Some(rows[0].seq));
    assert_eq!(
        mailbox.delivery_attempt_disposition(ATTEMPT).unwrap(),
        Some(MailboxDeliveryAttemptDisposition::Pending)
    );
    let window = mailbox.delivery_attempt_window(ATTEMPT).unwrap().unwrap();
    assert_eq!(window.attempt_id, ATTEMPT);
    assert_eq!(window.session_id, SESSION);
    assert_eq!(window.delivery_invocation_uuid, INVOCATION);
    assert!(window.acknowledged_at.is_some());
    assert!(window.resolved_at.is_none());
    assert_eq!(
        window.rows.iter().map(|row| row.seq).collect::<Vec<_>>(),
        [rows[0].seq]
    );
    let generation = mailbox
        .runtime_lifecycle_reader()
        .runtime_generation(&RuntimeGenerationId::parse(GENERATION).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(generation.spawn_invocation_uuid, INVOCATION);
    assert_eq!(generation.session_id.as_deref(), Some(SESSION));
    assert_eq!(generation.provider_name, "provider-read-only");
    let metadata = mailbox
        .wake_session_reader()
        .session_metadata(SESSION)
        .unwrap()
        .unwrap();
    let projection = mailbox
        .wake_session_reader()
        .legacy_runtime_projection(SESSION)
        .unwrap()
        .unwrap();
    assert_eq!(metadata.invocation_uuid.as_deref(), Some(INVOCATION));
    assert_eq!(projection.run_state, "idle");
    assert_eq!(
        metadata.provider_name.as_deref(),
        Some("provider-read-only")
    );
    drop(mailbox);

    assert_physical_snapshot_unchanged(&before, &physical_snapshot(parent));
    assert!(
        !wal.exists(),
        "read-only open must not create a WAL sidecar"
    );
    assert!(
        !shm.exists(),
        "read-only open must not create an SHM sidecar"
    );
}

#[test]
fn mailbox_open_read_only_recovers_committed_wal_state_without_mutating_source() {
    let fixture = Fixture::seeded();
    let parent = fixture.sidecar_path.parent().unwrap();
    let main_before = std::fs::read(&fixture.sidecar_path).unwrap();
    let mut writer = MailboxDb::open(&fixture.sidecar_path).unwrap();
    writer
        .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
            session_id: SESSION,
            handle: "h-committed-in-wal",
            payload_json: r#"{"schema_version":1,"kind":"agent_bash_complete","wal":true}"#,
            owner_invocation_uuid: Some(INVOCATION),
            matched_os_pid: None,
            matched_os_boot_id: None,
            matched_os_pid_starttime_ticks: None,
            matched_chain_index: None,
            state_dir: "/wal/state",
            meta_path: "/wal/meta.json",
            log_path: "/wal/log",
            rc_path: "/wal/rc",
            rc: 0,
        })
        .unwrap();
    assert_eq!(
        std::fs::read(&fixture.sidecar_path).unwrap(),
        main_before,
        "the committed row must still reside outside the main database"
    );
    assert!(path_with_suffix(&fixture.sidecar_path, "-wal").exists());
    let before = physical_snapshot(parent);

    let mailbox = MailboxDb::open_read_only(&fixture.sidecar_path).unwrap();
    let rows = mailbox.list_mailbox(SESSION, true).unwrap();

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].handle, "h-committed-in-wal");
    drop(mailbox);
    assert_physical_snapshot_unchanged(&before, &physical_snapshot(parent));
    drop(writer);
}

#[cfg(unix)]
#[test]
fn mailbox_open_read_only_through_leaf_symlink_recovers_canonical_wal_state() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::seeded();
    let alias_path = fixture.sidecar_path.with_file_name("pid-identity-alias.db");
    let mut writer = MailboxDb::open(&fixture.sidecar_path).unwrap();
    writer
        .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
            session_id: SESSION,
            handle: "h-canonical-wal",
            payload_json: r#"{"schema_version":1,"kind":"agent_bash_complete","wal":true}"#,
            owner_invocation_uuid: Some(INVOCATION),
            matched_os_pid: None,
            matched_os_boot_id: None,
            matched_os_pid_starttime_ticks: None,
            matched_chain_index: None,
            state_dir: "/wal/state",
            meta_path: "/wal/meta.json",
            log_path: "/wal/log",
            rc_path: "/wal/rc",
            rc: 0,
        })
        .unwrap();
    assert!(path_with_suffix(&fixture.sidecar_path, "-wal").exists());
    symlink(&fixture.sidecar_path, &alias_path).unwrap();
    let before = physical_snapshot(fixture.sidecar_path.parent().unwrap());

    let mailbox = MailboxDb::open_read_only(&alias_path).unwrap();
    let rows = mailbox.list_mailbox(SESSION, true).unwrap();

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].handle, "h-canonical-wal");
    drop(mailbox);
    assert_physical_snapshot_unchanged(
        &before,
        &physical_snapshot(fixture.sidecar_path.parent().unwrap()),
    );
    drop(writer);
}

#[test]
fn mailbox_open_read_only_rejects_multi_link_database_identity() {
    let fixture = Fixture::seeded();
    let alias_path = fixture
        .sidecar_path
        .with_file_name("pid-identity-hard-link.db");
    std::fs::hard_link(&fixture.sidecar_path, &alias_path).unwrap();

    let result = MailboxDb::open_read_only(&alias_path);

    assert!(
        result.is_err(),
        "multi-link SQLite identity must be rejected"
    );
}

#[test]
fn mailbox_open_read_only_rejects_multi_link_wal_identity() {
    let fixture = Fixture::seeded();
    let mut writer = MailboxDb::open(&fixture.sidecar_path).unwrap();
    writer
        .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
            session_id: SESSION,
            handle: "h-multi-link-wal",
            payload_json: r#"{"schema_version":1,"kind":"agent_bash_complete","wal":true}"#,
            owner_invocation_uuid: Some(INVOCATION),
            matched_os_pid: None,
            matched_os_boot_id: None,
            matched_os_pid_starttime_ticks: None,
            matched_chain_index: None,
            state_dir: "/wal/state",
            meta_path: "/wal/meta.json",
            log_path: "/wal/log",
            rc_path: "/wal/rc",
            rc: 0,
        })
        .unwrap();
    let wal = path_with_suffix(&fixture.sidecar_path, "-wal");
    let second_link = fixture.sidecar_path.with_file_name("linked-wal");
    assert!(wal.exists());
    std::fs::hard_link(&wal, &second_link).unwrap();

    let result = MailboxDb::open_read_only(&fixture.sidecar_path);

    assert!(
        result.is_err(),
        "a multi-link WAL must be rejected like a multi-link main database"
    );
    drop(writer);
}

#[test]
fn state_open_read_only_recovers_committed_wal_invocation_and_turn_without_mutating_source() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    let state_path = data_dir.join("state.db");
    let initial = StateDb::open(&state_path).unwrap();
    initial
        .start_invocation(&InvocationStart {
            invocation_uuid: INVOCATION.to_string(),
            model_name: "model-before-wal".to_string(),
            provider_name: "provider-read-only".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    drop(initial);

    let writer = StateDb::open(&state_path).unwrap();
    let writer_connection = Connection::open(&state_path).unwrap();
    writer_connection
        .execute_batch("PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    let main_before = std::fs::read(&state_path).unwrap();
    writer_connection
        .execute(
            "UPDATE invocations SET model_name = 'model-committed-in-wal' WHERE invocation_uuid = ?1",
            [INVOCATION],
        )
        .unwrap();
    writer
        .ingest_session_turns_batch(
            "provider-read-only",
            &[SessionTurnIngest {
                session_id: SESSION.to_string(),
                turn_id: "turn-committed-in-wal".to_string(),
                timestamp: Utc::now(),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: Some("committed WAL turn".to_string()),
            }],
        )
        .unwrap();
    assert_eq!(
        std::fs::read(&state_path).unwrap(),
        main_before,
        "the committed invocation and turn must still reside outside the main database"
    );
    assert!(path_with_suffix(&state_path, "-wal").exists());
    let before = physical_snapshot(&data_dir);

    let state = StateDb::open_read_only(&state_path).unwrap();
    let invocation = state.get_invocation_by_uuid(INVOCATION).unwrap().unwrap();
    assert_eq!(invocation.model_name, "model-committed-in-wal");
    let counts = state
        .count_session_turns("provider-read-only", SESSION)
        .unwrap();
    assert_eq!(counts.total, 1);
    assert_eq!(counts.assistant, 1);
    drop(state);
    assert_physical_snapshot_unchanged(&before, &physical_snapshot(&data_dir));
    drop(writer);
}

#[cfg(unix)]
#[test]
fn state_open_read_only_leaf_symlink_uses_only_canonical_sidecars() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    let state_path = data_dir.join("state.db");
    let alias_path = data_dir.join("state-alias.db");
    let initial = StateDb::open(&state_path).unwrap();
    initial
        .start_invocation(&InvocationStart {
            invocation_uuid: INVOCATION.to_string(),
            model_name: "model-before-wal".to_string(),
            provider_name: "provider-read-only".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    drop(initial);

    let writer = Connection::open(&state_path).unwrap();
    writer
        .execute_batch("PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    writer
        .execute(
            "UPDATE invocations SET model_name = 'model-in-canonical-wal' WHERE invocation_uuid = ?1",
            [INVOCATION],
        )
        .unwrap();
    assert!(path_with_suffix(&state_path, "-wal").exists());
    symlink(&state_path, &alias_path).unwrap();
    let alias_wal = path_with_suffix(&alias_path, "-wal");
    std::fs::write(&alias_wal, "unrelated alias artifact").unwrap();
    let mut permissions = std::fs::metadata(&alias_wal).unwrap().permissions();
    permissions.set_mode(0o000);
    std::fs::set_permissions(&alias_wal, permissions).unwrap();

    let state = StateDb::open_read_only(&alias_path).unwrap();
    let invocation = state.get_invocation_by_uuid(INVOCATION).unwrap().unwrap();

    assert_eq!(invocation.model_name, "model-in-canonical-wal");
    assert_eq!(state.path(), state_path.canonicalize().unwrap());
    let reopened = StateDb::open_read_only(state.path()).unwrap();
    let reopened_invocation = reopened
        .get_invocation_by_uuid(INVOCATION)
        .unwrap()
        .unwrap();
    assert_eq!(reopened_invocation.model_name, "model-in-canonical-wal");
    drop(reopened);
    drop(state);
    drop(writer);
}

#[test]
fn read_only_missing_paths_for_all_facades_create_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let parent = dir.path().join("missing-parent");
    let state_path = parent.join("state.db");
    let sidecar_path = parent.join("pid-identity.db");

    assert!(StateDb::open_read_only(&state_path).is_err());
    assert!(PidIdentityDb::open_read_only(&sidecar_path).is_err());
    assert!(MailboxDb::open_read_only(&sidecar_path).is_err());

    assert!(!parent.exists());
    for path in [state_path, sidecar_path] {
        assert!(!path.exists());
        assert!(!path_with_suffix(&path, "-wal").exists());
        assert!(!path_with_suffix(&path, "-shm").exists());
        assert!(!path_with_suffix(&path, "-journal").exists());
    }
}

#[test]
fn pid_identity_open_read_only_preserves_source_and_recovers_exact_identity() {
    let dir = tempfile::tempdir().unwrap();
    let sidecar_path = dir.path().join("pid-identity.db");
    let identity = ProcessIdentity {
        os_pid: 9_301,
        os_boot_id: "boot-pid-read-only".to_string(),
        os_pid_starttime_ticks: 789,
    };
    let writer = PidIdentityDb::open(&sidecar_path).unwrap();
    writer
        .record_identity(PidIdentityRecord {
            identity: &identity,
            os_pgid: Some(9_300),
            invocation_uuid: INVOCATION,
            session_id: Some(SESSION),
            provider_name: Some("provider-read-only"),
            model_name: Some("model-read-only"),
            recorded_at: "2026-08-07T00:00:00Z",
        })
        .unwrap();
    drop(writer);
    let before = physical_snapshot(dir.path());

    let pid = PidIdentityDb::open_read_only(&sidecar_path).unwrap();
    let row = pid.lookup_by_identity(&identity).unwrap().unwrap();

    assert_eq!(row.invocation_uuid, INVOCATION);
    assert_eq!(row.session_id.as_deref(), Some(SESSION));
    assert_eq!(row.identity(), identity);
    drop(pid);
    assert_physical_snapshot_unchanged(&before, &physical_snapshot(dir.path()));
}

#[test]
fn writer_open_paths_retain_current_schema_and_wal_behavior() {
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("state.db");
    let sidecar_path = dir.path().join("pid-identity.db");

    let state = StateDb::open(&state_path).unwrap();
    let state_journal: String = state
        .connection()
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    let schema_version: i32 = state
        .connection()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(state_journal.to_ascii_lowercase(), "wal");
    assert_eq!(schema_version, CURRENT_SCHEMA_VERSION);

    let mailbox = MailboxDb::open(&sidecar_path).unwrap();
    let mailbox_connection = Connection::open(&sidecar_path).unwrap();
    let mailbox_journal: String = mailbox_connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    let mailbox_table_count: u32 = mailbox_connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name = 'mailbox'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(mailbox_journal.to_ascii_lowercase(), "wal");
    assert_eq!(mailbox_table_count, 1);
    drop(mailbox);
}

#[test]
fn mailbox_open_read_only_missing_path_creates_no_parent_schema_or_sidecars() {
    let dir = tempfile::tempdir().unwrap();
    let parent = dir.path().join("missing-parent");
    let path = parent.join("pid-identity.db");

    let result = MailboxDb::open_read_only(&path);

    assert!(result.is_err());
    assert!(!parent.exists());
    assert!(!path_with_suffix(&path, "-wal").exists());
    assert!(!path_with_suffix(&path, "-shm").exists());
}

#[derive(Debug, PartialEq, Eq)]
struct PhysicalSnapshot {
    directories: Vec<(PathBuf, std::time::SystemTime, bool)>,
    files: Vec<(PathBuf, Vec<u8>)>,
}

fn physical_snapshot(root: &Path) -> PhysicalSnapshot {
    let mut snapshot = PhysicalSnapshot {
        directories: Vec::new(),
        files: Vec::new(),
    };
    collect_entries(root, root, &mut snapshot);
    snapshot
        .directories
        .sort_by(|left, right| left.0.cmp(&right.0));
    snapshot.files.sort_by(|left, right| left.0.cmp(&right.0));
    snapshot
}

fn assert_physical_snapshot_unchanged(before: &PhysicalSnapshot, after: &PhysicalSnapshot) {
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

fn collect_entries(root: &Path, current: &Path, snapshot: &mut PhysicalSnapshot) {
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
            collect_entries(root, &path, snapshot);
        } else {
            snapshot.files.push((
                path.strip_prefix(root).unwrap().to_path_buf(),
                std::fs::read(path).unwrap(),
            ));
        }
    }
}

fn path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", path.display()))
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
