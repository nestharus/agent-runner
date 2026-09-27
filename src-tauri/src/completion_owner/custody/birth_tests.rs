//! Relational owner admission with real direct-child waits. The native suite
//! separately exercises the first /proc read fault in the guardian/driver tree.
use super::*;

fn pending_birth(path: &Path, directory: &Path, label: &str) -> PendingUnreleased {
    let mut db = MailboxDb::open(path).unwrap();
    let owner = db.completion_continuation_owner().unwrap().unwrap();
    let attempt = ContinuationAttempt {
        attempt_id: uuid::Uuid::new_v4().to_string(),
        owner_generation: owner.owner_generation,
        operation: "source_recovery".into(),
        request_sha256: "a".repeat(64),
        source_registration_id: Some(label.into()),
        source_listener_revision: Some(1),
        session_id: None,
        claim_token: None,
        result_path: directory
            .join(label)
            .join("result.json")
            .to_string_lossy()
            .into_owned(),
    };
    db.reserve_continuation_attempt(&attempt).unwrap();
    db.accept_continuation_attempt(&attempt).unwrap();
    durable_write(&Path::new(&attempt.result_path).with_file_name("adopter-fork-gate.json"),
        &serde_json::to_vec(&serde_json::json!({"attempt_id":attempt.attempt_id,"driver":owner.driver_identity,"admitted":false})).unwrap()).unwrap();
    drop(db);
    let (socket, gate) = birth_channel().unwrap();
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0);
    if pid == 0 {
        // Actual preexec termination; no synthetic EOF, receipt or wait status.
        unsafe { libc::_exit(70) }
    }
    drop(gate);
    let mut info = unsafe { std::mem::zeroed() };
    assert_eq!(
        unsafe {
            libc::waitid(
                libc::P_PID,
                pid as u32,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        },
        0
    );
    PendingUnreleased {
        path: path.into(),
        attempt,
        adopter: UnreapedAdopter {
            pid,
            identity: None,
            ownership_lost: false,
        },
        driver: owner.driver_identity,
        socket: Some(socket),
        announced: None,
        custodian: None,
        grant: ExecutionGrant::NotSent,
    }
}

