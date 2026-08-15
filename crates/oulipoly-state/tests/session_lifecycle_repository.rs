//! ## Declared roles
//!
//! `orchestration`, `mapper`, `accessor`, `predicate`, `validator`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-state/tests/session_lifecycle_repository.rs
//!     role: adapter
//!     Translates:
//!       - session-lifecycle-repository-behavior-contract
//!       - StateDb-SQLite-test-fixture-contract
//!       - ordered-state-schema-migration-contract
//! ```

use oulipoly_state::{
    AcknowledgementStage, AcknowledgementWrite, DispositionWrite, EventDisposition,
    ExactProcessIdentity, ExternalIngress, ExternalIngressWrite, LeaseAcquire, LeaseReplace,
    NewLifecycleEvent, ProviderTurnGeneration, SessionLifecycleError, SessionLifecycleRepository,
    StateDb, SupervisorFence, TurnFence, TurnState,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};

fn store() -> (tempfile::TempDir, PathBuf, StateDb) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let db = StateDb::open(&path).unwrap();
    (dir, path, db)
}

fn process(pid: i64, suffix: &str) -> ExactProcessIdentity {
    ExactProcessIdentity {
        pid,
        boot_id: format!("boot-{suffix}"),
        start_time_ticks: pid * 10,
    }
}

fn supervisor(generation: i64, suffix: &str) -> SupervisorFence {
    SupervisorFence {
        generation,
        token: format!("token-{suffix}"),
        process: process(100 + generation, suffix),
    }
}

fn turn(
    generation_id: &str,
    invocation_id: &str,
    session_id: Option<&str>,
    state: TurnState,
    child: ExactProcessIdentity,
) -> ProviderTurnGeneration {
    ProviderTurnGeneration {
        generation_id: generation_id.to_owned(),
        spawn_invocation_id: invocation_id.to_owned(),
        session_id: session_id.map(str::to_owned),
        state,
        child,
    }
}

fn event(id: &str, kind: &str, cause: Option<&str>, correlation: &str) -> NewLifecycleEvent {
    NewLifecycleEvent {
        event_id: id.to_owned(),
        event_type: kind.to_owned(),
        cause_event_id: cause.map(str::to_owned),
        correlation_id: correlation.to_owned(),
        payload: format!("payload-{id}"),
        created_at: 100,
    }
}

#[test]
fn one_supervisor_lease_requires_the_exact_process_generation_and_token_fence() {
    let (_dir, _path, mut db) = store();
    let first = supervisor(1, "first");
    let replacement = supervisor(2, "replacement");

    assert_eq!(
        db.acquire_supervisor_lease("session-a", &first, 10)
            .unwrap(),
        LeaseAcquire::Acquired
    );
    assert_eq!(
        db.acquire_supervisor_lease("session-a", &first, 11)
            .unwrap(),
        LeaseAcquire::AlreadyOwned
    );
    assert!(matches!(
        db.acquire_supervisor_lease("session-a", &replacement, 12),
        Err(SessionLifecycleError::LeaseHeld)
    ));

    let mut pid_reused = first.clone();
    pid_reused.process.start_time_ticks += 1;
    assert!(matches!(
        db.replace_supervisor_lease("session-a", &pid_reused, &replacement, 20),
        Err(SessionLifecycleError::FenceMismatch)
    ));
    assert_eq!(
        db.replace_supervisor_lease("session-a", &first, &replacement, 20)
            .unwrap(),
        LeaseReplace::Replaced
    );
    assert!(matches!(
        db.release_supervisor_lease("session-a", &first),
        Err(SessionLifecycleError::FenceMismatch)
    ));
    assert_eq!(
        db.supervisor_lease("session-a").unwrap().unwrap().fence,
        replacement
    );

    let malformed = SupervisorFence {
        generation: 3,
        token: String::new(),
        process: process(103, "malformed"),
    };
    assert!(matches!(
        db.acquire_supervisor_lease("session-b", &malformed, 30),
        Err(SessionLifecycleError::Invalid("lease_token"))
    ));
}

