use oulipoly_runtime::delivery_evidence::{
    ManualAcknowledgementEvidence, PtyTransportAcknowledgementEvidence,
};
use oulipoly_state::{
    AcknowledgementStage, AcknowledgementWrite, SessionLifecycleError, SessionLifecycleRepository,
    StateDb,
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
        pty.record(&mut state).unwrap(),
        AcknowledgementWrite::AlreadyRecorded
    );
    let accepted = state.acknowledgement("attempt-a").unwrap().unwrap();
    assert_eq!(accepted.stage(), AcknowledgementStage::AcceptedPending);
    assert_eq!(accepted.submitted_at, None);
    assert_eq!(accepted.confirmed_at, None);

    ManualAcknowledgementEvidence {
        evidence_id: "manual:attempt-a".to_owned(),
        delivery_attempt_id: "attempt-a".to_owned(),
        session_id: "session-a".to_owned(),
        turn_generation_id: "generation-a".to_owned(),
        observed_at: 11,
    }
    .record(&mut state)
    .unwrap();
    assert_eq!(
        state.acknowledgement("attempt-a").unwrap().unwrap().stage(),
        AcknowledgementStage::AcceptedPending
    );

    let stale = ManualAcknowledgementEvidence {
        evidence_id: "manual:stale".to_owned(),
        delivery_attempt_id: "attempt-a".to_owned(),
        session_id: "session-a".to_owned(),
        turn_generation_id: "generation-old".to_owned(),
        observed_at: 12,
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
        observed_at: 13,
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
        observed_at: 14,
    };
    assert!(matches!(
        wrong_attempt.record(&mut state),
        Err(SessionLifecycleError::Missing("delivery acknowledgement"))
    ));
}
