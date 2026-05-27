//! Declared roles: parser, mapper

use oulipoly_state::CompositeInvocationId;
use uuid::Uuid;

pub(super) fn parse_parent_invocation_env(raw: &str) -> Option<CompositeInvocationId> {
    CompositeInvocationId::parse_env_value(raw).ok()
}

pub(crate) fn provider_session_marker_uuid(provider_session_id: Option<&str>) -> Option<Uuid> {
    provider_session_id.and_then(|session_id| Uuid::parse_str(session_id).ok())
}

pub(super) fn resume_target_arg<'a>(
    session_id: Option<&'a str>,
    chain_id: Option<&'a str>,
) -> &'a str {
    chain_id
        .or(session_id)
        .expect("clap group ensures one is set")
}
