//! ## Declared roles
//! orchestration, validator, accessor, mapper, formatter
//!
//! Typed trace failure boundary. This module maps malformed IDs and lookup
//! misses from their direct sources into `TraceServiceFailure` variants without
//! classifying raw diagnostic strings.

use super::dtos::{TraceServiceFailure, TraceServiceOutput, TraceServiceRequest};
use oulipoly_state::{InvocationRecord, StateDb};
use uuid::Uuid;

pub(super) fn trace_with_typed_failures(request: TraceServiceRequest<'_>) -> TraceServiceOutput {
    let result = validate_trace_invocation_id(request.invocation_uuid)
        .and_then(|_| load_trace_root(request.state, request.invocation_uuid))
        .and_then(|_| run_trace_report(request));
    TraceServiceOutput { result }
}

fn validate_trace_invocation_id(input: &str) -> Result<(), TraceServiceFailure> {
    Uuid::parse_str(input)
        .map(|_| ())
        .map_err(|err| invalid_invocation_id_trace_failure(input, err))
}

fn load_trace_root(
    state: &StateDb,
    invocation_uuid: &str,
) -> Result<InvocationRecord, TraceServiceFailure> {
    state
        .get_invocation_by_uuid(invocation_uuid)
        .map_err(operational_trace_failure)
        .and_then(|record| {
            record.ok_or_else(|| invocation_not_found_trace_failure(invocation_uuid))
        })
}

fn run_trace_report(
    request: TraceServiceRequest<'_>,
) -> Result<crate::trace::TraceReport, TraceServiceFailure> {
    crate::trace::trace_invocation_with_sessions(
        request.state,
        request.invocation_uuid,
        request.options,
        Some(request.sessions_cfg),
    )
    .map_err(operational_trace_failure)
}

fn invalid_invocation_id_trace_failure(input: &str, err: uuid::Error) -> TraceServiceFailure {
    TraceServiceFailure::InvalidInvocationId {
        input: input.to_string(),
        message: format!("Invalid invocation UUID '{input}': {err}"),
    }
}

fn invocation_not_found_trace_failure(input: &str) -> TraceServiceFailure {
    TraceServiceFailure::InvocationNotFound {
        input: input.to_string(),
        message: format!("Invocation not found: {input}"),
    }
}

fn operational_trace_failure(message: String) -> TraceServiceFailure {
    TraceServiceFailure::Operational { message }
}
