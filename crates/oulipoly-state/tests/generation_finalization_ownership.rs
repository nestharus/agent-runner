//! Regression adapted from the retained sleeping diagnostic's two-DB real-exit probe.
#![cfg(target_os = "linux")]
use oulipoly_state::mailbox::*;
use oulipoly_state::pid_identity::{ProcessIdentity, read_live_process_identity};
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};

const INV: &str = "finalization-owner";
const SESSION: &str = "finalization-session";
fn identity(pid: u32) -> ProcessIdentity {
    read_live_process_identity(i64::from(pid)).unwrap().unwrap()
}
struct HeldChild(Child);
impl HeldChild {
    fn new(code: i32) -> Self {
        Self(
            Command::new("/bin/sh")
                .args(["-c", &format!("cat >/dev/null; exit {code}")])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }
    fn finish(&mut self) -> i32 {
        drop(self.0.stdin.take());
        self.0.wait().unwrap().code().unwrap()
    }
}
impl Drop for HeldChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn fence(id: &RuntimeGenerationId) -> RuntimeGenerationFence<'_> {
    RuntimeGenerationFence {
        generation_id: id,
        spawn_invocation_uuid: INV,
    }
}
fn create(db: &mut MailboxDb, id: &RuntimeGenerationId, child: &ProcessIdentity) {
    assert!(matches!(
        db.runtime_lifecycle()
            .create_runtime_generation(CreateRuntimeGeneration {
                generation_id: id,
                spawn_invocation_uuid: INV,
                session_id: Some(SESSION),
                runtime_mode: "headless",
                provider_name: "native-fixture",
                model_name: None,
                pty_control_path: None,
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap(),
        GenerationMutation::Applied(_)
    ));
    assert!(matches!(
        db.runtime_lifecycle()
            .bind_runtime_generation_running(BindRuntimeGenerationRunning {
                fence: fence(id),
                spawned_os_pid: child.os_pid,
                exact_process_identity: child,
                os_pgid: None,
            })
            .unwrap(),
        GenerationMutation::Applied(_)
    ));
}
fn recover(
    db: &mut MailboxDb,
    id: &RuntimeGenerationId,
) -> GenerationMutation<RuntimeGenerationRow> {
    db.runtime_lifecycle()
        .exit_runtime_generation_non_orderly(ExitRuntimeGenerationNonOrderly {
            fence: fence(id),
            reason: RuntimeTerminalReason::RecoveredDead,
            exit_code: None,
        })
        .unwrap()
}
fn busy(db: &mut MailboxDb) {
    assert_eq!(
        db.runtime_lifecycle_reader()
            .classify_session_liveness(SESSION)
            .unwrap(),
        RuntimeGenerationReadOnlyLiveness::Busy
    );
    assert_eq!(
        db.runtime_lifecycle()
            .reconcile_session_liveness(SESSION)
            .unwrap(),
        SessionLiveness::Busy
    );
}
#[test]
fn live_owner_retains_successful_exit_and_exact_duplicate_drain_authority() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pid-identity.db");
    let mut owner = MailboxDb::open(&path).unwrap();
    let mut observer = MailboxDb::open(&path).unwrap();
    let id = RuntimeGenerationId::new();
    let mut child = HeldChild::new(0);
    create(&mut owner, &id, &identity(child.0.id()));
    busy(&mut observer);
    assert_eq!(child.finish(), 0);
    busy(&mut observer);
    assert_eq!(
        recover(&mut observer, &id),
        GenerationMutation::Rejected(GenerationRejection::ProcessIdentityConflict)
    );
    let drain = DrainRequestId::new();
    let request = RequestRuntimeGenerationDrain {
        fence: fence(&id),
        drain_request_id: &drain,
        requested_by_invocation_uuid: INV,
    };
    assert!(matches!(
        owner
            .runtime_lifecycle()
            .request_runtime_generation_drain(request)
            .unwrap(),
        DrainRequestResult::Installed(..)
    ));
    assert!(matches!(
        owner
            .runtime_lifecycle()
            .request_runtime_generation_drain(request)
            .unwrap(),
        DrainRequestResult::AlreadyInstalled(..)
    ));
    assert!(matches!(
        owner
            .runtime_lifecycle()
            .advance_runtime_generation_drain(AdvanceRuntimeGenerationDrain {
                fence: fence(&id),
                drain_request_id: &drain,
            })
            .unwrap(),
        DrainAdvanceResult::Advanced(_)
    ));
    busy(&mut observer);
    assert_eq!(
        recover(&mut observer, &id),
        GenerationMutation::Rejected(GenerationRejection::ProcessIdentityConflict)
    );
    let finish = FinishRuntimeGenerationDrain {
        fence: fence(&id),
        drain_request_id: &drain,
        exit_code: Some(0),
        compatibility_exit_code: Some(0),
    };
    assert!(matches!(
        owner
            .runtime_lifecycle()
            .finish_runtime_generation_drain(finish)
            .unwrap(),
        DrainFinishResult::Finished(_)
    ));
    assert!(matches!(
        owner
            .runtime_lifecycle()
            .finish_runtime_generation_drain(finish)
            .unwrap(),
        DrainFinishResult::AlreadyExited(_)
    ));
    let other = DrainRequestId::new();
    assert!(matches!(
        owner
            .runtime_lifecycle()
            .finish_runtime_generation_drain(FinishRuntimeGenerationDrain {
                drain_request_id: &other,
                ..finish
            })
            .unwrap(),
        DrainFinishResult::Rejected(_)
    ));
    let row = owner
        .runtime_lifecycle_reader()
        .runtime_generation(&id)
        .unwrap()
        .unwrap();
    assert_eq!(
        row.terminal_reason,
        Some(RuntimeTerminalReason::OrderlyCompletion)
    );
    assert_eq!(row.exit_code, Some(0));
}
#[test]
fn actual_nonzero_remains_abnormal_not_recovered_or_successful() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pid-identity.db");
    let mut owner = MailboxDb::open(&path).unwrap();
    let mut observer = MailboxDb::open(&path).unwrap();
    let id = RuntimeGenerationId::new();
    let mut child = HeldChild::new(7);
    create(&mut owner, &id, &identity(child.0.id()));
    assert_eq!(child.finish(), 7);
    busy(&mut observer);
    assert!(matches!(
        owner
            .runtime_lifecycle()
            .exit_runtime_generation_non_orderly(ExitRuntimeGenerationNonOrderly {
                fence: fence(&id),
                reason: RuntimeTerminalReason::AbnormalTermination,
                exit_code: Some(7),
            })
            .unwrap(),
        GenerationMutation::Applied(_)
    ));
    let row = owner
        .runtime_lifecycle_reader()
        .runtime_generation(&id)
        .unwrap()
        .unwrap();
    assert_eq!(
        row.terminal_reason,
        Some(RuntimeTerminalReason::AbnormalTermination)
    );
    assert_eq!(row.exit_code, Some(7));
    let drain = DrainRequestId::new();
    assert!(matches!(
        owner
            .runtime_lifecycle()
            .request_runtime_generation_drain(RequestRuntimeGenerationDrain {
                fence: fence(&id),
                drain_request_id: &drain,
                requested_by_invocation_uuid: INV,
            })
            .unwrap(),
        DrainRequestResult::Rejected(GenerationRejection::IllegalPredecessor { .. })
    ));
}

