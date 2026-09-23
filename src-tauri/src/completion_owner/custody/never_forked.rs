//! Original syscall testimony retained independently of sidecar availability.
//! This is no-child evidence, never a post-fork ECHILD certificate or replay grant.
use oulipoly_state::completion_continuation::{
    MAX_REGISTRATION_BYTES, SourceProcessIdentity, read_source_file,
};
use oulipoly_state::mailbox::{ContinuationAttempt, MailboxDb};
use std::path::{Path, PathBuf};

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct NeverForked {
    path: PathBuf,
    attempt: ContinuationAttempt,
    driver: SourceProcessIdentity,
    reason: String,
}

thread_local! {
    static PENDING: std::cell::RefCell<Vec<NeverForked>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Only the two conclusive pre-fork syscall error branches call this producer.
/// Memory custody is installed before either filesystem or SQLite persistence.
#[cfg(test)]
pub(super) fn retain(
    path: &Path,
    attempt: &ContinuationAttempt,
    driver: &SourceProcessIdentity,
    reason: String,
) -> String {
    let evidence = NeverForked {
        path: path.into(),
        attempt: attempt.clone(),
        driver: driver.clone(),
        reason: reason.clone(),
    };
    PENDING.with_borrow_mut(|pending| pending.push(evidence));
    retry_pending();
    reason
}

/// One attempt per existing outer custody pass, with the same original owner.
/// Fork copies cannot testify for the original process.
#[cfg(test)]
pub(super) fn retry_pending() {
    let pid = super::super::linux::current_identity().map(|identity| identity.pid);
    PENDING.with_borrow_mut(|pending| {
        pending.retain(|e| match &pid {
            Ok(pid) if e.driver.pid != *pid => false,
            Ok(_) => e.integrate().is_err(),
            Err(_) => true,
        })
    });
}

/// A process may not voluntarily exit while it is the only authorized original
/// witness. This is continuing evidence integration, never another launch or a
/// replacement's inference from its empty child set.
#[cfg(test)]
pub(super) fn has_pending() -> bool {
    let pid = super::super::linux::current_identity().map(|identity| identity.pid);
    PENDING.with_borrow(|pending| {
        pending
            .iter()
            .any(|e| pid.as_ref().map_or(true, |pid| e.driver.pid == *pid))
    })
}

impl NeverForked {
    fn integrate(&self) -> Result<(), String> {
        let driver = super::super::linux::current_identity()?;
        if self.driver != driver || self.reason.is_empty() {
            return Err("never-forked testimony original driver conflict".into());
        }
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        let evidence_path =
            Path::new(&self.attempt.result_path).with_file_name("never-forked.json");
        super::durable_write(&evidence_path, &bytes)?;
        MailboxDb::open(&self.path)?.record_continuation_never_forked(&self.attempt, &self.reason)
    }
}

/// Retained testimony can be integrated by its original living driver, including
/// after promotion. A replacement process has no new discharge right.
pub(super) fn replay(path: &Path, attempt: &ContinuationAttempt) -> Result<bool, String> {
    let directory = Path::new(&attempt.result_path)
        .parent()
        .ok_or("result parent absent")?;
    let file = directory.join("never-forked.json");
    if !file.exists() {
        return Ok(false);
    }
    let bytes = read_source_file(directory, "never-forked.json", MAX_REGISTRATION_BYTES)?;
    let evidence: NeverForked = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if evidence.path != path || evidence.attempt != *attempt {
        return Err("never-forked immutable request conflict".into());
    }
    evidence.integrate()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn storage_real_socket_failure_keeps_original_across_outer_storage_error() {
        let name = std::thread::current().name().unwrap().to_string();
        if std::env::var("AGE360_STORAGE_SYSCALL_CHILD").as_deref() != Ok("yes") {
            // RLIMIT is process-wide: never interfere with parallel libtest
            // neighbors. This child selects this one ID, not a serial suite.
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", &name, "--nocapture", "--test-threads=1"])
                .env("AGE360_STORAGE_SYSCALL_CHILD", "yes")
                .output()
                .unwrap();
            eprintln!("{}", String::from_utf8_lossy(&output.stdout));
            eprintln!("{}", String::from_utf8_lossy(&output.stderr));
            assert!(output.status.success());
            return;
        }
        for operation in ["activation", "source_recovery"] {
            real_socket_outer_error(operation);
        }
    }

    fn real_socket_outer_error(operation: &str) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let mut db = MailboxDb::open_completion_continuation_domain(&path).unwrap();
        crate::completion_owner::test_support::install_owner(&mut db);
        drop(db);
        let mut db = MailboxDb::open(&path).unwrap();
        let mut owner = db.completion_continuation_owner().unwrap().unwrap();
        // The fixture supplies owner admission, not a native guardian endpoint.
        // Match this process's real parent so run reaches its actual storage cut.
        owner.guardian_identity.pid = i64::from(unsafe { libc::getppid() });
        let sql = Connection::open(&path).unwrap();
        sql.execute_batch("INSERT INTO session_wake_claim(session_id,claim_token,claimed_at,reason,auto_wake_count) VALUES('session','token','fixture','fixture',1);
            CREATE TRIGGER fail_receipt BEFORE UPDATE ON completion_continuation_attempt WHEN NEW.phase='never_started' BEGIN SELECT RAISE(ABORT, 'selected receipt fault'); END;").unwrap();
        let attempt = ContinuationAttempt {
            attempt_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: owner.owner_generation.clone(),
            operation: operation.into(),
            request_sha256: "a".repeat(64),
            source_registration_id: (operation == "source_recovery")
                .then(|| "fixture-registration".into()),
            source_listener_revision: (operation == "source_recovery").then_some(1),
            session_id: (operation == "activation").then(|| "session".into()),
            claim_token: (operation == "activation").then(|| "token".into()),
            result_path: dir
                .path()
                .join("evidence/result.json")
                .to_string_lossy()
                .into_owned(),
        };
        db.reserve_continuation_attempt(&attempt).unwrap();
        let live = oulipoly_state::pid_identity::read_current_process_identity().unwrap();
        let original_driver = SourceProcessIdentity {
            pid: live.os_pid,
            boot_id: live.os_boot_id,
            starttime_ticks: live.os_pid_starttime_ticks,
        };
        assert_eq!(owner.driver_identity, original_driver);
        let expected_reason = format!(
            "custodian gate creation failed: {}",
            std::io::Error::from_raw_os_error(libc::EMFILE)
        );
        let mut original = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        assert_eq!(
            unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut original) },
            0
        );
        let maximum = original.rlim_max;
        super::super::AFTER_ACCEPT.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move || {
                let limit = libc::rlimit {
                    rlim_cur: 0,
                    rlim_max: maximum,
                };
                assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) }, 0);
            }))
        });
        let result = if operation == "activation" {
            super::super::spawn_activation(&path, &attempt, || {
                Ok(std::process::Command::new("/never-launched"))
            })
        } else {
            let fixture: serde_json::Value = serde_json::from_str(include_str!(
                "../../../../crates/oulipoly-state/tests/fixtures/age360-missing-output-wire.json"
            ))
            .unwrap();
            let registration = serde_json::to_vec(&fixture["registration"]).unwrap();
            let binding = oulipoly_state::completion_continuation::AdmittedSourceBinding::new(
                "fixture",
                &registration,
            )
            .unwrap();
            super::super::spawn_source(&path, &attempt, &binding)
        };
        assert_eq!(
            unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &original) },
            0
        );
        let error = result.unwrap_err();
        assert!(
            error.contains("custodian gate creation failed") && error.contains("24"),
            "{error}"
        );
        assert_eq!(error, expected_reason);
        assert!(has_pending());
        assert!(!dir.path().join("evidence/never-forked.json").exists());
        retry_pending(); // real journal now succeeds; SQL receipt trigger still fails
        assert!(has_pending());
        assert!(
            replay(&path, &attempt)
                .unwrap_err()
                .contains("selected receipt fault")
        );
        let authority = path.with_extension("db.authority.lock");
        let permissions = std::fs::metadata(&authority).unwrap().permissions();
        std::fs::set_permissions(&authority, std::fs::Permissions::from_mode(0o000)).unwrap();
        let (entered, await_entered) = std::sync::mpsc::channel();
        super::super::BEFORE_TESTIMONY_FINISH.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move || {
                assert!(has_pending());
                entered.send(()).unwrap();
            }))
        });
        let repair = std::thread::spawn(move || {
            await_entered.recv().unwrap();
            let phase: String = sql
                .query_row(
                    "SELECT phase FROM completion_continuation_attempt",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(phase, "accepted");
            let claims: i64 = sql
                .query_row("SELECT COUNT(*) FROM session_wake_claim", [], |r| r.get(0))
                .unwrap();
            assert_eq!(claims, 1);
            std::fs::set_permissions(&authority, permissions).unwrap();
            sql.execute_batch("DROP TRIGGER fail_receipt").unwrap();
        });
        let (driver_channel, _root_channel) = std::os::unix::net::UnixStream::pair().unwrap();
        let outer =
            crate::completion_owner::driver::run(&path, &owner, driver_channel).unwrap_err();
        repair.join().unwrap();
        assert!(outer.contains("Permission denied"), "{outer}");
        assert!(!has_pending());
        assert!(db.pending_continuation_attempts().unwrap().is_empty());
        assert_eq!(
            db.wake_session_reader()
                .wake_claim("session")
                .unwrap()
                .is_none(),
            operation == "activation"
        );
        let sql = Connection::open(&path).unwrap();
        let (phase, integrated, receipt): (String, bool, String) = sql.query_row(
            "SELECT phase,integrated,drain_receipt FROM completion_continuation_attempt WHERE attempt_id=?1",
            [&attempt.attempt_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).unwrap();
        assert_eq!(phase, "never_started");
        assert!(integrated);
        let receipt: serde_json::Value = serde_json::from_str(&receipt).unwrap();
        assert_eq!(
            receipt,
            serde_json::json!({
                "attempt_id": attempt.attempt_id,
                "driver": original_driver,
                "gate": "no_custodian_created",
                "reason": expected_reason,
            })
        );
        let retained = sql.query_row(
            "SELECT attempt_id,owner_generation,operation,request_sha256,source_registration_id,source_listener_revision,session_id,claim_token,result_path FROM completion_continuation_attempt WHERE attempt_id=?1",
            [&attempt.attempt_id], |row| Ok(ContinuationAttempt {
                attempt_id: row.get(0)?, owner_generation: row.get(1)?, operation: row.get(2)?,
                request_sha256: row.get(3)?, source_registration_id: row.get(4)?,
                source_listener_revision: row.get(5)?, session_id: row.get(6)?,
                claim_token: row.get(7)?, result_path: row.get(8)?,
            }),
        ).unwrap();
        assert_eq!(retained, attempt);
        let journal: NeverForked = serde_json::from_slice(
            &std::fs::read(dir.path().join("evidence/never-forked.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(journal.path, path);
        assert_eq!(journal.attempt, attempt);
        assert_eq!(journal.driver, original_driver);
        assert_eq!(journal.reason, expected_reason);
        eprintln!(
            "{operation}: exact EMFILE receipt, original incarnation, immutable request and journal asserted: {receipt}"
        );
        eprintln!(
            "{operation}: actual socketpair EMFILE; journal absent then SQL failure; actual driver storage error preserved; original {} integrated before return",
            std::process::id()
        );
    }

    #[test]
    fn storage_never_forked_original_testimony_survives_two_persistence_faults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let mut db = MailboxDb::open_completion_continuation_domain(&path).unwrap();
        crate::completion_owner::test_support::install_owner(&mut db);
        // Bootstrap retains an exclusive namespace election. The operational
        // driver uses ordinary shared opens after bootstrap has returned.
        drop(db);
        let mut db = MailboxDb::open(&path).unwrap();
        let owner = db.completion_continuation_owner().unwrap().unwrap();
        let sql = Connection::open(&path).unwrap();
        sql.execute_batch("INSERT INTO session_wake_claim(session_id,claim_token,claimed_at,reason,auto_wake_count) VALUES('session','token','fixture','fixture',1)").unwrap();
        let attempt = ContinuationAttempt {
            attempt_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: owner.owner_generation.clone(),
            operation: "activation".into(),
            request_sha256: "a".repeat(64),
            source_registration_id: None,
            source_listener_revision: None,
            session_id: Some("session".into()),
            claim_token: Some("token".into()),
            result_path: dir
                .path()
                .join("evidence/result.json")
                .to_string_lossy()
                .into_owned(),
        };
        db.reserve_continuation_attempt(&attempt).unwrap();
        db.accept_continuation_attempt(&attempt).unwrap();
        sql.execute_batch("CREATE TRIGGER fail_receipt BEFORE UPDATE ON completion_continuation_attempt WHEN NEW.phase='never_started' BEGIN SELECT RAISE(ABORT, 'selected receipt fault'); END;").unwrap();
        // This is a receipt-retention mechanism control, not a fork-failure
        // experiment: the fixture supplies original no-child testimony.
        std::fs::write(dir.path().join("evidence"), b"obstruct directory").unwrap();
        retain(
            &path,
            &attempt,
            &owner.driver_identity,
            "fixture conclusive pre-fork syscall failure".into(),
        );
        assert_eq!(PENDING.with_borrow(|p| p.len()), 1);
        assert!(
            db.wake_session_reader()
                .wake_claim("session")
                .unwrap()
                .is_some()
        );
        std::fs::remove_file(dir.path().join("evidence")).unwrap();
        retry_pending();
        assert_eq!(PENDING.with_borrow(|p| p.len()), 1);
        let retained = std::fs::read(dir.path().join("evidence/never-forked.json")).unwrap();
        assert!(db.revoke_unaccepted_continuation_attempt(&attempt).is_err());
        let error = replay(&path, &attempt).unwrap_err();
        assert!(
            error.contains("selected receipt fault"),
            "actual receipt error: {error}"
        );
        sql.execute_batch("DROP TRIGGER fail_receipt").unwrap();
        retry_pending();
        assert_eq!(PENDING.with_borrow(|p| p.len()), 0);
        assert!(db.pending_continuation_attempts().unwrap().is_empty());
        assert!(
            db.wake_session_reader()
                .wake_claim("session")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            std::fs::read(dir.path().join("evidence/never-forked.json")).unwrap(),
            retained
        );
        assert!(replay(&path, &attempt).unwrap());
        let mut changed = attempt.clone();
        changed.request_sha256 = "b".repeat(64);
        assert!(
            replay(&path, &changed)
                .unwrap_err()
                .contains("immutable request")
        );
        let mut evidence: NeverForked = serde_json::from_slice(&retained).unwrap();
        evidence.driver.starttime_ticks += 1;
        assert!(
            evidence
                .integrate()
                .unwrap_err()
                .contains("original driver conflict")
        );
    }
}
