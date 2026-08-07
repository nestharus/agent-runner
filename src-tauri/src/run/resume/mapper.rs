//! mapper

use oulipoly_runtime::services::{
    InvocationLifecycleFinalizeRequest, InvocationLifecycleStartRequest, MigrationServiceRequest,
    ResumeAcceptanceRequest, ResumeServiceRequest,
};
use oulipoly_state::{CompositeInvocationId, InvocationStart, ProviderSessionBinding, StateDb};
use uuid::Uuid;

use crate::migration_providers::ResumeExecutionEnvironment;
use crate::terminal_outcome_adapter::TerminalSignalContext;

pub(super) fn prepared_headless_resume_execution(
    mailbox_delivery: crate::mailbox_delivery::PreparedMailboxDelivery,
    env: ResumeExecutionEnvironment,
    resolved: oulipoly_state::ResolvedResume,
    effective_spawn_cwd: std::path::PathBuf,
    parent_invocation_id: Option<i64>,
    max_attempts: usize,
) -> super::execution::PreparedHeadlessResumeExecution {
    super::execution::PreparedHeadlessResumeExecution {
        answer: mailbox_delivery.answer,
        mailbox_session_id: mailbox_delivery.session_id,
        mailbox_delivery_seqs: mailbox_delivery.seqs,
        mailbox_delivery_nonce: mailbox_delivery.delivery_nonce,
        mailbox_delivery_requires_turn_confirmation: mailbox_delivery.requires_turn_confirmation,
        env,
        resolved,
        effective_spawn_cwd,
        parent_invocation_id,
        max_attempts,
    }
}

pub(super) fn resume_provider_registry(
    models: &[oulipoly_config::ModelConfig],
    options: oulipoly_runtime::provider_registry::ProviderRegistryOptions,
) -> Result<oulipoly_runtime::provider_registry::ProviderRegistry, String> {
    oulipoly_runtime::provider_registry::ProviderRegistry::from_model_configs(models, options)
        .map_err(|error| error.to_string())
}

pub(super) fn resume_provider_models(
    models: &std::collections::HashMap<String, oulipoly_config::ModelConfig>,
) -> Vec<oulipoly_config::ModelConfig> {
    models.values().cloned().collect()
}

pub(super) fn apply_migrated_resume_segment(
    resolved: &mut oulipoly_state::ResolvedResume,
    target: &mut crate::resume_cli::ResumeExecutionTarget,
    migrated: oulipoly_runtime::migration::MigratedSegment,
    providers_cfg: &oulipoly_config::ProvidersConfig,
) -> Result<(), i32> {
    resolved.active_provider = migrated.target_provider;
    resolved.active_session_id = migrated.target_session_id;
    *target = crate::resume_cli::renderable_resume_execution_target(resolved, providers_cfg)?;
    Ok(())
}

pub(super) fn legacy_resume_payload<'a>(
    session_id: &'a str,
    strategy: &'a oulipoly_config::ResumeStrategy,
) -> oulipoly_runtime::executor::cli::ResumePayload<'a> {
    oulipoly_runtime::executor::cli::ResumePayload {
        session_id,
        strategy,
    }
}

pub(super) fn resume_invocation_attempt<'state>(
    invocation: CompositeInvocationId,
    invocation_row_id: i64,
    guard: crate::invocation::finalize::FinalizerGuard<'state>,
) -> super::lifecycle::ResumeInvocationAttempt<'state> {
    super::lifecycle::ResumeInvocationAttempt {
        invocation,
        invocation_row_id,
        guard,
    }
}

pub(super) fn bound_resume_attempt<'state>(
    attempt: super::lifecycle::ResumeInvocationAttempt<'state>,
    provider_session_id: String,
    zero_turn_baseline: crate::zero_turn_orchestration::ZeroTurnBaseline,
    invocation_env: String,
) -> super::lifecycle::BoundResumeAttempt<'state> {
    super::lifecycle::BoundResumeAttempt {
        attempt,
        provider_session_id,
        zero_turn_baseline,
        invocation_env,
    }
}

pub(super) fn resume_terminal_disposition_outcome(
    control: super::disposition::ResumeLoopControl,
    exit_code: i32,
) -> super::terminal::ResumeTerminalDispositionOutcome {
    match control {
        super::disposition::ResumeLoopControl::Continue => {
            super::terminal::ResumeTerminalDispositionOutcome::Continue(exit_code)
        }
        super::disposition::ResumeLoopControl::Return(exit_code) => {
            super::terminal::ResumeTerminalDispositionOutcome::Return(exit_code)
        }
        super::disposition::ResumeLoopControl::CompletedAttempt => {
            super::terminal::ResumeTerminalDispositionOutcome::CompletedAttempt
        }
    }
}

pub(super) fn failure_exit_code(exit_code: i32) -> i32 {
    if exit_code == 0 { 1 } else { exit_code }
}

pub(super) fn resume_service_request<'a>(
    env: &'a ResumeExecutionEnvironment,
    session_id: &'a str,
    model_name: Option<&'a str>,
) -> ResumeServiceRequest<'a> {
    ResumeServiceRequest {
        state: &env.state,
        models: &env.models,
        providers_cfg: &env.providers_cfg,
        input: session_id,
        model_override: model_name,
    }
}

