//! Main-side zero-turn completion-classification adapters.
//!
//! Relocated from `src-tauri/src/main.rs` by AGE-204 (map row H13). Output-preserving.
//! These adapters wrap the pure classification core in `crate::zero_turn_orchestration`,
//! adding the state-DB session scan, baseline capture, and `ExecutionResult` signal
//! mutation that the main-side balancing/resume orchestrators drive.
//!
//! ## Declared roles
//!
//! `orchestration`, `mapper`, `predicate`, `accessor`, `formatter`
//!
//! - `orchestration`: `zero_turn_record_baseline` / `classify_after_completion`
//!   sequence execution, session scans, turn-count reads, and core classification.
//! - `mapper`: `apply_resume_completion_action` / `completion_classification` /
//!   `zero_turn_classification_for_action` / `apply_zero_turn_classification_to_*`
//!   project classifications onto assessment and executor fields.
//! - `predicate`: `recovered_generic_nonzero` /
//!   `zero_turn_completion_can_replace_signal` /
//!   `zero_turn_classification_is_non_productive` / `is_confirmed_zero_turn_exhaustion`.
//! - `accessor`: `provider_has_no_session_source` / `has_session_source` /
//!   `baseline_turn_count_from_scan` read session-source + turn-count state.
//! - `formatter`: `maybe_quota_exhausted_reason` builds the terminal-reason string.
//!
//! ## Adapter declarations
//!
//! This component is a coupling adapter: it bridges the pure zero-turn
//! classification core to the main-side execution context (session scan,
//! turn-count state, execution-result/terminal-signal mutation). Every external
//! reference is subordinate to one of the declared `Translates:` contracts. The
//! authoritative carrier is the Step 6a contract; this block mirrors it.
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/quota_zero_turn/completion_classification.rs
//!     role: adapter
//!     Translates:
//!       - zero-turn-classification-core-contract
//!       - executor-result-productivity-contract
//!       - state-db-session-turn-count-contract
//!       - sessions-config-contract
//!       - provider-session-scan-contract
//! ```

use oulipoly_runtime::executor;
use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;
use oulipoly_state::StateDb;

use crate::zero_turn_orchestration::{
    HostObservedCompletion, ZeroTurnAction, ZeroTurnBaseline, ZeroTurnClassification,
    ZeroTurnEvidence, classify_accepted_provider_turn, classify_completion,
    classify_completion_delta, is_incomplete_tool_boundary, record_baseline,
    record_baseline_with_completion,
};
fn zero_turn_zero_counts() -> oulipoly_state::SessionTurnCounts {
    session_turn_counts(0, 0, 0)
}

fn session_turn_counts(
    total: u64,
    assistant: u64,
    sidechain: u64,
) -> oulipoly_state::SessionTurnCounts {
    oulipoly_state::SessionTurnCounts {
        total,
        assistant,
        sidechain,
    }
}

fn provider_has_no_session_source(
    sessions_cfg: &oulipoly_config::SessionsConfig,
    provider_name: &str,
) -> bool {
    sessions_cfg.get(provider_name).is_none()
}

fn has_session_source(sessions_cfg: &oulipoly_config::SessionsConfig, provider_name: &str) -> bool {
    !provider_has_no_session_source(sessions_cfg, provider_name)
}

fn scan_report_has_errors(report: &oulipoly_runtime::sessions::ScanReport) -> bool {
    !report.errors.is_empty()
}

fn baseline_turn_count_from_scan(
    state: &StateDb,
    provider_name: &str,
    session_id: &str,
    scan_failed: bool,
) -> Option<oulipoly_state::SessionTurnCounts> {
    if scan_failed {
        None
    } else {
        state.count_session_turns(provider_name, session_id).ok()
    }
}

fn turn_counts_or_scan_failed<E>(
    count_result: Result<oulipoly_state::SessionTurnCounts, E>,
) -> Option<oulipoly_state::SessionTurnCounts> {
    count_result.ok()
}

pub(crate) fn zero_turn_record_baseline(
    state: &StateDb,
    sessions_cfg: &oulipoly_config::SessionsConfig,
    provider_name: &str,
    provider_session_id: Option<&str>,
) -> ZeroTurnBaseline {
    let Some(session_id) = provider_session_id else {
        return record_baseline(provider_name, None, None, false);
    };
    if provider_has_no_session_source(sessions_cfg, provider_name) {
        return record_baseline(provider_name, Some(session_id), None, true);
    }
    let report = oulipoly_runtime::sessions::scan_provider_session(
        provider_name,
        sessions_cfg,
        state,
        session_id,
    );
    let scan_failed = scan_report_has_errors(&report);
    let baseline_count =
        baseline_turn_count_from_scan(state, provider_name, session_id, scan_failed);
    let baseline_completion = report.assistant_completions.get(session_id).cloned();
    record_baseline_with_completion(
        provider_name,
        Some(session_id),
        baseline_count,
        baseline_completion,
        scan_failed,
    )
}

