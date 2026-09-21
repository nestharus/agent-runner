//! Deterministic sidecar schedules, not synthetic claims of physical drain.
use oulipoly_state::mailbox::{
    MailboxDb, ManualWakeCoordination, SessionAdmissionAttempt, WakeClaimAcquireResult,
    WakeClaimRequest,
};
use oulipoly_state::pid_identity::{ProcessIdentity, read_live_process_identity};
use rusqlite::{Connection, params};

struct Fixture {
    _dir: tempfile::TempDir,
    db: MailboxDb,
    state: oulipoly_state::StateDb,
    sql: Connection,
    live: ProcessIdentity,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let state = oulipoly_state::StateDb::open(&dir.path().join("state.db")).unwrap();
        let db = MailboxDb::open(&path).unwrap();
        let sql = Connection::open(&path).unwrap();
        let live = read_live_process_identity(i64::from(std::process::id()))
            .unwrap()
            .unwrap();
        Self {
            _dir: dir,
            db,
            state,
            sql,
            live,
        }
    }
    fn enqueue(&mut self, id: &str, session: &str, exact: bool) {
        let mut identity = self.live.clone();
        // Another exact live launcher is not available in this state-only fixture.
        // Matching/nonmatching persisted tuples are the decision inputs here.
        if !exact {
            identity.os_pid_starttime_ticks += 1;
        }
        self.db
            .session_admissions()
            .enqueue(id, id, Some(session), &identity, 1)
            .unwrap();
    }
    fn native(&self, phase: &str, launcher: bool) {
        self.sql.execute_batch("UPDATE completion_supervisor_authority
                SET phase='active',guardian_identity='{}'
                WHERE authority_id='00000000-0000-4000-8000-000000000021';
            INSERT INTO completion_continuation_owner(
                generation,domain_id,phase,guardian_identity,driver_identity,endpoint)
                SELECT 'owner',domain_id,'running','{}','{}','fixture'
                FROM completion_continuation_domain;
            INSERT INTO session_wake_claim(session_id,claim_token,reason,auto_wake_count,claimed_at,min_pending_seq_at_claim,max_pending_seq_at_claim)
                VALUES('session','token','fixture',1,'2026-01-01',1,1);
            INSERT INTO completion_continuation_attempt(attempt_id,domain_id,owner_generation,operation,request_sha256,session_id,claim_token,phase,result_path)
                SELECT 'native',domain_id,'owner','activation','hash','session','token','reserved','fixture' FROM completion_continuation_domain;").unwrap();
        self.sql.execute("UPDATE completion_continuation_owner SET driver_identity=?1",
            [serde_json::json!({"pid":self.live.os_pid,"boot_id":self.live.os_boot_id,"starttime_ticks":self.live.os_pid_starttime_ticks}).to_string()]).unwrap();
        let identity = launcher.then(|| serde_json::json!({"pid":self.live.os_pid,"boot_id":self.live.os_boot_id,"starttime_ticks":self.live.os_pid_starttime_ticks}).to_string());
        self.sql.execute("UPDATE completion_continuation_attempt SET phase=?1,launcher_identity=?2,revision=revision+1 WHERE attempt_id='native'",params![phase,identity]).unwrap();
        if launcher {
            self.sql.execute("UPDATE session_wake_claim SET wake_pid=?1,wake_os_boot_id=?2,wake_os_pid_starttime_ticks=?3",params![self.live.os_pid,self.live.os_boot_id,self.live.os_pid_starttime_ticks]).unwrap();
        }
    }
    fn next(&mut self) -> SessionAdmissionAttempt {
        self.db
            .session_admissions()
            .try_admit_next("admit", 2, 0)
            .unwrap()
    }
}

#[test]
fn native_publication_and_drain_debt_do_not_take_global_capacity() {
    for phase in [
        "reserved",
        "accepted",
        "starting",
        "running",
        "unknown_custody",
    ] {
        let mut f = Fixture::new();
        f.native(phase, false);
        f.enqueue("manual", "session", true);
        f.enqueue("other", "other-session", true);
        let SessionAdmissionAttempt::Admitted(row) = f.next() else {
            panic!("other session blocked by {phase}")
        };
        assert_eq!(row.registration_identity, "other");
        assert_eq!(
            f.db.session_admissions()
                .row("manual")
                .unwrap()
                .unwrap()
                .state,
            "queued"
        );
        assert_eq!(
            f.state.coordinate_manual_resume("session").unwrap(),
            if phase == "unknown_custody" {
                ManualWakeCoordination::UnknownCustody
            } else {
                ManualWakeCoordination::NativeBusy
            }
        );
        let phase_after: String = f
            .sql
            .query_row(
                "SELECT phase FROM completion_continuation_attempt",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            phase_after, phase,
            "manual observation must not retire native custody"
        );
    }
}

