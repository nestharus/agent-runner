//! ## Declared roles
//!
//! `orchestration`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/run/resume/orchestration.rs
//!     role: adapter
//!     Translates:
//!       - resume-execution-preparation-contract
//!       - resume-attempt-lifecycle-contract
//!       - resume-terminal-disposition-contract
//!       - resume-wake-contract
//! ```

use std::path::Path;

use super::{execution, formatter, lifecycle, terminal, wake};
use crate::migration_providers::ResumeExecutionEnvironment;
use crate::run::reservation::ReservedRun;
use crate::wiring;
use crate::zero_turn_orchestration::ZeroTurnConfirmationState;

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_resume(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    model_name: Option<&str>,
    session_id: &str,
    target_kind: oulipoly_state::InboxTargetKind,
    manual_migrate: Option<&str>,
    prompt: Option<&str>,
    file: Option<&Path>,
    submission_token: Option<&str>,
    working_dir: Option<&Path>,
    models_dir_override: Option<&Path>,
) -> Result<i32, String> {
    let mut prepared = match prepare_resume(
        agent_runtime_services,
        model_name,
        session_id,
        target_kind,
        prompt,
        file,
        submission_token,
        working_dir,
        models_dir_override,
    )? {
        Ok(prepared) => prepared,
        Err(exit_code) => return Ok(exit_code),
    };
    run_prepared_resume(
        agent_runtime_services,
        &mut prepared,
        None,
        manual_migrate,
        session_id,
        working_dir,
    )
}

