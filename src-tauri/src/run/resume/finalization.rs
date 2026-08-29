//! ## Declared roles
//!
//! `orchestration`, `accessor`, `mapper`, `predicate`, `formatter`

use std::path::Path;

use oulipoly_config::ModelConfig;
use oulipoly_runtime::services::InvocationLifecycleServicePort;
use oulipoly_state::CompositeInvocationId;

use super::{formatter, mapper};
use crate::invocation::finalize::{
    FinalizerGuard, finalize_retained_outcome_with_contention_retry,
};
use crate::migration_providers::ResumeExecutionEnvironment;
use crate::quota_zero_turn::resume_result_error_category;
use crate::session_ingest_cli::{
    ResumeIngestMode, SessionIngestRequest, ingest_and_emit_session_id_resume_aware,
};
use crate::wiring;

pub(super) enum CompletedAttemptControl {
    Continue,
    Return(i32),
}

#[derive(Clone, Copy)]
pub(super) struct ConfirmedDeliverySettlement<'a> {
    pub(super) delivery_id: &'a str,
    pub(super) session_id: &'a str,
    pub(super) turn_generation_id: &'a str,
    pub(super) submitted_evidence: &'a str,
    pub(super) confirmed_evidence: &'a str,
    pub(super) observed_at: i64,
}

pub(super) struct CompletedAttemptInput<'a, 'state> {
    pub(super) agent_runtime_services: &'a wiring::AgentRuntimeServices,
    pub(super) env: &'a ResumeExecutionEnvironment,
    pub(super) invocation: &'a CompositeInvocationId,
    pub(super) invocation_row_id: i64,
    pub(super) guard: &'a mut FinalizerGuard<'state>,
    pub(super) provider_name: &'a str,
    pub(super) provider_session_id: &'a str,
    pub(super) model: Option<&'a ModelConfig>,
    pub(super) result: &'a oulipoly_runtime::executor::ExecutionResult,
    pub(super) working_dir: Option<&'a Path>,
    pub(super) manual_migrate: Option<&'a str>,
    pub(super) session_id: &'a str,
    pub(super) active_session_id: &'a str,
    pub(super) effective_spawn_cwd: &'a Path,
    pub(super) attempts: usize,
    pub(super) max_attempts: usize,
    pub(super) recovered_generic_nonzero: bool,
    pub(super) terminal_completion_confirmed: bool,
    pub(super) confirmed_delivery: Option<ConfirmedDeliverySettlement<'a>>,
}

pub(super) fn finalize_completed_attempt(
    mut input: CompletedAttemptInput<'_, '_>,
) -> Result<CompletedAttemptControl, String> {
    let success = input.recovered_generic_nonzero
        || super::predicate::completed_attempt_success(
            input.result,
            input.terminal_completion_confirmed,
        );
    let error_category = (!input.recovered_generic_nonzero)
        .then(|| completed_attempt_error_category(&input))
        .flatten();
    let quota_exhausted =
        super::predicate::completed_attempt_quota_exhausted(error_category.as_deref());

    if input.confirmed_delivery.is_none()
        && let Err(err) = persist_returned_artifacts(&input)
    {
        return Ok(handle_returned_artifacts_persist_failure(&mut input, err));
    }

    finalize_regular_completed_attempt(&mut input, success, error_category.as_deref())?;

    if success {
        return Ok(handle_completed_success(&input, error_category.as_deref()));
    }

    if quota_exhausted {
        return Ok(handle_quota_exhausted(&input));
    }

    Ok(handle_completed_failure(&input, error_category.as_deref()))
}

fn completed_attempt_error_category(input: &CompletedAttemptInput<'_, '_>) -> Option<String> {
    resume_result_error_category(
        input.agent_runtime_services,
        input.result,
        &input.env.models,
        input.working_dir,
    )
}

fn persist_returned_artifacts(input: &CompletedAttemptInput<'_, '_>) -> Result<(), String> {
    input
        .env
        .state
        .record_returned_artifacts(input.invocation_row_id, &input.result.returned_artifacts)
        .map_err(|err| err.to_string())
}

fn handle_returned_artifacts_persist_failure(
    input: &mut CompletedAttemptInput<'_, '_>,
    err: String,
) -> CompletedAttemptControl {
    formatter::emit_returned_artifacts_error(&err);
    input
        .agent_runtime_services
        .invocation_lifecycle_service
        .finalize_invocation(mapper::returned_artifacts_finalize_request(
            &input.env.state,
            input.invocation_row_id,
        ))
        .map(|_| ())
        .unwrap_or_else(formatter::emit_finalize_invocation_warning);
    input.guard.mark_finalized();
    CompletedAttemptControl::Return(1)
}