#[test]
fn simultaneous_replacement_attempts_converge_on_one_exact_owner() {
    let (_dir, path, mut seed) = store();
    let first = supervisor(1, "first");
    let candidate_a = supervisor(2, "candidate-a");
    let candidate_b = supervisor(2, "candidate-b");
    seed.acquire_supervisor_lease("session-a", &first, 10)
        .unwrap();
    drop(seed);

    let db_a = StateDb::open(&path).unwrap();
    let db_b = StateDb::open(&path).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let thread_a = replace_in_thread(db_a, barrier.clone(), first.clone(), candidate_a.clone());
    let thread_b = replace_in_thread(db_b, barrier, first.clone(), candidate_b.clone());
    let result_a = thread_a.join().unwrap();
    let result_b = thread_b.join().unwrap();

    assert_eq!(
        [result_a.as_str(), result_b.as_str()]
            .into_iter()
            .filter(|result| *result == "replaced")
            .count(),
        1
    );
    assert_eq!(
        [result_a.as_str(), result_b.as_str()]
            .into_iter()
            .filter(|result| *result == "fence-mismatch")
            .count(),
        1
    );

    let mut db = StateDb::open(&path).unwrap();
    let winner = db.supervisor_lease("session-a").unwrap().unwrap().fence;
    assert!(winner == candidate_a || winner == candidate_b);
    assert_eq!(
        db.replace_supervisor_lease("session-a", &first, &winner, 21)
            .unwrap(),
        LeaseReplace::AlreadyReplaced
    );
}

fn replace_in_thread(
    mut db: StateDb,
    barrier: Arc<Barrier>,
    expected: SupervisorFence,
    replacement: SupervisorFence,
) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        barrier.wait();
        match db.replace_supervisor_lease("session-a", &expected, &replacement, 20) {
            Ok(LeaseReplace::Replaced) => "replaced".to_owned(),
            Ok(LeaseReplace::AlreadyReplaced) => "already-replaced".to_owned(),
            Err(SessionLifecycleError::FenceMismatch) => "fence-mismatch".to_owned(),
            result => panic!("unexpected replacement result: {result:?}"),
        }
    })
}

#[test]
fn one_nonterminal_turn_is_enforced_for_known_and_late_attached_sessions() {
    let (_dir, _path, mut db) = store();
    db.start_provider_turn(&turn(
        "generation-known",
        "invocation-known",
        Some("session-a"),
        TurnState::Running,
        process(301, "known"),
    ))
    .unwrap();
    assert!(matches!(
        db.start_provider_turn(&turn(
            "generation-rejected",
            "invocation-rejected",
            Some("session-a"),
            TurnState::Starting,
            process(302, "rejected"),
        )),
        Err(SessionLifecycleError::TurnAlreadyActive)
    ));

    db.start_provider_turn(&turn(
        "generation-late",
        "invocation-late",
        None,
        TurnState::Running,
        process(303, "late"),
    ))
    .unwrap();
    assert!(matches!(
        db.attach_provider_turn_session("generation-late", "invocation-late", "session-a"),
        Err(SessionLifecycleError::TurnAlreadyActive)
    ));
    db.attach_provider_turn_session("generation-late", "invocation-late", "session-b")
        .unwrap();
    assert_eq!(
        db.provider_turn("generation-late")
            .unwrap()
            .unwrap()
            .session_id
            .as_deref(),
        Some("session-b")
    );
}

#[test]
fn duplicate_turn_identity_takes_precedence_over_an_active_session_conflict() {
    let (_dir, _path, mut db) = store();
    db.start_provider_turn(&turn(
        "generation-existing",
        "invocation-existing",
        Some("session-a"),
        TurnState::Running,
        process(304, "existing"),
    ))
    .unwrap();

    assert!(matches!(
        db.start_provider_turn(&turn(
            "generation-duplicate",
            "invocation-existing",
            Some("session-a"),
            TurnState::Starting,
            process(305, "duplicate"),
        )),
        Err(SessionLifecycleError::Conflict("provider turn identity"))
    ));
}