// The fixture owns the real creator identity while the parent independently owns
// the provider child. Its abrupt termination must not mark a live child dead.
#[test]
fn creator_fixture() {
    let Ok(path) = std::env::var("GENERATION_OWNER_FIXTURE") else {
        return;
    };
    let id = RuntimeGenerationId::parse(&std::env::var("GENERATION_ID").unwrap()).unwrap();
    let child = identity(
        std::env::var("GENERATION_CHILD_PID")
            .unwrap()
            .parse()
            .unwrap(),
    );
    let mut db = MailboxDb::open(Path::new(&path)).unwrap();
    create(&mut db, &id, &child);
    std::fs::write(format!("{path}.ready"), "ready").unwrap();
    let _ = std::io::stdin().read(&mut [0]);
}
#[test]
fn true_creator_death_preserves_live_child_then_competing_recovery_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pid-identity.db");
    let id = RuntimeGenerationId::new();
    let mut child = HeldChild::new(0);
    let mut creator = HeldChild(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "creator_fixture"])
            .env("GENERATION_OWNER_FIXTURE", &path)
            .env("GENERATION_ID", id.to_string())
            .env("GENERATION_CHILD_PID", child.0.id().to_string())
            .stdin(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !path.with_extension("db.ready").exists() {
        assert!(std::time::Instant::now() < deadline);
        assert!(creator.0.try_wait().unwrap().is_none());
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    creator.0.kill().unwrap();
    assert!(!creator.0.wait().unwrap().success());
    let mut db = MailboxDb::open(&path).unwrap();
    busy(&mut db);
    assert_eq!(
        recover(&mut db, &id),
        GenerationMutation::Rejected(GenerationRejection::ProcessIdentityConflict)
    );
    assert_eq!(child.finish(), 0);
    assert_eq!(
        db.runtime_lifecycle_reader()
            .classify_session_liveness(SESSION)
            .unwrap(),
        RuntimeGenerationReadOnlyLiveness::StaleDead
    );
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let joins: Vec<_> = (0..2)
        .map(|_| {
            let path = path.clone();
            let id = id.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut db = MailboxDb::open(&path).unwrap();
                barrier.wait();
                recover(&mut db, &id)
            })
        })
        .collect();
    let outcomes: Vec<_> = joins.into_iter().map(|j| j.join().unwrap()).collect();
    assert_eq!(
        outcomes
            .iter()
            .filter(|r| matches!(r, GenerationMutation::Applied(_)))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|r| matches!(r, GenerationMutation::AlreadyApplied(_)))
            .count(),
        1
    );
    let row = db
        .runtime_lifecycle_reader()
        .runtime_generation(&id)
        .unwrap()
        .unwrap();
    assert_eq!(
        row.terminal_reason,
        Some(RuntimeTerminalReason::RecoveredDead)
    );
    assert_eq!(row.exit_code, None);
}

#[test]
fn uncertain_owner_or_child_identity_never_grants_recovery() {
    for column in ["creator_identity_os_pid", "identity_os_pid"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let mut db = MailboxDb::open(&path).unwrap();
        let id = RuntimeGenerationId::new();
        let mut replaced = identity(std::process::id());
        replaced.os_pid_starttime_ticks += 1;
        create(&mut db, &id, &replaced);
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute("UPDATE runtime_generation SET creator_identity_os_pid_starttime_ticks = creator_identity_os_pid_starttime_ticks + 1", []).unwrap();
        // Invalid/out-of-range identity is uncertain in the conservative observer,
        // even though the old generic observer calls it dead. The other identity
        // is provably replaced, so uncertainty is the sole blocker.
        let extra = if column == "identity_os_pid" {
            ", spawned_os_pid = ?1"
        } else {
            ""
        };
        conn.execute(
            &format!("UPDATE runtime_generation SET {column} = ?1{extra}"),
            [i64::MAX],
        )
        .unwrap();
        busy(&mut db);
        assert_eq!(
            recover(&mut db, &id),
            GenerationMutation::Rejected(GenerationRejection::InvariantViolation)
        );
    }
}
