//! ## Declared roles
//!
//! `mapper`, `orchestration`, `predicate`

use oulipoly_runtime::executor;

use super::disposition::{ResumeTerminalDispositionInput, handle_terminal_signal_disposition};
use super::lifecycle::{
    BoundResumeAttempt, ResumeInvocationAttempt, finalize_completed_attempt_for_resume,
    record_resume_acceptance_if_present,
};
use super::orchestration::{ResumeAttemptInput, ResumeAttemptLoopControl};
use super::{formatter, mapper, wake};
use crate::captured_child::emit_captured_child_marker_lines;
use crate::quota_zero_turn::{
    apply_zero_turn_classification_to_result, host_observed_completion_from_result,
    zero_turn_classification_for_action, zero_turn_classify_after_completion,
};
use crate::terminal_outcome_adapter::{
    TerminalSignalDisposition, apply_age153_terminal_signal_fixture_override,
    confirm_maybe_quota_exhausted, resume_terminal_signal_for_outcome,
};
use crate::zero_turn_orchestration::{ZeroTurnAction, next_action};

pub(super) fn resume_attempts_exhausted(attempts: usize, max_attempts: usize) -> bool {
    attempts >= max_attempts
}

pub(super) fn resume_attempts_exhausted_exit_code(last_exit_code: i32) -> i32 {
    formatter::emit_stderr("BLOCKED:all-providers-exhausted");
    nonzero_resume_exit_code(last_exit_code)
}

fn nonzero_resume_exit_code(last_exit_code: i32) -> i32 {
    if last_exit_code == 0 {
        1
    } else {
        last_exit_code
    }
}

pub(super) fn handle_resume_attempt_result(
    input: &mut ResumeAttemptInput<'_>,
    bound_attempt: &mut BoundResumeAttempt<'_>,
    provider: &oulipoly_config::ProviderConfig,
    result: &mut executor::ExecutionResult,
) -> Result<ResumeAttemptLoopControl, String> {
    let zero_turn_action = apply_resume_attempt_classification(
        input,
        &provider.name,
        &bound_attempt.provider_session_id,
        &bound_attempt.zero_turn_baseline,
        result,
    );
    record_resume_acceptance_if_present(input, bound_attempt.attempt.invocation_row_id, result)?;
    emit_captured_child_marker_lines(&result.captured_child_invocations);
    handle_resume_attempt_terminal_signal(
        input,
        &mut bound_attempt.attempt,
        provider,
        &bound_attempt.provider_session_id,
        result,
        zero_turn_action,
    )
}

fn apply_resume_attempt_classification(
    input: &mut ResumeAttemptInput<'_>,
    provider_name: &str,
    provider_session_id: &str,
    zero_turn_baseline: &crate::zero_turn_orchestration::ZeroTurnBaseline,
    result: &mut executor::ExecutionResult,
) -> ZeroTurnAction {
    apply_age153_terminal_signal_fixture_override(result);
    let zero_turn_classification = zero_turn_classify_after_completion(
        &input.env.state,
        &input.env.sessions_cfg,
        zero_turn_baseline,
        host_observed_completion_from_result(result),
    );
    apply_zero_turn_classification_to_result(result, provider_name, &zero_turn_classification);
    next_action(
        input.zero_turn_confirmation,
        zero_turn_classification_for_action(
            zero_turn_classification,
            result,
            provider_name,
            Some(provider_session_id),
        ),
    )
}

