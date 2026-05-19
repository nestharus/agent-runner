//! Trace CLI output mapping and rendering helpers.
//!
//! ## Declared roles
//!
//! `mapper`, `formatter`

use oulipoly_runtime::services::TraceServiceFailure;

pub(super) fn render_trace_result(
    result: Result<oulipoly_runtime::trace::TraceReport, TraceServiceFailure>,
    json: bool,
) -> Result<i32, String> {
    match result {
        Ok(report) => super::render_trace_report(&report, json),
        Err(TraceServiceFailure::InvocationNotFound { message, .. }) => {
            eprintln!("{message}");
            Ok(1)
        }
        Err(
            TraceServiceFailure::InvalidInvocationId { message, .. }
            | TraceServiceFailure::Operational { message },
        ) => Err(message),
    }
}