pub(crate) fn zero_turn_classify_after_completion(
    state: &StateDb,
    sessions_cfg: &oulipoly_config::SessionsConfig,
    baseline: &ZeroTurnBaseline,
    host_observed: HostObservedCompletion,
) -> ZeroTurnClassification {
    classify_after_completion(state, sessions_cfg, baseline, host_observed).classification
}

pub(crate) fn zero_turn_classify_after_completion_with_recovery(
    state: &StateDb,
    sessions_cfg: &oulipoly_config::SessionsConfig,
    baseline: &ZeroTurnBaseline,
    host_observed: HostObservedCompletion,
    result: &executor::ExecutionResult,
) -> CompletionClassificationOutput {
    classify_completion_with_recovery(state, sessions_cfg, baseline, host_observed, result)
}

pub(crate) struct CompletionClassificationOutput {
    pub(crate) classification: ZeroTurnClassification,
    pub(crate) recovered_generic_nonzero: bool,
    pub(crate) incomplete_tool_boundary: bool,
    pub(crate) accepted_provider_turn: bool,
}

fn classify_completion_with_recovery(
    state: &StateDb,
    sessions_cfg: &oulipoly_config::SessionsConfig,
    baseline: &ZeroTurnBaseline,
    host_observed: HostObservedCompletion,
    result: &executor::ExecutionResult,
) -> CompletionClassificationOutput {
    let completion = classify_after_completion(state, sessions_cfg, baseline, host_observed);
    let recovered = recovered_generic_nonzero(completion.accepted_provider_turn, result);
    CompletionClassificationOutput {
        classification: completion.classification,
        recovered_generic_nonzero: recovered,
        incomplete_tool_boundary: completion.incomplete_tool_boundary,
        accepted_provider_turn: completion.accepted_provider_turn,
    }
}

struct CompletionClassification {
    classification: ZeroTurnClassification,
    accepted_provider_turn: bool,
    incomplete_tool_boundary: bool,
}

fn classify_after_completion(
    state: &StateDb,
    sessions_cfg: &oulipoly_config::SessionsConfig,
    baseline: &ZeroTurnBaseline,
    host_observed: HostObservedCompletion,
) -> CompletionClassification {
    let Some(session_id) = baseline.provider_session_id.as_deref() else {
        return completion_classification(
            classify_completion(baseline, zero_turn_zero_counts(), host_observed),
            false,
            false,
        );
    };
    if baseline.scan_failed {
        return completion_classification(
            classify_completion(baseline, zero_turn_zero_counts(), host_observed),
            false,
            false,
        );
    }
    let report = oulipoly_runtime::sessions::scan_provider_session(
        &baseline.provider_name,
        sessions_cfg,
        state,
        session_id,
    );
    if scan_report_has_errors(&report) {
        return completion_classification(
            ZeroTurnClassification::UnclassifiedScanFailed,
            false,
            false,
        );
    }
    let Some(counts) =
        turn_counts_or_scan_failed(state.count_session_turns(&baseline.provider_name, session_id))
    else {
        return completion_classification(
            classify_completion(baseline, zero_turn_zero_counts(), host_observed),
            false,
            false,
        );
    };
    let current_completion = report.assistant_completions.get(session_id);
    let accepted_provider_turn =
        classify_accepted_provider_turn(baseline, counts.clone(), false, current_completion)
            .is_some();
    let incomplete_tool_boundary = is_incomplete_tool_boundary(
        baseline,
        counts.clone(),
        false,
        current_completion,
        report.assistant_tool_boundaries.get(session_id),
    );
    completion_classification(
        classify_completion(baseline, counts, host_observed),
        accepted_provider_turn,
        incomplete_tool_boundary,
    )
}

fn completion_classification(
    classification: ZeroTurnClassification,
    accepted_provider_turn: bool,
    incomplete_tool_boundary: bool,
) -> CompletionClassification {
    CompletionClassification {
        classification,
        accepted_provider_turn,
        incomplete_tool_boundary,
    }
}