fn handle_resume_attempt_terminal_signal(
    input: &ResumeAttemptInput<'_>,
    attempt: &mut ResumeInvocationAttempt<'_>,
    provider: &oulipoly_config::ProviderConfig,
    provider_session_id: &str,
    result: &executor::ExecutionResult,
    zero_turn_action: ZeroTurnAction,
) -> Result<ResumeAttemptLoopControl, String> {
    let terminal_signal_disposition = terminal_signal_disposition_for_result(
        &input.env.state,
        &attempt.invocation.id,
        &provider.name,
        provider_session_id,
        result,
        zero_turn_action,
    );
    let disposition_control = handle_terminal_signal_disposition(ResumeTerminalDispositionInput {
        agent_runtime_services: input.agent_runtime_services,
        env: input.env,
        invocation_row_id: attempt.invocation_row_id,
        guard: &mut attempt.guard,
        result,
        terminal_signal_disposition,
        zero_turn_action,
    })?;
    let outcome =
        mapper::resume_terminal_disposition_outcome(disposition_control, result.exit_code);
    apply_resume_terminal_disposition_effects(
        input,
        attempt,
        provider_session_id,
        result,
        &outcome,
    )?;
    if let Some(control) = terminal_disposition_loop_control(outcome) {
        return Ok(control);
    }

    wake::ingest_mailbox_delivery_confirmation_turn_if_needed(
        input,
        provider,
        result,
        zero_turn_action,
    );
    if let Some(control) = wake::handle_unconfirmed_mailbox_delivery_if_needed(
        input,
        attempt,
        provider,
        provider_session_id,
        result,
        zero_turn_action,
    )? {
        return Ok(control);
    }
    finalize_completed_attempt_for_resume(input, attempt, provider, provider_session_id, result)
}

pub(super) enum ResumeTerminalDispositionOutcome {
    Continue(i32),
    Return(i32),
    CompletedAttempt,
}

fn apply_resume_terminal_disposition_effects(
    input: &ResumeAttemptInput<'_>,
    attempt: &ResumeInvocationAttempt<'_>,
    provider_session_id: &str,
    result: &executor::ExecutionResult,
    outcome: &ResumeTerminalDispositionOutcome,
) -> Result<(), String> {
    let (exit_code, fallback) = match outcome {
        ResumeTerminalDispositionOutcome::Continue(exit_code) => (*exit_code, "resume_retry"),
        ResumeTerminalDispositionOutcome::Return(exit_code) => (*exit_code, "resume_failed"),
        ResumeTerminalDispositionOutcome::CompletedAttempt => return Ok(()),
    };
    wake::record_failed_mailbox_delivery_attempt(
        input,
        &wake::failed_delivery_error(result, fallback),
    )?;
    wake::mark_resume_attempt_idle(provider_session_id, &attempt.invocation.id, Some(exit_code))
}

fn terminal_disposition_loop_control(
    outcome: ResumeTerminalDispositionOutcome,
) -> Option<ResumeAttemptLoopControl> {
    match outcome {
        ResumeTerminalDispositionOutcome::Continue(exit_code) => {
            Some(ResumeAttemptLoopControl::Continue(exit_code))
        }
        ResumeTerminalDispositionOutcome::Return(exit_code) => {
            Some(ResumeAttemptLoopControl::Return(exit_code))
        }
        ResumeTerminalDispositionOutcome::CompletedAttempt => None,
    }
}

fn terminal_signal_disposition_for_result(
    state_db: &oulipoly_state::StateDb,
    invocation_id: &str,
    provider_name: &str,
    provider_session_id: &str,
    result: &executor::ExecutionResult,
    zero_turn_action: ZeroTurnAction,
) -> TerminalSignalDisposition {
    let context_ids = mapper::terminal_signal_context_ids(invocation_id, Some(provider_session_id));
    let mut terminal_signal_stderr = std::io::stderr();
    let mut terminal_signal_ctx = mapper::terminal_signal_context_for_attempt(
        &context_ids,
        provider_name,
        state_db,
        &mut terminal_signal_stderr,
    );
    let terminal_signal = resume_terminal_signal_for_outcome(&result.terminal_signal);
    if super::predicate::confirmed_zero_turn_maybe_quota(zero_turn_action, &terminal_signal) {
        if let Some(signal) = terminal_signal.as_ref() {
            let _ = confirm_maybe_quota_exhausted(signal, &mut terminal_signal_ctx);
        }
        TerminalSignalDisposition::MaybeQuotaVerify
    } else {
        crate::terminal_outcome_adapter::apply_terminal_signal_outcome(
            &terminal_signal,
            &mut terminal_signal_ctx,
        )
    }
}
