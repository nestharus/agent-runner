//! Zero-turn completion classification and confirmation state.
//!
//! ## Declared roles
//!
//! `orchestration`, `accessor`, `formatter`, `mapper`, `predicate`, `validator`
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/src/zero_turn_orchestration.rs
//!     role: intrinsic-surface
//!     Domain: zero_turn_confirmation
//!     Owns:
//!       - ZeroTurnConfirmationState
//!       - provider_session confirmation key
//!       - completion scan baseline bookkeeping
//!       - host-observed completion evidence
//!       - baseline/delta classification
//!       - same-provider verification planning
//!       - confirmed quota decision
//! ```

use oulipoly_runtime::executor::terminal_signal::build_zero_turn_evidence;
use oulipoly_state::SessionTurnCounts;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ZeroTurnConfirmationKey {
    pub provider_name: String,
    pub provider_session_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ZeroTurnConfirmationState {
    pub active_key: Option<ZeroTurnConfirmationKey>,
    pub verification_attempted: bool,
}

impl ZeroTurnConfirmationState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_maybe(&mut self, key: ZeroTurnConfirmationKey) {
        self.active_key = Some(key);
        self.verification_attempted = true;
    }

    pub fn clear(&mut self) {
        self.active_key = None;
        self.verification_attempted = false;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZeroTurnBaseline {
    pub provider_name: String,
    pub provider_session_id: Option<String>,
    pub baseline_assistant_turns: Option<u64>,
    pub scan_failed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZeroTurnEvidence {
    pub provider_name: String,
    pub provider_session_id: String,
    pub baseline_assistant_turns: u64,
    pub current_assistant_turns: u64,
    pub new_assistant_turns: u64,
    pub evidence: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostObservedCompletion {
    pub clean_terminal_signal: bool,
    pub produced_assistant_response: bool,
}

impl HostObservedCompletion {
    pub fn new(clean_terminal_signal: bool, produced_assistant_response: bool) -> Self {
        Self {
            clean_terminal_signal,
            produced_assistant_response,
        }
    }

    fn confirms_productive_turn(self) -> bool {
        self.clean_terminal_signal && self.produced_assistant_response
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ZeroTurnClassification {
    Productive,
    MaybeQuotaExhausted { evidence: ZeroTurnEvidence },
    UnclassifiedNoSessionId,
    UnclassifiedScanFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZeroTurnAction {
    Continue,
    VerifySameProvider,
    ConfirmedExhaustion,
    Unclassified,
}

fn is_baseline_count_eligible(provider_session_id: Option<&str>, scan_failed: bool) -> bool {
    provider_session_id.is_some() && !scan_failed
}

pub fn record_baseline(
    provider_name: &str,
    provider_session_id: Option<&str>,
    baseline_count: Option<SessionTurnCounts>,
    scan_failed: bool,
) -> ZeroTurnBaseline {
    ZeroTurnBaseline {
        provider_name: provider_name.to_string(),
        provider_session_id: provider_session_id.map(str::to_string),
        baseline_assistant_turns: if is_baseline_count_eligible(provider_session_id, scan_failed) {
            baseline_count.map(|counts| counts.assistant)
        } else {
            None
        },
        scan_failed,
    }
}

fn no_new_turns_produced(new_assistant_turns: u64) -> bool {
    new_assistant_turns == 0
}

fn build_maybe_quota_exhausted_evidence(
    baseline: &ZeroTurnBaseline,
    provider_session_id: &str,
    baseline_assistant_turns: u64,
    current_assistant_turns: u64,
    new_assistant_turns: u64,
) -> ZeroTurnEvidence {
    ZeroTurnEvidence {
        provider_name: baseline.provider_name.clone(),
        provider_session_id: provider_session_id.to_string(),
        baseline_assistant_turns,
        current_assistant_turns,
        new_assistant_turns,
        evidence: build_zero_turn_evidence(
            provider_session_id,
            baseline_assistant_turns,
            current_assistant_turns,
            new_assistant_turns,
        ),
    }
}

struct ValidatedTurnDelta<'a> {
    provider_session_id: &'a str,
    baseline_assistant_turns: u64,
    current_assistant_turns: u64,
    new_assistant_turns: u64,
}

/// Resolve the turn-count preconditions (session id, baseline, non-regressing
/// delta) for a scanned completion, or report the `Unclassified*` outcome that
/// the precondition failure implies.
fn validate_turn_delta(
    baseline: &ZeroTurnBaseline,
    end_count: SessionTurnCounts,
) -> Result<ValidatedTurnDelta<'_>, ZeroTurnClassification> {
    let Some(provider_session_id) = baseline.provider_session_id.as_deref() else {
        return Err(ZeroTurnClassification::UnclassifiedNoSessionId);
    };
    if baseline.scan_failed {
        return Err(ZeroTurnClassification::UnclassifiedScanFailed);
    }
    let Some(baseline_assistant_turns) = baseline.baseline_assistant_turns else {
        return Err(ZeroTurnClassification::UnclassifiedScanFailed);
    };
    let current_assistant_turns = end_count.assistant;
    let Some(new_assistant_turns) = current_assistant_turns.checked_sub(baseline_assistant_turns)
    else {
        return Err(ZeroTurnClassification::UnclassifiedScanFailed);
    };
    Ok(ValidatedTurnDelta {
        provider_session_id,
        baseline_assistant_turns,
        current_assistant_turns,
        new_assistant_turns,
    })
}

/// Map a validated turn-count delta onto its domain classification: no new
/// assistant turns is the speculative `MaybeQuotaExhausted`, otherwise `Productive`.
fn classify_validated_turn_delta(
    baseline: &ZeroTurnBaseline,
    delta: ValidatedTurnDelta<'_>,
) -> ZeroTurnClassification {
    if no_new_turns_produced(delta.new_assistant_turns) {
        return ZeroTurnClassification::MaybeQuotaExhausted {
            evidence: build_maybe_quota_exhausted_evidence(
                baseline,
                delta.provider_session_id,
                delta.baseline_assistant_turns,
                delta.current_assistant_turns,
                delta.new_assistant_turns,
            ),
        };
    }
    ZeroTurnClassification::Productive
}

pub fn classify_completion_delta(
    baseline: &ZeroTurnBaseline,
    end_count: SessionTurnCounts,
) -> ZeroTurnClassification {
    match validate_turn_delta(baseline, end_count) {
        Ok(delta) => classify_validated_turn_delta(baseline, delta),
        Err(classification) => classification,
    }
}

/// Classify completion productivity, treating clean host-observed assistant
/// output as authoritative `Productive` and otherwise deferring to the
/// session-store delta. The delta path is ground truth — a real assistant-turn
/// delta overrides a speculative non-clean signal (e.g. `MaybeQuotaExhausted`),
/// so it is never narrowed here.
pub fn classify_completion(
    baseline: &ZeroTurnBaseline,
    end_count: SessionTurnCounts,
    host_observed: HostObservedCompletion,
) -> ZeroTurnClassification {
    if host_observed.confirms_productive_turn() {
        return ZeroTurnClassification::Productive;
    }
    classify_completion_delta(baseline, end_count)
}

fn confirmation_key_from_evidence(evidence: ZeroTurnEvidence) -> ZeroTurnConfirmationKey {
    ZeroTurnConfirmationKey {
        provider_name: evidence.provider_name,
        provider_session_id: evidence.provider_session_id,
    }
}

fn is_same_provider_confirmed(
    state: &ZeroTurnConfirmationState,
    key: &ZeroTurnConfirmationKey,
) -> bool {
    state.active_key.as_ref() == Some(key) && state.verification_attempted
}

pub fn next_action(
    state: &mut ZeroTurnConfirmationState,
    classification: ZeroTurnClassification,
) -> ZeroTurnAction {
    match classification {
        ZeroTurnClassification::Productive => {
            state.clear();
            ZeroTurnAction::Continue
        }
        ZeroTurnClassification::MaybeQuotaExhausted { evidence } => {
            next_action_for_maybe_quota(state, evidence)
        }
        ZeroTurnClassification::UnclassifiedNoSessionId
        | ZeroTurnClassification::UnclassifiedScanFailed => {
            state.clear();
            ZeroTurnAction::Unclassified
        }
    }
}

fn next_action_for_maybe_quota(
    state: &mut ZeroTurnConfirmationState,
    evidence: ZeroTurnEvidence,
) -> ZeroTurnAction {
    let key = confirmation_key_from_evidence(evidence);
    if is_same_provider_confirmed(state, &key) {
        return ZeroTurnAction::ConfirmedExhaustion;
    }
    state.record_maybe(key);
    ZeroTurnAction::VerifySameProvider
}

#[cfg(test)]
mod tests {
    use super::{
        HostObservedCompletion, ZeroTurnAction, ZeroTurnBaseline, ZeroTurnClassification,
        ZeroTurnConfirmationKey, ZeroTurnConfirmationState, ZeroTurnEvidence, classify_completion,
        classify_completion_delta, next_action, record_baseline,
    };
    use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;
    use oulipoly_state::SessionTurnCounts;

    fn counts(assistant: u64) -> SessionTurnCounts {
        SessionTurnCounts {
            total: assistant,
            assistant,
            sidechain: 0,
        }
    }

    fn baseline(provider_session_id: Option<&str>, assistant: Option<u64>) -> ZeroTurnBaseline {
        record_baseline(
            "claude-a",
            provider_session_id,
            assistant.map(counts),
            false,
        )
    }

    #[test]
    fn record_baseline_captures_pre_session_assistant_count() {
        let baseline = record_baseline("claude-a", Some("session-1"), Some(counts(7)), false);

        assert_eq!(baseline.provider_name, "claude-a");
        assert_eq!(baseline.provider_session_id.as_deref(), Some("session-1"));
        assert_eq!(baseline.baseline_assistant_turns, Some(7));
        assert!(!baseline.scan_failed);
    }

    #[test]
    fn classify_completion_delta_zero_new_emits_maybe_quota_exhausted() {
        let classification =
            classify_completion_delta(&baseline(Some("session-1"), Some(3)), counts(3));

        match classification {
            ZeroTurnClassification::MaybeQuotaExhausted { evidence } => {
                assert_eq!(evidence.provider_name, "claude-a");
                assert_eq!(evidence.provider_session_id, "session-1");
                assert_eq!(evidence.baseline_assistant_turns, 3);
                assert_eq!(evidence.current_assistant_turns, 3);
                assert_eq!(evidence.new_assistant_turns, 0);
                assert!(evidence.evidence.contains("new_assistant_turns=0"));
                assert_eq!(
                    TerminalSignalKind::MaybeQuotaExhausted,
                    TerminalSignalKind::MaybeQuotaExhausted
                );
            }
            other => panic!("expected maybe quota classification, got {other:?}"),
        }
    }

    #[test]
    fn zero_turn_evidence_type_field_addressable() {
        let classification =
            classify_completion_delta(&baseline(Some("session-1"), Some(3)), counts(3));

        let ZeroTurnClassification::MaybeQuotaExhausted { evidence } = classification else {
            panic!("expected maybe quota classification");
        };
        let evidence: ZeroTurnEvidence = evidence;

        assert_eq!(evidence.provider_session_id, "session-1");
        assert_eq!(evidence.baseline_assistant_turns, 3);
    }

    #[test]
    fn classify_completion_delta_one_or_more_new_is_productive() {
        let classification =
            classify_completion_delta(&baseline(Some("session-1"), Some(3)), counts(4));

        assert_eq!(classification, ZeroTurnClassification::Productive);
    }

    #[test]
    fn classify_completion_host_productive_overrides_zero_delta() {
        let classification = classify_completion(
            &baseline(Some("session-1"), Some(3)),
            counts(3),
            HostObservedCompletion::new(true, true),
        );

        assert_eq!(classification, ZeroTurnClassification::Productive);
    }

    #[test]
    fn classify_completion_host_productive_overrides_scan_failure() {
        let baseline = record_baseline("claude-a", Some("session-1"), Some(counts(3)), true);
        let classification = classify_completion(
            &baseline,
            counts(0),
            HostObservedCompletion::new(true, true),
        );

        assert_eq!(classification, ZeroTurnClassification::Productive);
    }

    #[test]
    fn classify_completion_not_productive_zero_delta_stays_maybe_quota() {
        let classification = classify_completion(
            &baseline(Some("session-1"), Some(3)),
            counts(3),
            HostObservedCompletion::new(true, false),
        );

        assert!(matches!(
            classification,
            ZeroTurnClassification::MaybeQuotaExhausted { .. }
        ));
    }

    #[test]
    fn classify_completion_non_clean_signal_is_not_productive() {
        let classification = classify_completion(
            &baseline(Some("session-1"), Some(3)),
            counts(3),
            HostObservedCompletion::new(false, true),
        );

        assert_ne!(classification, ZeroTurnClassification::Productive);
    }

    #[test]
    fn classify_completion_scan_delta_overrides_speculative_non_clean_signal() {
        // A real assistant-turn delta (+1) is ground truth that the turn
        // produced output, so it stays Productive even when the host-observed
        // signal was non-clean (e.g. a speculative MaybeQuotaExhausted). The
        // delta path must not be narrowed by host-observed evidence.
        let classification = classify_completion(
            &baseline(Some("session-1"), Some(3)),
            counts(4),
            HostObservedCompletion::new(false, false),
        );

        assert_eq!(classification, ZeroTurnClassification::Productive);
    }

    #[test]
    fn classify_completion_delta_turn_count_regression_is_unclassified() {
        let classification =
            classify_completion_delta(&baseline(Some("session-1"), Some(3)), counts(2));

        assert_eq!(
            classification,
            ZeroTurnClassification::UnclassifiedScanFailed
        );
    }

    #[test]
    fn classify_completion_delta_no_session_id_is_unclassified() {
        let classification = classify_completion_delta(&baseline(None, Some(3)), counts(3));

        assert_eq!(
            classification,
            ZeroTurnClassification::UnclassifiedNoSessionId
        );
    }

    #[test]
    fn classify_completion_delta_scan_failure_is_unclassified() {
        let baseline = record_baseline("claude-a", Some("session-1"), Some(counts(3)), true);
        let classification = classify_completion_delta(&baseline, counts(3));

        assert_eq!(
            classification,
            ZeroTurnClassification::UnclassifiedScanFailed
        );
    }

    #[test]
    fn next_action_first_maybe_returns_verify_same_provider() {
        let mut state = ZeroTurnConfirmationState::default();
        let classification =
            classify_completion_delta(&baseline(Some("session-1"), Some(0)), counts(0));

        assert_eq!(
            next_action(&mut state, classification),
            ZeroTurnAction::VerifySameProvider
        );
        assert!(state.verification_attempted);
    }

    #[test]
    fn zero_turn_confirmation_key_field_addressable() {
        let expected_key = ZeroTurnConfirmationKey {
            provider_name: "claude-a".to_string(),
            provider_session_id: "session-1".to_string(),
        };
        assert_eq!(expected_key.provider_name, "claude-a");
        assert_eq!(expected_key.provider_session_id, "session-1");

        let mut state = ZeroTurnConfirmationState::default();
        let classification =
            classify_completion_delta(&baseline(Some("session-1"), Some(0)), counts(0));

        assert_eq!(
            next_action(&mut state, classification),
            ZeroTurnAction::VerifySameProvider
        );
        assert_eq!(state.active_key.as_ref(), Some(&expected_key));
    }

    #[test]
    fn next_action_second_maybe_returns_confirmed_exhaustion() {
        let mut state = ZeroTurnConfirmationState::default();
        let first = classify_completion_delta(&baseline(Some("session-1"), Some(0)), counts(0));
        assert_eq!(
            next_action(&mut state, first),
            ZeroTurnAction::VerifySameProvider
        );
        let second = classify_completion_delta(&baseline(Some("session-1"), Some(0)), counts(0));

        assert_eq!(
            next_action(&mut state, second),
            ZeroTurnAction::ConfirmedExhaustion
        );
    }

    #[test]
    fn next_action_one_or_more_returns_continue() {
        let mut state = ZeroTurnConfirmationState::default();
        let classification =
            classify_completion_delta(&baseline(Some("session-1"), Some(0)), counts(1));

        assert_eq!(
            next_action(&mut state, classification),
            ZeroTurnAction::Continue
        );
    }

    #[test]
    fn next_action_unclassified_returns_unclassified() {
        let mut state = ZeroTurnConfirmationState::default();
        let classification = classify_completion_delta(&baseline(None, Some(0)), counts(0));

        assert_eq!(
            next_action(&mut state, classification),
            ZeroTurnAction::Unclassified
        );
    }
}