#[allow(clippy::too_many_arguments)]
pub(in crate::run) fn prepare_resume(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    model_name: Option<&str>,
    session_id: &str,
    target_kind: oulipoly_state::InboxTargetKind,
    prompt: Option<&str>,
    file: Option<&Path>,
    submission_token: Option<&str>,
    working_dir: Option<&Path>,
    models_dir_override: Option<&Path>,
) -> Result<Result<execution::PreparedHeadlessResumeExecution, i32>, String> {
    if let Some(exit_code) = execution::reject_invalid_resume_input(session_id) {
        return Ok(Err(exit_code));
    }
    match prepare_resume_wake(session_id) {
        Ok(Some(exit_code)) => return Ok(Err(exit_code)),
        Ok(None) => {}
        Err(err) => {
            wake::release_claim_after_wake_preparation_error(session_id);
            return Err(err);
        }
    }
    let result = prepare_resume_inner(
        agent_runtime_services,
        model_name,
        session_id,
        target_kind,
        prompt,
        file,
        submission_token,
        working_dir,
        models_dir_override,
    );
    if !matches!(&result, Ok(Ok(_))) {
        wake::release_current_auto_wake_claim(session_id);
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn prepare_resume_inner(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    model_name: Option<&str>,
    session_id: &str,
    target_kind: oulipoly_state::InboxTargetKind,
    prompt: Option<&str>,
    file: Option<&Path>,
    submission_token: Option<&str>,
    working_dir: Option<&Path>,
    models_dir_override: Option<&Path>,
) -> Result<Result<execution::PreparedHeadlessResumeExecution, i32>, String> {
    // Source guard marker: agent_runtime_services.resume_service.resolve_resume(ResumeServiceRequest)

    match execution::prepare_headless_resume_execution(
        agent_runtime_services,
        model_name,
        session_id,
        target_kind,
        prompt,
        file,
        submission_token,
        working_dir,
        models_dir_override,
    )? {
        Ok(prepared) => Ok(Ok(prepared)),
        Err(exit_code) => Ok(Err(exit_code)),
    }
}

pub(in crate::run) fn run_prepared_resume(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    prepared: &mut execution::PreparedHeadlessResumeExecution,
    reservation: Option<&ReservedRun>,
    manual_migrate: Option<&str>,
    session_id: &str,
    working_dir: Option<&Path>,
) -> Result<i32, String> {
    execution::validate_reserved_resume_options(reservation, manual_migrate)?;
    let result = run_resume_loop(ResumeLoopInput {
        agent_runtime_services,
        prepared,
        reservation,
        manual_migrate,
        session_id,
        working_dir,
    });
    wake::recheck_after_failed_auto_wake(session_id, &result);
    result
}

struct ResumeLoopInput<'a> {
    agent_runtime_services: &'a wiring::AgentRuntimeServices,
    prepared: &'a mut execution::PreparedHeadlessResumeExecution,
    reservation: Option<&'a ReservedRun>,
    manual_migrate: Option<&'a str>,
    session_id: &'a str,
    working_dir: Option<&'a Path>,
}

fn run_resume_loop(input: ResumeLoopInput<'_>) -> Result<i32, String> {
    let mut attempts = 0usize;
    let mut last_exit_code = 1;
    let mut zero_turn_confirmation = ZeroTurnConfirmationState::new();

    loop {
        let max_attempts = super::max_attempts(input.prepared.max_attempts, input.reservation);
        if terminal::resume_attempts_exhausted(attempts, max_attempts) {
            return Ok(terminal::resume_attempts_exhausted_exit_code(
                last_exit_code,
            ));
        }
        attempts += 1;

        let registration = super::composite_invocation_id(
            &input.prepared.resolved.active_provider,
            input.reservation,
        )
        .id;
        let mut admitted_session = input.prepared.resolved.active_session_id.clone();
        let mut admission =
            crate::wake_coordinator::admit_session_launch(&registration, Some(&admitted_session))?;
        // A chain can advance during the queued wait. Refresh before publishing
        // tokenized input or selecting a notification batch, not afterward.
        if attempts == 1 {
            loop {
                execution::refresh_admitted_resume(
                    input.agent_runtime_services,
                    input.prepared,
                    input.session_id,
                    input.working_dir,
                )?;
                if input.prepared.resolved.active_session_id == admitted_session {
                    break;
                }
                admitted_session = input.prepared.resolved.active_session_id.clone();
                admission = admission.retarget(&admitted_session)?;
            }
        }
        wake::reset_manual_resume_wake_claim(&admitted_session)?;
        let mut target = crate::resume_cli::renderable_resume_execution_target(
            &input.prepared.resolved,
            &input.prepared.env.providers_cfg,
        )
        .map_err(|code| format!("resume target unavailable: {code}"))?;
        if super::migration_allowed(input.reservation) {
            super::migration::migrate_resume_target(
                input.agent_runtime_services,
                &input.prepared.env,
                &mut input.prepared.resolved,
                &mut target,
                input.manual_migrate,
                attempts,
                &input.prepared.effective_spawn_cwd,
            )
            .map_err(|code| format!("resume migration failed: {code}"))?;
        }
        while input.prepared.resolved.active_session_id != admitted_session {
            admitted_session = input.prepared.resolved.active_session_id.clone();
            admission = admission.retarget(&admitted_session)?;
            execution::refresh_admitted_resume(
                input.agent_runtime_services,
                input.prepared,
                input.session_id,
                input.working_dir,
            )?;
            target = crate::resume_cli::renderable_resume_execution_target(
                &input.prepared.resolved,
                &input.prepared.env.providers_cfg,
            )
            .map_err(|code| format!("migrated resume target unavailable: {code}"))?;
        }
        execution::prepare_admitted_delivery(input.prepared)?;
        if crate::wake_coordinator::is_auto_wake_invocation()
            && input.prepared.mailbox_delivery_seqs.is_empty()
            && input.prepared.answer.is_none()
        {
            wake::release_current_auto_wake_claim(input.session_id);
            return Ok(0);
        }
        let _admission = admission;
        match run_resume_attempt(
            ResumeAttemptInput {
                agent_runtime_services: input.agent_runtime_services,
                env: &input.prepared.env,
                resolved: &mut input.prepared.resolved,
                answer: input.prepared.answer.as_deref(),
                mailbox_session_id: &input.prepared.mailbox_session_id,
                mailbox_delivery_seqs: &input.prepared.mailbox_delivery_seqs,
                mailbox_delivery_nonce: input.prepared.mailbox_delivery_nonce.as_deref(),
                mailbox_delivery_requires_turn_confirmation: input
                    .prepared
                    .mailbox_delivery_requires_turn_confirmation,
                manual_migrate: input.manual_migrate,
                reservation: input.reservation,
                session_id: input.session_id,
                working_dir: input.working_dir,
                attempts,
                max_attempts,
                parent_invocation_id: super::parent_invocation_row_id(
                    input.prepared.parent_invocation_id,
                    input.reservation,
                ),
                effective_spawn_cwd: &input.prepared.effective_spawn_cwd,
                zero_turn_confirmation: &mut zero_turn_confirmation,
                provider_prompt_accepted: &mut input.prepared.provider_prompt_accepted,
            },
            target,
            &registration,
        )? {
            ResumeAttemptLoopControl::Continue(exit_code) => last_exit_code = exit_code,
            ResumeAttemptLoopControl::Return(exit_code) => return Ok(exit_code),
        }
    }
}

pub(super) struct ResumeAttemptInput<'a> {
    pub(super) agent_runtime_services: &'a wiring::AgentRuntimeServices,
    pub(super) env: &'a ResumeExecutionEnvironment,
    pub(super) resolved: &'a mut oulipoly_state::ResolvedResume,
    pub(super) answer: Option<&'a str>,
    pub(super) mailbox_session_id: &'a str,
    pub(super) mailbox_delivery_seqs: &'a [i64],
    pub(super) mailbox_delivery_nonce: Option<&'a str>,
    pub(super) mailbox_delivery_requires_turn_confirmation: bool,
    pub(super) manual_migrate: Option<&'a str>,
    pub(super) reservation: Option<&'a ReservedRun>,
    pub(super) session_id: &'a str,
    pub(super) working_dir: Option<&'a Path>,
    pub(super) attempts: usize,
    pub(super) max_attempts: usize,
    pub(super) parent_invocation_id: Option<i64>,
    pub(super) effective_spawn_cwd: &'a Path,
    pub(super) zero_turn_confirmation: &'a mut ZeroTurnConfirmationState,
    pub(super) provider_prompt_accepted: &'a mut bool,
}

