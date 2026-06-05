//! ## Declared roles
//!
//! Roles: orchestration, mapper.
//!
//! - orchestration: records verified child process identity in the independent
//!   PID sidecar after successful provider spawns.
//! - mapper: derives sidecar metadata from the existing parent-invocation env
//!   payload threaded through executor launches.

use oulipoly_state::CompositeInvocationId;
use oulipoly_state::mailbox::{MailboxDb, SessionRuntimeRunningUpdate};
use oulipoly_state::pid_identity::{self, LiveProcessIdentityRecord};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SpawnRuntimeMode {
    Headless,
    PtyInteractive,
}

impl SpawnRuntimeMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Headless => "headless",
            Self::PtyInteractive => "pty_interactive",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SpawnIdentityContext {
    invocation_uuid: String,
    provider_name: String,
    model_name: Option<String>,
    session_id: Option<String>,
    mode: SpawnRuntimeMode,
    effective_cwd: Option<String>,
}

pub(super) fn context_from_parent_invocation_env(
    parent_invocation_env: Option<&str>,
    provider_name: &str,
    model_name: Option<&str>,
    session_id: Option<&str>,
    mode: SpawnRuntimeMode,
    effective_cwd: Option<&Path>,
) -> Option<SpawnIdentityContext> {
    let invocation = parent_invocation_env.and_then(parse_invocation_env_silent)?;
    Some(SpawnIdentityContext {
        invocation_uuid: invocation.id,
        provider_name: provider_name.to_string(),
        model_name: model_name.map(str::to_string),
        session_id: session_id.map(str::to_string),
        mode,
        effective_cwd: effective_cwd.map(|path| path.to_string_lossy().into_owned()),
    })
}

pub(super) fn record_child_identity(child_id: u32, context: Option<&SpawnIdentityContext>) {
    let Some(context) = context else {
        return;
    };
    match pid_identity::record_live_process_identity(LiveProcessIdentityRecord {
        os_pid: i64::from(child_id),
        invocation_uuid: &context.invocation_uuid,
        session_id: context.session_id.as_deref(),
        provider_name: Some(&context.provider_name),
        model_name: context.model_name.as_deref(),
    }) {
        Ok(Some(row)) => mark_session_running(context, &row.identity()),
        Ok(None) => {}
        Err(err) => {
            tracing::warn!(
                invocation_uuid = %context.invocation_uuid,
                child_pid = child_id,
                "Failed to record PID identity sidecar row: {err}"
            );
        }
    }
}

fn mark_session_running(
    context: &SpawnIdentityContext,
    identity: &oulipoly_state::pid_identity::ProcessIdentity,
) {
    let Some(session_id) = context.session_id.as_deref() else {
        return;
    };
    match MailboxDb::open_default().and_then(|mut db| {
        db.mark_session_running(SessionRuntimeRunningUpdate {
            session_id,
            mode: context.mode.as_str(),
            invocation_uuid: &context.invocation_uuid,
            provider_name: Some(&context.provider_name),
            model_name: context.model_name.as_deref(),
            identity,
            turn_start_max_mailbox_seq: None,
            models_dir: None,
            effective_cwd: context.effective_cwd.as_deref(),
        })
    }) {
        Ok(()) => {}
        Err(err) => {
            tracing::warn!(
                invocation_uuid = %context.invocation_uuid,
                session_id,
                "Failed to mark session runtime running: {err}"
            );
        }
    }
}

fn parse_invocation_env_silent(value: &str) -> Option<CompositeInvocationId> {
    CompositeInvocationId::parse_env_value(value).ok()
}