#[test]
fn authoritative_transition_and_event_append_commit_or_roll_back_together() {
    let (_dir, _path, mut db) = store();
    db.start_provider_turn(&turn(
        "generation-a",
        "invocation-a",
        Some("session-a"),
        TurnState::Running,
        process(401, "turn"),
    ))
    .unwrap();
    db.append_lifecycle_event(
        "session-other",
        &event("duplicate-id", "seed", None, "corr-x"),
    )
    .unwrap();
    let fence = TurnFence {
        session_id: "session-a".to_owned(),
        generation_id: "generation-a".to_owned(),
        spawn_invocation_id: "invocation-a".to_owned(),
    };

    assert!(matches!(
        db.transition_turn_and_append_event(
            &fence,
            TurnState::Running,
            TurnState::Exited,
            &event("duplicate-id", "turn_exited", None, "corr-a"),
        ),
        Err(SessionLifecycleError::Sql(_))
    ));
    assert_eq!(
        db.provider_turn("generation-a").unwrap().unwrap().state,
        TurnState::Running
    );

    let committed = db
        .transition_turn_and_append_event(
            &fence,
            TurnState::Running,
            TurnState::Exited,
            &event("turn-exited-a", "turn_exited", None, "corr-a"),
        )
        .unwrap();
    assert_eq!(
        committed.sequence, 1,
        "failed append also rolled back sequence allocation"
    );
    assert_eq!(
        db.provider_turn("generation-a").unwrap().unwrap().state,
        TurnState::Exited
    );
}

#[test]
fn events_have_session_sequences_causality_and_replay_safe_dispositions() {
    let (_dir, _path, mut db) = store();
    let first = db
        .append_lifecycle_event("session-a", &event("event-a1", "accepted", None, "corr-a"))
        .unwrap();
    let second = db
        .append_lifecycle_event(
            "session-a",
            &event("event-a2", "scheduled", Some("event-a1"), "corr-a"),
        )
        .unwrap();
    let other = db
        .append_lifecycle_event("session-b", &event("event-b1", "accepted", None, "corr-b"))
        .unwrap();
    assert_eq!((first.sequence, second.sequence, other.sequence), (1, 2, 1));
    assert_eq!(second.cause_event_id.as_deref(), Some("event-a1"));
    assert_eq!(second.correlation_id, "corr-a");

    let before_retry = db
        .reconstruct_session("session-a", "scheduler", 10)
        .unwrap();
    let after_transient_retry = db
        .reconstruct_session("session-a", "scheduler", 10)
        .unwrap();
    assert_eq!(
        before_retry.undisposed_events, after_transient_retry.undisposed_events,
        "a transient consumer failure records no terminal disposition"
    );

    assert_eq!(
        db.record_event_disposition("event-a1", "scheduler", EventDisposition::Applied, 200)
            .unwrap(),
        DispositionWrite::Recorded
    );
    assert_eq!(
        db.record_event_disposition("event-a1", "scheduler", EventDisposition::Applied, 201)
            .unwrap(),
        DispositionWrite::AlreadyRecorded
    );
    assert!(matches!(
        db.record_event_disposition("event-a1", "scheduler", EventDisposition::Ignored, 202),
        Err(SessionLifecycleError::Conflict("event disposition"))
    ));
    let snapshot = db
        .reconstruct_session("session-a", "scheduler", 10)
        .unwrap();
    assert_eq!(
        snapshot
            .undisposed_events
            .iter()
            .map(|row| row.event_id.as_str())
            .collect::<Vec<_>>(),
        vec!["event-a2"]
    );
}

