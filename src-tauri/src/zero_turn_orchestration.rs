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
//!       - ZeroTurnConfirmationState and ZeroTurnConfirmationKey
//!       - completion scan baseline bookkeeping
//!       - host-observed completion evidence
//!       - baseline/delta classification for assistant-turn productivity
//!       - exact-session accepted-provider-turn classification and cursor ordering
//!       - same-provider verification action selection
//!       - confirmed quota decision
//! ```

use oulipoly_runtime::executor::terminal_signal::build_zero_turn_evidence;
use oulipoly_runtime::sessions::AssistantCompletionRecord;
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
    pub baseline_assistant_completion: Option<AssistantCompletionRecord>,
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
pub struct AcceptedProviderTurnEvidence {
    pub provider_name: String,
    pub provider_session_id: String,
    pub baseline_assistant_turns: u64,
    pub current_assistant_turns: u64,
    pub baseline_completion: AssistantCompletionRecord,
    pub accepted_completion: AssistantCompletionRecord,
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
    record_baseline_with_completion(
        provider_name,
        provider_session_id,
        baseline_count,
        None,
        scan_failed,
    )
}

pub fn record_baseline_with_completion(
    provider_name: &str,
    provider_session_id: Option<&str>,
    baseline_count: Option<SessionTurnCounts>,
    baseline_assistant_completion: Option<AssistantCompletionRecord>,
    scan_failed: bool,
) -> ZeroTurnBaseline {
    let eligible = is_baseline_count_eligible(provider_session_id, scan_failed);
    ZeroTurnBaseline {
        provider_name: provider_name.to_string(),
        provider_session_id: provider_session_id.map(str::to_string),
        baseline_assistant_turns: if eligible {
            baseline_count.map(|counts| counts.assistant)
        } else {
            None
        },
        baseline_assistant_completion: eligible.then_some(baseline_assistant_completion).flatten(),
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

pub fn classify_accepted_provider_turn(
    baseline: &ZeroTurnBaseline,
    end_count: SessionTurnCounts,
    post_scan_failed: bool,
    current_assistant_completion: Option<&AssistantCompletionRecord>,
) -> Option<AcceptedProviderTurnEvidence> {
    let validated = validate_accepted_provider_turn(
        baseline,
        end_count,
        post_scan_failed,
        current_assistant_completion,
    )?;
    Some(map_accepted_provider_turn_evidence(baseline, validated))
}

pub fn is_incomplete_tool_boundary(
    baseline: &ZeroTurnBaseline,
    end_count: SessionTurnCounts,
    post_scan_failed: bool,
    current: Option<&AssistantCompletionRecord>,
) -> bool {
    if post_scan_failed {
        return false;
    }
    let Ok(delta) = validate_turn_delta(baseline, end_count) else {
        return false;
    };
    let (Some(baseline_completion), Some(current)) =
        (baseline.baseline_assistant_completion.as_ref(), current)
    else {
        return false;
    };
    delta.new_assistant_turns > 0
        && baseline_completion.session_id == delta.provider_session_id
        && current.session_id == delta.provider_session_id
        && assistant_completion_cursor_is_newer(current, baseline_completion)
        && current.completion_outcome.as_deref() == Some("tool-calls")
}

struct ValidatedAcceptedProviderTurn<'a> {
    delta: ValidatedTurnDelta<'a>,
    baseline_completion: &'a AssistantCompletionRecord,
    current_completion: &'a AssistantCompletionRecord,
}

fn validate_accepted_provider_turn<'a>(
    baseline: &'a ZeroTurnBaseline,
    end_count: SessionTurnCounts,
    post_scan_failed: bool,
    current: Option<&'a AssistantCompletionRecord>,
) -> Option<ValidatedAcceptedProviderTurn<'a>> {
    if post_scan_failed {
        return None;
    }
    let delta = validate_turn_delta(baseline, end_count).ok()?;
    if delta.new_assistant_turns == 0 {
        return None;
    }
    let baseline_completion = baseline.baseline_assistant_completion.as_ref()?;
    let current = current?;
    if baseline_completion.session_id != delta.provider_session_id
        || current.session_id != delta.provider_session_id
        || !assistant_completion_cursor_is_newer(current, baseline_completion)
        || current.completion_outcome.as_deref() != Some("stop")
    {
        return None;
    }
    Some(ValidatedAcceptedProviderTurn {
        delta,
        baseline_completion,
        current_completion: current,
    })
}

fn map_accepted_provider_turn_evidence(
    baseline: &ZeroTurnBaseline,
    validated: ValidatedAcceptedProviderTurn<'_>,
) -> AcceptedProviderTurnEvidence {
    AcceptedProviderTurnEvidence {
        provider_name: baseline.provider_name.clone(),
        provider_session_id: validated.delta.provider_session_id.to_string(),
        baseline_assistant_turns: validated.delta.baseline_assistant_turns,
        current_assistant_turns: validated.delta.current_assistant_turns,
        baseline_completion: validated.baseline_completion.clone(),
        accepted_completion: validated.current_completion.clone(),
    }
}