pub(super) enum ResumeAttemptLoopControl {
    Continue(i32),
    Return(i32),
}

fn run_resume_attempt(
    mut input: ResumeAttemptInput<'_>,
    target: crate::resume_cli::ResumeExecutionTarget,
    registration_identity: &str,
) -> Result<ResumeAttemptLoopControl, String> {
    execution::validate_reserved_resume_options(input.reservation, input.manual_migrate)?;
    let provider_index = target.provider_index;
    let provider = target.provider;
    let account_endpoint_configured = input
        .agent_runtime_services
        .provider_registry_handle
        .current()
        .has_account_endpoint(&provider.name);
    let strategy = match resolve_resume_attempt_strategy(&provider, account_endpoint_configured) {
        Ok(strategy) => strategy,
        Err(exit_code) => return Ok(ResumeAttemptLoopControl::Return(exit_code)),
    };
    let mut bound_attempt = lifecycle::setup_bound_resume_attempt_with_identity(
        &input,
        &provider,
        provider_index,
        registration_identity,
    )?;
    wake::bind_headless_resume_delivery_attempt(
        &input,
        &provider,
        &bound_attempt.attempt.invocation.id,
    )?;
    let _receipt_observer = if input.mailbox_delivery_seqs.is_empty() {
        None
    } else {
        Some(crate::native_receipt::start_headless_receipt_polling()?)
    };
    let execution = if let Some(allocation) = bound_attempt.attempt.allocation.clone() {
        execution::execute_allocated_resume_attempt(
            &input,
            &provider,
            provider_index,
            target.prompt_mode,
            &bound_attempt.invocation_env,
            allocation,
        )
    } else {
        execution::execute_resume_attempt_command(
            oulipoly_runtime::services::LiveSessionAuthorityTarget {
                state_path: input.env.state.path().to_path_buf(),
                invocation_row_id: bound_attempt.attempt.invocation_row_id,
                invocation_uuid: bound_attempt.attempt.invocation.id.clone(),
            },
            &input,
            &provider,
            provider_index,
            target.prompt_mode,
            &bound_attempt.invocation_env,
            strategy,
        )
    };
    let mut result = match execution {
        Ok(result) => result,
        Err(spawn_err) => {
            formatter::emit_resume_spawn_error(&spawn_err);
            wake::record_failed_mailbox_delivery_attempt(&input, "resume_spawn_error")?;
            lifecycle::finalize_resume_spawn_error(&input, &mut bound_attempt.attempt)?;
            return Ok(ResumeAttemptLoopControl::Return(1));
        }
    };

    if let Err(error) = lifecycle::commit_resume_session_authority(
        &input,
        &bound_attempt.attempt,
        &provider,
        &result,
        account_endpoint_configured,
    ) {
        let error = super::super::authority_rejection::retain_and_finalize(
            super::super::authority_rejection::RejectedResult {
                service: input
                    .agent_runtime_services
                    .invocation_lifecycle_service
                    .as_ref(),
                state: &input.env.state,
                invocation_row_id: bound_attempt.attempt.invocation_row_id,
                invocation_uuid: &bound_attempt.attempt.invocation.id,
                guard: &mut bound_attempt.attempt.guard,
                result: &result,
                launch_owner: bound_attempt
                    .attempt
                    .allocation
                    .as_ref()
                    .map(|allocation| &allocation.lease.owner),
            },
            error,
        );
        // Failure bookkeeping must not preempt the already-produced evidence handoff.
        if let Err(bookkeeping) = wake::record_failed_mailbox_delivery_attempt(&input, &error) {
            formatter::emit_stderr(&bookkeeping);
        }
        return Err(error);
    }

    let control = terminal::handle_resume_attempt_result(
        &mut input,
        &mut bound_attempt,
        &provider,
        &mut result,
    )?;
    if let Some(allocation) = &bound_attempt.attempt.allocation {
        // The original activation owner still owes physical drain. Its domain
        // driver may settle cancellation only afterward, from retained receipts.
        if !input
            .env
            .state
            .launch_cancellation_requested(&allocation.lease.owner)?
        {
            input
                .env
                .state
                .complete_native_launch(&allocation.lease.owner)?;
        }
    }
    Ok(control)
}

fn prepare_resume_wake(session_id: &str) -> Result<Option<i32>, String> {
    if let Some(exit_code) = wake::validate_auto_wake_child(session_id)? {
        return Ok(Some(exit_code));
    }

    Ok(None)
}

fn resolve_resume_attempt_strategy(
    provider: &oulipoly_config::ProviderConfig,
    account_endpoint_configured: bool,
) -> Result<Option<&oulipoly_config::ResumeStrategy>, i32> {
    execution::resume_attempt_strategy_for_target(provider, account_endpoint_configured)
}