#[test]
fn bounded_reads_reject_a_zero_limit() {
    let (_dir, _path, mut db) = store();

    assert!(matches!(
        db.read_external_ingress("session-a", 0),
        Err(SessionLifecycleError::Invalid("limit"))
    ));
    assert!(matches!(
        db.reconstruct_session("session-a", "scheduler", 0),
        Err(SessionLifecycleError::Invalid("limit"))
    ));
}

#[test]
fn external_ingress_reads_do_not_advance_the_acceptance_cursor() {
    let (dir, path, mut db) = store();
    for (sequence, id) in [(1, "a1"), (2, "a2")] {
        db.append_external_ingress(&ExternalIngress {
            session_id: "session-a".to_owned(),
            sequence,
            ingress_id: id.to_owned(),
            payload: format!("payload-{id}"),
        })
        .unwrap();
    }

    assert_eq!(
        ingress_ids(db.read_external_ingress("session-a", 1).unwrap()),
        vec!["a1"]
    );
    assert_eq!(db.external_ingress_cursor("session-a").unwrap(), 0);

    let owner = supervisor(1, "owner");
    db.acquire_supervisor_lease("session-a", &owner, 1).unwrap();
    db.start_provider_turn(&turn(
        "generation-a",
        "invocation-a",
        Some("session-a"),
        TurnState::Running,
        process(601, "ingress"),
    ))
    .unwrap();
    let first = db.read_external_ingress("session-a", 1).unwrap().remove(0);
    assert_eq!(
        db.accept_external_ingress(&first, &owner, "generation-a", 10)
            .unwrap(),
        ExternalIngressWrite::Accepted
    );
    assert_eq!(db.external_ingress_cursor("session-a").unwrap(), 1);
    drop(db);

    let mut reopened = StateDb::open(&path).unwrap();
    assert_eq!(
        ingress_ids(reopened.read_external_ingress("session-a", 1).unwrap()),
        vec!["a2"]
    );
    assert_eq!(reopened.external_ingress_cursor("session-a").unwrap(), 1);
    drop(reopened);
    drop(dir);
}

#[test]
fn external_ingress_rejects_a_missing_turn_generation_without_advancing() {
    let (_dir, _path, mut db) = store();
    let owner = supervisor(1, "owner");
    db.acquire_supervisor_lease("session-a", &owner, 1).unwrap();
    let ingress = ExternalIngress {
        session_id: "session-a".to_owned(),
        sequence: 1,
        ingress_id: "mailbox:session-a:1".to_owned(),
        payload: "payload".to_owned(),
    };

    assert!(matches!(
        db.accept_external_ingress(&ingress, &owner, "generation-missing", 10),
        Err(SessionLifecycleError::Missing("provider turn generation"))
    ));
    assert_eq!(db.external_ingress_cursor("session-a").unwrap(), 0);
    assert!(db.acknowledgement(&ingress.ingress_id).unwrap().is_none());
    assert!(db.read_external_ingress("session-a", 1).unwrap().is_empty());
}

#[test]
fn external_ingress_rejects_a_generation_owned_by_another_session() {
    let (_dir, _path, mut db) = store();
    let owner = supervisor(1, "owner");
    db.acquire_supervisor_lease("session-a", &owner, 1).unwrap();
    db.start_provider_turn(&turn(
        "generation-b",
        "invocation-b",
        Some("session-b"),
        TurnState::Running,
        process(602, "wrong-session"),
    ))
    .unwrap();
    let ingress = ExternalIngress {
        session_id: "session-a".to_owned(),
        sequence: 1,
        ingress_id: "mailbox:session-a:1".to_owned(),
        payload: "payload".to_owned(),
    };

    assert!(matches!(
        db.accept_external_ingress(&ingress, &owner, "generation-b", 10),
        Err(SessionLifecycleError::FenceMismatch)
    ));
    assert_eq!(db.external_ingress_cursor("session-a").unwrap(), 0);
    assert!(db.acknowledgement(&ingress.ingress_id).unwrap().is_none());
    assert!(db.read_external_ingress("session-a", 1).unwrap().is_empty());
}

