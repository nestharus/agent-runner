//! ## Declared roles
//!
//! Roles: formatter, mapper, orchestration, parser.
//!
//! - orchestration: records verified child process identity in the independent
//!   PID sidecar after successful provider spawns.
//! - mapper: derives sidecar metadata from the existing parent-invocation env
//!   payload threaded through executor launches.

use oulipoly_state::CompositeInvocationId;
use oulipoly_state::mailbox::{
    AdvanceRuntimeGenerationDrain, AttachRuntimeGenerationSession, BindRuntimeGenerationRunning,
    CreateRuntimeGeneration, DrainAdvanceResult, DrainFinishResult, DrainHandoff, DrainRequestId,
    DrainRequestResult, ExactProcessEvidence, ExitRuntimeGenerationNonOrderly,
    FinishRuntimeGenerationDrain, GenerationMutation, MailboxDb, RequestRuntimeGenerationDrain,
    RuntimeGenerationFence, RuntimeGenerationId, RuntimeLifecycleState, RuntimeTerminalReason,
    SessionRuntimeRunningUpdate,
};
use oulipoly_state::pid_identity::{
    self, ProcessIdentity, ProcessIdentityObservation, observe_live_process_identity,
};
use std::path::Path;

const AUTO_WAKE_ENV: &str = "OULIPOLY_AUTO_WAKE";
const PARENT_INVOCATION_ENV: &str = "OULIPOLY_PARENT_INVOCATION";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SpawnRuntimeMode {
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
pub(crate) struct SpawnIdentityContext {
    generation_id: RuntimeGenerationId,
    invocation_uuid: String,
    provider_name: String,
    model_name: Option<String>,
    session_id: Option<String>,
    mode: SpawnRuntimeMode,
    pty_control_path: Option<String>,
    effective_cwd: Option<String>,
    models_dir: Option<String>,
}

impl SpawnIdentityContext {
    pub(super) fn invocation_uuid(&self) -> &str {
        &self.invocation_uuid
    }

    pub(super) fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub(super) fn model_name(&self) -> Option<&str> {
        self.model_name.as_deref()
    }

    pub(super) fn effective_cwd(&self) -> Option<&str> {
        self.effective_cwd.as_deref()
    }

    pub(super) fn with_pty_control_path(&self, path: impl Into<String>) -> Self {
        let mut cloned = self.clone();
        cloned.pty_control_path = Some(path.into());
        cloned
    }
}

pub(crate) fn context_from_parent_invocation_env(
    parent_invocation_env: Option<&str>,
    provider_name: &str,
    model_name: Option<&str>,
    session_id: Option<&str>,
    mode: SpawnRuntimeMode,
    effective_cwd: Option<&Path>,
    models_dir: Option<&Path>,
) -> Option<SpawnIdentityContext> {
    let invocation = parse_parent_invocation_env(parent_invocation_env)?;
    Some(spawn_identity_context_from_invocation(
        invocation,
        provider_name,
        model_name,
        session_id,
        mode,
        effective_cwd,
        models_dir,
    ))
}

pub(crate) fn provider_parent_invocation_env(current: Option<&str>) -> Option<String> {
    let auto_wake = std::env::var(AUTO_WAKE_ENV).ok().as_deref() == Some("1");
    let inherited = std::env::var(PARENT_INVOCATION_ENV).ok();
    provider_parent_invocation_env_for(current, auto_wake, inherited.as_deref())
}

fn provider_parent_invocation_env_for(
    current: Option<&str>,
    auto_wake: bool,
    inherited: Option<&str>,
) -> Option<String> {
    if auto_wake
        && let Some(inherited) = inherited
        && CompositeInvocationId::parse_env_value(inherited).is_ok()
    {
        return Some(inherited.to_string());
    }
    current.map(str::to_string)
}

fn parse_parent_invocation_env(
    parent_invocation_env: Option<&str>,
) -> Option<CompositeInvocationId> {
    parent_invocation_env.and_then(parse_invocation_env_silent)
}

fn spawn_identity_context_from_invocation(
    invocation: CompositeInvocationId,
    provider_name: &str,
    model_name: Option<&str>,
    session_id: Option<&str>,
    mode: SpawnRuntimeMode,
    effective_cwd: Option<&Path>,
    models_dir: Option<&Path>,
) -> SpawnIdentityContext {
    SpawnIdentityContext {
        generation_id: RuntimeGenerationId::new(),
        invocation_uuid: invocation.id,
        provider_name: provider_name.to_string(),
        model_name: model_name.map(str::to_string),
        session_id: session_id.map(str::to_string),
        mode,
        pty_control_path: None,
        effective_cwd: effective_cwd.map(|path| path.to_string_lossy().into_owned()),
        models_dir: models_dir.map(|path| path.to_string_lossy().into_owned()),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RunningRuntimeGeneration {
    pub generation_id: RuntimeGenerationId,
    pub spawned_os_pid: i64,
    pub exact_process_identity: Option<ProcessIdentity>,
}

pub(crate) fn register_runtime_generation_starting(
    context: Option<&SpawnIdentityContext>,
) -> Result<(), String> {
    let Some(context) = context else {
        return Ok(());
    };
    let mut db = MailboxDb::open_default()?;
    recover_stale_session_generations(&mut db, context)?;
    match db
        .create_runtime_generation(CreateRuntimeGeneration {
            generation_id: &context.generation_id,
            spawn_invocation_uuid: &context.invocation_uuid,
            session_id: context.session_id.as_deref(),
            runtime_mode: context.mode.as_str(),
            provider_name: &context.provider_name,
            model_name: context.model_name.as_deref(),
            pty_control_path: context.pty_control_path.as_deref(),
            models_dir: context.models_dir.as_deref(),
            effective_cwd: context.effective_cwd.as_deref(),
        })
        .map_err(|err| err.to_string())?
    {
        GenerationMutation::Applied(_) | GenerationMutation::AlreadyApplied(_) => Ok(()),
        GenerationMutation::Rejected(rejection) => Err(format!(
            "Runtime generation starting registration rejected: {rejection:?}"
        )),
    }
}

fn recover_stale_session_generations(
    db: &mut MailboxDb,
    context: &SpawnIdentityContext,
) -> Result<(), String> {
    let Some(session_id) = context.session_id() else {
        return Ok(());
    };
    let generations = db
        .runtime_generation_history(session_id)
        .map_err(|err| err.to_string())?;
    for generation in generations {
        if generation.lifecycle_state == RuntimeLifecycleState::Exited {
            continue;
        }
        let ExactProcessEvidence::Recorded(identity) = &generation.exact_process_evidence else {
            continue;
        };
        let stale = match observe_live_process_identity(identity.os_pid) {
            ProcessIdentityObservation::ExactLive(live) => live != *identity,
            ProcessIdentityObservation::Dead => true,
            ProcessIdentityObservation::Unsupported | ProcessIdentityObservation::ReadError(_) => {
                false
            }
        };
        if !stale {
            continue;
        }
        let mutation = db
            .exit_runtime_generation_non_orderly(ExitRuntimeGenerationNonOrderly {
                fence: RuntimeGenerationFence {
                    generation_id: &generation.generation_id,
                    spawn_invocation_uuid: &generation.spawn_invocation_uuid,
                },
                reason: RuntimeTerminalReason::RecoveredDead,
                exit_code: None,
            })
            .map_err(|err| err.to_string())?;
        if let GenerationMutation::Rejected(rejection) = mutation {
            return Err(format!(
                "Stale runtime generation recovery rejected: {rejection:?}"
            ));
        }
    }
    let _ = db.session_liveness(session_id)?;
    Ok(())
}

pub(crate) fn record_child_identity(
    child_id: u32,
    context: Option<&SpawnIdentityContext>,
) -> Result<Option<RunningRuntimeGeneration>, String> {
    let Some(context) = context else {
        return Ok(None);
    };
    let os_pid = i64::from(child_id);
    let exact_process_identity = match pid_identity::read_live_process_identity(os_pid) {
        Ok(identity) => identity,
        Err(err) => {
            warn_child_identity_record_failed(context, child_id, &err);
            None
        }
    };
    let mut db = MailboxDb::open_default()?;
    let mutation = db
        .bind_runtime_generation_running(BindRuntimeGenerationRunning {
            fence: generation_fence(context),
            spawned_os_pid: os_pid,
            exact_process_identity: exact_process_identity.as_ref(),
            os_pgid: None,
        })
        .map_err(|err| err.to_string())?;
    match mutation {
        GenerationMutation::Applied(_) | GenerationMutation::AlreadyApplied(_) => {
            if let (Some(session_id), Some(identity)) = (
                context.session_id.as_deref(),
                exact_process_identity.as_ref(),
            ) {
                mark_session_running_with_session_id(context, session_id, identity);
            }
            Ok(Some(RunningRuntimeGeneration {
                generation_id: context.generation_id.clone(),
                spawned_os_pid: os_pid,
                exact_process_identity,
            }))
        }
        GenerationMutation::Rejected(rejection) => Err(format!(
            "Runtime generation child binding rejected: {rejection:?}"
        )),
    }
}

pub(crate) fn backfill_captured_session_id(
    context: Option<&SpawnIdentityContext>,
    generation: Option<&RunningRuntimeGeneration>,
    session_id: &str,
) -> Result<(), String> {
    let (Some(context), Some(generation)) = (context, generation) else {
        return Ok(());
    };
    if generation.generation_id != context.generation_id {
        return Err("Captured session generation does not match its spawn context".to_string());
    }
    let mut db = MailboxDb::open_default()?;
    match db
        .attach_runtime_generation_session(AttachRuntimeGenerationSession {
            fence: generation_fence(context),
            session_id,
        })
        .map_err(|err| err.to_string())?
    {
        GenerationMutation::Applied(_) | GenerationMutation::AlreadyApplied(_) => {}
        GenerationMutation::Rejected(rejection) => {
            return Err(format!(
                "Runtime generation session attachment rejected: {rejection:?}"
            ));
        }
    }
    if let Some(identity) = generation.exact_process_identity.as_ref() {
        mark_session_running_with_session_id(context, session_id, identity);
    }
    Ok(())
}

fn warn_child_identity_record_failed(context: &SpawnIdentityContext, child_id: u32, err: &str) {
    tracing::warn!(
        invocation_uuid = %context.invocation_uuid,
        child_pid = child_id,
        "Failed to record PID identity sidecar row: {err}"
    );
}

fn mark_session_running_with_session_id(
    context: &SpawnIdentityContext,
    session_id: &str,
    identity: &ProcessIdentity,
) {
    match MailboxDb::open_default().and_then(|mut db| {
        db.mark_session_running(session_runtime_running_update(
            context, session_id, identity,
        ))
    }) {
        Ok(()) => {}
        Err(err) => warn_mark_session_running_failed(context, session_id, &err),
    }
}

pub(crate) fn mark_runtime_generation_spawn_failed(
    context: Option<&SpawnIdentityContext>,
) -> Result<(), String> {
    exit_runtime_generation(context, RuntimeTerminalReason::StartupFailed, None)
}

pub(crate) fn mark_runtime_generation_exited(
    context: Option<&SpawnIdentityContext>,
    exit_code: Option<i32>,
) -> Result<(), String> {
    exit_runtime_generation(
        context,
        RuntimeTerminalReason::AbnormalTermination,
        exit_code,
    )
}

pub(crate) fn mark_runtime_generation_orderly_completed(
    context: Option<&SpawnIdentityContext>,
    exit_code: Option<i32>,
) -> Result<(), String> {
    let Some(context) = context else {
        return Ok(());
    };
    let drain_request_id = DrainRequestId::new();
    let mut db = MailboxDb::open_default()?;
    let handoff = match db
        .request_runtime_generation_drain(RequestRuntimeGenerationDrain {
            fence: generation_fence(context),
            drain_request_id: &drain_request_id,
            requested_by_invocation_uuid: &context.invocation_uuid,
        })
        .map_err(|err| err.to_string())?
    {
        DrainRequestResult::Installed(_, handoff)
        | DrainRequestResult::AlreadyInstalled(_, handoff) => handoff,
        DrainRequestResult::Rejected(rejection) => {
            return Err(format!(
                "Runtime generation drain request rejected: {rejection:?}"
            ));
        }
    };
    if matches!(handoff, DrainHandoff::ClaimOutstanding { .. }) {
        drop(db);
        return exit_runtime_generation(
            Some(context),
            RuntimeTerminalReason::AbnormalTermination,
            exit_code,
        );
    }
    match db
        .advance_runtime_generation_drain(AdvanceRuntimeGenerationDrain {
            fence: generation_fence(context),
            drain_request_id: &drain_request_id,
        })
        .map_err(|err| err.to_string())?
    {
        DrainAdvanceResult::Advanced(_) | DrainAdvanceResult::AlreadyDraining(_) => {}
        DrainAdvanceResult::WaitingOnClaim(_) => {
            drop(db);
            return exit_runtime_generation(
                Some(context),
                RuntimeTerminalReason::AbnormalTermination,
                exit_code,
            );
        }
        DrainAdvanceResult::AlreadyExited(_) => return Ok(()),
        DrainAdvanceResult::Rejected(rejection) => {
            return Err(format!(
                "Runtime generation drain advance rejected: {rejection:?}"
            ));
        }
    }
    match db
        .finish_runtime_generation_drain(FinishRuntimeGenerationDrain {
            fence: generation_fence(context),
            drain_request_id: &drain_request_id,
            exit_code,
        })
        .map_err(|err| err.to_string())?
    {
        DrainFinishResult::Finished(_) | DrainFinishResult::AlreadyExited(_) => Ok(()),
        DrainFinishResult::NotDraining(actual) => Err(format!(
            "Runtime generation was {actual:?} while finishing orderly drain"
        )),
        DrainFinishResult::Rejected(rejection) => Err(format!(
            "Runtime generation drain finish rejected: {rejection:?}"
        )),
    }
}

fn exit_runtime_generation(
    context: Option<&SpawnIdentityContext>,
    reason: RuntimeTerminalReason,
    exit_code: Option<i32>,
) -> Result<(), String> {
    let Some(context) = context else {
        return Ok(());
    };
    let mut db = MailboxDb::open_default()?;
    match db
        .exit_runtime_generation_non_orderly(ExitRuntimeGenerationNonOrderly {
            fence: generation_fence(context),
            reason,
            exit_code,
        })
        .map_err(|err| err.to_string())?
    {
        GenerationMutation::Applied(_) | GenerationMutation::AlreadyApplied(_) => Ok(()),
        GenerationMutation::Rejected(rejection) => {
            Err(format!("Runtime generation exit rejected: {rejection:?}"))
        }
    }
}

fn generation_fence(context: &SpawnIdentityContext) -> RuntimeGenerationFence<'_> {
    RuntimeGenerationFence {
        generation_id: &context.generation_id,
        spawn_invocation_uuid: &context.invocation_uuid,
    }
}

fn session_runtime_running_update<'a>(
    context: &'a SpawnIdentityContext,
    session_id: &'a str,
    identity: &'a ProcessIdentity,
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
        models_dir: context.models_dir.as_deref(),
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

#[cfg(test)]
mod tests {
    use super::provider_parent_invocation_env_for;

    const CURRENT: &str = r#"{"source":"opencode3","id":"11111111-1111-4111-8111-111111111111"}"#;
    const OWNER: &str = r#"{"source":"opencode3","id":"22222222-2222-4222-8222-222222222222"}"#;

    #[test]
    fn auto_wake_provider_keeps_inherited_semantic_owner() {
        assert_eq!(
            provider_parent_invocation_env_for(Some(CURRENT), true, Some(OWNER)).as_deref(),
            Some(OWNER)
        );
    }

    #[test]
    fn ordinary_provider_uses_current_invocation() {
        assert_eq!(
            provider_parent_invocation_env_for(Some(CURRENT), false, Some(OWNER)).as_deref(),
            Some(CURRENT)
        );
    }

    #[test]
    fn auto_wake_rejects_malformed_inherited_owner() {
        assert_eq!(
            provider_parent_invocation_env_for(Some(CURRENT), true, Some("not-json")).as_deref(),
            Some(CURRENT)
        );
    }
}
