//! Error-category projection for typed terminal signals.
//!
//! ## Declared roles
//!
//! `mapper`, `predicate`

use oulipoly_runtime::diagnostics::ErrorCategory;
use oulipoly_runtime::executor::ExecutionResult;
use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalOutcomeCategory {
    QuotaExhausted,
    HungSubprocess,
    ProviderUnavailable,
}

impl TerminalOutcomeCategory {
    pub fn as_error_category(self) -> Option<String> {
        match self {
            TerminalOutcomeCategory::QuotaExhausted => {
                Some(ErrorCategory::QuotaExhausted.as_str().to_string())
            }
            TerminalOutcomeCategory::HungSubprocess => {
                Some(ErrorCategory::HungSubprocess.as_str().to_string())
            }
            TerminalOutcomeCategory::ProviderUnavailable => {
                Some(ErrorCategory::ProviderUnavailable.as_str().to_string())
            }
        }
    }
}

pub fn classify_error_category_with_fallback<F>(
    result: &ExecutionResult,
    diagnostics_fallback: F,
) -> Option<String>
where
    F: FnOnce() -> Option<String>,
{
    if result.exit_code == 0 {
        return None;
    }

    if let Some(signal) = result.terminal_signal.as_ref()
        && let Some(category) = category_for_signal_kind(signal.kind)
    {
        return category.as_error_category();
    }

    diagnostics_fallback()
}

fn category_for_signal_kind(kind: TerminalSignalKind) -> Option<TerminalOutcomeCategory> {
    match kind {
        TerminalSignalKind::QuotaExhaustedInband => Some(TerminalOutcomeCategory::QuotaExhausted),
        TerminalSignalKind::ProlongedSilence => Some(TerminalOutcomeCategory::HungSubprocess),
        TerminalSignalKind::ProviderUnavailable => {
            Some(TerminalOutcomeCategory::ProviderUnavailable)
        }
        TerminalSignalKind::CleanExit
        | TerminalSignalKind::NonzeroExit
        | TerminalSignalKind::SignalExit
        | TerminalSignalKind::SpawnError
        | TerminalSignalKind::MaybeQuotaExhausted
        | TerminalSignalKind::RateLimited
        | TerminalSignalKind::ProviderStorageContention
        | TerminalSignalKind::Unknown => None,
    }
}