fn ingress_ids(rows: Vec<ExternalIngress>) -> Vec<String> {
    rows.into_iter().map(|row| row.ingress_id).collect()
}

#[test]
fn acknowledgement_stages_are_distinct_idempotent_and_exact_fenced() {
    let (_dir, _path, mut db) = store();
    assert_eq!(
        db.accept_pending("delivery-a", "session-a", "generation-a", 10)
            .unwrap(),
        AcknowledgementWrite::Advanced
    );
    assert_eq!(
        db.accept_pending("delivery-a", "session-a", "generation-a", 99)
            .unwrap(),
        AcknowledgementWrite::AlreadyRecorded
    );
    let accepted = db.acknowledgement("delivery-a").unwrap().unwrap();
    assert_eq!(accepted.stage(), AcknowledgementStage::AcceptedPending);
    assert_eq!(
        accepted.accepted_at, 10,
        "the first acceptance is preserved"
    );
    assert!(
        accepted.confirmed_at.is_none(),
        "acceptance is not confirmation"
    );
    assert!(matches!(
        db.mark_confirmed(
            "delivery-a",
            "session-a",
            "generation-a",
            "assistant-turn-a",
            11,
        ),
        Err(SessionLifecycleError::InvalidTransition)
    ));
    assert!(matches!(
        db.mark_submitted(
            "delivery-a",
            "session-b",
            "generation-a",
            "provider-accepted-a",
            12,
        ),
        Err(SessionLifecycleError::FenceMismatch)
    ));

    assert_eq!(
        db.mark_submitted(
            "delivery-a",
            "session-a",
            "generation-a",
            "provider-accepted-a",
            12,
        )
        .unwrap(),
        AcknowledgementWrite::Advanced
    );
    assert_eq!(
        db.mark_submitted(
            "delivery-a",
            "session-a",
            "generation-a",
            "provider-accepted-a",
            13,
        )
        .unwrap(),
        AcknowledgementWrite::AlreadyRecorded
    );
    assert!(matches!(
        db.mark_submitted(
            "delivery-a",
            "session-a",
            "generation-a",
            "different-evidence",
            13,
        ),
        Err(SessionLifecycleError::Conflict("submission evidence"))
    ));
    let submitted = db.acknowledgement("delivery-a").unwrap().unwrap();
    assert_eq!(submitted.stage(), AcknowledgementStage::Submitted);
    assert_eq!(submitted.submitted_at, Some(12));
    assert_eq!(
        submitted.submitted_evidence.as_deref(),
        Some("provider-accepted-a")
    );
    assert!(matches!(
        db.mark_confirmed(
            "delivery-a",
            "session-a",
            "generation-stale",
            "assistant-turn-a",
            14,
        ),
        Err(SessionLifecycleError::FenceMismatch)
    ));

    assert_eq!(
        db.mark_confirmed(
            "delivery-a",
            "session-a",
            "generation-a",
            "assistant-turn-a",
            14,
        )
        .unwrap(),
        AcknowledgementWrite::Advanced
    );
    assert_eq!(
        db.mark_confirmed(
            "delivery-a",
            "session-a",
            "generation-a",
            "assistant-turn-a",
            15,
        )
        .unwrap(),
        AcknowledgementWrite::AlreadyRecorded
    );
    let confirmed = db.acknowledgement("delivery-a").unwrap().unwrap();
    assert_eq!(confirmed.stage(), AcknowledgementStage::Confirmed);
    assert_eq!(confirmed.confirmed_at, Some(14));
    assert_eq!(
        confirmed.confirmed_evidence.as_deref(),
        Some("assistant-turn-a")
    );
}

