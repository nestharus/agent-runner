#![cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]

use oulipoly_state::StateDb;
use oulipoly_state::mailbox::{
    BrokerSourceCandidate, BrokerSourceEffectGrant, FreshV30Lane, MailboxDb,
};
use rusqlite::{Connection, params};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::process::Command;
use std::sync::mpsc;

#[test]
fn private_fresh_dual_lane_live_old_wal_collision_and_restart() {
    if std::env::var_os("AGE319_FRESH_LANE_CHILD").is_none() {
        let status = Command::new("unshare")
            .arg("-Ur")
            .arg(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("private_fresh_dual_lane_live_old_wal_collision_and_restart")
            .arg("--nocapture")
            .env("AGE319_FRESH_LANE_CHILD", "1")
            .status()
            .expect("private user namespace is required for this executable test");
        assert!(status.success(), "private dual-lane child failed");
        return;
    }

    let private = tempfile::tempdir().unwrap();
    let old_root = private.path().join("old-v29");
    let broker_root = private.path().join("broker");
    fs::create_dir(&old_root).unwrap();
    fs::create_dir(&broker_root).unwrap();
    fs::set_permissions(&broker_root, fs::Permissions::from_mode(0o700)).unwrap();
    let incomplete_root = private.path().join("incomplete-broker");
    fs::create_dir(&incomplete_root).unwrap();
    fs::set_permissions(&incomplete_root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(incomplete_root.join("v30")).unwrap();
    assert!(FreshV30Lane::initialize_at(&incomplete_root).is_err());
    assert!(FreshV30Lane::open_at(&broker_root).is_err());
    let old_state = old_root.join("state.db");
    let old_mailbox = old_root.join("pid-identity.db");
    drop(StateDb::open(&old_state).unwrap());
    drop(MailboxDb::open(&old_mailbox).unwrap());
    let old = Connection::open(&old_mailbox).unwrap();
    // Historical direct writers keep their v29 island while the fresh lane
    // starts from absent files under a different root.
    old.execute_batch(
        "DROP TRIGGER completion_uncertain_input_preserve;
         DROP TABLE completion_uncertain_input;
         DROP INDEX idx_mailbox_deliverable_session_live;
         DROP INDEX idx_mailbox_deliverable_target_live;
         DROP INDEX idx_mailbox_deliverable_global;
         PRAGMA user_version=29;",
    )
    .unwrap();
    old.execute_batch(include_str!(
        "../src/mailbox/migrations/0022_live_history_barrier.sql"
    ))
    .unwrap();
    assert_eq!(insert_pending(&old, "legacy-existing", "old-debt"), 1);

    let (writer_ready, ready) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let old_writer_path = old_state.clone();
    let writer = std::thread::spawn(move || {
        let db = Connection::open(old_writer_path).unwrap();
        db.execute_batch(
            "PRAGMA journal_mode=WAL; BEGIN IMMEDIATE;
            CREATE TABLE old_live_writer_debt(value TEXT NOT NULL);",
        )
        .unwrap();
        writer_ready.send(()).unwrap();
        released.recv().unwrap();
        db.execute(
            "INSERT INTO old_live_writer_debt VALUES('committed-after-v30-init')",
            [],
        )
        .unwrap();
        db.execute_batch("COMMIT").unwrap();
    });
    ready.recv().unwrap();

    let first = FreshV30Lane::initialize_at(&broker_root).unwrap();
    let fresh_state = broker_root.join("v30/state.db");
    let fresh_mailbox = broker_root.join("v30/sidecar/pid-identity.db");
    for path in [
        &fresh_state,
        &fresh_mailbox,
        &broker_root.join("v30/sidecar/state-source.json"),
    ] {
        let meta = fs::symlink_metadata(path).unwrap();
        assert_eq!(meta.uid(), 0);
        assert_eq!(meta.mode() & 0o077, 0);
    }
    let state = Connection::open(&fresh_state).unwrap();
    let mailbox = Connection::open(&fresh_mailbox).unwrap();
    assert_eq!(
        state
            .query_row("SELECT count(*) FROM invocations", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        state
            .query_row(
                "SELECT count(*) FROM fresh_lane_session_admission",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        state
            .query_row("SELECT lane_id FROM fresh_lane_state_identity", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
        first.lane_id
    );
    assert_eq!(
        mailbox
            .query_row("SELECT count(*) FROM mailbox", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(state.prepare("SELECT * FROM old_live_writer_debt").is_err());
    assert_eq!(
        mailbox
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        32
    );
    assert_eq!(
        state
            .query_row("SELECT count(*) FROM fresh_bash_child", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0,
        "new child schema has no imported v29 or root work"
    );

    // Simulate a lost initialization reply and broker restart. Identity is
    // read back from the published files; a second generation is never made.
    assert_eq!(FreshV30Lane::initialize_at(&broker_root).unwrap(), first);
    let mut lane = FreshV30Lane::open_at(&broker_root).unwrap();
    assert_eq!(lane.identity(), &first);
    let request_id = uuid::Uuid::new_v4().to_string();
    assert!(lane.read_session(&request_id).unwrap().is_none());
    let session = lane.allocate_session(&request_id).unwrap();
    let mut unadmitted = BrokerSourceEffectGrant {
        grant_id: uuid::Uuid::new_v4().to_string(),
        source_generation: first.source_generation.clone(),
        root_id: uuid::Uuid::new_v4().to_string(),
        owner_generation: uuid::Uuid::new_v4().to_string(),
        driver_identity: oulipoly_state::completion_continuation::SourceProcessIdentity {
            pid: 1,
            boot_id: uuid::Uuid::new_v4().to_string(),
            starttime_ticks: 1,
        },
        authority_ordinal: 1,
        candidate: BrokerSourceCandidate {
            registration_id: uuid::Uuid::new_v4().to_string(),
            registration_digest: "0".repeat(64),
            listener_revision: 1,
            listener: oulipoly_state::completion_continuation::ListenerIdentity {
                listener_id: uuid::Uuid::new_v4().to_string(),
                session_id: session.session_id.clone(),
                owner_invocation_uuid: uuid::Uuid::new_v4().to_string(),
            },
        },
        phase: "consumed".into(),
        revision: 2,
    };
    // A caller string that spells an allocated session has no source
    // authority. Nor can a v29 generation or a sibling session join it.
    assert!(lane.source_provenance(&unadmitted).is_err());
    unadmitted.source_generation = uuid::Uuid::new_v4().to_string();
    assert!(lane.source_provenance(&unadmitted).is_err());
    unadmitted.source_generation = first.source_generation.clone();
    unadmitted.candidate.listener.session_id =
        format!("v30:{}:{}", first.lane_id, uuid::Uuid::new_v4());
    assert!(lane.source_provenance(&unadmitted).is_err());
    assert_eq!(
        mailbox
            .query_row(
                "SELECT count(*) FROM broker_fresh_source_admission",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(lane.allocate_session(&request_id).unwrap(), session);
    assert_eq!(
        lane.read_session(&request_id).unwrap(),
        Some(session.clone())
    );
    assert_eq!(
        mailbox
            .query_row("SELECT count(*) FROM fresh_lane_session", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let admitted: (String, String, String, String) = state
        .query_row(
            "SELECT session_id,allocation_id,lane_id,source_generation
             FROM fresh_lane_session_admission WHERE request_id=?1",
            [&request_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        admitted,
        (
            session.session_id.clone(),
            session.allocation_id.clone(),
            first.lane_id.clone(),
            first.source_generation.clone()
        )
    );
    // A State-first crash leaves no usable d readback. The same D key
    // completes the exact allocation after restart without minting a new ID.
    let incomplete_request = uuid::Uuid::new_v4().to_string();
    let incomplete_session = format!("v30:{}:{}", first.lane_id, uuid::Uuid::new_v4());
    let incomplete_allocation = uuid::Uuid::new_v4().to_string();
    state
        .execute(
            "INSERT INTO fresh_lane_session_admission
         (request_id,session_id,allocation_id,lane_id,source_generation,admitted_at)
         VALUES(?1,?2,?3,?4,?5,'2026-09-24T00:00:00Z')",
            params![
                incomplete_request,
                incomplete_session,
                incomplete_allocation,
                first.lane_id,
                first.source_generation
            ],
        )
        .unwrap();
    assert!(lane.read_session(&incomplete_request).is_err());
    drop(lane);
    let mut lane = FreshV30Lane::open_at(&broker_root).unwrap();
    let completed = lane.allocate_session(&incomplete_request).unwrap();
    assert_eq!(completed.session_id, incomplete_session);
    assert_eq!(completed.allocation_id, incomplete_allocation);
    assert_eq!(
        lane.read_session(&incomplete_request).unwrap(),
        Some(completed)
    );
    let old_only_request = uuid::Uuid::new_v4().to_string();
    mailbox.execute(
        "INSERT INTO fresh_lane_session(session_id,request_id,allocation_id,lane_id,source_generation,allocated_at)
         VALUES(?1,?2,?3,?4,?5,'2026-09-24T00:00:00Z')",
        params![format!("v30:{}:{}", first.lane_id, uuid::Uuid::new_v4()),
            old_only_request, uuid::Uuid::new_v4().to_string(), first.lane_id,
            first.source_generation],
    ).unwrap();
    assert!(lane.read_session(&old_only_request).is_err());
    assert!(lane.allocate_session(&old_only_request).is_err());
    assert_eq!(
        state
            .query_row("SELECT count(*) FROM invocations", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0,
        "D admission is not a Runner invocation or result"
    );
    assert_eq!(
        mailbox
            .query_row("SELECT count(*) FROM completion_native_kernel_q", [], |r| r
                .get::<_, i64>(0),)
            .unwrap(),
        0,
        "D admission cannot imply physical Q"
    );
    assert_eq!(
        mailbox
            .query_row(
                "SELECT count(*) FROM broker_fresh_source_admission",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap(),
        0,
        "D admission cannot import a v29 source"
    );
    assert!(
        lane.read_session(&uuid::Uuid::new_v4().to_string())
            .unwrap()
            .is_none()
    );
    assert!(lane.allocate_session("not-a-request-id").is_err());
    assert!(lane.read_session(&uuid::Uuid::nil().to_string()).is_err());
    assert_eq!(insert_pending(&mailbox, "v30-primer", "new-primer"), 1);
    let new_row = insert_pending(&mailbox, &session.session_id, "same-handle");
    assert_eq!(new_row, 2);
    lane.require_mailbox_row(&session, new_row).unwrap();
    let mut wrong = session.clone();
    wrong.lane_id = uuid::Uuid::new_v4().to_string();
    assert!(lane.require_session(&wrong).is_err());
    wrong = session.clone();
    wrong.source_generation = uuid::Uuid::new_v4().to_string();
    assert!(lane.require_mailbox_row(&wrong, 1).is_err());
    wrong = session.clone();
    wrong.allocation_id = uuid::Uuid::new_v4().to_string();
    assert!(lane.require_session(&wrong).is_err());
    assert!(lane.require_mailbox_row(&session, 1).is_err());
    drop(lane);

    let old_row = insert_pending(&old, &session.session_id, "same-handle");
    assert_eq!(old_row, 2); // same (session, seq), independently pending
    release.send(()).unwrap();
    writer.join().unwrap();
    assert_eq!(
        Connection::open(&old_state)
            .unwrap()
            .query_row("SELECT value FROM old_live_writer_debt", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "committed-after-v30-init"
    );
    assert!(state.prepare("SELECT * FROM old_live_writer_debt").is_err());
    assert_eq!(
        old.query_row(
            "SELECT count(*) FROM mailbox WHERE delivered_at IS NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    assert_eq!(
        mailbox
            .query_row(
                "SELECT count(*) FROM mailbox WHERE delivered_at IS NULL",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
    assert_eq!(
        mailbox
            .query_row(
                "SELECT count(*) FROM mailbox WHERE handle='old-debt'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        old.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        29
    );
    let reopened = FreshV30Lane::open_at(&broker_root).unwrap();
    reopened.require_mailbox_row(&session, 2).unwrap();
    assert_eq!(reopened.read_session(&request_id).unwrap(), Some(session));

    // A published lane from before the root effect Act gains only the
    // additive State objects. The sidecar stays at broker version 32.
    state.execute_batch("DROP TABLE fresh_root_effect").unwrap();
    assert_eq!(FreshV30Lane::initialize_at(&broker_root).unwrap(), first);
    assert_eq!(
        state
            .query_row("SELECT count(*) FROM fresh_root_effect", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        mailbox
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        32
    );
    assert_eq!(
        FreshV30Lane::open_at(&broker_root).unwrap().identity(),
        &first
    );

    // Existing names alone do not establish the ledger's immutable shape.
    state
        .execute_batch(
            "CREATE TRIGGER fresh_root_effect_extra BEFORE DELETE ON fresh_root_effect
         BEGIN SELECT 1; END;",
        )
        .unwrap();
    assert!(
        FreshV30Lane::open_at(&broker_root)
            .err()
            .unwrap()
            .contains("fresh root effect schema differs")
    );
    state
        .execute_batch("DROP TRIGGER fresh_root_effect_extra")
        .unwrap();
    state
        .execute_batch(
            "DROP TRIGGER fresh_root_effect_return_once;
             CREATE TRIGGER fresh_root_effect_return_once BEFORE UPDATE ON fresh_root_effect
             BEGIN SELECT 1; END;",
        )
        .unwrap();
    assert!(
        FreshV30Lane::open_at(&broker_root)
            .err()
            .unwrap()
            .contains("fresh root effect schema differs")
    );
    assert_eq!(
        mailbox
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        32
    );
    state.execute_batch("DROP TABLE fresh_root_effect").unwrap();
    assert_eq!(
        FreshV30Lane::open_at(&broker_root).unwrap().identity(),
        &first
    );
    state
        .execute_batch(
            "CREATE TRIGGER fresh_bash_extra BEFORE DELETE ON fresh_bash_child
         BEGIN SELECT 1; END;",
        )
        .unwrap();
    assert!(
        FreshV30Lane::open_at(&broker_root)
            .err()
            .unwrap()
            .contains("fresh Bash child schema differs")
    );
    state
        .execute_batch("DROP TRIGGER fresh_bash_extra")
        .unwrap();
    state
        .execute_batch("DROP TABLE fresh_bash_private_result")
        .unwrap();
    assert!(
        FreshV30Lane::open_at(&broker_root)
            .err()
            .unwrap()
            .contains("fresh Bash child schema is incomplete")
    );
    state
        .execute_batch("DROP TABLE fresh_bash_private_work; DROP TABLE fresh_bash_child")
        .unwrap();
    assert_eq!(
        FreshV30Lane::open_at(&broker_root).unwrap().identity(),
        &first
    );
    assert_eq!(
        state
            .query_row("SELECT count(*) FROM fresh_bash_private_result", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
    assert_eq!(
        mailbox
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        32
    );
    state.execute_batch(
        "DROP TRIGGER fresh_bash_private_result_no_delete;
         CREATE TRIGGER fresh_bash_private_result_no_delete BEFORE DELETE ON fresh_bash_private_result
         BEGIN SELECT 1; END;",
    ).unwrap();
    assert!(
        FreshV30Lane::open_at(&broker_root)
            .err()
            .unwrap()
            .contains("fresh Bash child schema differs")
    );
}

fn insert_pending(db: &Connection, session: &str, handle: &str) -> i64 {
    db.execute(
        "INSERT INTO mailbox(session_id,kind,handle,payload_json,enqueued_at,
            state_dir,meta_path,log_path,rc_path,rc)
        VALUES(?1,'fixture',?2,'{}','2026-09-24T00:00:00Z',
            '/fixture','/fixture/meta','/fixture/log','/fixture/rc',0)",
        params![session, handle],
    )
    .unwrap();
    db.last_insert_rowid()
}
