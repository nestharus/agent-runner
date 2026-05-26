//! Declared roles: orchestration

use super::mapper::DiagnosticsContext;
use crate::wiring::AgentRuntimeServices;
use oulipoly_runtime::diagnostics::Diagnosis;
use std::path::Path;

pub(super) fn run_diagnostics_service(
    agent_runtime_services: &AgentRuntimeServices,
    context: DiagnosticsContext,
    provider_output: &str,
    exit_code: i32,
    working_dir: Option<&Path>,
) -> Result<Diagnosis, String> {
    agent_runtime_services
        .diagnostics_service
        .diagnose(super::mapper::diagnostics_service_request(
            context,
            provider_output,
            exit_code,
            working_dir,
        ))
        .map_err(super::formatter::diagnostics_service_error)
        .and_then(super::validator::diagnostics_output_diagnosis)
}
