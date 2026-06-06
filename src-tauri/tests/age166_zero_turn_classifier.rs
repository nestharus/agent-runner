//! AGE-166 unit-level zero-turn classifier coverage.
//!
//! ## Declared roles
//!
//! `validator`, `mapper`
//!
//! Drives `zero_turn_orchestration::record_baseline` +
//! `classify_completion_delta` + `next_action` plus the
//! `terminal_outcome_adapter::apply_terminal_signal_outcome` disposition for
//! the `MaybeQuotaExhausted` kind (mapper from signal kind to disposition;
//! validator over the classifier's Unclassified-no-session-id and
//! Unclassified-scan-failed branches).

use agent_runner_lib::terminal_outcome_adapter::{
    TerminalSignalContext, TerminalSignalDisposition, apply_terminal_signal_outcome,
    classify_error_category_with_fallback,
};
use agent_runner_lib::zero_turn_orchestration::{
    ZeroTurnAction, ZeroTurnClassification, classify_completion_delta, next_action, record_baseline,
};
use oulipoly_runtime::diagnostics::ErrorCategory;
use oulipoly_runtime::executor::{
    CapturedChildInvocation, ExecutionResult, SessionCaptureMethod, SessionCaptureResult,
};
use oulipoly_state::{SessionTurnCounts, StateDb};
use uuid::Uuid;

fn counts(assistant: u64) -> SessionTurnCounts {
    SessionTurnCounts {
        total: assistant,
        assistant,
        sidechain: 0,
    }
}

fn in_memory_db_with_provider(provider: &str) -> StateDb {
    let db = StateDb::open(std::path::Path::new(":memory:")).unwrap();
    db.upsert_quota_refresh(provider, &[]).unwrap();
    db
}

fn apply_absent_signal(db: &StateDb, provider: &str) -> (TerminalSignalDisposition, String) {
    let invocation_id = Uuid::nil();
    let mut stderr = Vec::new();
    let mut ctx = TerminalSignalContext {
        invocation_id: &invocation_id,
        session_id: None,
        provider,
        state_db: db,
        stderr: &mut stderr,
    };
    let disposition = apply_terminal_signal_outcome(&None, &mut ctx);
    (disposition, String::from_utf8(stderr).unwrap())
}

fn nonzero_result_without_typed_signal() -> ExecutionResult {
    ExecutionResult {
        stdout: Vec::new(),
        stderr: "ordinary provider failure".to_string(),
        exit_code: 1,
        provider_index: 0,
        session_capture: SessionCaptureResult {
            session_id: None,
            method: SessionCaptureMethod::None,
        },
        resume_acceptance: None,
        terminal_reason: Some("exit_nonzero".to_string()),
        terminal_signal: None,
        submitted_user_turn: None,
        captured_child_invocations: Vec::<CapturedChildInvocation>::new(),
        returned_artifacts: Vec::new(),
    }
}

#[test]
fn provider_without_session_id_does_not_emit_maybe_quota() {
    let provider = "claude-a";
    let db = in_memory_db_with_provider(provider);
    let baseline = record_baseline(provider, None, Some(counts(0)), false);

    let classification = classify_completion_delta(&baseline, counts(0));
    assert_eq!(
        classification,
        ZeroTurnClassification::UnclassifiedNoSessionId
    );
    assert_eq!(
        next_action(&mut Default::default(), classification),
        ZeroTurnAction::Unclassified
    );

    let (disposition, marker) = apply_absent_signal(&db, provider);
    assert!(matches!(
        disposition,
        TerminalSignalDisposition::NotApplicable
    ));
    assert!(!marker.contains("OULIPOLY_TERMINAL_SIGNAL="), "{marker}");
    assert!(!marker.contains("MaybeQuotaExhausted"), "{marker}");
    assert_eq!(db.get_quota(provider).unwrap().unwrap().exhausted_at, None);
}

#[test]
fn completion_scan_failure_does_not_synthesize_quota() {
    let provider = "claude-a";
    let db = in_memory_db_with_provider(provider);
    let baseline = record_baseline(provider, Some("session-1"), Some(counts(0)), true);

    let classification = classify_completion_delta(&baseline, counts(0));
    assert_eq!(
        classification,
        ZeroTurnClassification::UnclassifiedScanFailed
    );
    assert_eq!(
        next_action(&mut Default::default(), classification),
        ZeroTurnAction::Unclassified
    );

    let (disposition, marker) = apply_absent_signal(&db, provider);
    let category =
        classify_error_category_with_fallback(&nonzero_result_without_typed_signal(), || {
            Some(ErrorCategory::Unknown.as_str().to_string())
        });

    assert!(matches!(
        disposition,
        TerminalSignalDisposition::NotApplicable
    ));
    assert_eq!(category.as_deref(), Some(ErrorCategory::Unknown.as_str()));
    assert!(!marker.contains("OULIPOLY_TERMINAL_SIGNAL="), "{marker}");
    assert!(!marker.contains("MaybeQuotaExhausted"), "{marker}");
    assert_eq!(db.get_quota(provider).unwrap().unwrap().exhausted_at, None);
}