fn finalize_regular_completed_attempt(
    input: &mut CompletedAttemptInput<'_, '_>,
    success: bool,
    error_category: Option<&str>,
) -> Result<(), String> {
    if let Some(settlement) = input.confirmed_delivery {
        finalize_confirmed_delivery(
            &input.env.state,
            input.invocation_row_id,
            input.result,
            success,
            error_category,
            input.result.terminal_reason.as_deref(),
            settlement,
        )?;
        input.guard.mark_finalized();
        return Ok(());
    }
    let finalize_result = finalize_retained_outcome_with_contention_retry(
        input
            .agent_runtime_services
            .invocation_lifecycle_service
            .as_ref(),
        mapper::finalize_request(
            &input.env.state,
            input.invocation_row_id,
            success,
            input.result.exit_code,
            error_category,
            input.result.terminal_reason.as_deref(),
        ),
    );
    if let Err(error) = &finalize_result
        && success
    {
        input.guard.preserve_running_after_process_integrity(error);
    }
    finalize_result.map_err(|err| err.to_string())?;
    input.guard.mark_finalized();
    Ok(())
}

pub(super) fn finalize_confirmed_delivery(
    state: &oulipoly_state::StateDb,
    invocation_row_id: i64,
    result: &oulipoly_runtime::executor::ExecutionResult,
    success: bool,
    error_category: Option<&str>,
    terminal_reason: Option<&str>,
    settlement: ConfirmedDeliverySettlement<'_>,
) -> Result<(), String> {
    let delivery_ids = [settlement.delivery_id.to_string()];
    let acceptance = result.resume_acceptance.as_ref();
    state.apply_provider_turn_effects(oulipoly_state::ProviderTurnEffectInput {
        invocation_row_id,
        delivery_ids: &delivery_ids,
        accept_delivery_if_missing: true,
        session_id: settlement.session_id,
        turn_generation_id: settlement.turn_generation_id,
        submitted_evidence: Some(settlement.submitted_evidence),
        confirmed_evidence: Some(settlement.confirmed_evidence),
        observed_at: settlement.observed_at,
        returned_artifacts: &result.returned_artifacts,
        resume_acceptance_status: acceptance.map(|value| value.status.db_value()),
        resume_acceptance_evidence: acceptance.and_then(|value| value.evidence.as_deref()),
        success,
        exit_code: result.exit_code,
        error_category,
        terminal_reason,
    })?;
    Ok(())
}

fn handle_completed_success(
    input: &CompletedAttemptInput<'_, '_>,
    error_category: Option<&str>,
) -> CompletedAttemptControl {
    ingest_and_emit_session_id_resume_aware(
        input.agent_runtime_services,
        SessionIngestRequest {
            state: &input.env.state,
            sessions_cfg: &input.env.sessions_cfg,
            providers_cfg: Some(&input.env.providers_cfg),
            provider_name: input.provider_name,
            external_provider: crate::session_ingest_cli::session_external_provider_identity(
                input.agent_runtime_services,
                input.model,
                input.provider_name,
            ),
            invocation_row_id: input.invocation_row_id,
            invocation_uuid: &input.invocation.id,
            effective_cwd: Some(input.effective_spawn_cwd),
            mode: ResumeIngestMode::Pinned {
                resume_target: super::filter::resumed_session_target(
                    input.manual_migrate,
                    input.session_id,
                    input.active_session_id,
                ),
            },
        },
    );
    formatter::emit_resume_success_output(
        &input.invocation.id,
        input.result.exit_code,
        error_category,
        input.result.terminal_reason.as_deref(),
        &input.result.stdout,
    );
    CompletedAttemptControl::Return(if input.recovered_generic_nonzero {
        0
    } else {
        input.result.exit_code
    })
}

fn handle_quota_exhausted(input: &CompletedAttemptInput<'_, '_>) -> CompletedAttemptControl {
    if super::predicate::retry_available(input.attempts, input.max_attempts) {
        formatter::emit_routing_retry(input.provider_name);
    }
    CompletedAttemptControl::Continue
}

fn handle_completed_failure(
    input: &CompletedAttemptInput<'_, '_>,
    error_category: Option<&str>,
) -> CompletedAttemptControl {
    formatter::emit_resume_failure_output(formatter::ResumeFailureOutputInput {
        state: &input.env.state,
        invocation_id: &input.invocation.id,
        provider_name: input.provider_name,
        provider_session_id: input.provider_session_id,
        exit_code: input.result.exit_code,
        error_category,
        terminal_reason: input.result.terminal_reason.as_deref(),
        stderr: &input.result.stderr,
    });
    if let Some(category) = error_category {
        formatter::emit_diagnostics_category(category);
    }
    CompletedAttemptControl::Return(mapper::failure_exit_code(input.result.exit_code))
}
