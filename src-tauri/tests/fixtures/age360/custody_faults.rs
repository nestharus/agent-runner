//! Fault characterizations deliberately distinguish retained debt from recovery.
//! `observes_unresolved_*` passing means the gap was observed, NOT AC acceptance.
use super::*;
use oulipoly_state::completion_continuation::SourceProcessIdentity;
use oulipoly_state::mailbox::ContinuationAttempt;

fn enqueue(f: &Fixture) {
    MailboxDb::open(&f.data.join("pid-identity.db"))
        .unwrap()
        .enqueue_submitted_input(&oulipoly_state::mailbox::SubmittedInputEnqueue {
            submission_token: "native-custody-input",
            target: oulipoly_state::mailbox::InboxTarget {
                kind: oulipoly_state::mailbox::InboxTargetKind::Session,
                id: SESSION,
            },
            input: b"native-custody-input",
        })
        .unwrap();
}
fn reached(f: &Fixture, name: &str) -> i64 {
    wait(|| {
        fs::read_to_string(f.root.path().join(format!("{name}.reached")))
            .ok()?
            .parse()
            .ok()
    })
}
fn remove_hold(f: &Fixture, name: &str) {
    fs::remove_file(f.root.path().join(format!("{name}.hold"))).unwrap();
}
fn kill_exact(identity: &SourceProcessIdentity) {
    assert!(current_identity_matches(identity));
    assert_eq!(unsafe { libc::kill(identity.pid as i32, libc::SIGKILL) }, 0);
}
fn process_identity(pid: i64) -> SourceProcessIdentity {
    let v = read_live_process_identity(pid).unwrap().unwrap();
    SourceProcessIdentity {
        pid,
        boot_id: v.os_boot_id,
        starttime_ticks: v.os_pid_starttime_ticks,
    }
}
fn attempt(f: &Fixture) -> ContinuationAttempt {
    let claim = f
        .mailbox()
        .wake_session_reader()
        .wake_claim(SESSION)
        .unwrap()
        .unwrap();
    f.mailbox()
        .continuation_activation(SESSION, &claim.claim_token)
        .unwrap()
        .unwrap()
}
fn assert_unresolved(f: &Fixture, a: &ContinuationAttempt) {
    let (phase, integrated, receipt): (String, i64, Option<String>) = f.sidecar_connection()
        .query_row("SELECT phase,integrated,drain_receipt FROM completion_continuation_attempt WHERE attempt_id=?1", [&a.attempt_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert!(!matches!(phase.as_str(), "drained" | "never_started"));
    assert_eq!(integrated, 0);
    assert!(receipt.is_none());
    assert!(
        f.mailbox()
            .wake_session_reader()
            .wake_claim(SESSION)
            .unwrap()
            .is_some()
    );
    assert!(!PathBuf::from(&a.result_path).exists());
    assert!(
        !PathBuf::from(&a.result_path)
            .with_file_name("adopting-result.json")
            .exists()
    );
    println!(
        "UNRESOLVED RECOVERY (not acceptance): attempt={} phase={phase} integrated={integrated} receipt={receipt:?}",
        a.attempt_id
    );
}
fn start_pre_attachment(f: &Fixture) -> oulipoly_state::mailbox::CompletionDomainOwner {
    let mut initial = f.start_with_hold(true);
    let owner = f.owner();
    wait(|| {
        f.root
            .path()
            .join("provider-initial-ready")
            .exists()
            .then_some(())
    });
    enqueue(f);
    f.gate("release-initial-provider");
    f.wait_initial(&mut initial);
    owner
}

#[test]
fn native_original_driver_recovers_adopter_loss_before_ac_fork() {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    f.gate("adopter-before-ac-fork.hold");
    let owner = start_pre_attachment(&f);
    let adopter = process_identity(reached(&f, "adopter-before-ac-fork"));
    let a = attempt(&f);
    f.gate("driver-reaped-echild.hold");
    kill_exact(&adopter);
    assert_eq!(
        reached(&f, "driver-reaped-echild"),
        owner.driver_identity.pid
    );
    let (phase, integrated, receipt): (String, i64, String) = f.sidecar_connection().query_row(
        "SELECT phase,integrated,drain_receipt FROM completion_continuation_attempt WHERE attempt_id=?1", [&a.attempt_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_eq!((phase.as_str(), integrated), ("never_started", 1));
    assert!(receipt.contains("waitid_wnowait"));
    assert!(
        PathBuf::from(&a.result_path)
            .with_file_name("pre-fork-result.json")
            .exists()
    );
    assert!(
        f.mailbox()
            .wake_session_reader()
            .wake_claim(SESSION)
            .unwrap()
            .is_none()
    );
    assert!(!f.root.path().join("resume-prompts.jsonl").exists());
    println!("original driver integrated actual adopter wait plus explicit unspent fork gate");
}

#[test]
fn native_original_ac_receipt_after_adopter_loss_before_ac_announcement() {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    f.gate("adopter-before-ac-announce.hold");
    start_pre_attachment(&f);
    let adopter = process_identity(reached(&f, "adopter-before-ac-announce"));
    let children = fs::read_to_string(format!(
        "/proc/{}/task/{}/children",
        adopter.pid, adopter.pid
    ))
    .unwrap();
    let ac = process_identity(children.split_whitespace().next().unwrap().parse().unwrap());
    let a = attempt(&f);
    kill_exact(&adopter);
    wait(|| {
        f.mailbox()
            .continuation_activation(SESSION, a.claim_token.as_ref().unwrap())
            .ok()?
            .is_none()
            .then_some(())
    });
    assert!(!f.root.path().join("resume-prompts.jsonl").exists());
    let receipt: String = f
        .sidecar_connection()
        .query_row(
            "SELECT drain_receipt FROM completion_continuation_attempt WHERE attempt_id=?1",
            [&a.attempt_id],
            |r| r.get(0),
        )
        .unwrap();
    let receipt: serde_json::Value = serde_json::from_str(&receipt).unwrap();
    assert_eq!(receipt["gate"], "unreleased_eof");
    assert_eq!(receipt["custodian"], serde_json::to_value(ac).unwrap());
    println!("adopter-loss original AC receipt={receipt}");
}

#[test]
fn native_original_ac_unreleased_receipt_survives_driver_loss_before_attachment() {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    f.gate("attempt-before-attachment.hold");
    let owner = start_pre_attachment(&f);
    assert_eq!(
        reached(&f, "attempt-before-attachment"),
        owner.driver_identity.pid
    );
    let a = attempt(&f);
    let attached: Option<String> = f
        .sidecar_connection()
        .query_row(
            "SELECT custodian_identity FROM completion_continuation_attempt WHERE attempt_id=?1",
            [&a.attempt_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(attached.is_none());
    kill_exact(&owner.driver_identity);
    wait(|| {
        f.mailbox()
            .continuation_activation(SESSION, a.claim_token.as_ref().unwrap())
            .ok()?
            .is_none()
            .then_some(())
    });
    let (phase, receipt): (String, String) = f
        .sidecar_connection()
        .query_row(
            "SELECT phase,drain_receipt FROM completion_continuation_attempt WHERE attempt_id=?1",
            [&a.attempt_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(phase, "never_started");
    let value: serde_json::Value = serde_json::from_str(&receipt).unwrap();
    assert_eq!(value["gate"], "unreleased_eof");
    assert_eq!(value["owned_children"], "ECHILD");
    assert_eq!(fs::read_to_string(&a.result_path).unwrap(), receipt);
    assert!(!f.root.path().join("resume-prompts.jsonl").exists());
    println!("pre-attachment original AC receipt={receipt}");
}

#[test]
fn native_observes_unresolved_both_attempt_owners_lost_before_receipt() {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    f.gate("test-descendant-enabled");
    f.gate("release-resume");
    let owner = start_pre_attachment(&f);
    let descendant = wait(|| {
        fs::read_to_string(f.root.path().join("descendant.pid"))
            .ok()?
            .parse::<i64>()
            .ok()
    });
    wait(|| {
        f.root
            .path()
            .join("recipient-exact-ack.json")
            .exists()
            .then_some(())
    });
    let a = attempt(&f);
    let (ac, adopter): (String, String) = f.sidecar_connection().query_row("SELECT custodian_identity,adopter_identity FROM completion_continuation_attempt WHERE attempt_id=?1", [&a.attempt_id], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
    // Stop CD after a real reap pass so it cannot consume the next waits until
    // both original boundaries are killed. No successor is used as the reaper.
    f.gate("driver-replay.hold");
    reached(&f, "driver-replay");
    f.gate("driver-reaped-echild.hold");
    kill_exact(&serde_json::from_str(&adopter).unwrap());
    kill_exact(&serde_json::from_str(&ac).unwrap());
    let launcher = f
        .mailbox()
        .continuation_launcher_identity(&a)
        .unwrap()
        .unwrap();
    wait(|| (parent(launcher.pid) == owner.driver_identity.pid).then_some(()));
    assert_unresolved(&f, &a);
    println!(
        "actual original CD adopted launcher={} parent={} retaining descendant={descendant}",
        launcher.pid,
        parent(launcher.pid)
    );
    f.gate("release-descendant");
    remove_hold(&f, "driver-replay");
    assert_eq!(
        reached(&f, "driver-reaped-echild"),
        owner.driver_identity.pid
    );
    assert_unresolved(&f, &a);
    assert_eq!(
        fs::read_to_string(f.root.path().join("resume-prompts.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    // Existing retained-result replay has no producer evidence to integrate.
    // This test is a deterministic characterization, not a recovery PASS.
}

#[test]
fn native_original_ac_receipt_after_adopter_loss_before_relaying_grant() {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    f.gate("adopter-before-ac-release.hold");
    start_pre_attachment(&f);
    let adopter = process_identity(reached(&f, "adopter-before-ac-release"));
    let a = attempt(&f);
    let attached: String = f
        .sidecar_connection()
        .query_row(
            "SELECT adopter_identity FROM completion_continuation_attempt WHERE attempt_id=?1",
            [&a.attempt_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<SourceProcessIdentity>(&attached).unwrap(),
        adopter
    );
    kill_exact(&adopter);
    wait(|| {
        f.mailbox()
            .continuation_activation(SESSION, a.claim_token.as_ref().unwrap())
            .ok()?
            .is_none()
            .then_some(())
    });
    let (phase, receipt): (String, String) = f
        .sidecar_connection()
        .query_row(
            "SELECT phase,drain_receipt FROM completion_continuation_attempt WHERE attempt_id=?1",
            [&a.attempt_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(phase, "never_started");
    let value: serde_json::Value = serde_json::from_str(&receipt).unwrap();
    assert_eq!(value["gate"], "unreleased_eof");
    assert_eq!(value["owned_children"], "ECHILD");
    assert_eq!(fs::read_to_string(&a.result_path).unwrap(), receipt);
    assert!(!f.root.path().join("resume-prompts.jsonl").exists());
    println!("granted but not relayed: original AC receipt={receipt}");
}

fn prelaunch_cancellation(operation: &str) {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    let mut initial = f.start_with_hold(true);
    f.owner();
    wait(|| {
        f.root
            .path()
            .join("provider-initial-ready")
            .exists()
            .then_some(())
    });
    f.gate(&format!("hold-native-{operation}"));
    enqueue(&f);
    f.gate("release-initial-provider");
    f.wait_initial(&mut initial);
    reached(&f, &format!("native-{operation}"));
    let a = attempt(&f);
    request_linked_cancel(&f, &a);
    let token = fs::read_to_string(f.root.path().join("state-cancel-token")).unwrap();
    let launch = token.split(':').next().unwrap();
    wait(|| {
        let state = oulipoly_state::StateDb::open_read_only(&f.data.join("state.db")).ok()?;
        let status: String = state
            .connection()
            .query_row(
                "SELECT status FROM provider_logical_launches WHERE logical_launch_id=?1",
                [launch],
                |r| r.get(0),
            )
            .ok()?;
        (status == "cancelled").then_some(())
    });
    let (phase, integrated): (String, i64) = f
        .sidecar_connection()
        .query_row(
            "SELECT phase,integrated FROM completion_continuation_attempt WHERE attempt_id=?1",
            [&a.attempt_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((phase.as_str(), integrated), ("drained", 1));
    assert!(!f.root.path().join("resume-prompts.jsonl").exists());
    println!("actual {operation} cancellation settled before launch; no recipient invocation");
}
#[test]
fn native_prelaunch_describe_cancellation_settles() {
    prelaunch_cancellation("describe");
}
#[test]
fn native_prelaunch_policy_cancellation_settles() {
    prelaunch_cancellation("policy.evaluate");
}

#[test]
fn native_request_io_failure_recovers_under_same_driver() {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("native_missing");
    f.gate("source-request-before-write.hold");
    let mut initial = f.start();
    let owner = f.owner();
    let source = f.source().registration().unwrap();
    assert_eq!(
        reached(&f, "source-request-before-write"),
        owner.driver_identity.pid
    );
    let a = f
        .mailbox()
        .pending_continuation_attempts()
        .unwrap()
        .into_iter()
        .find(|a| a.source_registration_id.as_deref() == Some(&source.registration_id))
        .unwrap();
    let directory = PathBuf::from(&a.result_path)
        .parent()
        .unwrap()
        .to_path_buf();
    // Actual request I/O obstruction, not a callback stub. No request has been
    // accepted and no actor can exist for this reserved attempt.
    fs::create_dir_all(directory.parent().unwrap()).unwrap();
    fs::write(&directory, b"private non-directory obstruction").unwrap();
    remove_hold(&f, "source-request-before-write");
    wait(|| {
        let phase: String = f
            .sidecar_connection()
            .query_row(
                "SELECT phase FROM completion_continuation_attempt WHERE attempt_id=?1",
                [&a.attempt_id],
                |r| r.get(0),
            )
            .ok()?;
        (phase == "never_started").then_some(())
    });
    fs::remove_file(&directory).unwrap();
    wait(|| {
        let count: i64 = f.sidecar_connection().query_row("SELECT COUNT(*) FROM completion_continuation_attempt WHERE source_registration_id=?1 AND attempt_id!=?2 AND phase='drained'", rusqlite::params![source.registration_id,a.attempt_id], |r|r.get(0)).ok()?;
        (count > 0).then_some(())
    });
    assert_eq!(f.owner().owner_generation, owner.owner_generation);
    assert!(current_identity_matches(&owner.driver_identity));
    println!(
        "same live driver revoked unaccepted I/O failure and actually drained a subsequent recovery producer"
    );
    initial.kill().unwrap();
    initial.wait().unwrap();
}

#[test]
fn native_repeated_excluded_recovery_allocates_no_untracked_directories() {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("native_missing");
    f.gate("adopter-before-ac-fork.hold");
    let mut initial = f.start();
    let owner = f.owner();
    let source = f.source().registration().unwrap();
    reached(&f, "adopter-before-ac-fork");
    let a = f
        .mailbox()
        .pending_continuation_attempts()
        .unwrap()
        .into_iter()
        .find(|a| a.source_registration_id.as_deref() == Some(&source.registration_id))
        .unwrap();
    kill_exact(&owner.driver_identity);
    wait(|| (f.owner().owner_generation != owner.owner_generation).then_some(()));
    let attempts = PathBuf::from(&a.result_path)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let count = || fs::read_dir(&attempts).unwrap().count();
    let before = count();
    std::thread::sleep(Duration::from_millis(2300));
    assert_eq!(
        count(),
        before,
        "rejected scans must not allocate UUID directories"
    );
    assert!(
        f.mailbox()
            .pending_continuation_attempt_ids(&source.registration_id)
            .unwrap()
            .contains(&a.attempt_id)
    );
    println!("repeated excluded scans retained predecessor debt without filesystem allocation");
    initial.kill().unwrap();
    initial.wait().unwrap();
}

#[test]
fn native_guardian_succession_is_not_blocked_by_live_adopter_announcement() {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    f.gate("adopter-before-ac-fork.hold");
    let owner = start_pre_attachment(&f);
    let adopter = process_identity(reached(&f, "adopter-before-ac-fork"));
    let a = attempt(&f);
    kill_exact(&owner.guardian_identity);
    let replacement = wait(|| {
        let next = f.owner();
        (next.owner_generation != owner.owner_generation).then_some(next)
    });
    assert_eq!(replacement.guardian_identity, owner.driver_identity);
    assert!(current_identity_matches(&adopter));
    assert_unresolved(&f, &a); // succession is not an attempt-drain certificate
    println!(
        "endpoint succession executed while original adopter remains live; attempt debt explicitly retained"
    );
}
