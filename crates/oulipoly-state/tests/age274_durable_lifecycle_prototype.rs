use oulipoly_state::durable_lifecycle_prototype::{
    AcknowledgementStage, AcknowledgementWrite, DispositionWrite, DurableLifecyclePrototype,
    EventDisposition, ExactProcessIdentity, ExternalIngress, LeaseAcquire, NewLifecycleEvent,
    PrototypeError, SupervisorFence, TurnFence, TurnState,
};

fn store() -> (tempfile::TempDir, DurableLifecyclePrototype) {
    let dir = tempfile::tempdir().unwrap();
    let db =
        DurableLifecyclePrototype::open(dir.path().join("durable-lifecycle-prototype.db")).unwrap();
    (dir, db)
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
    let (_dir, mut db) = store();
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
        Err(PrototypeError::LeaseHeld)
    ));

    let mut stale = first.clone();
    stale.process.start_time_ticks += 1;
    assert!(matches!(
        db.replace_supervisor_lease("session-a", &stale, &replacement, 20),
        Err(PrototypeError::FenceMismatch)
    ));
    db.replace_supervisor_lease("session-a", &first, &replacement, 20)
        .unwrap();
    assert!(matches!(
        db.release_supervisor_lease("session-a", &first),
        Err(PrototypeError::FenceMismatch)
    ));
    assert_eq!(
        db.supervisor_lease("session-a").unwrap().unwrap().fence,
        replacement
    );
}

#[test]
fn one_nonterminal_turn_is_enforced_for_known_and_late_attached_sessions() {
    let (_dir, mut db) = store();
    let child = process(301, "child");

    db.start_provider_turn(
        "generation-known",
        "invocation-known",
        Some("session-a"),
        TurnState::Running,
        &child,
    )
    .unwrap();
    assert!(matches!(
        db.start_provider_turn(
            "generation-rejected",
            "invocation-rejected",
            Some("session-a"),
            TurnState::Starting,
            &process(302, "rejected"),
        ),
        Err(PrototypeError::TurnAlreadyActive)
    ));

    db.start_provider_turn(
        "generation-late",
        "invocation-late",
        None,
        TurnState::Running,
        &process(303, "late"),
    )
    .unwrap();
    assert!(matches!(
        db.attach_provider_turn_session("generation-late", "invocation-late", "session-a"),
        Err(PrototypeError::TurnAlreadyActive)
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
fn authoritative_transition_and_event_append_commit_or_roll_back_together() {
    let (_dir, mut db) = store();
    db.start_provider_turn(
        "generation-a",
        "invocation-a",
        Some("session-a"),
        TurnState::Running,
        &process(401, "turn"),
    )
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
        Err(PrototypeError::Sql(_))
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
    let (_dir, mut db) = store();
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
        Err(PrototypeError::Conflict("event disposition"))
    ));

    let snapshot = db
        .reconstruct_session("session-a", "scheduler", 10)
        .unwrap();
    assert_eq!(
        snapshot
            .unconsumed_events
            .iter()
            .map(|row| row.event_id.as_str())
            .collect::<Vec<_>>(),
        vec!["event-a2"]
    );
}

#[test]
fn external_ingress_cursor_is_monotonic_persisted_and_session_local() {
    let (dir, mut db) = store();
    for (session, sequence, id) in [
        ("session-a", 1, "a1"),
        ("session-b", 1, "b1"),
        ("session-a", 2, "a2"),
        ("session-b", 2, "b2"),
        ("session-a", 3, "a3"),
    ] {
        db.append_external_ingress(&ExternalIngress {
            session_id: session.to_owned(),
            sequence,
            ingress_id: id.to_owned(),
            payload: format!("payload-{id}"),
        })
        .unwrap();
    }
    db.advance_external_ingress_cursor("session-a", 1).unwrap();
    db.advance_external_ingress_cursor("session-a", 0).unwrap();

    let first_read = db.read_external_ingress("session-a", 1).unwrap();
    assert_eq!(
        first_read
            .iter()
            .map(|row| row.ingress_id.as_str())
            .collect::<Vec<_>>(),
        vec!["a2"]
    );
    drop(db);

    let mut reopened =
        DurableLifecyclePrototype::open(dir.path().join("durable-lifecycle-prototype.db")).unwrap();
    assert_eq!(
        reopened
            .read_external_ingress("session-a", 10)
            .unwrap()
            .iter()
            .map(|row| row.ingress_id.as_str())
            .collect::<Vec<_>>(),
        vec!["a3"]
    );
    assert_eq!(
        reopened
            .read_external_ingress("session-b", 10)
            .unwrap()
            .iter()
            .map(|row| row.ingress_id.as_str())
            .collect::<Vec<_>>(),
        vec!["b1", "b2"]
    );
}

#[test]
fn acknowledgement_stages_are_distinct_idempotent_and_exact_fenced() {
    let (_dir, mut db) = store();
    assert_eq!(
        db.accept_pending("delivery-a", "session-a", "generation-a", 10)
            .unwrap(),
        AcknowledgementWrite::Advanced
    );
    let accepted = db.acknowledgement("delivery-a").unwrap().unwrap();
    assert_eq!(accepted.stage(), AcknowledgementStage::AcceptedPending);
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
        Err(PrototypeError::InvalidTransition)
    ));
    assert!(matches!(
        db.mark_submitted(
            "delivery-a",
            "session-b",
            "generation-a",
            "provider-accepted-a",
            12,
        ),
        Err(PrototypeError::FenceMismatch)
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
    assert_eq!(
        db.acknowledgement("delivery-a").unwrap().unwrap().stage(),
        AcknowledgementStage::Submitted
    );
    assert!(matches!(
        db.mark_confirmed(
            "delivery-a",
            "session-a",
            "generation-stale",
            "assistant-turn-a",
            14,
        ),
        Err(PrototypeError::FenceMismatch)
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
    assert_eq!(
        db.acknowledgement("delivery-a").unwrap().unwrap().stage(),
        AcknowledgementStage::Confirmed
    );
}

#[test]
fn bounded_reconstruction_returns_only_one_sessions_authoritative_state() {
    let (_dir, mut db) = store();
    let lease_a = supervisor(1, "a");
    let lease_b = supervisor(1, "b");
    db.acquire_supervisor_lease("session-a", &lease_a, 10)
        .unwrap();
    db.acquire_supervisor_lease("session-b", &lease_b, 10)
        .unwrap();
    db.start_provider_turn(
        "generation-a",
        "invocation-a",
        Some("session-a"),
        TurnState::Running,
        &process(501, "child-a"),
    )
    .unwrap();
    db.start_provider_turn(
        "generation-b",
        "invocation-b",
        Some("session-b"),
        TurnState::Running,
        &process(502, "child-b"),
    )
    .unwrap();
    db.advance_external_ingress_cursor("session-a", 7).unwrap();
    db.advance_external_ingress_cursor("session-b", 99).unwrap();
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
    assert_eq!(snapshot.active_child.unwrap().generation_id, "generation-a");
    assert_eq!(snapshot.ingress_cursor, 7);
    assert_eq!(snapshot.acknowledgements.len(), 1);
    assert_eq!(snapshot.acknowledgements[0].delivery_id, "delivery-a");
    assert_eq!(
        snapshot.unconsumed_events.len(),
        1,
        "reconstruction is bounded"
    );
    assert_eq!(snapshot.unconsumed_events[0].event_id, "event-a1");
}
