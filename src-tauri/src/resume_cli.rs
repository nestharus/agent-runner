//! Headless resume CLI orchestration helpers.
//!
//! ## Declared roles
//!
//! `orchestration`, `mapper`, `predicate`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/resume_cli.rs
//!     role: adapter
//!     Translates:
//!       - resume acceptance result
//!       - typed terminal outcome category
//!       - diagnostics fallback category
//! ```

use oulipoly_config::ModelConfig;
use oulipoly_runtime::executor;
use std::collections::HashMap;
use std::path::Path;

pub(super) fn resume_result_error_category(
    agent_runtime_services: &super::wiring::AgentRuntimeServices,
    result: &executor::ExecutionResult,
    models: &HashMap<String, ModelConfig>,
    working_dir: Option<&Path>,
) -> Option<String> {
    if super::execution_succeeded(result.exit_code) {
        return None;
    }
    if super::resume_acceptance_adapter::classify(result.resume_acceptance.as_ref())
        == super::resume_acceptance_adapter::ResumeAcceptanceCategory::SessionMismatch
    {
        return Some(super::resume_session_mismatch_category());
    }
    super::terminal_outcome_adapter::classify_error_category_with_fallback(result, || {
        super::diagnose_execution_error(agent_runtime_services, result, models, working_dir)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_outcome_adapter::resume_terminal_signal_for_outcome;
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

        let quota = resume_result_error_category(
            &services,
            &result_with_signal(TerminalSignalKind::QuotaExhaustedInband, 1),
            &models,
            None,
        );
        let maybe = resume_result_error_category(
            &services,
            &result_with_signal(TerminalSignalKind::MaybeQuotaExhausted, 1),
            &models,
            None,
        );
        let clean = resume_result_error_category(
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
    fn resume_terminal_signal_for_outcome_handles_new_kind() {
        let maybe = result_with_signal(TerminalSignalKind::MaybeQuotaExhausted, 1).terminal_signal;
        let quota = result_with_signal(TerminalSignalKind::QuotaExhaustedInband, 1).terminal_signal;

        assert_eq!(
            resume_terminal_signal_for_outcome(&maybe).map(|signal| signal.kind),
            Some(TerminalSignalKind::MaybeQuotaExhausted)
        );
        assert_eq!(
            resume_terminal_signal_for_outcome(&quota).map(|signal| signal.kind),
            Some(TerminalSignalKind::QuotaExhaustedInband)
        );
    }
}