pub(super) struct ResumeMigrationRequestInput<'a> {
    pub(super) env: &'a ResumeExecutionEnvironment,
    pub(super) resolved: &'a oulipoly_state::ResolvedResume,
    pub(super) manual_target: Option<&'a str>,
    pub(super) migration_model: &'a oulipoly_config::ModelConfig,
    pub(super) effective_cwd: &'a std::path::Path,
    pub(super) stderr: &'a mut std::io::Stderr,
    pub(super) active_exhausted: bool,
}

pub(super) fn migration_service_request<'a>(
    input: ResumeMigrationRequestInput<'a>,
) -> MigrationServiceRequest<'a> {
    MigrationServiceRequest {
        state: &input.env.state,
        sessions_cfg: &input.env.sessions_cfg,
        resolved: input.resolved,
        manual_target: input.manual_target,
        active_exhausted: input.active_exhausted,
        migration_model: input.migration_model,
        effective_cwd: input.effective_cwd,
        stderr: input.stderr,
    }
}

pub(super) fn composite_invocation_id(provider_name: &str) -> CompositeInvocationId {
    CompositeInvocationId {
        source: provider_name.to_string(),
        id: Uuid::new_v4().to_string(),
    }
}

pub(super) fn resume_invocation_start(
    invocation: &CompositeInvocationId,
    model_name: Option<&str>,
    provider_name: &str,
    provider_index: usize,
    parent_invocation_id: Option<i64>,
) -> InvocationStart {
    InvocationStart {
        invocation_uuid: invocation.id.clone(),
        model_name: model_name.unwrap_or("<unknown>").to_string(),
        provider_name: provider_name.to_string(),
        provider_index,
        parent_invocation_id,
    }
}

pub(super) fn invocation_lifecycle_start_request<'a>(
    state: &'a StateDb,
    start: &'a InvocationStart,
) -> InvocationLifecycleStartRequest<'a> {
    InvocationLifecycleStartRequest { state, start }
}

pub(super) fn resumed_provider_session_binding(
    provider: &oulipoly_config::ProviderConfig,
    provider_session_id: &str,
    resume_input_id: Option<String>,
) -> ProviderSessionBinding {
    ProviderSessionBinding {
        provider_session_id: provider_session_id.to_string(),
        capture_method: "resumed",
        resume_input_id,
        provider_session_resolved_account:
            crate::migration_providers::provider_session_resolved_account(
                provider,
                provider_session_id,
            ),
    }
}

pub(super) fn finalize_request<'a>(
    state: &'a StateDb,
    invocation_row_id: i64,
    success: bool,
    exit_code: i32,
    error_category: Option<&'a str>,
    terminal_reason: Option<&'a str>,
) -> InvocationLifecycleFinalizeRequest<'a> {
    InvocationLifecycleFinalizeRequest {
        state,
        invocation_row_id,
        success,
        exit_code,
        error_category,
        terminal_reason,
    }
}

pub(super) fn spawn_error_finalize_request(
    state: &StateDb,
    invocation_row_id: i64,
) -> InvocationLifecycleFinalizeRequest<'_> {
    finalize_request(
        state,
        invocation_row_id,
        false,
        1,
        Some("spawn_error"),
        Some("spawn_error"),
    )
}

pub(super) fn returned_artifacts_finalize_request(
    state: &StateDb,
    invocation_row_id: i64,
) -> InvocationLifecycleFinalizeRequest<'_> {
    finalize_request(
        state,
        invocation_row_id,
        false,
        1,
        Some("returned_artifacts"),
        Some("returned_artifacts_persist_failed"),
    )
}

pub(super) fn resume_acceptance_request<'a>(
    state: &'a StateDb,
    invocation_row_id: i64,
    acceptance: &'a oulipoly_runtime::executor::ResumeAcceptanceResult,
) -> ResumeAcceptanceRequest<'a> {
    ResumeAcceptanceRequest {
        state,
        invocation_row_id,
        status: acceptance.status.db_value(),
        evidence: acceptance.evidence.as_deref(),
    }
}

pub(super) struct TerminalSignalContextIds {
    pub(super) invocation_uuid: Uuid,
    pub(super) session_uuid: Option<Uuid>,
}

pub(super) fn terminal_signal_context_ids(
    invocation_id: &str,
    provider_session_id: Option<&str>,
) -> TerminalSignalContextIds {
    TerminalSignalContextIds {
        invocation_uuid: Uuid::parse_str(invocation_id)
            .expect("generated invocation id must be a UUID"),
        session_uuid: crate::dispatch::provider_session_marker_uuid(provider_session_id),
    }
}

pub(super) fn terminal_signal_context_for_attempt<'a, W: std::io::Write>(
    ids: &'a TerminalSignalContextIds,
    provider_name: &'a str,
    state: &'a StateDb,
    stderr: &'a mut W,
) -> TerminalSignalContext<'a, W> {
    TerminalSignalContext {
        invocation_id: &ids.invocation_uuid,
        session_id: ids.session_uuid.as_ref(),
        provider: provider_name,
        state_db: state,
        stderr,
    }
}