#[test]
fn owner_acceptance_atomically_persists_ingress_cursor_and_accepted_pending() {
    let (_dir, path, mut db) = store();
    let owner = supervisor(1, "owner");
    db.acquire_supervisor_lease("session-a", &owner, 1).unwrap();
    db.start_provider_turn(&turn(
        "generation-a",
        "invocation-a",
        None,
        TurnState::Running,
        process(603, "acceptance"),
    ))
    .unwrap();
    let ingress = ExternalIngress {
        session_id: "session-a".to_owned(),
        sequence: 7,
        ingress_id: "mailbox:session-a:7".to_owned(),
        payload: "immutable-payload".to_owned(),
    };

    assert_eq!(
        db.accept_external_ingress(&ingress, &owner, "generation-a", 10)
            .unwrap(),
        ExternalIngressWrite::Accepted
    );
    assert_eq!(db.external_ingress_cursor("session-a").unwrap(), 7);
    let acknowledgement = db.acknowledgement("mailbox:session-a:7").unwrap().unwrap();
    assert_eq!(
        acknowledgement.stage(),
        AcknowledgementStage::AcceptedPending
    );
    assert_eq!(acknowledgement.submitted_at, None);
    assert_eq!(acknowledgement.confirmed_at, None);
    assert_eq!(
        db.accepted_pending_external_ingress("session-a", 0, 1)
            .unwrap(),
        vec![ingress.clone()]
    );
    assert_eq!(
        db.accept_external_ingress(&ingress, &owner, "generation-a", 99)
            .unwrap(),
        ExternalIngressWrite::AlreadyAccepted
    );

    drop(db);
    let mut reopened = StateDb::open(&path).unwrap();
    assert_eq!(reopened.external_ingress_cursor("session-a").unwrap(), 7);
    assert!(matches!(
        reopened.accept_external_ingress(
            &ExternalIngress {
                payload: "changed".to_owned(),
                ..ingress
            },
            &owner,
            "generation-a",
            100,
        ),
        Err(SessionLifecycleError::Conflict("external ingress cursor"))
    ));
    assert!(matches!(
        reopened.accept_external_ingress(
            &ExternalIngress {
                session_id: "session-b".to_owned(),
                sequence: 8,
                ingress_id: "mailbox:session-b:8".to_owned(),
                payload: "other".to_owned(),
            },
            &owner,
            "generation-a",
            101,
        ),
        Err(SessionLifecycleError::Missing("supervisor lease"))
    ));
}

#[test]
fn bounded_reconstruction_returns_only_one_sessions_authoritative_state() {
    let (_dir, _path, mut db) = store();
    let lease_a = supervisor(1, "a");
    let lease_b = supervisor(1, "b");
    db.acquire_supervisor_lease("session-a", &lease_a, 10)
        .unwrap();
    db.acquire_supervisor_lease("session-b", &lease_b, 10)
        .unwrap();
    db.start_provider_turn(&turn(
        "generation-a",
        "invocation-a",
        Some("session-a"),
        TurnState::Running,
        process(501, "child-a"),
    ))
    .unwrap();
    db.start_provider_turn(&turn(
        "generation-b",
        "invocation-b",
        Some("session-b"),
        TurnState::Running,
        process(502, "child-b"),
    ))
    .unwrap();
    for (session, id, owner, generation) in [
        ("session-a", "delivery-a", &lease_a, "generation-a"),
        ("session-b", "delivery-b", &lease_b, "generation-b"),
    ] {
        db.accept_external_ingress(
            &ExternalIngress {
                session_id: session.to_owned(),
                sequence: 1,
                ingress_id: id.to_owned(),
                payload: id.to_owned(),
            },
            owner,
            generation,
            20,
        )
        .unwrap();
    }
    db.accept_pending("delivery-a", "session-a", "generation-a", 20)
        .unwrap();
    db.accept_pending("delivery-b", "session-b", "generation-b", 20)
        .unwrap();
    db.append_lifecycle_event("session-a", &event("event-a1", "one", None, "corr-a"))
        .unwrap();
    db.append_lifecycle_event("session-a", &event("event-a2", "two", None, "corr-a"))
        .unwrap();
    db.append_lifecycle_event("session-b", &event("event-b1", "one", None, "corr-b"))
        .unwrap();

    let snapshot = db.reconstruct_session("session-a", "scheduler", 1).unwrap();
    assert_eq!(snapshot.session_id, "session-a");
    assert_eq!(snapshot.lease.unwrap().fence, lease_a);
    assert_eq!(snapshot.active_turn.unwrap().generation_id, "generation-a");
    assert_eq!(snapshot.ingress_cursor, 1);
    assert_eq!(snapshot.acknowledgements.len(), 1);
    assert_eq!(snapshot.acknowledgements[0].delivery_id, "delivery-a");
    assert_eq!(
        snapshot.undisposed_events.len(),
        1,
        "reconstruction is bounded"
    );
    assert_eq!(snapshot.undisposed_events[0].event_id, "event-a1");
}

