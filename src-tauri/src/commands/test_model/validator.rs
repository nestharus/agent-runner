//! ## Declared roles
//!
//! `validator`, `predicate`

use oulipoly_config::ModelConfig;
use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;
use oulipoly_runtime::services::DiagnosticsServiceOutput;

use super::formatter;

pub(crate) fn validate_provider_index(
    model: &ModelConfig,
    provider_index: usize,
) -> Result<(), String> {
    if provider_index < model.providers.len() {
        Ok(())
    } else {
        Err("provider_index out of range".to_string())
    }
}

pub(crate) fn validate_diagnostics_output_variant(
    output: DiagnosticsServiceOutput,
) -> Result<bool, String> {
    let DiagnosticsServiceOutput::ExhaustionClassification { is_exhausted } = output else {
        return Err(formatter::format_unexpected_diagnostics_output_error());
    };
    Ok(is_exhausted)
}

pub(crate) fn typed_signal_is_quota_exhausted_inband(kind: TerminalSignalKind) -> bool {
    matches!(kind, TerminalSignalKind::QuotaExhaustedInband)
}

pub(crate) fn should_run_diagnostics_fallback(exit_code: i32) -> bool {
    exit_code != 0
}

pub(crate) fn diagnostics_output_is_quota_exhausted(is_exhausted: bool) -> bool {
    is_exhausted
}

pub(crate) fn should_mark_quota_exhausted(should_mark_exhausted: bool) -> bool {
    should_mark_exhausted
}

pub(crate) fn model_command_provider_is_non_empty(command: &str) -> bool {
    !command.trim().is_empty()
}