fn assistant_completion_cursor_is_newer(
    current: &AssistantCompletionRecord,
    baseline: &AssistantCompletionRecord,
) -> bool {
    current.timestamp > baseline.timestamp
        || (current.timestamp == baseline.timestamp && current.turn_id != baseline.turn_id)
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
        ZeroTurnConfirmationKey, ZeroTurnConfirmationState, ZeroTurnEvidence,
        classify_accepted_provider_turn, classify_completion, classify_completion_delta,
        is_incomplete_tool_boundary, next_action, record_baseline, record_baseline_with_completion,
    };
    use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;
    use oulipoly_runtime::sessions::AssistantCompletionRecord;
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

    fn completion(
        session_id: &str,
        turn_id: &str,
        timestamp: &str,
        outcome: Option<&str>,
    ) -> AssistantCompletionRecord {
        AssistantCompletionRecord {
            session_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            timestamp: chrono::DateTime::parse_from_rfc3339(timestamp)
                .unwrap()
                .with_timezone(&chrono::Utc),
            completion_outcome: outcome.map(str::to_string),
        }
    }

    fn completion_baseline() -> ZeroTurnBaseline {
        record_baseline_with_completion(
            "provider-a",
            Some("session-1"),
            Some(counts(3)),
            Some(completion(
                "session-1",
                "turn-3",
                "2026-04-17T08:00:00Z",
                None,
            )),
            false,
        )
    }

    #[test]
    fn accepted_provider_turn_requires_new_same_session_stop_cursor() {
        let current = completion("session-1", "turn-4", "2026-04-17T08:00:01Z", Some("stop"));

        let evidence = classify_accepted_provider_turn(
            &completion_baseline(),
            counts(4),
            false,
            Some(&current),
        )
        .expect("new exact-session stop should be accepted");

        assert_eq!(evidence.provider_session_id, "session-1");
        assert_eq!(evidence.baseline_completion.turn_id, "turn-3");
        assert_eq!(evidence.accepted_completion.turn_id, "turn-4");
        assert_eq!(
            evidence.accepted_completion.completion_outcome.as_deref(),
            Some("stop")
        );
    }

    #[test]
    fn accepted_provider_turn_allows_distinct_cursor_at_equal_timestamp() {
        let current = completion("session-1", "turn-4", "2026-04-17T08:00:00Z", Some("stop"));

        let evidence = classify_accepted_provider_turn(
            &completion_baseline(),
            counts(4),
            false,
            Some(&current),
        );

        assert!(evidence.is_some());
    }

    #[test]
    fn provider_turn_acceptance_fails_closed_for_scan_cursor_and_outcome_evidence() {
        let baseline = completion_baseline();
        let cases = [
            None,
            Some(completion(
                "session-1",
                "turn-3",
                "2026-04-17T08:00:00Z",
                Some("stop"),
            )),
            Some(completion(
                "session-2",
                "turn-4",
                "2026-04-17T08:00:01Z",
                Some("stop"),
            )),
            Some(completion(
                "session-1",
                "turn-4",
                "2026-04-17T08:00:01Z",
                None,
            )),
            Some(completion(
                "session-1",
                "turn-4",
                "2026-04-17T08:00:01Z",
                Some(""),
            )),
            Some(completion(
                "session-1",
                "turn-4",
                "2026-04-17T08:00:01Z",
                Some("tool-calls"),
            )),
            Some(completion(
                "session-1",
                "turn-4",
                "2026-04-17T08:00:01Z",
                Some("error"),
            )),
        ];

        for current in cases {
            let evidence =
                classify_accepted_provider_turn(&baseline, counts(4), false, current.as_ref());
            assert!(evidence.is_none(), "unexpected acceptance for {current:?}");
        }

        let current = completion("session-1", "turn-4", "2026-04-17T08:00:01Z", Some("stop"));
        assert!(
            classify_accepted_provider_turn(&baseline, counts(4), true, Some(&current)).is_none(),
            "degraded post-scan must not be accepted"
        );
    }

    #[test]
    fn incomplete_tool_boundary_requires_new_exact_session_tool_call_cursor() {
        let current = completion(
            "session-1",
            "turn-4",
            "2026-04-17T08:00:01Z",
            Some("tool-calls"),
        );

        assert!(is_incomplete_tool_boundary(
            &completion_baseline(),
            counts(4),
            false,
            Some(&current),
        ));
        assert!(!is_incomplete_tool_boundary(
            &completion_baseline(),
            counts(4),
            true,
            Some(&current),
        ));
        assert!(!is_incomplete_tool_boundary(
            &completion_baseline(),
            counts(3),
            false,
            Some(&current),
        ));
    }

    #[test]
    fn provider_turn_acceptance_requires_usable_baseline_and_positive_delta() {
        let no_cursor_baseline = record_baseline_with_completion(
            "provider-a",
            Some("session-1"),
            Some(counts(0)),
            None,
            false,
        );
        let current = completion("session-1", "turn-1", "2026-04-17T08:00:01Z", Some("stop"));

        assert!(
            classify_accepted_provider_turn(&no_cursor_baseline, counts(1), false, Some(&current))
                .is_none()
        );
        assert!(
            classify_accepted_provider_turn(
                &completion_baseline(),
                counts(3),
                false,
                Some(&current)
            )
            .is_none()
        );
        assert!(
            classify_accepted_provider_turn(
                &completion_baseline(),
                counts(2),
                false,
                Some(&current)
            )
            .is_none()
        );
    }

    #[test]
    fn record_baseline_captures_pre_session_assistant_count() {
        let baseline = record_baseline("claude-a", Some("session-1"), Some(counts(7)), false);

        assert_eq!(baseline.provider_name, "claude-a");
        assert_eq!(baseline.provider_session_id.as_deref(), Some("session-1"));
        assert_eq!(baseline.baseline_assistant_turns, Some(7));
        assert_eq!(baseline.baseline_assistant_completion, None);
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
