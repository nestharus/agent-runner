#![cfg(unix)]
//! AGE-166 end-to-end zero-turn orchestration coverage.
//!
//! ## Declared roles
//!
//! `validator`, `orchestration`, `formatter`
//!
//! Sequences `record_baseline` -> classifier -> `next_action` ->
//! `apply_terminal_signal_outcome` -> `confirm_maybe_quota_exhausted` against
//! a file-backed `StateDb` (orchestration). Validates the typed marker
//! shape, the durable `mark_exhausted` write-on-confirm contract, and the
//! productive-retry false-alarm clearance (validator). Builds in-memory
//! `ExecutionResult` + JSONL transcript rows (formatter).

use agent_runner_lib::terminal_outcome_adapter::{
    TerminalSignalContext, TerminalSignalDisposition, apply_terminal_signal_outcome,
    classify_error_category_with_fallback, confirm_maybe_quota_exhausted, terminal_signal_reason,
};
use agent_runner_lib::zero_turn_orchestration::{
    ZeroTurnAction, ZeroTurnClassification, ZeroTurnConfirmationState, classify_completion_delta,
    next_action, record_baseline,
};
use oulipoly_runtime::diagnostics::ErrorCategory;
use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;
use oulipoly_runtime::executor::{
    CapturedChildInvocation, ExecutionResult, SessionCaptureMethod, SessionCaptureResult,
    TerminalSignal,
};
use oulipoly_state::{SessionTurnCounts, SessionTurnIngest, StateDb};
use std::time::SystemTime;
use uuid::Uuid;

const PROVIDER: &str = "provider-a";
const SESSION_ID: &str = "session-1";

struct ZeroTurnFixture {
    db: StateDb,
    state: ZeroTurnConfirmationState,
    invocation_id: Uuid,
    session_uuid: Uuid,
}

impl ZeroTurnFixture {
    fn new() -> Self {
        let db = StateDb::open(std::path::Path::new(":memory:")).unwrap();
        db.upsert_quota_refresh(PROVIDER, &[]).unwrap();
        Self {
            db,
            state: ZeroTurnConfirmationState::default(),
            invocation_id: Uuid::nil(),
            session_uuid: Uuid::parse_str("5169694d-de0f-40d1-890c-6e28e55bab27").unwrap(),
        }
    }

    fn classify(&self, baseline: u64, current: u64) -> ZeroTurnClassification {
        let baseline = record_baseline(PROVIDER, Some(SESSION_ID), Some(counts(baseline)), false);
        classify_completion_delta(&baseline, counts(current))
    }

    fn apply_maybe(&self, signal: &TerminalSignal) -> (TerminalSignalDisposition, String) {
        self.apply_signal(signal, None)
    }

    fn apply_resume_maybe(&self, signal: &TerminalSignal) -> (TerminalSignalDisposition, String) {
        self.apply_signal(signal, Some(&self.session_uuid))
    }

    fn apply_signal(
        &self,
        signal: &TerminalSignal,
        session_id: Option<&Uuid>,
    ) -> (TerminalSignalDisposition, String) {
        let mut stderr = Vec::new();
        let mut ctx = TerminalSignalContext {
            invocation_id: &self.invocation_id,
            session_id,
            provider: PROVIDER,
            state_db: &self.db,
            stderr: &mut stderr,
        };
        let disposition = apply_terminal_signal_outcome(&Some(signal.clone()), &mut ctx);
        (disposition, String::from_utf8(stderr).unwrap())
    }

    fn confirm(&self, signal: &TerminalSignal) -> (&'static str, String) {
        self.confirm_with_session(signal, None)
    }

    fn confirm_resume(&self, signal: &TerminalSignal) -> (&'static str, String) {
        self.confirm_with_session(signal, Some(&self.session_uuid))
    }

    fn confirm_with_session(
        &self,
        signal: &TerminalSignal,
        session_id: Option<&Uuid>,
    ) -> (&'static str, String) {
        let mut stderr = Vec::new();
        let mut ctx = TerminalSignalContext {
            invocation_id: &self.invocation_id,
            session_id,
            provider: PROVIDER,
            state_db: &self.db,
            stderr: &mut stderr,
        };
        let category = confirm_maybe_quota_exhausted(signal, &mut ctx);
        (category, String::from_utf8(stderr).unwrap())
    }

    fn baseline_from_db(&self) -> agent_runner_lib::zero_turn_orchestration::ZeroTurnBaseline {
        let baseline_count = self.db.count_session_turns(PROVIDER, SESSION_ID).unwrap();
        record_baseline(PROVIDER, Some(SESSION_ID), Some(baseline_count), false)
    }

