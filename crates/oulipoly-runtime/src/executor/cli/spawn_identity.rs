//! ## Declared roles
//!
//! Roles: formatter, mapper, orchestration, parser.
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
    pty_control_path: Option<String>,
    effective_cwd: Option<String>,
}

impl SpawnIdentityContext {
    pub(super) fn invocation_uuid(&self) -> &str {
        &self.invocation_uuid
    }

    pub(super) fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub(super) fn with_pty_control_path(&self, path: impl Into<String>) -> Self {
        let mut cloned = self.clone();
        cloned.pty_control_path = Some(path.into());
        cloned
    }
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
        pty_control_path: None,
        effective_cwd: effective_cwd.map(|path| path.to_string_lossy().into_owned()),
    })
}

pub(super) fn record_child_identity(child_id: u32, context: Option<&SpawnIdentityContext>) {
    let Some(context) = context else {
        return;
    };
    match pid_identity::record_live_process_identity(live_process_identity_record(
        child_id, context,
    )) {
        Ok(Some(row)) => mark_session_running(context, &row.identity()),
        Ok(None) => {}
        Err(err) => warn_child_identity_record_failed(context, child_id, &err),
    }
}

fn live_process_identity_record<'a>(
    child_id: u32,
    context: &'a SpawnIdentityContext,
) -> LiveProcessIdentityRecord<'a> {
    LiveProcessIdentityRecord {
        os_pid: i64::from(child_id),
        invocation_uuid: &context.invocation_uuid,
        session_id: context.session_id.as_deref(),
        provider_name: Some(&context.provider_name),
        model_name: context.model_name.as_deref(),
    }
}

fn warn_child_identity_record_failed(context: &SpawnIdentityContext, child_id: u32, err: &str) {
    tracing::warn!(
        invocation_uuid = %context.invocation_uuid,
        child_pid = child_id,
        "Failed to record PID identity sidecar row: {err}"
    );
}

fn mark_session_running(
    context: &SpawnIdentityContext,
    identity: &oulipoly_state::pid_identity::ProcessIdentity,
) {
    let Some(session_id) = context.session_id.as_deref() else {
        return;
    };
    match MailboxDb::open_default().and_then(|mut db| {
        db.mark_session_running(session_runtime_running_update(
            context, session_id, identity,
        ))
    }) {
        Ok(()) => {}
        Err(err) => warn_mark_session_running_failed(context, session_id, &err),
    }
}

fn session_runtime_running_update<'a>(
    context: &'a SpawnIdentityContext,
    session_id: &'a str,
    identity: &'a oulipoly_state::pid_identity::ProcessIdentity,
) -> SessionRuntimeRunningUpdate<'a> {
    SessionRuntimeRunningUpdate {
        session_id,
        mode: context.mode.as_str(),
        invocation_uuid: &context.invocation_uuid,
        provider_name: Some(&context.provider_name),
        model_name: context.model_name.as_deref(),
        identity,
        pty_control_path: context.pty_control_path.as_deref(),
        turn_start_max_mailbox_seq: None,
        models_dir: None,
        effective_cwd: context.effective_cwd.as_deref(),
    }
}

fn warn_mark_session_running_failed(context: &SpawnIdentityContext, session_id: &str, err: &str) {
    tracing::warn!(
        invocation_uuid = %context.invocation_uuid,
        session_id,
        "Failed to mark session runtime running: {err}"
    );
}

fn parse_invocation_env_silent(value: &str) -> Option<CompositeInvocationId> {
    CompositeInvocationId::parse_env_value(value).ok()
}
