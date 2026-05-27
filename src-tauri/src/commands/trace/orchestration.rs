//! Declared roles: orchestration

use crate::wiring::AgentRuntimeServices;
use oulipoly_runtime::trace::TraceOptions;

pub(crate) fn run_trace_command(
    options: TraceOptions,
    invocation_uuid: &str,
    agent_runtime_services: &AgentRuntimeServices,
) -> Result<i32, String> {
    let env = super::accessor::load_trace_environment()?;
    let request = super::mapper::trace_request(&env, invocation_uuid, options);
    let output = agent_runtime_services
        .trace_service
        .trace(request)
        .map_err(super::formatter::format_trace_service_dispatch_error)?;
    let outcome = match super::validator::trace_service_result(output.result) {
        Ok(report) => super::mapper::map_trace_success(report),
        Err(failure) => super::mapper::map_trace_failure(failure),
    };
    match outcome {
        super::mapper::TraceResultOutcome::Success(report) => {
            super::formatter::render_trace_report(&report, options.json)
        }
        super::mapper::TraceResultOutcome::InvocationNotFound { message } => {
            super::formatter::report_trace_invocation_not_found(&message);
            Ok(1)
        }
        super::mapper::TraceResultOutcome::Failure(message) => Err(message),
    }
}