    fn append_assistant_turn(&self, turn_id: &str) {
        let turn = SessionTurnIngest {
            session_id: SESSION_ID.to_string(),
            turn_id: turn_id.to_string(),
            timestamp: chrono::DateTime::parse_from_rfc3339("2026-04-17T08:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            role: "assistant".to_string(),
            parent_turn_id: None,
            is_sidechain: false,
            is_compaction_boundary: false,
            body: None,
        };
        self.db
            .ingest_session_turns_batch(PROVIDER, &[turn])
            .unwrap();
    }

    fn exhausted_at_is_set(&self) -> bool {
        self.db
            .get_quota(PROVIDER)
            .unwrap()
            .unwrap()
            .exhausted_at
            .is_some()
    }
}

fn counts(assistant: u64) -> SessionTurnCounts {
    SessionTurnCounts {
        total: assistant,
        assistant,
        sidechain: 0,
    }
}

fn maybe_signal(classification: &ZeroTurnClassification) -> TerminalSignal {
    match classification {
        ZeroTurnClassification::MaybeQuotaExhausted { evidence } => TerminalSignal {
            kind: TerminalSignalKind::MaybeQuotaExhausted,
            provider_name: evidence.provider_name.clone(),
            evidence: evidence.evidence.clone(),
            observed_at: SystemTime::UNIX_EPOCH,
        },
        other => panic!("expected MaybeQuotaExhausted classification, got {other:?}"),
    }
}

fn execution_result_with_signal(
    signal: Option<TerminalSignalKind>,
    exit_code: i32,
) -> ExecutionResult {
    ExecutionResult {
        stdout: Vec::new(),
        stderr: "ordinary provider failure".to_string(),
        exit_code,
        provider_index: 0,
        session_capture: SessionCaptureResult {
            session_id: None,
            method: SessionCaptureMethod::None,
        },
        resume_acceptance: None,
        terminal_reason: signal
            .map(|kind| match kind {
                TerminalSignalKind::MaybeQuotaExhausted => "maybe_quota_exhausted",
                TerminalSignalKind::NonzeroExit => "exit_nonzero",
                _ => "typed_terminal_reason",
            })
            .map(str::to_string),
        terminal_signal: signal.map(|kind| TerminalSignal {
            kind,
            provider_name: PROVIDER.to_string(),
            evidence: "typed evidence".to_string(),
            observed_at: SystemTime::UNIX_EPOCH,
        }),
        captured_child_invocations: Vec::<CapturedChildInvocation>::new(),
        returned_artifacts: Vec::new(),
    }
}

#[test]
fn e2e_first_zero_turn_resumes_same_provider_without_exhausted_write() {
    let mut fixture = ZeroTurnFixture::new();

    let first = fixture.classify(0, 0);
    assert_eq!(
        next_action(&mut fixture.state, first.clone()),
        ZeroTurnAction::VerifySameProvider
    );
    let signal = maybe_signal(&first);
    let (disposition, marker) = fixture.apply_maybe(&signal);

    assert!(matches!(
        disposition,
        TerminalSignalDisposition::MaybeQuotaVerify
    ));
    assert!(marker.contains("OULIPOLY_TERMINAL_SIGNAL="), "{marker}");
    assert!(
        marker.contains("\"kind\":\"MaybeQuotaExhausted\""),
        "{marker}"
    );
    assert!(marker.contains("provider_session_id=session-1"), "{marker}");
    assert!(!fixture.exhausted_at_is_set());

    let second = fixture.classify(0, 1);
    assert_eq!(second, ZeroTurnClassification::Productive);
    assert_eq!(
        next_action(&mut fixture.state, second),
        ZeroTurnAction::Continue
    );
    let (disposition, marker) = fixture.apply_maybe(&TerminalSignal {
        kind: TerminalSignalKind::CleanExit,
        provider_name: PROVIDER.to_string(),
        evidence: "productive verification produced an assistant turn".to_string(),
        observed_at: SystemTime::UNIX_EPOCH,
    });
    assert!(matches!(
        disposition,
        TerminalSignalDisposition::InteractiveClean
    ));
    assert!(!marker.contains("MaybeQuotaExhausted"), "{marker}");
    assert!(!fixture.exhausted_at_is_set());
}

#[test]
fn e2e_second_zero_turn_confirms_quota_and_migrates() {
    let mut fixture = ZeroTurnFixture::new();

    let first = fixture.classify(0, 0);
    assert_eq!(
        next_action(&mut fixture.state, first.clone()),
        ZeroTurnAction::VerifySameProvider
    );
    let first_signal = maybe_signal(&first);
    let (disposition, _) = fixture.apply_maybe(&first_signal);
    assert!(matches!(
        disposition,
        TerminalSignalDisposition::MaybeQuotaVerify
    ));
    assert!(!fixture.exhausted_at_is_set());

    let second = fixture.classify(0, 0);
    assert_eq!(
        next_action(&mut fixture.state, second.clone()),
        ZeroTurnAction::ConfirmedExhaustion
    );
    let second_signal = maybe_signal(&second);
    let (category, marker) = fixture.confirm(&second_signal);

    assert_eq!(category, ErrorCategory::QuotaExhausted.as_str());
    assert!(fixture.exhausted_at_is_set());
    assert!(
        marker.contains("\"kind\":\"MaybeQuotaExhausted\""),
        "{marker}"
    );
    assert_eq!(
        terminal_signal_reason(&Some(second_signal), None),
        Some("maybe_quota_exhausted")
    );
}

#[test]
fn e2e_resume_zero_turn_then_zero_turn_confirms_quota() {
    let mut fixture = ZeroTurnFixture::new();

    let first = fixture.classify(7, 7);
    assert_eq!(
        next_action(&mut fixture.state, first.clone()),
        ZeroTurnAction::VerifySameProvider
    );
    let first_signal = maybe_signal(&first);
    let (disposition, marker) = fixture.apply_resume_maybe(&first_signal);
    assert!(matches!(
        disposition,
        TerminalSignalDisposition::MaybeQuotaVerify
    ));
    assert!(marker.contains("baseline_assistant_turns=7"), "{marker}");
    assert!(
        marker.contains("\"session_id\":\"5169694d-de0f-40d1-890c-6e28e55bab27\""),
        "{marker}"
    );
    assert!(!fixture.exhausted_at_is_set());

    let second = fixture.classify(7, 7);
    assert_eq!(
        next_action(&mut fixture.state, second.clone()),
        ZeroTurnAction::ConfirmedExhaustion
    );
    let second_signal = maybe_signal(&second);
    let (category, marker) = fixture.confirm_resume(&second_signal);

    assert_eq!(category, ErrorCategory::QuotaExhausted.as_str());
    assert!(fixture.exhausted_at_is_set());
    assert!(
        marker.contains("\"kind\":\"MaybeQuotaExhausted\""),
        "{marker}"
    );
    assert!(
        marker.contains("\"session_id\":\"5169694d-de0f-40d1-890c-6e28e55bab27\""),
        "{marker}"
    );
}

#[test]
fn e2e_resume_zero_turn_then_turn_clears_false_alarm() {
    let mut fixture = ZeroTurnFixture::new();

    let first = fixture.classify(3, 3);
    assert_eq!(
        next_action(&mut fixture.state, first.clone()),
        ZeroTurnAction::VerifySameProvider
    );
    let first_signal = maybe_signal(&first);
    let (disposition, marker) = fixture.apply_resume_maybe(&first_signal);
    assert!(matches!(
        disposition,
        TerminalSignalDisposition::MaybeQuotaVerify
    ));
    assert!(
        marker.contains("\"kind\":\"MaybeQuotaExhausted\""),
        "{marker}"
    );
    assert!(
        marker.contains("\"session_id\":\"5169694d-de0f-40d1-890c-6e28e55bab27\""),
        "{marker}"
    );

    let second = fixture.classify(3, 4);
    assert_eq!(second, ZeroTurnClassification::Productive);
    assert_eq!(
        next_action(&mut fixture.state, second),
        ZeroTurnAction::Continue
    );
    assert!(!fixture.exhausted_at_is_set());
}

#[test]
fn e2e_productive_turn_with_nonzero_exit_not_quota_exhausted() {
    let mut fixture = ZeroTurnFixture::new();

    let baseline = fixture.baseline_from_db();
    fixture.append_assistant_turn("turn-after-baseline");
    let current = fixture
        .db
        .count_session_turns(PROVIDER, SESSION_ID)
        .unwrap();
    let classification = classify_completion_delta(&baseline, current);
    assert_eq!(classification, ZeroTurnClassification::Productive);
    assert_eq!(
        next_action(&mut fixture.state, classification),
        ZeroTurnAction::Continue
    );

    let nonzero = execution_result_with_signal(Some(TerminalSignalKind::NonzeroExit), 1);
    let category = classify_error_category_with_fallback(&nonzero, || {
        Some(ErrorCategory::Unknown.as_str().to_string())
    });

    assert_eq!(category.as_deref(), Some(ErrorCategory::Unknown.as_str()));
    assert_eq!(nonzero.terminal_reason.as_deref(), Some("exit_nonzero"));
    assert!(!fixture.exhausted_at_is_set());
}

#[test]
fn e2e_interactive_maybe_signal_no_auto_relaunch() {
    let fixture = ZeroTurnFixture::new();
    let classification = fixture.classify(0, 0);
    let signal = maybe_signal(&classification);

    let (disposition, marker) = fixture.apply_maybe(&signal);

    assert!(matches!(
        disposition,
        TerminalSignalDisposition::MaybeQuotaVerify
    ));
    assert!(marker.contains("OULIPOLY_TERMINAL_SIGNAL="), "{marker}");
    assert!(
        marker.contains("\"kind\":\"MaybeQuotaExhausted\""),
        "{marker}"
    );
    assert!(!fixture.exhausted_at_is_set());
    assert_eq!(
        fixture.state,
        ZeroTurnConfirmationState::default(),
        "interactive/no-prompt path must not mutate same-provider relaunch state"
    );
}
