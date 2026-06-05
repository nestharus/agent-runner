//! ## Declared roles
//!
//! Roles: orchestration, mapper.
//!
//! - orchestration: records verified child process identity in the independent
//!   PID sidecar after successful provider spawns.
//! - mapper: derives sidecar metadata from the existing parent-invocation env
//!   payload threaded through executor launches.

use oulipoly_state::CompositeInvocationId;
use oulipoly_state::pid_identity::{self, LiveProcessIdentityRecord};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SpawnIdentityContext {
    invocation_uuid: String,
    provider_name: String,
    model_name: Option<String>,
    session_id: Option<String>,
}

pub(super) fn context_from_parent_invocation_env(
    parent_invocation_env: Option<&str>,
    provider_name: &str,
    model_name: Option<&str>,
    session_id: Option<&str>,
) -> Option<SpawnIdentityContext> {
    let invocation = parent_invocation_env.and_then(parse_invocation_env_silent)?;
    Some(SpawnIdentityContext {
        invocation_uuid: invocation.id,
        provider_name: provider_name.to_string(),
        model_name: model_name.map(str::to_string),
        session_id: session_id.map(str::to_string),
    })
}

pub(super) fn record_child_identity(child_id: u32, context: Option<&SpawnIdentityContext>) {
    let Some(context) = context else {
        return;
    };
    if let Err(err) = pid_identity::record_live_process_identity(LiveProcessIdentityRecord {
        os_pid: i64::from(child_id),
        invocation_uuid: &context.invocation_uuid,
        session_id: context.session_id.as_deref(),
        provider_name: Some(&context.provider_name),
        model_name: context.model_name.as_deref(),
    }) {
        tracing::warn!(
            invocation_uuid = %context.invocation_uuid,
            child_pid = child_id,
            "Failed to record PID identity sidecar row: {err}"
        );
    }
}

fn parse_invocation_env_silent(value: &str) -> Option<CompositeInvocationId> {
    CompositeInvocationId::parse_env_value(value).ok()
}