#[test]
fn lost_original_wait_cannot_resolve_or_abort_later_pending_birth() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pid-identity.db");
    let mut db = MailboxDb::open_completion_continuation_domain(&path).unwrap();
    crate::completion_owner::test_support::install_owner(&mut db);
    drop(db);
    let lost = pending_birth(&path, directory.path(), "lost");
    let lost_pid = lost.adopter.pid;
    let mut status = 0;
    // Deliberately violate sole-wait custody for this negative control. The
    // implementation must refuse identity resolution forever, not infer no fork.
    assert_eq!(unsafe { libc::waitpid(lost_pid, &mut status, 0) }, lost_pid);
    let ready = pending_birth(&path, directory.path(), "ready");
    let ready_pid = ready.adopter.pid;
    let expected_adopter = super::super::linux::identity(i64::from(ready_pid)).unwrap();
    let attempt = ready.attempt.clone();
    PENDING_UNRELEASED.with_borrow_mut(|pending| {
        assert!(pending.is_empty());
        pending.extend([lost, ready]);
    });
    retry_unreleased();
    PENDING_UNRELEASED.with_borrow_mut(|pending| {
        assert_eq!(
            pending.len(),
            1,
            "failed first original must not skip the second"
        );
        let lost = &mut pending[0];
        assert!(lost.adopter.ownership_lost);
        assert!(lost.adopter.identity.is_none());
        assert!(
            lost.adopter
                .resolve()
                .unwrap_err()
                .contains("ownership lost")
        );
        assert!(lost.socket.is_some());
        assert!(
            !Path::new(&lost.attempt.result_path)
                .with_file_name("unreleased-announcement-result.json")
                .exists()
        );
        assert!(
            !Path::new(&lost.attempt.result_path)
                .with_file_name("never-forked.json")
                .exists()
        );
    });
    let sql = rusqlite::Connection::open(&path).unwrap();
    let (phase, integrated, receipt): (String, bool, String) = sql.query_row(
        "SELECT phase,integrated,drain_receipt FROM completion_continuation_attempt WHERE attempt_id=?1",
        [&attempt.attempt_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    assert_eq!(phase, "never_started");
    assert!(integrated);
    let receipt: serde_json::Value = serde_json::from_str(&receipt).unwrap();
    assert_eq!(receipt["gate"], "unreleased_announcement_eof");
    let reason: serde_json::Value =
        serde_json::from_str(receipt["reason"].as_str().unwrap()).unwrap();
    assert_eq!(
        reason["adopter"],
        serde_json::to_value(expected_adopter).unwrap()
    );
    assert_eq!(reason["execution_grant"], "not_sent");
    assert_eq!(reason["observation"], "waitid_wnowait");
    // Test housekeeping consumes only the now-integrated second child's wait.
    assert_eq!(
        unsafe { libc::waitpid(ready_pid, &mut status, 0) },
        ready_pid
    );
    PENDING_UNRELEASED.with_borrow_mut(Vec::clear); // lost case remains unsettled, not a receipt
    eprintln!("actual ECHILD is sticky; later original integrated via real EOF/WNOWAIT: {receipt}");
}

#[test]
fn rejects_automatic_reaping_without_mutating_signal_policy() {
    let name = std::thread::current().name().unwrap().to_owned();
    if std::env::var("AGE360_BIRTH_SIGNAL_CHILD").as_deref() != Ok("yes") {
        // Signal disposition is process-global: isolate this exact control from
        // all parallel libtest neighbors. No workload or fabricated wait occurs.
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--nocapture"])
            .env("AGE360_BIRTH_SIGNAL_CHILD", "yes")
            .output()
            .unwrap();
        eprintln!("{}", String::from_utf8_lossy(&output.stdout));
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        assert!(output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line == format!("test {name} ... ok"))
        );
        return;
    }
    let mut original: libc::sigaction = unsafe { std::mem::zeroed() };
    assert_eq!(
        unsafe { libc::sigaction(libc::SIGCHLD, std::ptr::null(), &mut original) },
        0
    );
    for (handler, flags) in [(libc::SIG_IGN, 0), (libc::SIG_DFL, libc::SA_NOCLDWAIT)] {
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = handler;
        action.sa_flags = flags;
        assert_eq!(
            unsafe { libc::sigaction(libc::SIGCHLD, &action, std::ptr::null_mut()) },
            0
        );
        assert!(
            require_retained_child_waits()
                .unwrap_err()
                .contains("automatic reaping")
        );
        let mut after: libc::sigaction = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { libc::sigaction(libc::SIGCHLD, std::ptr::null(), &mut after) },
            0
        );
        assert_eq!(after.sa_sigaction, handler);
        assert_eq!(after.sa_flags & libc::SA_NOCLDWAIT, flags);
    }
    assert_eq!(
        unsafe { libc::sigaction(libc::SIGCHLD, &original, std::ptr::null_mut()) },
        0
    );
    require_retained_child_waits().unwrap();
    eprintln!("actual SIG_IGN and SA_NOCLDWAIT refused unchanged; original disposition restored");
}

#[test]
fn independent_terminal_wait_progresses_beside_unknown_original() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pid-identity.db");
    let mut db = MailboxDb::open_completion_continuation_domain(&path).unwrap();
    crate::completion_owner::test_support::install_owner(&mut db);
    drop(db);
    let unknown = pending_birth(&path, directory.path(), "unknown");
    let unknown_pid = unknown.adopter.pid;
    let unknown_attempt = unknown.attempt.clone();
    let unknown_identity = super::super::linux::identity(i64::from(unknown_pid)).unwrap();
    PENDING_UNRELEASED.with_borrow_mut(|pending| {
        assert!(pending.is_empty());
        pending.push(unknown);
    });
    // Real terminal source-adopter analogue with no unknown original duty.
    // Retry performs the actual EOF/WNOWAIT receipt and original-only SQL
    // integration before enrolling this direct child for independent reaping.
    let mut independent = pending_birth(&path, directory.path(), "independent");
    let independent_pid = independent.adopter.pid;
    let independent_attempt = independent.attempt.clone();
    let independent_identity = super::super::linux::identity(i64::from(independent_pid)).unwrap();
    assert!(!retain_unfinished_birth(&mut independent));
    assert_exact_original_receipt(&path, &independent_attempt, &independent_identity);
    let db = MailboxDb::open(&path).unwrap();
    assert!(
        db.pending_continuation_attempts()
            .unwrap()
            .contains(&unknown_attempt)
    );
    assert!(
        !db.pending_continuation_attempts()
            .unwrap()
            .contains(&independent_attempt)
    );
    let mut status = 0;
    assert_eq!(reap_unprotected(&mut status, &[]), independent_pid);
    assert!(libc::WIFEXITED(status));
    assert_eq!(libc::WEXITSTATUS(status), 70);
    // The unknown child is genuinely terminal too, but no positive independent
    // attribution authorizes consuming its wait. Zero is NOT ECHILD or drain.
    assert_eq!(reap_unprotected(&mut status, &[]), 0);
    assert_eq!(
        super::super::linux::identity(i64::from(unknown_pid)).unwrap(),
        unknown_identity
    );
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    assert_eq!(
        unsafe {
            libc::waitid(
                libc::P_PID,
                unknown_pid as u32,
                &mut info,
                libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
            )
        },
        0
    );
    assert_eq!(unsafe { info.si_pid() }, unknown_pid);
    assert!(
        db.pending_continuation_attempts()
            .unwrap()
            .contains(&unknown_attempt)
    );
    // Restore normal original processing, then require its own receipt before
    // its exact wait can be consumed. Test teardown supplies no settlement.
    retry_unreleased();
    assert!(db.pending_continuation_attempts().unwrap().is_empty());
    assert_exact_original_receipt(&path, &unknown_attempt, &unknown_identity);
    assert_eq!(reap_unprotected(&mut status, &[]), unknown_pid);
    assert!(PENDING_UNRELEASED.with_borrow(|pending| pending.is_empty()));
    eprintln!(
        "independent exact wait={independent_pid}; protected unknown={unknown_pid}; both original SQL receipts asserted separately from waits"
    );
}

