//! predicate

use std::collections::HashMap;

use oulipoly_config::ModelConfig;
use oulipoly_runtime::executor;

use crate::zero_turn_orchestration::ZeroTurnAction;
use crate::zero_turn_orchestration::ZeroTurnBaseline;

pub(super) fn attempts_exhausted(attempts: usize, max_attempts: usize) -> bool {
    attempts >= max_attempts
}

pub(super) fn provider_selection_pool_exhausted(attempts: usize, provider_count: usize) -> bool {
    attempts > provider_count.max(1)
}

pub(super) fn retry_available(attempts: usize, max_attempts: usize) -> bool {
    attempts < max_attempts
}

pub(super) fn confirmed_zero_turn_exhaustion(
    action: ZeroTurnAction,
    signal: &Option<executor::TerminalSignal>,
) -> bool {
    crate::quota_zero_turn::is_confirmed_zero_turn_exhaustion(action, signal)
}

pub(super) fn confirmed_zero_turn_action(action: ZeroTurnAction) -> bool {
    matches!(action, ZeroTurnAction::ConfirmedExhaustion)
}

pub(super) fn has_provider_session_id(provider_session_id: Option<&str>) -> bool {
    provider_session_id.is_some()
}

pub(super) fn should_defer_generic_exit(
    all_models: &HashMap<String, ModelConfig>,
    result: &executor::ExecutionResult,
) -> bool {
    crate::dispatch::diagnostics_model_configured(all_models)
        || !result.returned_artifacts.is_empty()
}

pub(super) fn should_late_bind_zero_turn_baseline(
    baseline: &ZeroTurnBaseline,
    provider_session_id: Option<&str>,
) -> bool {
    baseline.provider_session_id.is_none() && provider_session_id.is_some()
}

pub(super) fn settled_unknown(error_category: Option<&str>) -> bool {
    error_category == Some(oulipoly_runtime::diagnostics::ErrorCategory::Unknown.as_str())
}

pub(super) fn diagnostic_input_is_exhaustion(input: &str) -> bool {
    oulipoly_runtime::diagnostics::classify_exhaustion(input)
}

pub(super) fn execution_succeeded(result: &executor::ExecutionResult) -> bool {
    crate::dispatch::execution_succeeded(result.exit_code)
}

pub(super) fn error_category_is_quota_exhausted(error_category: Option<&str>) -> bool {
    crate::quota_zero_turn::error_category_is_quota_exhausted(error_category)
}