fn recovered_generic_nonzero(
    accepted_provider_turn: bool,
    result: &executor::ExecutionResult,
) -> bool {
    accepted_provider_turn
        && result.exit_code != 0
        && result.terminal_reason.as_deref() == Some("exit_nonzero")
        && result
            .terminal_signal
            .as_ref()
            .is_some_and(|signal| signal.kind == TerminalSignalKind::NonzeroExit)
}

pub(crate) fn host_observed_completion_from_result(
    result: &executor::ExecutionResult,
) -> HostObservedCompletion {
    HostObservedCompletion::new(
        terminal_signal_is_clean_exit(result.exit_code, &result.terminal_signal),
        result.produced_assistant_response,
    )
}

pub(crate) fn host_observed_completion_from_interactive_result(
    result: &executor::cli::InteractiveExecutionResult,
) -> HostObservedCompletion {
    HostObservedCompletion::new(
        terminal_signal_is_clean_exit(result.exit_code, &result.terminal_signal),
        false,
    )
}

fn terminal_signal_is_clean_exit(
    exit_code: i32,
    signal: &Option<executor::TerminalSignal>,
) -> bool {
    exit_code == 0
        && signal
            .as_ref()
            .is_some_and(|signal| signal.kind == TerminalSignalKind::CleanExit)
}

fn zero_turn_classification_is_non_productive(c: &ZeroTurnClassification) -> bool {
    matches!(
        zero_turn_productivity(c),
        ZeroTurnProductivity::NonProductive
    )
}

enum ZeroTurnProductivity {
    Productive,
    NonProductive,
}

fn zero_turn_productivity(c: &ZeroTurnClassification) -> ZeroTurnProductivity {
    matches!(
        c,
        ZeroTurnClassification::MaybeQuotaExhausted { .. }
            | ZeroTurnClassification::UnclassifiedNoSessionId
            | ZeroTurnClassification::UnclassifiedScanFailed
    )
    .then_some(ZeroTurnProductivity::NonProductive)
    .unwrap_or(ZeroTurnProductivity::Productive)
}

pub(crate) fn zero_turn_classification_for_action(
    classification: ZeroTurnClassification,
    result: &executor::ExecutionResult,
    provider_name: &str,
    provider_session_id: Option<&str>,
) -> ZeroTurnClassification {
    if zero_turn_classification_is_non_productive(&classification) {
        return classification;
    }
    if zero_turn_completion_can_replace_signal(&result.terminal_signal)
        && let Some(session_id) = provider_session_id
    {
        let baseline = record_baseline(
            provider_name,
            Some(session_id),
            Some(zero_turn_zero_counts()),
            false,
        );
        return classify_completion_delta(&baseline, zero_turn_zero_counts());
    }
    classification
}

fn zero_turn_completion_can_replace_signal(signal: &Option<executor::TerminalSignal>) -> bool {
    signal
        .as_ref()
        .is_some_and(|signal| signal.kind == TerminalSignalKind::MaybeQuotaExhausted)
}

fn build_maybe_quota_exhausted_signal(
    provider_name: &str,
    evidence: &ZeroTurnEvidence,
) -> executor::TerminalSignal {
    maybe_quota_exhausted_signal(
        provider_name.to_string(),
        evidence.evidence.clone(),
        std::time::SystemTime::now(),
    )
}

fn maybe_quota_exhausted_signal(
    provider_name: String,
    evidence: String,
    observed_at: std::time::SystemTime,
) -> executor::TerminalSignal {
    executor::TerminalSignal {
        kind: TerminalSignalKind::MaybeQuotaExhausted,
        provider_name,
        evidence,
        observed_at,
    }
}

pub(crate) fn apply_zero_turn_classification_to_signal_fields(
    terminal_signal: &mut Option<executor::TerminalSignal>,
    terminal_reason: &mut Option<String>,
    provider_name: &str,
    classification: &ZeroTurnClassification,
) {
    match classification {
        ZeroTurnClassification::Productive => {
            apply_productive_zero_turn_classification(terminal_signal, terminal_reason)
        }
        ZeroTurnClassification::MaybeQuotaExhausted { evidence } => {
            apply_maybe_quota_zero_turn_classification(
                terminal_signal,
                terminal_reason,
                provider_name,
                evidence,
            );
        }
        ZeroTurnClassification::UnclassifiedNoSessionId
        | ZeroTurnClassification::UnclassifiedScanFailed => {}
    }
}

fn apply_productive_zero_turn_classification(
    terminal_signal: &mut Option<executor::TerminalSignal>,
    terminal_reason: &mut Option<String>,
) {
    if zero_turn_completion_can_replace_signal(terminal_signal) {
        *terminal_signal = None;
        *terminal_reason = None;
    }
}

