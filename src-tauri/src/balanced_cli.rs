//! One-shot balanced CLI helper boundaries.
//!
//! ## Declared roles
//!
//! `orchestration`, `mapper`

use oulipoly_config::ModelConfig;
use oulipoly_runtime::{diagnostics, executor};
use oulipoly_state::StateDb;
use std::collections::HashMap;
use std::path::Path;

pub(super) fn mark_provider_exhausted(state: &StateDb, provider_name: &str) {
    state
        .mark_exhausted(provider_name)
        .unwrap_or_else(|e| eprintln!("Warning: Failed to mark provider exhausted: {e}"));
}

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
            super::run_diagnostics(
                agent_runtime_services,
                &input,
                result.exit_code,
                models,
                working_dir,
            )
        }
    })
}
