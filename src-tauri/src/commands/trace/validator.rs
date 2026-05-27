//! Declared roles: validator

use oulipoly_runtime::services::TraceServiceFailure;
use oulipoly_runtime::trace::TraceReport;

pub(super) fn trace_service_result(
    result: Result<TraceReport, TraceServiceFailure>,
) -> Result<TraceReport, TraceServiceFailure> {
    result
}
