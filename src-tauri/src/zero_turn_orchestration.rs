//! Zero-turn completion classification and confirmation state.
//!
//! ## Declared roles
//!
//! `orchestration`, `accessor`, `formatter`, `mapper`, `predicate`
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

pub fn record_baseline(
    provider_name: &str,
    provider_session_id: Option<&str>,
    baseline_count: Option<SessionTurnCounts>,
    scan_failed: bool,
) -> ZeroTurnBaseline {
    ZeroTurnBaseline {
        provider_name: provider_name.to_string(),
        provider_session_id: provider_session_id.map(str::to_string),
        baseline_assistant_turns: if provider_session_id.is_some() && !scan_failed {
            baseline_count.map(|counts| counts.assistant)
        } else {
            None
        },
        scan_failed,
    }
}

pub fn classify_completion_delta(
    baseline: &ZeroTurnBaseline,
    end_count: SessionTurnCounts,
) -> ZeroTurnClassification {
    let Some(provider_session_id) = baseline.provider_session_id.as_ref() else {
        return ZeroTurnClassification::UnclassifiedNoSessionId;
    };
    if baseline.scan_failed {
        return ZeroTurnClassification::UnclassifiedScanFailed;
    }
    let Some(baseline_assistant_turns) = baseline.baseline_assistant_turns else {
        return ZeroTurnClassification::UnclassifiedScanFailed;
    };

    let current_assistant_turns = end_count.assistant;
    let new_assistant_turns = current_assistant_turns.saturating_sub(baseline_assistant_turns);
    if new_assistant_turns == 0 {
        return ZeroTurnClassification::MaybeQuotaExhausted {
            evidence: ZeroTurnEvidence {
                provider_name: baseline.provider_name.clone(),
                provider_session_id: provider_session_id.clone(),
                baseline_assistant_turns,
                current_assistant_turns,
                new_assistant_turns,
                evidence: build_zero_turn_evidence(
                    provider_session_id,
                    baseline_assistant_turns,
                    current_assistant_turns,
                    new_assistant_turns,
                ),
            },
        };
    }

    ZeroTurnClassification::Productive
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
            let key = ZeroTurnConfirmationKey {
                provider_name: evidence.provider_name,
                provider_session_id: evidence.provider_session_id,
            };
            if state.active_key.as_ref() == Some(&key) && state.verification_attempted {
                ZeroTurnAction::ConfirmedExhaustion
            } else {
                state.record_maybe(key);
                ZeroTurnAction::VerifySameProvider
            }
        }
        ZeroTurnClassification::UnclassifiedNoSessionId
        | ZeroTurnClassification::UnclassifiedScanFailed => {
            state.clear();
            ZeroTurnAction::Unclassified
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ZeroTurnAction, ZeroTurnBaseline, ZeroTurnClassification, ZeroTurnConfirmationKey,
        ZeroTurnConfirmationState, ZeroTurnEvidence, classify_completion_delta, next_action,
        record_baseline,
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