#[test]
fn authorized_automatic_launcher_bypasses_older_manual_without_erasing_intent() {
    let mut f = Fixture::new();
    f.native("starting", true);
    f.enqueue("manual", "session", true);
    // Manual is another launcher. Preserve a live PID, but a mismatched exact
    // tuple, and put a separate live row first so dead-head cleanup is irrelevant.
    f.sql.execute("UPDATE session_admission_queue SET launcher_os_pid_starttime_ticks=launcher_os_pid_starttime_ticks+1 WHERE registration_identity='manual'",[]).unwrap();
    f.enqueue("auto", "session", true);
    // next() first performs legitimate stale head cancellation. Use the selector
    // after a live unrelated head is admitted/materialized instead below? A real
    // second process supplies a live manual identity for the actual schedule.
    let mut child = std::process::Command::new("/bin/sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let manual = read_live_process_identity(i64::from(child.id()))
        .unwrap()
        .unwrap();
    f.sql.execute("UPDATE session_admission_queue SET launcher_os_pid=?1,launcher_os_boot_id=?2,launcher_os_pid_starttime_ticks=?3 WHERE registration_identity='manual'",params![manual.os_pid,manual.os_boot_id,manual.os_pid_starttime_ticks]).unwrap();
    let result = f.next();
    child.kill().unwrap();
    child.wait().unwrap();
    let SessionAdmissionAttempt::Admitted(row) = result else {
        panic!("authorized launcher blocked")
    };
    assert_eq!(row.registration_identity, "auto");
    assert_eq!(
        f.db.session_admissions()
            .row("manual")
            .unwrap()
            .unwrap()
            .state,
        "queued"
    );
}

#[test]
fn manual_intent_prevents_fresh_automatic_overtaking_and_storage_errors_propagate() {
    let mut f = Fixture::new();
    f.enqueue("manual", "session", true);
    let request = WakeClaimRequest {
        session_id: "session",
        claim_token: "fresh",
        reason: "fixture",
        auto_wake_count: 1,
        wake_invocation_uuid: None,
        stale_after_seconds: 600,
    };
    assert!(matches!(
        f.db.wake_sessions()
            .try_acquire_startable_wake_claim(request, None)
            .unwrap(),
        WakeClaimAcquireResult::Busy
    ));
    assert_eq!(
        f.state.coordinate_manual_resume("session").unwrap(),
        ManualWakeCoordination::Absent
    );
    f.sql
        .execute_batch("DROP TABLE session_wake_claim")
        .unwrap();
    assert!(f.state.coordinate_manual_resume("session").is_err());
}

#[test]
fn unknown_native_custody_waits_only_for_a_live_original_custodian() {
    let f = Fixture::new();
    f.native("unknown_custody", false);
    assert_eq!(
        f.state.coordinate_manual_resume("session").unwrap(),
        ManualWakeCoordination::UnknownCustody
    );
    let identity = serde_json::json!({"pid":f.live.os_pid,"boot_id":f.live.os_boot_id,"starttime_ticks":f.live.os_pid_starttime_ticks}).to_string();
    f.sql
        .execute(
            "UPDATE completion_continuation_attempt SET custodian_identity=?1,revision=revision+1",
            [identity],
        )
        .unwrap();
    assert_eq!(
        f.state.coordinate_manual_resume("session").unwrap(),
        ManualWakeCoordination::NativeBusy
    );
    assert!(
        f.db.wake_session_reader()
            .wake_claim("session")
            .unwrap()
            .is_some()
    );
}

#[test]
fn manual_legacy_identity_unknown_live_and_exact_releasable_are_distinct() {
    let f = Fixture::new();
    f.sql.execute_batch("INSERT INTO session_wake_claim(session_id,claim_token,reason,auto_wake_count,claimed_at,wake_invocation_uuid) VALUES('session','legacy','fixture',1,'fixture','invocation')").unwrap();
    assert_eq!(
        f.state.coordinate_manual_resume("session").unwrap(),
        ManualWakeCoordination::UnknownCustody
    );
    f.sql.execute("UPDATE session_wake_claim SET wake_pid=?1,wake_os_boot_id=?2,wake_os_pid_starttime_ticks=?3", params![f.live.os_pid,f.live.os_boot_id,f.live.os_pid_starttime_ticks]).unwrap();
    assert_eq!(
        f.state.coordinate_manual_resume("session").unwrap(),
        ManualWakeCoordination::LegacyLiveBusy
    );
    // A mismatched recorded incarnation is releasable, not a live-owner timeout.
    f.sql.execute("UPDATE session_wake_claim SET wake_os_pid_starttime_ticks=wake_os_pid_starttime_ticks+1",[]).unwrap();
    assert_eq!(
        f.state.coordinate_manual_resume("session").unwrap(),
        ManualWakeCoordination::Released
    );
    assert_eq!(
        f.state.coordinate_manual_resume("session").unwrap(),
        ManualWakeCoordination::Absent
    );
}

#[test]
fn target_change_releases_global_capacity_but_keeps_exact_command_identity() {
    let mut f = Fixture::new();
    f.enqueue("manual", "source", true);
    let SessionAdmissionAttempt::Admitted(row) = f.next() else {
        panic!("missing initial admission")
    };
    let token = row.claim_token.unwrap();
    assert!(
        f.db.session_admissions()
            .begin_launch("manual", &token, 3)
            .unwrap()
    );
    assert!(
        f.db.session_admissions()
            .requeue_unmaterialized("manual", "wrong-token", "destination")
            .is_err()
    );
    f.enqueue("other", "unrelated", true);
    f.db.session_admissions()
        .requeue_unmaterialized("manual", &token, "destination")
        .unwrap();
    let moved = f.db.session_admissions().row("manual").unwrap().unwrap();
    assert_eq!(moved.session_id.as_deref(), Some("destination"));
    assert_eq!(moved.state, "queued");
    assert!(moved.claim_token.is_none());
    let SessionAdmissionAttempt::Admitted(next) = f.next() else {
        panic!("retarget retained global capacity")
    };
    assert_eq!(next.registration_identity, "other");
}

#[test]
fn native_launcher_boot_mismatch_cannot_borrow_the_admission_exception() {
    let mut f = Fixture::new();
    f.native("starting", false);
    f.sql.execute("UPDATE session_wake_claim SET wake_pid=?1,wake_os_boot_id=?2,wake_os_pid_starttime_ticks=?3",params![f.live.os_pid,f.live.os_boot_id,f.live.os_pid_starttime_ticks]).unwrap();
    f.enqueue("manual", "session", true);
    f.enqueue("other", "unrelated", true);
    let wrong=serde_json::json!({"pid":f.live.os_pid,"boot_id":"wrong-boot","starttime_ticks":f.live.os_pid_starttime_ticks}).to_string();
    f.sql
        .execute(
            "UPDATE completion_continuation_attempt SET launcher_identity=?1,revision=revision+1",
            [wrong],
        )
        .unwrap();
    let SessionAdmissionAttempt::Admitted(next) = f.next() else {
        panic!("unrelated missing")
    };
    assert_eq!(next.registration_identity, "other");
}

// Act1 commitment 3: observation has no mutation duty when no claim exists.
// The actual independent writer remains held until after the observation.
#[test]
fn absent_manual_observation_does_not_request_a_sidecar_writer() {
    let f = Fixture::new();
    f.sql.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert_eq!(
        f.state.coordinate_manual_resume("session").unwrap(),
        ManualWakeCoordination::Absent
    );
    f.sql.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn native_busy_observation_does_not_request_a_sidecar_writer_or_release_custody() {
    let f = Fixture::new();
    f.native("accepted", false);
    f.sql.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert_eq!(
        f.state.coordinate_manual_resume("session").unwrap(),
        ManualWakeCoordination::NativeBusy
    );
    assert!(
        f.db.wake_session_reader()
            .wake_claim("session")
            .unwrap()
            .is_some()
    );
    f.sql.execute_batch("ROLLBACK").unwrap();
}
