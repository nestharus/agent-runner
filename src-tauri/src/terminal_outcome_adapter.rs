//! Typed terminal-outcome classification for CLI result handling.
//!
//! ## Declared roles
//!
//! `mapper`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/terminal_outcome_adapter.rs
//!     role: adapter
//!     Translates:
//!       - oulipoly_runtime::executor::ExecutionResult terminal_signal contract
//!       - oulipoly_runtime::executor::terminal_signal::TerminalSignalKind contract
//!       - oulipoly_runtime::diagnostics error-category contract
//!       - src-tauri CLI quota retry/error-category contract
//! ```

use oulipoly_runtime::diagnostics::ErrorCategory;
use oulipoly_runtime::executor::ExecutionResult;
use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalOutcomeCategory {
    QuotaExhausted,
    HungSubprocess,
    Unknown,
}

impl TerminalOutcomeCategory {
    pub(crate) fn as_error_category(self) -> Option<String> {
        match self {
            TerminalOutcomeCategory::QuotaExhausted => {
                Some(ErrorCategory::QuotaExhausted.as_str().to_string())
            }
            TerminalOutcomeCategory::HungSubprocess => {
                Some(ErrorCategory::HungSubprocess.as_str().to_string())
            }
            TerminalOutcomeCategory::Unknown => Some(ErrorCategory::Unknown.as_str().to_string()),
        }
    }
}

pub(crate) fn classify_error_category_with_fallback<F>(
    result: &ExecutionResult,
    diagnostics_fallback: F,
) -> Option<String>
where
    F: FnOnce() -> Option<String>,
{
    if result.exit_code == 0 {
        return None;
    }

    if let Some(signal) = result.terminal_signal.as_ref() {
        return category_for_signal_kind(signal.kind).as_error_category();
    }

    diagnostics_fallback()
}

fn category_for_signal_kind(kind: TerminalSignalKind) -> TerminalOutcomeCategory {
    match kind {
        TerminalSignalKind::QuotaExhaustedInband => TerminalOutcomeCategory::QuotaExhausted,
        TerminalSignalKind::ProlongedSilence => TerminalOutcomeCategory::HungSubprocess,
        TerminalSignalKind::CleanExit
        | TerminalSignalKind::NonzeroExit
        | TerminalSignalKind::SignalExit
        | TerminalSignalKind::SpawnError
        | TerminalSignalKind::Unknown => TerminalOutcomeCategory::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_runtime::executor::{
        CapturedChildInvocation, SessionCaptureMethod, SessionCaptureResult, TerminalSignal,
    };
    use std::time::SystemTime;

    fn result_with_signal(kind: Option<TerminalSignalKind>) -> ExecutionResult {
        ExecutionResult {
            stdout: Vec::new(),
            stderr: "legacy quota text".to_string(),
            exit_code: 1,
            provider_index: 0,
            session_capture: SessionCaptureResult {
                session_id: None,
                method: SessionCaptureMethod::None,
            },
            resume_acceptance: None,
            terminal_reason: Some("quota_exhausted_inband".to_string()),
            terminal_signal: kind.map(|kind| TerminalSignal {
                kind,
                provider_name: "provider-a".to_string(),
                evidence: "typed evidence".to_string(),
                observed_at: SystemTime::UNIX_EPOCH,
            }),
            captured_child_invocations: Vec::<CapturedChildInvocation>::new(),
            returned_artifacts: Vec::new(),
        }
    }

    #[test]
    fn age151_terminal_outcome_typed_quota_exhausted_inband_maps_to_quota_exhausted_before_diagnostics()
     {
        let result = result_with_signal(Some(TerminalSignalKind::QuotaExhaustedInband));
        let mut fallback_called = false;

        let category = classify_error_category_with_fallback(&result, || {
            fallback_called = true;
            Some(ErrorCategory::Unknown.as_str().to_string())
        });

        assert_eq!(
            category.as_deref(),
            Some(ErrorCategory::QuotaExhausted.as_str())
        );
        assert!(!fallback_called);
    }

    #[test]
    fn age151_terminal_outcome_typed_prolonged_silence_is_non_quota_terminal_outcome() {
        let result = result_with_signal(Some(TerminalSignalKind::ProlongedSilence));

        let category = classify_error_category_with_fallback(&result, || {
            Some(ErrorCategory::QuotaExhausted.as_str().to_string())
        });

        assert_eq!(
            category.as_deref(),
            Some(ErrorCategory::HungSubprocess.as_str())
        );
    }

    #[test]
    fn age151_terminal_outcome_legacy_diagnostics_runs_only_when_typed_signal_absent() {
        let typed_result = result_with_signal(Some(TerminalSignalKind::QuotaExhaustedInband));
        let legacy_result = result_with_signal(None);
        let mut fallback_calls = 0;

        let typed_category = classify_error_category_with_fallback(&typed_result, || {
            fallback_calls += 1;
            Some(ErrorCategory::Unknown.as_str().to_string())
        });
        let legacy_category = classify_error_category_with_fallback(&legacy_result, || {
            fallback_calls += 1;
            Some(ErrorCategory::QuotaExhausted.as_str().to_string())
        });

        assert_eq!(
            typed_category.as_deref(),
            Some(ErrorCategory::QuotaExhausted.as_str())
        );
        assert_eq!(
            legacy_category.as_deref(),
            Some(ErrorCategory::QuotaExhausted.as_str())
        );
        assert_eq!(fallback_calls, 1);
    }

    #[test]
    fn age151_terminal_outcome_terminal_reason_strings_do_not_create_typed_behavior() {
        let result = result_with_signal(None);

        let category = classify_error_category_with_fallback(&result, || None);

        assert_eq!(
            result.terminal_reason.as_deref(),
            Some("quota_exhausted_inband")
        );
        assert_eq!(category, None);
    }
}
