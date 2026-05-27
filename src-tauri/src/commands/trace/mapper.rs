//! Declared roles: mapper

use oulipoly_runtime::services::{TraceServiceFailure, TraceServiceRequest};
use oulipoly_runtime::trace::{TraceOptions, TraceReport};
use oulipoly_state::StateDb;

pub(super) fn trace_environment(
    state: StateDb,
    sessions_cfg: oulipoly_config::SessionsConfig,
) -> super::accessor::TraceEnvironment {
    super::accessor::TraceEnvironment {
        state,
        sessions_cfg,
    }
}

pub(crate) fn trace_options(
    max_depth: usize,
    json: bool,
    inline_transcript: bool,
    transcript: bool,
) -> TraceOptions {
    TraceOptions {
        max_depth,
        json,
        inline_transcript,
        transcript,
    }
}

pub(super) fn trace_request<'a>(
    env: &'a super::accessor::TraceEnvironment,
    invocation_uuid: &'a str,
    options: TraceOptions,
) -> TraceServiceRequest<'a> {
    TraceServiceRequest {
        state: &env.state,
        sessions_cfg: &env.sessions_cfg,
        invocation_uuid,
        options,
    }
}

pub(super) enum TraceResultOutcome {
    Success(Box<TraceReport>),
    InvocationNotFound { message: String },
    Failure(String),
}

pub(super) fn map_trace_success(report: TraceReport) -> TraceResultOutcome {
    TraceResultOutcome::Success(Box::new(report))
}

pub(super) fn map_trace_failure(failure: TraceServiceFailure) -> TraceResultOutcome {
    match failure {
        TraceServiceFailure::InvocationNotFound { message, .. } => {
            TraceResultOutcome::InvocationNotFound { message }
        }
        TraceServiceFailure::InvalidInvocationId { message, .. }
        | TraceServiceFailure::Operational { message } => TraceResultOutcome::Failure(message),
    }
}