#[test]
fn schema_v15_is_created_fresh_and_upgrades_from_schema_v10() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = StateDb::open(&dir.path().join("fresh.db")).unwrap();
    let fresh_connection = rusqlite::Connection::open(fresh.path()).unwrap();
    assert_eq!(user_version(&fresh_connection), 15);
    let lifecycle_tables = [
        "session_supervisor_leases",
        "provider_turn_generations",
        "session_lifecycle_sequences",
        "session_lifecycle_events",
        "session_lifecycle_event_dispositions",
        "session_external_ingress",
        "session_external_ingress_cursors",
        "session_delivery_acknowledgements",
        "session_delivery_evidence",
    ];
    for table in lifecycle_tables {
        assert!(table_exists(&fresh_connection, table), "missing {table}");
    }
    assert!(
        table_exists(&fresh_connection, "fresh_continuations"),
        "missing fresh_continuations"
    );
    assert!(
        table_exists(&fresh_connection, "invocation_completion_obligations"),
        "missing invocation_completion_obligations"
    );

    let upgrade_path = dir.path().join("upgrade.db");
    let mut conn = rusqlite::Connection::open(&upgrade_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE preserved_rows (id INTEGER PRIMARY KEY, label TEXT NOT NULL);
         INSERT INTO preserved_rows (id, label) VALUES (1, 'keep-me');
         PRAGMA user_version = 10;",
    )
    .unwrap();
    let plan = oulipoly_state::migrations::plan(10, 15).unwrap();
    assert_eq!(
        plan.iter()
            .map(|migration| migration.id)
            .collect::<Vec<_>>(),
        vec![
            "0011_durable_session_lifecycle",
            "0012_session_ingress_evidence",
            "0013_fresh_continuations",
            "0014_invocation_completion_obligations",
            "0015_invocation_completion_continuity"
        ]
    );
    oulipoly_state::migrations::run_with_db_path(&mut conn, &plan, upgrade_path).unwrap();
    assert_eq!(user_version(&conn), 15);
    for table in lifecycle_tables {
        assert!(table_exists(&conn, table), "missing {table}");
    }
    assert!(
        table_exists(&conn, "fresh_continuations"),
        "missing fresh_continuations"
    );
    assert!(
        table_exists(&conn, "invocation_completion_obligations"),
        "missing invocation_completion_obligations"
    );
    assert_eq!(
        conn.query_row("SELECT label FROM preserved_rows WHERE id = 1", [], |row| {
            row.get::<_, String>(0)
        })
        .unwrap(),
        "keep-me"
    );
}

fn user_version(conn: &rusqlite::Connection) -> i64 {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap()
}

fn table_exists(conn: &rusqlite::Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?)",
        [table],
        |row| row.get(0),
    )
    .unwrap()
}

#[test]
fn repository_surface_is_typed_and_requires_no_embedded_sql() {
    fn consume(repository: &mut dyn SessionLifecycleRepository) {
        let _ = repository.supervisor_lease("session-a");
    }

    let mut db = StateDb::open(Path::new(":memory:")).unwrap();
    consume(&mut db);
}
