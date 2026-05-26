//! One-shot balanced CLI helper boundaries.
//!
//! ## Declared roles
//!
//! `orchestration`, `mapper`, `predicate`, `validator`
//!
//! AGE-166 added test coverage that exercises both the typed-signal-first
//! parity for `balanced_terminal_signal_for_outcome` (validator/predicate
//! over `TerminalSignal.kind`, including `MaybeQuotaExhausted`) and the
//! diagnostic fallback's error-category mapping (mapper). The file-local
//! role set widens to reflect the validator/predicate work added by the
//! AGE-166 typed-signal-first contract.

use oulipoly_config::ModelConfig;
use oulipoly_runtime::{diagnostics, executor};
use std::collections::HashMap;
use std::path::Path;

pub(super) fn balanced_result_error_category(
    agent_runtime_services: &super::wiring::AgentRuntimeServices,
    result: &executor::ExecutionResult,
    models: &HashMap<String, ModelConfig>,
    working_dir: Option<&Path>,
) -> Option<String> {
    super::terminal_outcome_adapter::classify_error_category_with_fallback(result, || {
        let input = super::diagnostic_input(&result.stderr, &result.stdout);
        if diagnostics::classify_exhaustion(&input) {
            Some(super::quota_exhausted_category())
        } else {
            crate::commands::diagnostics::run_diagnostics(
                agent_runtime_services,
                &input,
                result.exit_code,
                models,
                working_dir,
            )
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_outcome_adapter::balanced_terminal_signal_for_outcome;
    use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;
    use oulipoly_runtime::executor::{
        CapturedChildInvocation, ExecutionResult, SessionCaptureMethod, SessionCaptureResult,
        TerminalSignal,
    };
    use std::time::SystemTime;

    fn result_with_signal(kind: TerminalSignalKind, exit_code: i32) -> ExecutionResult {
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
            terminal_reason: None,
            terminal_signal: Some(TerminalSignal {
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
    fn resume_fallback_typed_signal_parity() {
        let services = crate::wiring::AgentRuntimeServices::cli_defaults();
        let models = HashMap::new();

        let quota = balanced_result_error_category(
            &services,
            &result_with_signal(TerminalSignalKind::QuotaExhaustedInband, 1),
            &models,
            None,
        );
        let maybe = balanced_result_error_category(
            &services,
            &result_with_signal(TerminalSignalKind::MaybeQuotaExhausted, 1),
            &models,
            None,
        );
        let clean = balanced_result_error_category(
            &services,
            &result_with_signal(TerminalSignalKind::CleanExit, 0),
            &models,
            None,
        );

        assert_eq!(quota.as_deref(), Some("quota_exhausted"));
        assert_eq!(maybe, None);
        assert_eq!(clean, None);
    }

    #[test]
    fn balanced_terminal_signal_for_outcome_handles_new_kind() {
        let maybe = result_with_signal(TerminalSignalKind::MaybeQuotaExhausted, 1);
        let quota = result_with_signal(TerminalSignalKind::QuotaExhaustedInband, 1);

        assert_eq!(
            balanced_terminal_signal_for_outcome(&maybe, true).map(|signal| signal.kind),
            Some(TerminalSignalKind::MaybeQuotaExhausted)
        );
        assert_eq!(
            balanced_terminal_signal_for_outcome(&quota, true).map(|signal| signal.kind),
            Some(TerminalSignalKind::QuotaExhaustedInband)
        );
    }
}
