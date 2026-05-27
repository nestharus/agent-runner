//! Declared roles: orchestration, mapper

use oulipoly_config::ModelConfig;
use oulipoly_runtime::executor;
use std::collections::HashMap;
use std::path::Path;

pub(crate) fn balanced_result_error_category(
    agent_runtime_services: &crate::wiring::AgentRuntimeServices,
    result: &executor::ExecutionResult,
    models: &HashMap<String, ModelConfig>,
    working_dir: Option<&Path>,
) -> Option<String> {
    crate::terminal_outcome_adapter::classify_error_category_with_fallback(result, || {
        balanced_result_error_category_fallback(agent_runtime_services, result, models, working_dir)
    })
}

fn balanced_result_error_category_fallback(
    agent_runtime_services: &crate::wiring::AgentRuntimeServices,
    result: &executor::ExecutionResult,
    models: &HashMap<String, ModelConfig>,
    working_dir: Option<&Path>,
) -> Option<String> {
    let input = super::mapper::diagnostics_fallback_input(result);
    if let Some(category) = super::mapper::diagnostic_exhaustion_category(&input.diagnostic_input) {
        return Some(category);
    }
    crate::commands::diagnostics::run_diagnostics(
        agent_runtime_services,
        &input.diagnostic_input,
        input.exit_code,
        models,
        working_dir,
    )
}
