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

fn unreleased_adopter_loss(barrier: &str, admitted: bool) {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    f.gate(&format!("{barrier}.hold"));
    let owner = start_pre_attachment(&f);
    let adopter = process_identity(reached(&f, barrier));
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
    let value: serde_json::Value = serde_json::from_str(&receipt).unwrap();
    assert_eq!(value["gate"], "unreleased_announcement_eof");
    let reason: serde_json::Value =
        serde_json::from_str(value["reason"].as_str().unwrap()).unwrap();
    assert_eq!(reason["original_ac_fork_gate"]["admitted"], admitted);
    assert_eq!(reason["execution_grant"], "not_sent");
    assert_eq!(reason["observation"], "waitid_wnowait");
    assert_eq!(
        reason["driver"],
        serde_json::to_value(&owner.driver_identity).unwrap()
    );
    assert_eq!(reason["adopter"], serde_json::to_value(&adopter).unwrap());
    assert!(
        PathBuf::from(&a.result_path)
            .with_file_name("unreleased-announcement-result.json")
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
    println!("original unreleased boundary receipt={receipt}; no fork existence claim");
}

#[test]
fn native_original_driver_recovers_adopter_loss_before_ac_fork() {
    unreleased_adopter_loss("adopter-before-ac-fork", false);
}

#[test]
fn native_original_driver_recovers_admitted_adopter_loss_before_actual_ac_fork() {
    unreleased_adopter_loss("adopter-admitted-before-ac-fork", true);
}

#[test]
fn native_created_ac_announces_after_adopter_loss_at_fork_return() {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    f.gate("adopter-after-ac-fork.hold");
    f.gate("ac-created-before-announce.hold");
    let owner = start_pre_attachment(&f);
    let adopter = process_identity(reached(&f, "adopter-after-ac-fork"));
    let ac = process_identity(reached(&f, "ac-created-before-announce"));
    assert_eq!(parent(ac.pid), adopter.pid);
    let a = attempt(&f);
    kill_exact(&adopter);
    wait(|| (parent(ac.pid) == owner.driver_identity.pid).then_some(()));
    // The inherited birth endpoint keeps this an outstanding real AC, not EOF.
    assert_unresolved(&f, &a);
    remove_hold(&f, "ac-created-before-announce");
    wait(|| {
        f.mailbox()
            .continuation_activation(SESSION, a.claim_token.as_ref().unwrap())
            .ok()?
            .is_none()
            .then_some(())
    });
    let receipt: String = f
        .sidecar_connection()
        .query_row(
            "SELECT drain_receipt FROM completion_continuation_attempt WHERE attempt_id=?1",
            [&a.attempt_id],
            |r| r.get(0),
        )
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&receipt).unwrap();
    assert_eq!(value["gate"], "unreleased_eof");
    assert_eq!(value["custodian"], serde_json::to_value(ac).unwrap());
    assert_eq!(value["owned_children"], "ECHILD");
    assert!(
        !PathBuf::from(&a.result_path)
            .with_file_name("unreleased-announcement-result.json")
            .exists()
    );
    assert!(!f.root.path().join("resume-prompts.jsonl").exists());
    assert!(current_identity_matches(&owner.driver_identity));
    println!("actual forked original AC retained and integrated {receipt}");
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
    f.gate("unreleased-before-commit.hold");
    f.gate("unreleased-commit-returned-ok.hold");
    f.gate("unreleased-commit-returned-error.hold");
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
    let ac_pid = reached(&f, "unreleased-before-commit");
    let ac = process_identity(ac_pid);
    let observe_live = || -> Option<String> {
        use rusqlite::OptionalExtension;
        f.sidecar_connection().query_row(
            "SELECT json_object('phase',phase,'revision',revision,'integrated',integrated,'receipt',drain_receipt,'claim',EXISTS(SELECT 1 FROM session_wake_claim c WHERE c.session_id=a.session_id AND c.claim_token=a.claim_token)) FROM completion_continuation_attempt a WHERE attempt_id=?1",
            [&a.attempt_id], |r| r.get(0)).optional().unwrap()
    };
    let artifact_header = |name: &str, limit: u64| -> Option<(u64, Vec<u8>)> {
        use std::io::Read;
        let file = fs::File::open(f.data.join(name)).ok()?;
        let len = file.metadata().ok()?.len();
        let mut bytes = Vec::new();
        file.take(limit).read_to_end(&mut bytes).ok()?;
        Some((len, bytes))
    };
    let copied_before = f.mailbox().age360_observe_attempt(&a.attempt_id).unwrap();
    println!("before original commit copied exact row={copied_before:?}");
    let before: serde_json::Value = serde_json::from_str(&observe_live().unwrap()).unwrap();
    assert_eq!(before["integrated"], 0);
    assert!(before["receipt"].is_null());
    assert_eq!(before["claim"], 1);
    let bytes: serde_json::Value =
        serde_json::from_slice(&fs::read(&a.result_path).unwrap()).unwrap();
    assert_eq!(bytes["custodian"], serde_json::to_value(&ac).unwrap());
    println!("actual original receipt persisted, commit not entered: row={before} receipt={bytes}");
    remove_hold(&f, "unreleased-before-commit");
    // Preserve distinct observations, never use copied activation absence as
    // an integration oracle. Each live row/receipt/claim is one coherent SELECT.
    let mut last = None;
    let live: serde_json::Value = wait(|| {
        let copied = f.mailbox();
        let copied_row = copied.age360_observe_attempt(&a.attempt_id).unwrap();
        let activation_absent = copied
            .continuation_activation(SESSION, a.claim_token.as_ref().unwrap())
            .unwrap()
            .is_none();
        let live_row = observe_live();
        let pair = (copied_row, live_row);
        if last.as_ref() != Some(&pair) {
            println!(
                "visibility attempt={} copied_absent_or_terminal={activation_absent} copied={:?} live={:?} wal_header={:?} shm_publication={:?}",
                a.attempt_id,
                pair.0,
                pair.1,
                artifact_header("pid-identity.db-wal", 32),
                artifact_header("pid-identity.db-shm", 96)
            );
            last = Some(pair.clone());
        }
        assert!(
            !f.root
                .path()
                .join("unreleased-commit-returned-error.reached")
                .exists(),
            "actual commit returned error"
        );
        let live: serde_json::Value =
            serde_json::from_str(pair.1.as_ref().expect("live exact row must remain")).unwrap();
        (live["phase"] == "never_started"
            && live["integrated"] == 1
            && live["receipt"].is_string()
            && live["claim"] == 0)
            .then_some(live)
    });
    assert_eq!(reached(&f, "unreleased-commit-returned-ok"), ac.pid);
    assert!(current_identity_matches(&ac));
    let receipt = live["receipt"].as_str().unwrap();
    println!("original AC commit returned Ok; live integrated row={live}");
    remove_hold(&f, "unreleased-commit-returned-ok");
    let value: serde_json::Value = serde_json::from_str(receipt).unwrap();
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

fn uncertain_aggregate_recovery(restore_producer: bool) {
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
    f.gate("hold-native-describe");
    f.gate("native-after-custody-retention.hold");
    enqueue(&f);
    f.gate("release-initial-provider");
    f.wait_initial(&mut initial);
    reached(&f, "native-describe");
    let a = attempt(&f);
    let (generation, invocation) =
        wait(|| f.mailbox().continuation_runtime_identity(&a).ok().flatten());
    let generation = uuid::Uuid::parse_str(&generation).unwrap();
    let invocation = uuid::Uuid::parse_str(&invocation).unwrap();
    let state = oulipoly_state::StateDb::open(&f.data.join("state.db")).unwrap();
    let source = state
        .native_attempt_recovery(generation, invocation)
        .unwrap()
        .unwrap();
    let actors = PathBuf::from(source["journal"].as_str().unwrap()).join("actors");
    let actor = fs::read_dir(&actors)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.join("proxy.stat").exists())
        .unwrap();
    let terminal = actor.join("terminal.json");
    // A real rename-to-directory failure in the producer's terminal journal.
    // The fixture never writes a receipt, aggregate, wait, or public proof.
    fs::create_dir(&terminal).unwrap();
    fs::remove_file(f.root.path().join("hold-native-describe")).unwrap();
    reached(&f, "native-after-custody-retention");
    let original = state
        .native_attempt_custody(generation, invocation)
        .unwrap()
        .unwrap();
    assert!(
        original["actors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["uncertain"] == true),
        "{original}"
    );
    assert!(terminal.is_dir());
    println!(
        "actual producer-terminal I/O failure retained original uncertain aggregate={original}"
    );
    if restore_producer {
        fs::remove_dir(&terminal).unwrap();
        // Deferred original cleanup can now preserve the actual terminal wait.
        wait(|| terminal.is_file().then_some(()));
        let stronger = fs::read_to_string(&terminal).unwrap();
        println!("stronger original producer terminal record={stronger}");
    }
    // Otherwise keep the producer journal unavailable through cancellation:
    // only the original AC/adopter's actual adopted WNOWAIT can recover it.
    request_linked_cancel(&f, &a);
    let token = fs::read_to_string(f.root.path().join("state-cancel-token")).unwrap();
    let launch = token.split(':').next().unwrap();
    wait(|| {
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
    assert_eq!(
        state
            .native_attempt_custody(generation, invocation)
            .unwrap()
            .unwrap(),
        original
    );
    let recovered = state
        .native_recovered_attempt_custody(generation, invocation)
        .unwrap()
        .unwrap();
    assert!(recovered["recovery_evidence"].is_object());
    let observations = recovered["recovery_evidence"]["actor_observations"]
        .as_array()
        .unwrap();
    if restore_producer {
        assert!(
            observations
                .iter()
                .any(|v| v["terminal"]["producer_terminal"].is_string())
        );
    } else {
        assert!(
            terminal.is_dir(),
            "no producer terminal record was repaired by the fixture"
        );
        assert!(
            observations
                .iter()
                .any(|v| v["terminal"]["observation"] == "waitid_wnowait")
        );
    }
    assert!(
        recovered["actors"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["uncertain"] == false)
    );
    assert!(!f.root.path().join("resume-prompts.jsonl").exists());
    println!("original uncertain aggregate unchanged; independent original recovery={recovered}");
}

#[test]
fn native_uncertain_aggregate_preserves_observation_after_producer_journal_recovery() {
    uncertain_aggregate_recovery(true);
}

#[test]
fn native_uncertain_aggregate_preserves_observation_after_original_owner_wait() {
    uncertain_aggregate_recovery(false);
}

#[test]
fn native_complete_prelaunch_aggregate_does_not_skip_original_runtime_settlement() {
    complete_prelaunch_runtime_settlement(false);
}

#[test]
fn native_birth_recovery_cancel_after_uncancelled_drain() {
    complete_prelaunch_runtime_settlement(true);
}

fn complete_prelaunch_runtime_settlement(late_cancel: bool) {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    f.gate("hold-native-policy.evaluate");
    f.gate("native-after-custody-retention.hold");
    start_pre_attachment(&f);
    let policy = process_identity(reached(&f, "native-policy.evaluate"));
    let a = attempt(&f);
    // Real transient sidecar failure while the original executor projects its
    // startup exit. Custody retention must remain independently usable later.
    let writable_sidecar = rusqlite::Connection::open(f.data.join("pid-identity.db")).unwrap();
    writable_sidecar.execute_batch("CREATE TRIGGER fixture_fail_runtime_exit BEFORE UPDATE ON runtime_generation WHEN NEW.lifecycle_state='exited' BEGIN SELECT RAISE(FAIL, 'fixture transient runtime projection failure'); END;").unwrap();
    // A genuine failed policy process, with the native executor still alive to
    // retain complete actor receipts; no fabricated terminal/custody records.
    kill_exact(&policy);
    reached(&f, "native-after-custody-retention");
    let (generation, invocation) = f
        .mailbox()
        .continuation_runtime_identity(&a)
        .unwrap()
        .unwrap();
    let g = uuid::Uuid::parse_str(&generation).unwrap();
    let i = uuid::Uuid::parse_str(&invocation).unwrap();
    let state = oulipoly_state::StateDb::open(&f.data.join("state.db")).unwrap();
    let original = state.native_attempt_custody(g, i).unwrap().unwrap();
    let actors: Vec<oulipoly_provider::custody::ActorSettlementReceipt> =
        serde_json::from_value(original["actors"].clone()).unwrap();
    assert!(
        actors
            .iter()
            .all(oulipoly_provider::custody::ActorSettlementReceipt::effect_incapable)
    );
    assert!(actors.iter().any(|a| a.operation
        == oulipoly_provider::custody::ProviderOperation::Launch
        && !a.spawned));
    assert!(
        state
            .native_recovered_attempt_custody(g, i)
            .unwrap()
            .is_none()
    );
    println!("complete original prelaunch aggregate before native finalization={original}");
    let lifecycle: String = f
        .sidecar_connection()
        .query_row(
            "SELECT lifecycle_state FROM runtime_generation WHERE generation_uuid=?1",
            [&generation],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(lifecycle, "exited");
    writable_sidecar
        .execute_batch("DROP TRIGGER fixture_fail_runtime_exit")
        .unwrap();
    if !late_cancel {
        // Select the direct-projection history by obstructing only generic
        // recovery. Do not author terminal evidence or permit arbitrary reasons.
        writable_sidecar.execute_batch("CREATE TRIGGER fixture_hold_generic_recovery BEFORE UPDATE ON runtime_generation WHEN NEW.terminal_reason='recovered_dead' BEGIN SELECT RAISE(FAIL, 'fixture direct-projection schedule'); END;").unwrap();
    }
    let original_drain = if late_cancel {
        let launcher = f
            .mailbox()
            .continuation_launcher_identity(&a)
            .unwrap()
            .unwrap();
        kill_exact(&launcher);
        let drain = wait_drained_attempt(&f, &a);
        assert!(drain["accepted_cancellation"].is_null());
        assert_eq!(drain["root_wait_status"], libc::SIGKILL);
        println!("original uncancelled physical drain before cancellation API={drain}");
        // Real lifecycle recovery, after actual launcher death and original drain,
        // must commit BEFORE cancellation can interpret the retained aggregate.
        let mut writer = MailboxDb::open(&f.data.join("pid-identity.db")).unwrap();
        writer
            .runtime_lifecycle()
            .reconcile_session_liveness(SESSION)
            .unwrap();
        let before: (String, String) = f.sidecar_connection().query_row(
            "SELECT terminal_reason,spawn_invocation_uuid FROM runtime_generation WHERE generation_uuid=?1 AND lifecycle_state='exited'",
            [&generation], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(before, ("recovered_dead".into(), invocation.clone()));
        println!(
            "committed exact recovered-dead history before cancellation generation={generation} invocation={invocation}"
        );
        Some(drain)
    } else {
        None
    };
    request_linked_cancel(&f, &a);
    let token = fs::read_to_string(f.root.path().join("state-cancel-token")).unwrap();
    let launch = token.split(':').next().unwrap();
    wait(|| {
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
    assert_eq!(
        state.native_attempt_custody(g, i).unwrap().unwrap(),
        original
    );
    assert!(
        state
            .native_recovered_attempt_custody(g, i)
            .unwrap()
            .is_none()
    );
    let reason: String = f
        .sidecar_connection()
        .query_row(
            "SELECT terminal_reason FROM runtime_generation WHERE generation_uuid=?1",
            [&generation],
            |r| r.get(0),
        )
        .unwrap();
    let drain = wait_drained_attempt(&f, &a);
    assert_eq!(drain["attempt_id"], a.attempt_id);
    let exact: (String, i64) = f.sidecar_connection().query_row(
        "SELECT spawn_invocation_uuid,(spawned_os_pid IS NOT NULL OR identity_os_pid IS NOT NULL) FROM runtime_generation WHERE generation_uuid=?1",
        [&generation], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(exact, (invocation.clone(), 0));
    if let Some(original_drain) = original_drain {
        assert_eq!(
            reason, "recovered_dead",
            "committed history must not be relabeled"
        );
        let supplement: String = state.connection().query_row("SELECT result_json FROM provider_launch_transition_replays WHERE logical_launch_id=?1 AND operation_key LIKE '%/native-runtime-cancellation-receipts'", [launch], |r|r.get(0)).unwrap();
        let supplement: serde_json::Value = serde_json::from_str(&supplement).unwrap();
        assert_eq!(
            supplement["original_runtime_row"]["terminal_reason"],
            "recovered_dead"
        );
        assert_eq!(supplement["cancellation_terminal_code"], "startup_failed");
        assert_eq!(drain, original_drain);
        assert_eq!(supplement["original_drain"]["receipt"], drain);
        assert!(drain["accepted_cancellation"].is_null());
        assert_eq!(supplement["logical_cancellation"]["token"], token);
        state
            .request_cancel(uuid::Uuid::parse_str(launch).unwrap())
            .unwrap();
        oulipoly_runtime::executor::settle_retained_native_cancellation(&state, g, i).unwrap();
        println!("preserved runtime observation and separate cancellation evidence={supplement}");
    } else {
        assert_eq!(reason, "startup_failed", "direct original-drain projection");
        assert_eq!(drain["accepted_cancellation"], token);
        writable_sidecar
            .execute_batch("DROP TRIGGER fixture_hold_generic_recovery")
            .unwrap();
        println!(
            "direct original-drain startup failure generation={generation} invocation={invocation} receipt={drain}"
        );
    }
    assert!(!f.root.path().join("resume-prompts.jsonl").exists());
    println!(
        "complete aggregate + actual original cancellation drain settled without invented recovery evidence"
    );
}

#[test]
fn native_birth_recovery_dead_ac_before_announcement() {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    f.gate("ac-created-before-announce.hold");
    let owner = start_pre_attachment(&f);
    let ac = process_identity(reached(&f, "ac-created-before-announce"));
    let a = attempt(&f);
    assert_unresolved(&f, &a);
    kill_exact(&ac);
    let receipt = wait_drained_attempt(&f, &a);
    assert_eq!(
        receipt["classification"],
        "original_adopting_boundary_drained"
    );
    assert_eq!(receipt["custodian"], serde_json::to_value(&ac).unwrap());
    assert_eq!(receipt["custodian_wait_status"], libc::SIGKILL);
    assert_eq!(receipt["owned_children"], "ECHILD");
    assert!(current_identity_matches(&owner.driver_identity));
    assert!(!f.root.path().join("resume-prompts.jsonl").exists());
    println!("dead unannounced AC actual adopter wait/drain={receipt}");
}

fn wait_drained_attempt(f: &Fixture, a: &ContinuationAttempt) -> serde_json::Value {
    wait(|| {
        let receipt: String = f.sidecar_connection().query_row(
            "SELECT drain_receipt FROM completion_continuation_attempt WHERE attempt_id=?1 AND integrated=1 AND phase IN ('drained','never_started')",
            [&a.attempt_id], |r| r.get(0)).ok()?;
        serde_json::from_str(&receipt).ok()
    })
}

#[test]
fn native_birth_recovery_closed_driver_receiver() {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    f.gate("ac-created-before-announce.hold");
    let owner = start_pre_attachment(&f);
    let ac = process_identity(reached(&f, "ac-created-before-announce"));
    let a = attempt(&f);
    kill_exact(&owner.driver_identity);
    wait(|| (f.owner().owner_generation != owner.owner_generation).then_some(()));
    remove_hold(&f, "ac-created-before-announce");
    let receipt = wait_drained_attempt(&f, &a);
    assert_eq!(receipt["gate"], "unreleased_eof");
    assert_eq!(receipt["custodian"], serde_json::to_value(&ac).unwrap());
    assert_eq!(receipt["owned_children"], "ECHILD");
    assert!(!f.root.path().join("resume-prompts.jsonl").exists());
    println!("closed original receiver self-receipt={receipt}");
}

#[test]
fn native_birth_recovery_guardian_loss_then_adopter_resumes() {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    f.gate("adopter-before-ac-fork.hold");
    let owner = start_pre_attachment(&f);
    let adopter = process_identity(reached(&f, "adopter-before-ac-fork"));
    let a = attempt(&f);
    kill_exact(&owner.guardian_identity);
    let next = wait(|| {
        let next = f.owner();
        (next.owner_generation != owner.owner_generation).then_some(next)
    });
    assert_eq!(next.guardian_identity, owner.driver_identity);
    assert!(current_identity_matches(&adopter));
    assert_unresolved(&f, &a);
    remove_hold(&f, "adopter-before-ac-fork");
    let receipt = wait_drained_attempt(&f, &a);
    assert_eq!(receipt["gate"], "unreleased_eof");
    assert_eq!(receipt["owned_children"], "ECHILD");
    assert!(!f.root.path().join("resume-prompts.jsonl").exists());
    println!("succession followed by original adopter resumption={receipt}");
}

#[test]
fn native_birth_recovery_prepublication_failure_retries_original_wait() {
    prepublication_recovery(false);
}

#[test]
fn native_birth_recovery_prepublication_failure_survives_guardian_succession() {
    prepublication_recovery(true);
}

fn prepublication_recovery(succeed_guardian: bool) {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    f.gate("adopter-before-ac-fork.hold");
    let owner = start_pre_attachment(&f);
    let adopter = process_identity(reached(&f, "adopter-before-ac-fork"));
    let a = attempt(&f);
    let result =
        PathBuf::from(&a.result_path).with_file_name("unreleased-announcement-result.json");
    fs::create_dir(&result).unwrap(); // genuine rename failure before publication
    f.gate("driver-replay.hold");
    kill_exact(&adopter);
    assert_eq!(reached(&f, "driver-replay"), owner.driver_identity.pid);
    // The scan is responsive despite one failing receipt. No fixture writes evidence.
    assert!(result.is_dir());
    let temporary = fs::read_dir(result.parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("unreleased-announcement-result.tmp-")
        })
        .expect("actual publication write must have been attempted before scan resumed");
    let unpublished: serde_json::Value =
        serde_json::from_slice(&fs::read(temporary.path()).unwrap()).unwrap();
    assert_eq!(unpublished["observation"], "waitid_wnowait");
    assert_eq!(unpublished["attempt_id"], a.attempt_id);
    println!("actual unpublished producer write before failed rename={unpublished}");
    assert_unresolved(&f, &a);
    if succeed_guardian {
        kill_exact(&owner.guardian_identity);
        remove_hold(&f, "driver-replay");
        let next = wait(|| {
            let next = f.owner();
            (next.owner_generation != owner.owner_generation).then_some(next)
        });
        assert_eq!(next.guardian_identity, owner.driver_identity);
        assert_unresolved(&f, &a);
    } else {
        remove_hold(&f, "driver-replay");
    }
    fs::remove_dir(&result).unwrap();
    let receipt = wait_drained_attempt(&f, &a);
    assert_eq!(receipt["gate"], "unreleased_announcement_eof");
    let reason: serde_json::Value =
        serde_json::from_str(receipt["reason"].as_str().unwrap()).unwrap();
    assert_eq!(reason["adopter"], serde_json::to_value(adopter).unwrap());
    assert_eq!(reason["observation"], "waitid_wnowait");
    assert_eq!(reason["execution_grant"], "not_sent");
    assert!(current_identity_matches(&owner.driver_identity));
    assert!(!f.root.path().join("resume-prompts.jsonl").exists());
    println!("storage recovery retained exact original wait={receipt}");
}

#[test]
fn native_birth_composition_early_guardian_loss_before_eof() {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    f.gate("adopter-before-ac-fork.hold");
    let owner = start_pre_attachment(&f);
    let adopter = process_identity(reached(&f, "adopter-before-ac-fork"));
    let a = attempt(&f);
    stop_exact(&owner.driver_identity);
    kill_exact(&owner.guardian_identity);
    wait_parent_change(&owner.driver_identity, owner.guardian_identity.pid);
    kill_exact(&adopter);
    wait_task_state(&adopter, "Z");
    continue_exact(&owner.driver_identity);
    let next = wait(|| {
        let next = f.owner();
        (next.owner_generation != owner.owner_generation).then_some(next)
    });
    assert_eq!(next.guardian_identity, owner.driver_identity);
    let receipt = wait_drained_attempt(&f, &a);
    assert_eq!(receipt["gate"], "unreleased_announcement_eof");
    let reason: serde_json::Value =
        serde_json::from_str(receipt["reason"].as_str().unwrap()).unwrap();
    assert_eq!(reason["adopter"], serde_json::to_value(&adopter).unwrap());
    assert_eq!(reason["si_status"], libc::SIGKILL);
    assert_eq!(reason["execution_grant"], "not_sent");
    assert!(!f.root.path().join("resume-prompts.jsonl").exists());
    println!("early guardian loss before EOF registration original wait={receipt}");
}

fn stop_exact(identity: &SourceProcessIdentity) {
    assert!(current_identity_matches(identity));
    assert_eq!(unsafe { libc::kill(identity.pid as i32, libc::SIGSTOP) }, 0);
    wait_task_state(identity, "T");
}
// Scheduling observations only. The fixture never promotes these proc states
// into custody; assertions below require the original producers' actual waits.
fn wait_task_state(identity: &SourceProcessIdentity, state: &str) {
    wait(|| {
        let stat = fs::read_to_string(format!("/proc/{}/stat", identity.pid)).ok()?;
        let fields: Vec<_> = stat.rsplit_once(')')?.1.split_whitespace().collect();
        (fields[0] == state && fields[19].parse::<i64>().ok()? == identity.starttime_ticks)
            .then_some(())
    });
}
fn wait_parent_change(driver: &SourceProcessIdentity, guardian: i64) {
    wait(|| {
        let stat = fs::read_to_string(format!("/proc/{}/stat", driver.pid)).ok()?;
        let fields: Vec<_> = stat.rsplit_once(')')?.1.split_whitespace().collect();
        (fields[0] == "T"
            && fields[1].parse::<i64>().ok()? != guardian
            && fields[19].parse::<i64>().ok()? == driver.starttime_ticks)
            .then_some(())
    });
    println!(
        "original driver {} remains stopped after actual guardian reparenting, before its next birth read",
        driver.pid
    );
}

fn continue_exact(identity: &SourceProcessIdentity) {
    assert!(current_identity_matches(identity));
    assert_eq!(unsafe { libc::kill(identity.pid as i32, libc::SIGCONT) }, 0);
}

#[test]
fn native_birth_composition_guardian_loss_with_dead_ac() {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    f.gate("ac-created-before-announce.hold");
    let owner = start_pre_attachment(&f);
    let ac = process_identity(reached(&f, "ac-created-before-announce"));
    let a = attempt(&f);
    stop_exact(&owner.driver_identity);
    kill_exact(&ac);
    wait_task_state(&ac, "Z");
    kill_exact(&owner.guardian_identity);
    wait_parent_change(&owner.driver_identity, owner.guardian_identity.pid);
    continue_exact(&owner.driver_identity);
    let next = wait(|| {
        let next = f.owner();
        (next.owner_generation != owner.owner_generation).then_some(next)
    });
    assert_eq!(next.guardian_identity, owner.driver_identity);
    assert_original_adopting_drain(&f, &a, &ac);
}

#[test]
fn native_birth_composition_attachment_precommit_failure() {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    f.gate("ac-created-before-announce.hold");
    let owner = start_pre_attachment(&f);
    let ac = process_identity(reached(&f, "ac-created-before-announce"));
    let a = attempt(&f);
    let db = rusqlite::Connection::open(f.data.join("pid-identity.db")).unwrap();
    db.execute_batch("CREATE TABLE fixture_attachment_failures(n INTEGER); CREATE TRIGGER fixture_fail_attachment BEFORE UPDATE OF custodian_identity ON completion_continuation_attempt WHEN OLD.custodian_identity IS NULL AND NEW.custodian_identity IS NOT NULL BEGIN INSERT INTO fixture_attachment_failures VALUES(1); SELECT RAISE(FAIL, 'fixture actual attachment precommit failure'); END;").unwrap();
    f.gate("driver-replay.hold");
    kill_exact(&ac);
    assert_eq!(reached(&f, "driver-replay"), owner.driver_identity.pid);
    let failures: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM fixture_attachment_failures",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(failures >= 1, "must execute actual failed attachment");
    let attachment: Option<String> = db
        .query_row(
            "SELECT custodian_identity FROM completion_continuation_attempt WHERE attempt_id=?1",
            [&a.attempt_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(attachment.is_none());
    println!(
        "actual attachment precommit failures={failures}; custodian NULL before storage recovery"
    );
    db.execute_batch("DROP TRIGGER fixture_fail_attachment")
        .unwrap();
    remove_hold(&f, "driver-replay");
    assert_original_adopting_drain(&f, &a, &ac);
}

fn assert_original_adopting_drain(
    f: &Fixture,
    a: &ContinuationAttempt,
    ac: &SourceProcessIdentity,
) {
    let receipt = wait_drained_attempt(f, a);
    assert_eq!(
        receipt["classification"],
        "original_adopting_boundary_drained"
    );
    assert_eq!(receipt["custodian"], serde_json::to_value(ac).unwrap());
    assert_eq!(receipt["custodian_wait_status"], libc::SIGKILL);
    assert_eq!(receipt["owned_children"], "ECHILD");
    assert!(!f.root.path().join("resume-prompts.jsonl").exists());
    println!("original dead AC custody after birth composition={receipt}");
}

#[test]
fn native_birth_composition_live_ac_survives_succession_without_grant() {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    f.gate("ac-created-before-announce.hold");
    let owner = start_pre_attachment(&f);
    let ac = process_identity(reached(&f, "ac-created-before-announce"));
    let a = attempt(&f);
    kill_exact(&owner.guardian_identity);
    let next = wait(|| {
        let next = f.owner();
        (next.owner_generation != owner.owner_generation).then_some(next)
    });
    assert_eq!(next.guardian_identity, owner.driver_identity);
    assert!(current_identity_matches(&ac));
    assert_unresolved(&f, &a);
    remove_hold(&f, "ac-created-before-announce");
    let receipt = wait_drained_attempt(&f, &a);
    assert_eq!(receipt["gate"], "unreleased_eof");
    assert_eq!(receipt["custodian"], serde_json::to_value(&ac).unwrap());
    assert!(!f.root.path().join("resume-prompts.jsonl").exists());
    println!("held live AC was not terminal/granted; original self drain after release={receipt}");
}

#[test]
fn native_birth_composition_spawned_late_cancel_all_exit_projections_fail() {
    spawned_late_cancellation(true, false, false);
}

#[test]
fn native_birth_composition_spawned_both_projection_errors_observed() {
    spawned_late_cancellation(true, true, false);
}

#[test]
fn native_birth_composition_spawned_successful_projection_control() {
    spawned_late_cancellation(false, false, false);
}

#[test]
fn native_birth_composition_zero_exit_preserves_failed_protocol_runtime_outcome() {
    spawned_late_cancellation(true, false, true);
}

fn spawned_late_cancellation(obstruct: bool, observe_errors: bool, zero_exit: bool) {
    spawned_outcome_schedule(obstruct, observe_errors, zero_exit, "retained");
}
#[test]
fn native_outcome_retention_launcher_loss_before_aggregate() {
    spawned_outcome_schedule(true, false, true, "before");
}
#[test]
fn native_outcome_retention_aggregate_write_failure() {
    spawned_outcome_schedule(true, false, true, "retention-failure");
}
#[test]
fn native_outcome_retention_draining_rejected_abnormal() {
    spawned_outcome_schedule(true, false, false, "draining");
}
#[test]
fn native_outcome_retention_operation_result_write_failure() {
    spawned_outcome_schedule(true, false, false, "result-failure");
}
fn spawned_outcome_schedule(obstruct: bool, observe_errors: bool, zero_exit: bool, mode: &str) {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    f.gate("hold-native-launch");
    if zero_exit {
        f.gate("native-launch-exit-zero-without-output");
    }
    let preaggregate = matches!(mode, "before" | "retention-failure");
    let boundary = if preaggregate {
        "native-before-custody-retention"
    } else {
        "native-after-custody-retention"
    };
    f.gate(&format!("{boundary}.hold"));
    if mode == "result-failure" {
        f.gate("native-exit-result-retention.hold");
    }
    start_pre_attachment(&f);
    let launch_process = process_identity(reached(&f, "native-launch"));
    let a = attempt(&f);
    let db = rusqlite::Connection::open(f.data.join("pid-identity.db")).unwrap();
    if obstruct {
        db.execute_batch("CREATE TRIGGER fixture_fail_all_exit BEFORE UPDATE ON runtime_generation WHEN NEW.lifecycle_state='exited' BEGIN SELECT RAISE(FAIL, 'fixture all actual exit projections fail'); END;").unwrap();
    }
    if observe_errors {
        f.gate("native-dispatch-exit-projection-failed.hold");
        f.gate("native-outer-exit-projection-failed.hold");
    }
    if matches!(mode, "draining" | "result-failure") {
        f.gate("release-resume");
        fs::remove_file(f.root.path().join("hold-native-launch")).unwrap();
    } else if zero_exit {
        fs::remove_file(f.root.path().join("hold-native-launch")).unwrap();
    } else {
        kill_exact(&launch_process);
    }
    if observe_errors {
        let dispatch_owner = reached(&f, "native-dispatch-exit-projection-failed");
        println!("actual original dispatch cleanup returned failure; owner={dispatch_owner}");
        remove_hold(&f, "native-dispatch-exit-projection-failed");
        let outer_owner = reached(&f, "native-outer-exit-projection-failed");
        assert_eq!(outer_owner, dispatch_owner);
        println!("actual original outer runtime exit returned failure; owner={outer_owner}");
        remove_hold(&f, "native-outer-exit-projection-failed");
    }
    if mode == "result-failure" {
        reached(&f, "native-exit-result-retention");
        let root = fs::read_dir(f.data.join("state.native-producer-custody"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
            .join("runtime-exit");
        let finish = fs::read_dir(root)
            .unwrap()
            .filter_map(Result::ok)
            .find(|entry| {
                let intent: serde_json::Value =
                    serde_json::from_slice(&fs::read(entry.path().join("intent.json")).unwrap())
                        .unwrap();
                intent["operation"] == "FinishDrain"
            })
            .unwrap()
            .path();
        fs::create_dir(finish.join("result.json")).unwrap();
        f.gate("native-exit-result-retention-failed.hold");
        remove_hold(&f, "native-exit-result-retention");
        reached(&f, "native-exit-result-retention-failed");
        // The write has actually failed, not merely been scheduled to fail.
        // Restore the path before allowing that error to return to dispatch.
        fs::remove_dir(finish.join("result.json")).unwrap();
        remove_hold(&f, "native-exit-result-retention-failed");
        println!(
            "actual operation-result rename failed; original intent/predecessor retained at {}",
            finish.display()
        );
    }
    reached(&f, boundary);
    let (generation, invocation) = f
        .mailbox()
        .continuation_runtime_identity(&a)
        .unwrap()
        .unwrap();
    let g = uuid::Uuid::parse_str(&generation).unwrap();
    let i = uuid::Uuid::parse_str(&invocation).unwrap();
    let state = oulipoly_state::StateDb::open(&f.data.join("state.db")).unwrap();
    if mode == "retention-failure" {
        rusqlite::Connection::open(f.data.join("state.db")).unwrap().execute_batch("CREATE TRIGGER fixture_custody_retention_failure BEFORE INSERT ON provider_launch_transition_replays WHEN NEW.operation_key LIKE '%/native-custody-receipts' BEGIN SELECT RAISE(FAIL, 'actual aggregate retention failure'); END;").unwrap();
        f.gate("native-custody-retention-failed.hold");
        remove_hold(&f, boundary);
        reached(&f, "native-custody-retention-failed");
    }
    let original_stored = state.native_attempt_custody(g, i).unwrap();
    assert_eq!(original_stored.is_none(), preaggregate);
    let source = state.native_attempt_recovery(g, i).unwrap().unwrap();
    let journal = PathBuf::from(source["journal"].as_str().unwrap());
    let original = original_stored.clone().unwrap_or_else(|| {
        let actors: Vec<serde_json::Value> = fs::read_dir(journal.join("actors"))
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|e| fs::read(e.path().join("finished.json")).ok())
            .map(|bytes| serde_json::from_slice(&bytes).unwrap())
            .collect();
        serde_json::json!({"lease":source["lease"],"actors":actors})
    });
    let operations: Vec<serde_json::Value> = if journal.join("runtime-exit").exists() {
        fs::read_dir(journal.join("runtime-exit")).unwrap().filter_map(Result::ok).map(|e| {
            let read = |name| fs::read(e.path().join(name)).ok().map(|bytes|serde_json::from_slice::<serde_json::Value>(&bytes).unwrap());
            serde_json::json!({"intent":read("intent.json"),"before":read("before.json"),"result":read("result.json")})
        }).collect()
    } else {
        vec![]
    };
    println!("actual original runtime operations before launcher loss={operations:?}");
    if matches!(mode, "draining" | "result-failure") && !operations.is_empty() {
        assert!(operations.iter().any(|o| {
            o["intent"]["operation"] == "FinishDrain"
                && o["before"]["lifecycle_state"] == "draining"
                && (mode == "result-failure" && o["result"].is_null()
                    || o["result"]["returned"]
                        .as_str()
                        .is_some_and(|r| r.contains("StorageFailure")))
        }));
        assert!(operations.iter().any(|o| {
            o["intent"]["operation"] == "NonOrderly"
                && o["result"]["rejected"] == true
                && o["result"]["returned"]
                    .as_str()
                    .unwrap()
                    .contains("IllegalPredecessor")
        }));
    }
    let actors: Vec<oulipoly_provider::custody::ActorSettlementReceipt> =
        serde_json::from_value(original["actors"].clone()).unwrap();
    assert!(
        actors
            .iter()
            .all(oulipoly_provider::custody::ActorSettlementReceipt::effect_incapable)
    );
    let launch = actors
        .iter()
        .find(|a| a.operation == oulipoly_provider::custody::ProviderOperation::Launch)
        .unwrap();
    assert!(launch.spawned);
    if zero_exit || matches!(mode, "draining" | "result-failure") {
        assert!(matches!(
            launch.process_status,
            Some(oulipoly_provider::generated::ProcessStatus::Exited { code: 0 })
        ));
    } else {
        assert!(matches!(
            launch.process_status,
            Some(
                oulipoly_provider::generated::ProcessStatus::SignalTerminated {
                    signal: libc::SIGKILL
                }
            )
        ));
    }
    // The baseline has no original projection journal. Its discriminating
    // failure remains the background settlement, not missing new instrumentation.
    if let Some(attempts) = original["runtime_exit_attempts"].as_array() {
        assert!(!attempts.is_empty());
        assert_eq!(
            attempts.last().unwrap()["terminal_code"],
            "abnormal_termination"
        );
        if obstruct && !matches!(mode, "draining" | "result-failure") {
            assert!(attempts.iter().all(|a| {
                a["projection_result"]
                    .as_str()
                    .unwrap()
                    .contains("StorageFailure")
            }));
        }
        if observe_errors {
            assert!(attempts.iter().any(|a| a["site"] == "dispatch_cleanup"));
            assert!(attempts.iter().any(|a| a["site"] == "outer_attempt"));
        }
    }
    let lifecycle: String = db
        .query_row(
            "SELECT lifecycle_state FROM runtime_generation WHERE generation_uuid=?1",
            [&generation],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(lifecycle == "exited", !obstruct);
    if matches!(mode, "draining" | "result-failure") {
        assert_eq!(lifecycle, "draining");
    }
    println!(
        "exit UPDATE obstruction={obstruct} across dispatch/outer finalization; lifecycle={lifecycle}; original spawned aggregate={original}"
    );
    // Keep the returning obstruction across launcher death and genuine drain;
    // no exit projection is allowed to establish a hidden successful prefix.
    kill_exact(
        &f.mailbox()
            .continuation_launcher_identity(&a)
            .unwrap()
            .unwrap(),
    );
    let drain = wait_drained_attempt(&f, &a);
    assert!(drain["accepted_cancellation"].is_null());
    assert_eq!(drain["root_wait_status"], libc::SIGKILL);
    println!("spawned uncancelled original drain BEFORE API acceptance={drain}");
    if obstruct {
        db.execute_batch("DROP TRIGGER fixture_fail_all_exit")
            .unwrap();
    }
    if mode == "retention-failure" {
        rusqlite::Connection::open(f.data.join("state.db"))
            .unwrap()
            .execute_batch("DROP TRIGGER fixture_custody_retention_failure")
            .unwrap();
    }
    request_linked_cancel(&f, &a);
    let token = fs::read_to_string(f.root.path().join("state-cancel-token")).unwrap();
    let logical = token.split(':').next().unwrap();
    wait(|| {
        let status: String = state
            .connection()
            .query_row(
                "SELECT status FROM provider_logical_launches WHERE logical_launch_id=?1",
                [logical],
                |r| r.get(0),
            )
            .ok()?;
        (status == "cancelled").then_some(())
    });
    assert_eq!(state.native_attempt_custody(g, i).unwrap(), original_stored);
    assert_eq!(
        state
            .native_recovered_attempt_custody(g, i)
            .unwrap()
            .is_some(),
        preaggregate
    );
    assert_eq!(wait_drained_attempt(&f, &a), drain);
    let reason: String = db
        .query_row(
            "SELECT terminal_reason FROM runtime_generation WHERE generation_uuid=?1",
            [&generation],
            |r| r.get(0),
        )
        .unwrap();
    let expected_reason = if matches!(mode, "draining" | "result-failure") {
        "orderly_completion"
    } else {
        "abnormal_termination"
    };
    if reason == "recovered_dead" {
        // Generic recovery may win the restored-storage race. Its existing
        // observation must remain unchanged; the cancellation-only supplement
        // must carry the exact original executor outcome separately.
        let raw: String = state.connection().query_row("SELECT result_json FROM provider_launch_transition_replays WHERE logical_launch_id=?1 AND operation_key LIKE '%/native-runtime-cancellation-receipts'", [logical], |r|r.get(0)).unwrap();
        let supplement: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let id = oulipoly_state::mailbox::RuntimeGenerationId::parse(&generation).unwrap();
        let row = f
            .mailbox()
            .runtime_lifecycle_reader()
            .runtime_generation(&id)
            .unwrap()
            .unwrap();
        assert_eq!(
            supplement["original_runtime_row"],
            serde_json::to_value(row).unwrap()
        );
        assert_eq!(supplement["cancellation_terminal_code"], expected_reason);
        if !preaggregate {
            assert_eq!(
                supplement["original_runtime_exit_attempts"],
                original["runtime_exit_attempts"]
            );
        }
        assert!(
            !supplement["original_runtime_exit_operations"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(supplement["original_actor_receipts"], original["actors"]);
        assert_eq!(supplement["original_drain"]["receipt"], drain);
        assert_eq!(supplement["logical_cancellation"]["token"], token);
        println!("unchanged generic recovery plus original attempted runtime outcome={supplement}");
    } else {
        assert_eq!(
            reason, expected_reason,
            "original attempted runtime outcome, not raw process status or late cancellation"
        );
    }
    state
        .request_cancel(uuid::Uuid::parse_str(logical).unwrap())
        .unwrap();
    oulipoly_runtime::executor::settle_retained_native_cancellation(&state, g, i).unwrap();
    println!(
        "late acceptance token={token}; settled logical cancellation with original runtime outcome={reason}"
    );
}

#[test]
fn native_outcome_retention_consumed_pid_lookup_failure() {
    consumed_pid_lookup_failure(false);
}
#[test]
fn native_outcome_retention_retry_consumed_pid_lookup_failure() {
    consumed_pid_lookup_failure(true);
}
fn consumed_pid_lookup_failure(retry: bool) {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    f.gate("birth-pid-consumed.hold");
    f.gate("birth-identity-lookup-failed.hold");
    fs::create_dir(f.root.path().join("birth-identity-read-obstructed")).unwrap();
    if retry {
        f.gate("ac-created-before-announce.hold");
    }
    let owner = start_pre_attachment(&f);
    if retry {
        reached(&f, "ac-created-before-announce");
        kill_exact(&owner.guardian_identity);
        wait(|| (f.owner().guardian_identity == owner.driver_identity).then_some(()));
        remove_hold(&f, "ac-created-before-announce");
    }
    assert_eq!(reached(&f, "birth-pid-consumed"), owner.driver_identity.pid);
    let a = attempt(&f);
    remove_hold(&f, "birth-pid-consumed");
    assert_eq!(
        reached(&f, "birth-identity-lookup-failed"),
        owner.driver_identity.pid
    );
    assert_unresolved(&f, &a);
    // Let the returning read error unwind and retry while the same actual read
    // remains obstructed. Only then restore readability; no second PID is sent.
    if !retry {
        f.gate("driver-replay.hold");
        remove_hold(&f, "birth-identity-lookup-failed");
        reached(&f, "driver-replay");
        assert_unresolved(&f, &a);
        fs::remove_dir(f.root.path().join("birth-identity-read-obstructed")).unwrap();
        remove_hold(&f, "driver-replay");
    } else {
        fs::remove_dir(f.root.path().join("birth-identity-read-obstructed")).unwrap();
        remove_hold(&f, "birth-identity-lookup-failed");
    }
    let receipt = wait_drained_attempt(&f, &a);
    assert_eq!(receipt["gate"], "unreleased_eof");
    assert!(!f.root.path().join("resume-prompts.jsonl").exists());
    println!(
        "consumed original PID survived actual returning read error; healthy peers drained without grant={receipt}"
    );
}

#[test]
fn native_outcome_retention_orphan_pid_preserves_incarnation_not_drain() {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    f.gate("ac-created-before-announce.hold");
    f.gate("birth-identity-lookup-failed.hold");
    fs::create_dir(f.root.path().join("birth-identity-read-obstructed")).unwrap();
    let owner = start_pre_attachment(&f);
    let ac = process_identity(reached(&f, "ac-created-before-announce"));
    let stat = fs::read_to_string(format!("/proc/{}/stat", ac.pid)).unwrap();
    let adopter = process_identity(
        stat.rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap(),
    );
    let a = attempt(&f);
    remove_hold(&f, "ac-created-before-announce");
    reached(&f, "birth-identity-lookup-failed");
    kill_exact(&ac);
    kill_exact(&adopter);
    wait_task_state(&ac, "Z");
    wait_task_state(&adopter, "Z");
    f.gate("driver-replay.hold");
    remove_hold(&f, "birth-identity-lookup-failed");
    reached(&f, "driver-replay"); // generic reap has run after the failed lookup
    assert!(
        current_identity_matches(&ac),
        "generic reap must not consume the announced incarnation"
    );
    wait_task_state(&ac, "Z");
    assert_unresolved(&f, &a);
    fs::remove_dir(f.root.path().join("birth-identity-read-obstructed")).unwrap();
    remove_hold(&f, "driver-replay");
    wait(|| {
        let identity: Option<String> = f.sidecar_connection().query_row("SELECT custodian_identity FROM completion_continuation_attempt WHERE attempt_id=?1", [&a.attempt_id], |r|r.get(0)).ok()?;
        (serde_json::from_str::<SourceProcessIdentity>(&identity?).ok()? == ac).then_some(())
    });
    assert!(current_identity_matches(&owner.driver_identity));
    assert_unresolved(&f, &a);
    assert!(!f.root.path().join("resume-prompts.jsonl").exists());
    println!(
        "retained original announced incarnation after actual orphan adoption={ac:?}; both custodians lost, NO drain/recovery acceptance asserted"
    );
}
