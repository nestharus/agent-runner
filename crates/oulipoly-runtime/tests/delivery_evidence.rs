use oulipoly_runtime::delivery_evidence::{
    ManualAcknowledgementEvidence, PtyTransportAcknowledgementEvidence,
};
use oulipoly_state::{
    AcknowledgementStage, AcknowledgementWrite, DeliveryEvidence, DeliveryEvidenceKind,
    SessionLifecycleError, SessionLifecycleRepository, StateDb,
};

#[test]
fn pty_and_manual_acknowledgements_remain_transport_evidence_under_exact_fences() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = StateDb::open(&dir.path().join("state.db")).unwrap();
    let pty = PtyTransportAcknowledgementEvidence {
        evidence_id: "pty:attempt-a".to_owned(),
        delivery_attempt_id: "attempt-a".to_owned(),
        session_id: "session-a".to_owned(),
        turn_generation_id: "generation-a".to_owned(),
        observed_at: 10,
    };
    assert_eq!(
        pty.record(&mut state).unwrap(),
        AcknowledgementWrite::Advanced
    );
    assert_eq!(
        PtyTransportAcknowledgementEvidence {
            observed_at: 10,
            ..pty.clone()
        }
        .record(&mut state)
        .unwrap(),
        AcknowledgementWrite::AlreadyRecorded
    );
    let accepted = state.acknowledgement("attempt-a").unwrap().unwrap();
    assert_eq!(accepted.stage(), AcknowledgementStage::AcceptedPending);
    assert_eq!(accepted.submitted_at, None);
    assert_eq!(accepted.confirmed_at, None);
    assert_eq!(
        state.delivery_evidence("pty:attempt-a").unwrap(),
        Some(DeliveryEvidence {
            evidence_id: "pty:attempt-a".to_owned(),
            kind: DeliveryEvidenceKind::PtyTransportAck,
            delivery_id: "attempt-a".to_owned(),
            session_id: "session-a".to_owned(),
            turn_generation_id: "generation-a".to_owned(),
            observed_at: 10,
        })
    );

    let stale_pty = PtyTransportAcknowledgementEvidence {
        evidence_id: "pty:stale".to_owned(),
        delivery_attempt_id: "attempt-a".to_owned(),
        session_id: "session-a".to_owned(),
        turn_generation_id: "generation-old".to_owned(),
        observed_at: 12,
    };
    assert!(matches!(
        stale_pty.record(&mut state),
        Err(SessionLifecycleError::FenceMismatch)
    ));

    let wrong_session_pty = PtyTransportAcknowledgementEvidence {
        evidence_id: "pty:wrong-session".to_owned(),
        delivery_attempt_id: "attempt-a".to_owned(),
        session_id: "session-b".to_owned(),
        turn_generation_id: "generation-a".to_owned(),
        observed_at: 13,
    };
    assert!(matches!(
        wrong_session_pty.record(&mut state),
        Err(SessionLifecycleError::FenceMismatch)
    ));

    ManualAcknowledgementEvidence {
        evidence_id: "manual:attempt-a".to_owned(),
        delivery_attempt_id: "attempt-a".to_owned(),
        session_id: "session-a".to_owned(),
        turn_generation_id: "generation-a".to_owned(),
        observed_at: 14,
    }
    .record(&mut state)
    .unwrap();
    assert_eq!(
        state.acknowledgement("attempt-a").unwrap().unwrap().stage(),
        AcknowledgementStage::AcceptedPending
    );
    assert_eq!(
        state.delivery_evidence("manual:attempt-a").unwrap(),
        Some(DeliveryEvidence {
            evidence_id: "manual:attempt-a".to_owned(),
            kind: DeliveryEvidenceKind::ManualAcknowledgement,
            delivery_id: "attempt-a".to_owned(),
            session_id: "session-a".to_owned(),
            turn_generation_id: "generation-a".to_owned(),
            observed_at: 14,
        })
    );

    let stale = ManualAcknowledgementEvidence {
        evidence_id: "manual:stale".to_owned(),
        delivery_attempt_id: "attempt-a".to_owned(),
        session_id: "session-a".to_owned(),
        turn_generation_id: "generation-old".to_owned(),
        observed_at: 15,
    };
    assert!(matches!(
        stale.record(&mut state),
        Err(SessionLifecycleError::FenceMismatch)
    ));

    let wrong_session = ManualAcknowledgementEvidence {
        evidence_id: "manual:wrong-session".to_owned(),
        delivery_attempt_id: "attempt-a".to_owned(),
        session_id: "session-b".to_owned(),
        turn_generation_id: "generation-a".to_owned(),
        observed_at: 16,
    };
    assert!(matches!(
        wrong_session.record(&mut state),
        Err(SessionLifecycleError::FenceMismatch)
    ));

    let wrong_attempt = ManualAcknowledgementEvidence {
        evidence_id: "manual:wrong-attempt".to_owned(),
        delivery_attempt_id: "attempt-missing".to_owned(),
        session_id: "session-a".to_owned(),
        turn_generation_id: "generation-a".to_owned(),
        observed_at: 17,
    };
    assert!(matches!(
        wrong_attempt.record(&mut state),
        Err(SessionLifecycleError::Missing("delivery acknowledgement"))
    ));
}

#[test]
fn pty_acceptance_and_evidence_roll_back_together_when_evidence_insert_fails() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let mut state = StateDb::open(&path).unwrap();
    let fault = rusqlite::Connection::open(&path).unwrap();
    fault
        .execute_batch(
            "CREATE TRIGGER fail_pty_evidence_insert
             BEFORE INSERT ON session_delivery_evidence
             BEGIN SELECT RAISE(FAIL, 'injected evidence failure'); END;",
        )
        .unwrap();
    let evidence = PtyTransportAcknowledgementEvidence {
        evidence_id: "pty:atomic-attempt".to_owned(),
        delivery_attempt_id: "atomic-attempt".to_owned(),
        session_id: "session-a".to_owned(),
        turn_generation_id: "generation-a".to_owned(),
        observed_at: 10,
    };

    assert!(evidence.record(&mut state).is_err());
    assert!(state.acknowledgement("atomic-attempt").unwrap().is_none());
    assert!(
        state
            .delivery_evidence("pty:atomic-attempt")
            .unwrap()
            .is_none()
    );
    fault
        .execute_batch("DROP TRIGGER fail_pty_evidence_insert")
        .unwrap();

    assert_eq!(
        evidence.record(&mut state).unwrap(),
        AcknowledgementWrite::Advanced
    );
    assert!(state.acknowledgement("atomic-attempt").unwrap().is_some());
    assert!(
        state
            .delivery_evidence("pty:atomic-attempt")
            .unwrap()
            .is_some()
    );
    assert_eq!(
        evidence.record(&mut state).unwrap(),
        AcknowledgementWrite::AlreadyRecorded
    );
}
