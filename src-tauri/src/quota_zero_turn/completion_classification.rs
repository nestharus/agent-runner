//! Main-side zero-turn completion-classification adapters.
//!
//! Relocated from `src-tauri/src/main.rs` by AGE-204 (map row H13). Output-preserving.
//! These adapters wrap the pure classification core in `crate::zero_turn_orchestration`,
//! adding the state-DB session scan, baseline capture, and `ExecutionResult` signal
//! mutation that the main-side balancing/resume orchestrators drive.
//!
//! ## Declared roles
//!
//! `orchestration`, `mapper`, `predicate`, `accessor`
//!
//! - `orchestration`: `zero_turn_record_baseline` / `zero_turn_classify_after_completion`
//!   sequence the session scan + turn-count read + core classification.
//! - `mapper`: `zero_turn_classification_for_action` / `apply_zero_turn_classification_to_*`
//!   map a classification onto the executor `terminal_signal` / `terminal_reason` fields.
//! - `predicate`: `zero_turn_completion_can_replace_signal`,
//!   `zero_turn_classification_is_non_productive`, `is_confirmed_zero_turn_exhaustion`.
//! - `accessor`: `provider_has_no_session_source` / `has_session_source` /
//!   `baseline_turn_count_from_scan` read session-source + turn-count state.

use oulipoly_runtime::executor;
use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;
use oulipoly_state::StateDb;

use crate::zero_turn_orchestration::{
    ZeroTurnAction, ZeroTurnBaseline, ZeroTurnClassification, ZeroTurnEvidence,
    classify_completion_delta, record_baseline,
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

fn classify_from_turn_count_result<E>(
    baseline: &ZeroTurnBaseline,
    count_result: Result<oulipoly_state::SessionTurnCounts, E>,
) -> ZeroTurnClassification {
    classify_turn_counts_or_scan_failed(baseline, turn_counts_or_scan_failed(count_result))
}

fn turn_counts_or_scan_failed<E>(
    count_result: Result<oulipoly_state::SessionTurnCounts, E>,
) -> Option<oulipoly_state::SessionTurnCounts> {
    count_result.ok()
}

fn classify_turn_counts_or_scan_failed(
    baseline: &ZeroTurnBaseline,
    counts: Option<oulipoly_state::SessionTurnCounts>,
) -> ZeroTurnClassification {
    counts.map_or(ZeroTurnClassification::UnclassifiedScanFailed, |counts| {
        classify_completion_delta(baseline, counts)
    })
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
    let report = oulipoly_runtime::sessions::scan_provider(provider_name, sessions_cfg, state);
    let scan_failed = scan_report_has_errors(&report);
    let baseline_count =
        baseline_turn_count_from_scan(state, provider_name, session_id, scan_failed);
    record_baseline(provider_name, Some(session_id), baseline_count, scan_failed)
}

pub(crate) fn zero_turn_classify_after_completion(
    state: &StateDb,
    sessions_cfg: &oulipoly_config::SessionsConfig,
    baseline: &ZeroTurnBaseline,
) -> ZeroTurnClassification {
    let Some(session_id) = baseline.provider_session_id.as_deref() else {
        return classify_completion_delta(baseline, zero_turn_zero_counts());
    };
    if baseline.scan_failed {
        return classify_completion_delta(baseline, zero_turn_zero_counts());
    }
    let report =
        oulipoly_runtime::sessions::scan_provider(&baseline.provider_name, sessions_cfg, state);
    if scan_report_has_errors(&report) {
        return ZeroTurnClassification::UnclassifiedScanFailed;
    }
    classify_from_turn_count_result(
        baseline,
        state.count_session_turns(&baseline.provider_name, session_id),
    )
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