#[test]
fn externally_owned_original_worker_is_never_consumed_by_generic_reaping() {
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0);
    if pid == 0 {
        unsafe { libc::_exit(23) }
    }
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    assert_eq!(
        unsafe {
            libc::waitid(
                libc::P_PID,
                pid as u32,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        },
        0
    );
    let mut status = 0;
    assert_eq!(reap_unprotected(&mut status, &[i64::from(pid)]), 0);
    assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
    assert!(libc::WIFEXITED(status));
    assert_eq!(libc::WEXITSTATUS(status), 23);
}

fn assert_exact_original_receipt(
    path: &Path,
    attempt: &ContinuationAttempt,
    adopter: &oulipoly_state::completion_continuation::SourceProcessIdentity,
) {
    let sql = rusqlite::Connection::open(path).unwrap();
    let (phase, integrated, receipt): (String, bool, String) = sql.query_row(
        "SELECT phase,integrated,drain_receipt FROM completion_continuation_attempt WHERE attempt_id=?1",
        [&attempt.attempt_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).unwrap();
    assert_eq!(phase, "never_started");
    assert!(integrated);
    let receipt: serde_json::Value = serde_json::from_str(&receipt).unwrap();
    assert_eq!(receipt["attempt_id"], attempt.attempt_id);
    assert_eq!(receipt["gate"], "unreleased_announcement_eof");
    let reason: serde_json::Value =
        serde_json::from_str(receipt["reason"].as_str().unwrap()).unwrap();
    assert_eq!(reason["adopter"], serde_json::to_value(adopter).unwrap());
    assert_eq!(reason["attempt_id"], attempt.attempt_id);
    assert_eq!(
        reason["driver"],
        serde_json::to_value(super::super::linux::identity(i64::from(std::process::id())).unwrap())
            .unwrap()
    );
    assert_eq!(reason["execution_grant"], "not_sent");
    assert_eq!(reason["observation"], "waitid_wnowait");
    assert_eq!(reason["si_code"], libc::CLD_EXITED);
    assert_eq!(reason["si_status"], 70);
    let retained = sql.query_row(
        "SELECT attempt_id,owner_generation,operation,request_sha256,source_registration_id,source_listener_revision,session_id,claim_token,result_path FROM completion_continuation_attempt WHERE attempt_id=?1",
        [&attempt.attempt_id], |row| Ok(ContinuationAttempt {
            attempt_id: row.get(0)?, owner_generation: row.get(1)?, operation: row.get(2)?,
            request_sha256: row.get(3)?, source_registration_id: row.get(4)?, source_listener_revision: row.get(5)?,
            session_id: row.get(6)?, claim_token: row.get(7)?, result_path: row.get(8)?,
        }),
    ).unwrap();
    assert_eq!(&retained, attempt);
    eprintln!(
        "independent-wait control immutable request={attempt:?}; original incarnation={adopter:?}; actual receipt={receipt}"
    );
}
