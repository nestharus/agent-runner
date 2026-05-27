//! Declared roles: orchestration

use crate::wiring::AgentRuntimeServices;
use oulipoly_config::ModelConfig;
use std::collections::HashMap;
use std::path::Path;

pub(crate) fn run_diagnostics(
    agent_runtime_services: &AgentRuntimeServices,
    provider_output: &str,
    exit_code: i32,
    models: &HashMap<String, ModelConfig>,
    working_dir: Option<&Path>,
) -> Option<String> {
    let context = super::mapper::diagnostics_context(models)?;
    super::formatter::render_diagnostics_result(super::service::run_diagnostics_service(
        agent_runtime_services,
        context,
        provider_output,
        exit_code,
        working_dir,
    ))
}
