//! Admission recovery must visit later certifiable rows without clearing unknowns.
use super::*;
use oulipoly_core::launch_custody::LaunchCustody;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const FIXTURE_ROOT: &str = "OULIPOLY_TEST_LATER_RECOVERY_ROOT";
const ENDED_ID: &str = "90111111-1111-4111-8111-111111111112";
const UNKNOWN_ID: &str = "90111111-1111-4111-8111-111111111113";

fn identity() -> ProcessIdentity {
    pid_identity::read_live_process_identity(std::process::id().into())
        .unwrap()
        .unwrap()
}

fn create(db: &mut MailboxDb, id: &RuntimeGenerationId, invocation: &str, proof: Option<&Path>) {
    assert!(matches!(
        db.runtime_lifecycle()
            .create_runtime_generation_with_custody(
                CreateRuntimeGeneration {
                    generation_id: id,
                    spawn_invocation_uuid: invocation,
                    session_id: Some(invocation),
                    runtime_mode: "headless",
                    provider_name: "private-fixture",
                    model_name: None,
                    pty_control_path: None,
                    models_dir: None,
                    effective_cwd: None,
                },
                proof
            )
            .unwrap(),
        GenerationMutation::Applied(_)
    ));
}

fn eventually(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "private fixture deadline");
        std::thread::sleep(Duration::from_millis(5));
    }
}

struct Creator(Child);
impl Drop for Creator {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn later_recovery_creator_fixture() {
    let Some(root) = std::env::var_os(FIXTURE_ROOT) else {
        return;
    };
    let root = Path::new(&root);
    let mut db = MailboxDb::open(&root.join("pid-identity.db")).unwrap();
    db.session_admissions()
        .enqueue("ended", "ended", Some("ended"), &identity(), 1)
        .unwrap();
    assert!(matches!(
        db.session_admissions()
            .try_admit_next("ended-claim", 2, 0)
            .unwrap(),
        SessionAdmissionAttempt::Admitted(_)
    ));
    assert!(
        db.session_admissions()
            .begin_launch("ended", "ended-claim", 3)
            .unwrap()
    );
    let custody = LaunchCustody::start(root.join("ended-proof")).unwrap();
    create(
        &mut db,
        &RuntimeGenerationId::parse(ENDED_ID).unwrap(),
        "ended",
        Some(&root.join("ended-proof")),
    );
    create(
        &mut db,
        &RuntimeGenerationId::parse(UNKNOWN_ID).unwrap(),
        "unknown",
        None,
    );
    std::fs::write(root.join("ready"), b"ready").unwrap();
    // Real pre-launch creator death closes the endpoint. No time limit is Q.
    std::thread::sleep(Duration::from_secs(15));
    drop(custody);
    panic!("creator was not crashed by parent");
}

fn old_boot(current: &str) -> &'static str {
    if current == "11111111-1111-4111-8111-111111111111" {
        return "22222222-2222-4222-8222-222222222222";
    }
    "11111111-1111-4111-8111-111111111111"
}

fn state(db: &MailboxDb, id: &RuntimeGenerationId) -> RuntimeLifecycleState {
    db.runtime_lifecycle_reader()
        .runtime_generation(id)
        .unwrap()
        .unwrap()
        .lifecycle_state
}

#[test]
fn admission_recovers_later_ended_rows_behind_unknown_oldest() {
    let root = tempfile::tempdir().unwrap();
    let mut creator = Creator(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "mailbox::starting_recovery_tests::later_recovery_creator_fixture",
            ])
            .env_clear()
            .env("HOME", root.path())
            .env("XDG_CONFIG_HOME", root.path())
            .env("XDG_DATA_HOME", root.path())
            .env(FIXTURE_ROOT, root.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    eventually(|| root.path().join("ready").exists());
    let mut db = MailboxDb::open(&root.path().join("pid-identity.db")).unwrap();
    let unknown = RuntimeGenerationId::parse(UNKNOWN_ID).unwrap();
    // Persisted ordering fixture: an older unresolved historical row. Only
    // timestamps/old epoch are synthesized, not same-boot custody evidence.
    db.conn.execute("UPDATE runtime_generation SET created_at = '2000-01-01T00:00:00Z' WHERE generation_uuid = ?1",
        params![unknown.to_string()]).unwrap();
    let boot_ended = RuntimeGenerationId::new();
    create(&mut db, &boot_ended, "boot-ended", None);
    db.conn.execute("UPDATE runtime_generation SET creator_identity_os_boot_id = ?1 WHERE generation_uuid = ?2",
        params![old_boot(&identity().os_boot_id), boot_ended.to_string()]).unwrap();
    let live = RuntimeGenerationId::new();
    let live_custody = LaunchCustody::start(root.path().join("live-proof")).unwrap();
    create(
        &mut db,
        &live,
        "live",
        Some(&root.path().join("live-proof")),
    );
    live_custody.seal();
    eventually(|| live_custody.quiescent());
    creator.0.kill().unwrap();
    eventually(|| {
        matches!(
            pid_identity::observe_finalizer_process_identity(creator.0.id().into()),
            pid_identity::FinalizerProcessIdentityObservation::ExactExited(_)
        )
    });
    eventually(|| oulipoly_core::launch_custody::is_quiescent(&root.path().join("ended-proof")));
    let ended = RuntimeGenerationId::parse(ENDED_ID).unwrap();
    db.session_admissions()
        .enqueue("successor", "successor", None, &identity(), 4)
        .unwrap();
    for now in [5, 6] {
        assert_eq!(
            db.session_admissions()
                .try_admit_next("successor-claim", now, 0)
                .unwrap(),
            SessionAdmissionAttempt::LaunchMaterializing
        );
        assert_eq!(state(&db, &ended), RuntimeLifecycleState::Exited);
        assert_eq!(state(&db, &boot_ended), RuntimeLifecycleState::Exited);
        assert_eq!(state(&db, &unknown), RuntimeLifecycleState::Starting);
        assert_eq!(state(&db, &live), RuntimeLifecycleState::Starting);
        assert_eq!(
            db.session_admissions().row("ended").unwrap().unwrap().state,
            "settled"
        );
        assert_eq!(
            db.session_admissions()
                .row("successor")
                .unwrap()
                .unwrap()
                .state,
            "queued"
        );
    }
    assert_eq!(
        db.runtime_lifecycle_reader()
            .runtime_generation(&ended)
            .unwrap()
            .unwrap()
            .terminal_reason,
        Some(RuntimeTerminalReason::RecoveredDead)
    );
    assert!(matches!(
        db.runtime_lifecycle()
            .bind_runtime_generation_running(BindRuntimeGenerationRunning {
                fence: RuntimeGenerationFence {
                    generation_id: &ended,
                    spawn_invocation_uuid: "ended"
                },
                spawned_os_pid: identity().os_pid,
                exact_process_identity: &identity(),
                os_pgid: None,
            })
            .unwrap(),
        GenerationMutation::Rejected(_)
    ));
    assert_eq!(
        db.runtime_lifecycle()
            .reconcile_session_liveness("unknown")
            .unwrap(),
        SessionLiveness::Busy
    );
    assert_eq!(
        db.runtime_lifecycle()
            .reconcile_session_liveness("ended")
            .unwrap(),
        SessionLiveness::Idle
    );
    creator.0.wait().unwrap(); // retained real zombie through reconciliation
}