fn apply_maybe_quota_zero_turn_classification(
    terminal_signal: &mut Option<executor::TerminalSignal>,
    terminal_reason: &mut Option<String>,
    provider_name: &str,
    evidence: &ZeroTurnEvidence,
) {
    if !zero_turn_completion_can_replace_signal(terminal_signal) {
        return;
    }
    *terminal_signal = Some(build_maybe_quota_exhausted_signal(provider_name, evidence));
    *terminal_reason = Some(maybe_quota_exhausted_reason());
}

fn maybe_quota_exhausted_reason() -> String {
    "maybe_quota_exhausted".to_string()
}

pub(crate) fn apply_zero_turn_classification_to_result(
    result: &mut executor::ExecutionResult,
    provider_name: &str,
    classification: &ZeroTurnClassification,
) {
    apply_zero_turn_classification_to_signal_fields(
        &mut result.terminal_signal,
        &mut result.terminal_reason,
        provider_name,
        classification,
    );
}

pub(crate) fn is_confirmed_zero_turn_exhaustion(
    action: ZeroTurnAction,
    signal: &Option<executor::TerminalSignal>,
) -> bool {
    matches!(action, ZeroTurnAction::ConfirmedExhaustion)
        && zero_turn_completion_can_replace_signal(signal)
}

pub(crate) fn zero_turn_late_bind_baseline(
    sessions_cfg: &oulipoly_config::SessionsConfig,
    provider_name: &str,
    session_id: &str,
) -> ZeroTurnBaseline {
    let has_source = has_session_source(sessions_cfg, provider_name);
    record_baseline(
        provider_name,
        Some(session_id),
        has_source.then(zero_turn_zero_counts),
        !has_source,
    )
}

#[cfg(test)]
mod tests {
    use super::recovered_generic_nonzero;
    use oulipoly_runtime::executor::{
        CapturedChildInvocation, ExecutionResult, SessionCaptureMethod, SessionCaptureResult,
        TerminalSignal,
    };
    use std::time::SystemTime;

    fn result(
        kind: Option<super::TerminalSignalKind>,
        exit_code: i32,
        reason: &str,
    ) -> ExecutionResult {
        ExecutionResult {
            stdout: Vec::new(),
            stderr: "physical provider error".to_string(),
            exit_code,
            provider_index: 0,
            session_capture: SessionCaptureResult {
                session_id: Some("session-1".to_string()),
                method: SessionCaptureMethod::ForcedFlagVerified,
            },
            resume_acceptance: None,
            terminal_reason: Some(reason.to_string()),
            terminal_signal: kind.map(|kind| TerminalSignal {
                kind,
                provider_name: "provider-a".to_string(),
                evidence: "typed evidence".to_string(),
                observed_at: SystemTime::UNIX_EPOCH,
            }),
            produced_assistant_response: false,
            submitted_user_turn: None,
            captured_child_invocations: Vec::<CapturedChildInvocation>::new(),
            returned_artifacts: Vec::new(),
        }
    }

    #[test]
    fn accepted_turn_recovers_only_exact_generic_physical_nonzero() {
        assert!(recovered_generic_nonzero(
            true,
            &result(
                Some(super::TerminalSignalKind::NonzeroExit),
                1,
                "exit_nonzero"
            )
        ));

        for kind in [
            super::TerminalSignalKind::CleanExit,
            super::TerminalSignalKind::SignalExit,
            super::TerminalSignalKind::SpawnError,
            super::TerminalSignalKind::QuotaExhaustedInband,
            super::TerminalSignalKind::MaybeQuotaExhausted,
            super::TerminalSignalKind::RateLimited,
            super::TerminalSignalKind::ProviderStorageContention,
            super::TerminalSignalKind::ProlongedSilence,
            super::TerminalSignalKind::Unknown,
        ] {
            assert!(!recovered_generic_nonzero(
                true,
                &result(Some(kind), 1, "exit_nonzero")
            ));
        }
        assert!(!recovered_generic_nonzero(
            true,
            &result(None, 1, "exit_nonzero")
        ));
        assert!(!recovered_generic_nonzero(
            true,
            &result(
                Some(super::TerminalSignalKind::NonzeroExit),
                0,
                "exit_nonzero"
            )
        ));
        assert!(!recovered_generic_nonzero(
            true,
            &result(Some(super::TerminalSignalKind::NonzeroExit), 1, "unknown")
        ));
        assert!(!recovered_generic_nonzero(
            false,
            &result(
                Some(super::TerminalSignalKind::NonzeroExit),
                1,
                "exit_nonzero"
            )
        ));
    }
}
