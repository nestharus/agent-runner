//! ## Declared roles
//!
//! `mapper`, `orchestration`, `predicate`

use oulipoly_runtime::executor;

use super::disposition::{ResumeTerminalDispositionInput, handle_terminal_signal_disposition};
use super::lifecycle::{
    BoundResumeAttempt, ConfirmedPromptAcceptanceFailure, ResumeCompletionClassification,
    ResumeInvocationAttempt, finalize_completed_attempt_for_resume,
    record_resume_acceptance_if_present,
};
use super::orchestration::{ResumeAttemptInput, ResumeAttemptLoopControl};
use super::{formatter, mapper, wake};
use crate::captured_child::emit_captured_child_marker_lines;
use crate::quota_zero_turn::{
    apply_zero_turn_classification_to_result, host_observed_completion_from_result,
    zero_turn_classification_for_action, zero_turn_classify_after_completion_with_recovery,
};
use crate::terminal_outcome_adapter::{
    TerminalSignalDisposition, apply_age153_terminal_signal_fixture_override,
    confirm_maybe_quota_exhausted, resume_terminal_signal_for_outcome,
};
use crate::zero_turn_orchestration::{ZeroTurnAction, next_action};

const ACCEPTED_PROMPT_PROVIDER_FAILED_TERMINAL_REASON: &str =
    "resume_prompt_accepted_provider_failed";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Age270MailboxEligibility {
    Ineligible,
    PreMutationCleanExit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Age270MailboxProvenance {
    physical_clean_exit_candidate: bool,
    effective_clean_exit_candidate: bool,
    age270_failure_applied: bool,
}

struct ResumeAttemptClassification {
    zero_turn_action: ZeroTurnAction,
    recovered_generic_nonzero: bool,
    terminal_completion_confirmed: bool,
    age270_mailbox_provenance: Age270MailboxProvenance,
}

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
    let classification = apply_resume_attempt_classification(
        input,
        &provider.name,
        &bound_attempt.provider_session_id,
        &bound_attempt.zero_turn_baseline,
        result,
    );
    let prompt_acceptance_confirmation =
        wake::validate_prompt_acceptance_attestation(input, result);
    wake::project_validated_prompt_acceptance(result, prompt_acceptance_confirmation);
    let confirmed_prompt_acceptance_failure = classify_confirmed_prompt_acceptance_failure(
        result.exit_code,
        prompt_acceptance_confirmation,
        classification.recovered_generic_nonzero,
    );
    if confirmed_prompt_acceptance_failure.is_some() {
        result.terminal_reason = Some(ACCEPTED_PROMPT_PROVIDER_FAILED_TERMINAL_REASON.to_string());
    }
    record_resume_acceptance_if_present(input, bound_attempt.attempt.invocation_row_id, result)?;
    emit_captured_child_marker_lines(&result.captured_child_invocations);
    let completion_evidence = wake::ResumeCompletionEvidence {
        zero_turn_action: classification.zero_turn_action,
        recovered_generic_nonzero: classification.recovered_generic_nonzero,
        prompt_acceptance_confirmation,
    };
    let provenance = classification.age270_mailbox_provenance;
    let mailbox_delivery_outcome = if confirmed_prompt_acceptance_failure.is_some() {
        None
    } else {
        match age270_mailbox_eligibility_for_classification(
            provenance.physical_clean_exit_candidate,
            provenance.effective_clean_exit_candidate,
            provenance.age270_failure_applied,
        ) {
            Age270MailboxEligibility::Ineligible => None,
            Age270MailboxEligibility::PreMutationCleanExit => {
                Some(wake::resolve_mailbox_delivery_outcome(
                    input,
                    provider,
                    result,
                    completion_evidence,
                ))
            }
        }
    };
    handle_resume_attempt_terminal_signal(
        input,
        &mut bound_attempt.attempt,
        provider,
        &bound_attempt.provider_session_id,
        result,
        completion_evidence,
        classification.terminal_completion_confirmed,
        mailbox_delivery_outcome,
        confirmed_prompt_acceptance_failure,
    )
}

fn classify_confirmed_prompt_acceptance_failure(
    physical_exit_code: i32,
    confirmation: Option<wake::ValidatedPromptAcceptance>,
    recovered_generic_nonzero: bool,
) -> Option<ConfirmedPromptAcceptanceFailure> {
    (physical_exit_code != 0 && confirmation.is_some() && !recovered_generic_nonzero)
        .then_some(ConfirmedPromptAcceptanceFailure { physical_exit_code })
}

fn apply_resume_attempt_classification(
    input: &mut ResumeAttemptInput<'_>,
    provider_name: &str,
    provider_session_id: &str,
    zero_turn_baseline: &crate::zero_turn_orchestration::ZeroTurnBaseline,
    result: &mut executor::ExecutionResult,
) -> ResumeAttemptClassification {
    let physical_clean_exit_candidate = clean_exit_completion_candidate(result);
    apply_age153_terminal_signal_fixture_override(result);
    let effective_clean_exit_candidate = clean_exit_completion_candidate(result);
    let provider_confirmed_assistant_response = result.produced_assistant_response;
    let completion = zero_turn_classify_after_completion_with_recovery(
        &input.env.state,
        &input.env.sessions_cfg,
        zero_turn_baseline,
        host_observed_completion_from_result(result),
        result,
    );
    let terminal_completion_confirmed =
        provider_confirmed_assistant_response || completion.accepted_provider_turn;
    let zero_turn_classification = completion.classification;
    apply_zero_turn_classification_to_result(result, provider_name, &zero_turn_classification);
    let mut age270_failure_applied = false;
    if completion.incomplete_tool_boundary {
        apply_incomplete_tool_boundary_failure(result, provider_name);
        age270_failure_applied = true;
    } else if effective_clean_exit_candidate && !terminal_completion_confirmed {
        apply_unconfirmed_resume_completion_failure(result, provider_name);
        age270_failure_applied = true;
    }
    let action = next_action(
        input.zero_turn_confirmation,
        zero_turn_classification_for_action(
            zero_turn_classification,
            result,
            provider_name,
            Some(provider_session_id),
        ),
    );
    ResumeAttemptClassification {
        zero_turn_action: action,
        recovered_generic_nonzero: completion.recovered_generic_nonzero,
        terminal_completion_confirmed: terminal_completion_confirmed && !age270_failure_applied,
        age270_mailbox_provenance: Age270MailboxProvenance {
            physical_clean_exit_candidate,
            effective_clean_exit_candidate,
            age270_failure_applied,
        },
    }
}

fn age270_mailbox_eligibility_for_classification(
    physical_clean_exit_candidate: bool,
    effective_clean_exit_candidate: bool,
    age270_failure_applied: bool,
) -> Age270MailboxEligibility {
    if physical_clean_exit_candidate && effective_clean_exit_candidate && age270_failure_applied {
        Age270MailboxEligibility::PreMutationCleanExit
    } else {
        Age270MailboxEligibility::Ineligible
    }
}

fn clean_exit_completion_candidate(result: &executor::ExecutionResult) -> bool {
    result.exit_code == 0
        && result.terminal_signal.as_ref().is_some_and(|signal| {
            signal.kind
                == oulipoly_runtime::executor::terminal_signal::TerminalSignalKind::CleanExit
        })
}

fn apply_incomplete_tool_boundary_failure(
    result: &mut executor::ExecutionResult,
    provider_name: &str,
) {
    const REASON: &str = "incomplete_tool_boundary";
    result.terminal_reason = Some(REASON.to_string());
    result.terminal_signal = Some(executor::TerminalSignal {
        kind: oulipoly_runtime::executor::terminal_signal::TerminalSignalKind::Unknown,
        provider_name: provider_name.to_string(),
        evidence:
            "provider exited after a new assistant tool-calls boundary without a terminal stop"
                .to_string(),
        observed_at: std::time::SystemTime::now(),
    });
    result.produced_assistant_response = false;
    result.resume_acceptance = Some(executor::ResumeAcceptanceResult {
        status: executor::ResumeAcceptanceStatus::Rejected,
        evidence: Some(REASON.to_string()),
    });
}

fn apply_unconfirmed_resume_completion_failure(
    result: &mut executor::ExecutionResult,
    provider_name: &str,
) {
    const REASON: &str = "resume_completion_unconfirmed";
    result.terminal_reason = Some(REASON.to_string());
    result.terminal_signal = Some(executor::TerminalSignal {
        kind: oulipoly_runtime::executor::terminal_signal::TerminalSignalKind::Unknown,
        provider_name: provider_name.to_string(),
        evidence: "provider exited cleanly without affirmative terminal assistant completion"
            .to_string(),
        observed_at: std::time::SystemTime::now(),
    });
    result.produced_assistant_response = false;
}

#[allow(clippy::too_many_arguments)]
fn handle_resume_attempt_terminal_signal(
    input: &ResumeAttemptInput<'_>,
    attempt: &mut ResumeInvocationAttempt<'_>,
    provider: &oulipoly_config::ProviderConfig,
    provider_session_id: &str,
    result: &executor::ExecutionResult,
    completion_evidence: wake::ResumeCompletionEvidence,
    terminal_completion_confirmed: bool,
    mailbox_delivery_outcome: Option<wake::MailboxDeliveryOutcome>,
    confirmed_prompt_acceptance_failure: Option<ConfirmedPromptAcceptanceFailure>,
) -> Result<ResumeAttemptLoopControl, String> {
    let terminal_signal_disposition = terminal_signal_disposition_for_result(
        &input.env.state,
        &attempt.invocation.id,
        &provider.name,
        provider_session_id,
        result,
        completion_evidence.zero_turn_action,
        completion_evidence.recovered_generic_nonzero,
    );
    let disposition_control = handle_terminal_signal_disposition(ResumeTerminalDispositionInput {
        agent_runtime_services: input.agent_runtime_services,
        env: input.env,
        invocation_id: &attempt.invocation.id,
        invocation_row_id: attempt.invocation_row_id,
        guard: &mut attempt.guard,
        provider_name: &provider.name,
        provider_session_id,
        result,
        terminal_signal_disposition,
        zero_turn_action: completion_evidence.zero_turn_action,
        recovered_generic_nonzero: completion_evidence.recovered_generic_nonzero,
    })?;
    let outcome =
        mapper::resume_terminal_disposition_outcome(disposition_control, result.exit_code);
    let outcome = stop_retry_after_confirmed_prompt_acceptance_failure(
        outcome,
        confirmed_prompt_acceptance_failure,
    );
    apply_resume_terminal_disposition_effects(
        input,
        attempt,
        provider_session_id,
        result,
        &outcome,
        mailbox_delivery_outcome,
        confirmed_prompt_acceptance_failure,
    )?;
    if let Some(control) = terminal_disposition_loop_control(outcome) {
        return Ok(control);
    }

    wake::ingest_mailbox_delivery_confirmation_turn_if_needed(
        input,
        provider,
        result,
        completion_evidence,
    );
    if let Some(control) = wake::handle_unconfirmed_mailbox_delivery_if_needed(
        input,
        attempt,
        provider,
        provider_session_id,
        result,
        completion_evidence,
    )? {
        return Ok(control);
    }
    finalize_completed_attempt_for_resume(
        input,
        attempt,
        provider,
        provider_session_id,
        result,
        ResumeCompletionClassification {
            recovered_generic_nonzero: completion_evidence.recovered_generic_nonzero,
            terminal_completion_confirmed,
            confirmed_prompt_acceptance_failure,
        },
    )
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ResumeTerminalDispositionOutcome {
    Continue(i32),
    Return(i32),
    CompletedAttempt,
}

fn stop_retry_after_confirmed_prompt_acceptance_failure(
    outcome: ResumeTerminalDispositionOutcome,
    failure: Option<ConfirmedPromptAcceptanceFailure>,
) -> ResumeTerminalDispositionOutcome {
    match (failure, outcome) {
        (Some(failure), ResumeTerminalDispositionOutcome::Continue(_)) => {
            ResumeTerminalDispositionOutcome::Return(nonzero_resume_exit_code(
                failure.physical_exit_code,
            ))
        }
        (_, outcome) => outcome,
    }
}

fn apply_resume_terminal_disposition_effects(
    input: &ResumeAttemptInput<'_>,
    attempt: &ResumeInvocationAttempt<'_>,
    provider_session_id: &str,
    result: &executor::ExecutionResult,
    outcome: &ResumeTerminalDispositionOutcome,
    mailbox_delivery_outcome: Option<wake::MailboxDeliveryOutcome>,
    confirmed_prompt_acceptance_failure: Option<ConfirmedPromptAcceptanceFailure>,
) -> Result<(), String> {
    if let Some(failure) = confirmed_prompt_acceptance_failure {
        let shell_exit_code = match outcome {
            ResumeTerminalDispositionOutcome::Return(shell_exit_code) => *shell_exit_code,
            ResumeTerminalDispositionOutcome::CompletedAttempt => return Ok(()),
            ResumeTerminalDispositionOutcome::Continue(_) => {
                return Err(
                    "confirmed prompt acceptance failure cannot continue provider routing"
                        .to_string(),
                );
            }
        };
        return super::lifecycle::settle_confirmed_prompt_acceptance_failure(
            input,
            provider_session_id,
            &attempt.invocation.id,
            failure,
            shell_exit_code,
        );
    }
    if let Some(mailbox_delivery_outcome) = mailbox_delivery_outcome {
        let shell_exit_code = match outcome {
            ResumeTerminalDispositionOutcome::Return(shell_exit_code) => *shell_exit_code,
            ResumeTerminalDispositionOutcome::CompletedAttempt => return Ok(()),
            ResumeTerminalDispositionOutcome::Continue(_) => {
                return Err(
                    "confirmed mailbox submission cannot continue provider routing".to_string(),
                );
            }
        };
        return wake::settle_age270_mailbox_delivery_outcome(
            input,
            provider_session_id,
            &attempt.invocation.id,
            result.exit_code,
            shell_exit_code,
            mailbox_delivery_outcome,
        );
    }
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
    recovered_generic_nonzero: bool,
) -> TerminalSignalDisposition {
    let context_ids = mapper::terminal_signal_context_ids(invocation_id, Some(provider_session_id));
    let mut terminal_signal_stderr = std::io::stderr();
    let mut terminal_signal_ctx = mapper::terminal_signal_context_for_attempt(
        &context_ids,
        provider_name,
        state_db,
        &mut terminal_signal_stderr,
    );
    if recovered_generic_nonzero {
        emit_recovered_resume_terminal_signal_marker(result, &mut terminal_signal_ctx);
        return TerminalSignalDisposition::InteractiveFail;
    }
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

fn emit_recovered_resume_terminal_signal_marker(
    result: &executor::ExecutionResult,
    terminal_signal_ctx: &mut crate::terminal_outcome_adapter::TerminalSignalContext<
        '_,
        std::io::Stderr,
    >,
) {
    let signal = result
        .terminal_signal
        .as_ref()
        .expect("recovered generic nonzero requires terminal evidence");
    let _ = crate::terminal_outcome_adapter::emit_terminal_signal_marker(
        signal,
        terminal_signal_ctx.invocation_id,
        terminal_signal_ctx.session_id,
        &mut terminal_signal_ctx.stderr,
    );
}

#[cfg(test)]
mod tests {
    use super::{
        Age270MailboxEligibility, ResumeTerminalDispositionOutcome,
        age270_mailbox_eligibility_for_classification, apply_incomplete_tool_boundary_failure,
        apply_unconfirmed_resume_completion_failure, classify_confirmed_prompt_acceptance_failure,
        stop_retry_after_confirmed_prompt_acceptance_failure,
    };
    use crate::run::resume::wake::ValidatedPromptAcceptance;
    use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;
    use oulipoly_runtime::executor::{
        ExecutionResult, ResumeAcceptanceStatus, SessionCaptureMethod, SessionCaptureResult,
    };

    fn clean_result() -> ExecutionResult {
        ExecutionResult {
            stdout: b"tool output".to_vec(),
            stderr: String::new(),
            exit_code: 0,
            provider_index: 0,
            session_capture: SessionCaptureResult {
                session_id: None,
                method: SessionCaptureMethod::None,
            },
            resume_acceptance: None,
            terminal_reason: None,
            terminal_signal: None,
            produced_assistant_response: true,
            prompt_acceptance_attestation: None,
            captured_child_invocations: Vec::new(),
            returned_artifacts: Vec::new(),
        }
    }

    #[test]
    fn incomplete_tool_boundary_projects_non_success_resume_result() {
        let mut result = clean_result();

        apply_incomplete_tool_boundary_failure(&mut result, "opencode");

        assert_eq!(
            result.exit_code, 0,
            "preserve the provider's physical exit code"
        );
        assert_eq!(
            result.terminal_reason.as_deref(),
            Some("incomplete_tool_boundary")
        );
        assert!(!result.produced_assistant_response);
        assert_eq!(
            result.terminal_signal.as_ref().map(|signal| signal.kind),
            Some(TerminalSignalKind::Unknown)
        );
        assert_eq!(
            result
                .resume_acceptance
                .as_ref()
                .map(|acceptance| acceptance.status),
            Some(ResumeAcceptanceStatus::Rejected)
        );
    }

    #[test]
    fn unconfirmed_completion_preserves_physical_exit_without_rejecting_delivery() {
        let mut result = clean_result();

        apply_unconfirmed_resume_completion_failure(&mut result, "opencode");

        assert_eq!(result.exit_code, 0);
        assert_eq!(
            result.terminal_reason.as_deref(),
            Some("resume_completion_unconfirmed")
        );
        assert!(!result.produced_assistant_response);
        assert_eq!(
            result.terminal_signal.as_ref().map(|signal| signal.kind),
            Some(TerminalSignalKind::Unknown)
        );
        assert!(result.resume_acceptance.is_none());
    }

    #[test]
    fn age270_mailbox_eligibility_requires_physical_effective_and_applied_provenance() {
        let rows = [
            (false, false, false, Age270MailboxEligibility::Ineligible),
            (false, false, true, Age270MailboxEligibility::Ineligible),
            (false, true, false, Age270MailboxEligibility::Ineligible),
            (false, true, true, Age270MailboxEligibility::Ineligible),
            (true, false, false, Age270MailboxEligibility::Ineligible),
            (true, false, true, Age270MailboxEligibility::Ineligible),
            (true, true, false, Age270MailboxEligibility::Ineligible),
            (
                true,
                true,
                true,
                Age270MailboxEligibility::PreMutationCleanExit,
            ),
        ];
        for (physical, effective, applied, expected) in rows {
            assert_eq!(
                age270_mailbox_eligibility_for_classification(physical, effective, applied),
                expected,
                "unexpected eligibility for P={physical}, E={effective}, A={applied}"
            );
        }
    }

    #[test]
    fn confirmed_prompt_acceptance_failure_is_classified_once_and_stops_retry() {
        let failure = classify_confirmed_prompt_acceptance_failure(
            29,
            Some(ValidatedPromptAcceptance::DeliveryNonceAndPromptSha256),
            false,
        )
        .expect("validated nonzero mailbox acceptance must classify as a failure outcome");

        assert_eq!(
            stop_retry_after_confirmed_prompt_acceptance_failure(
                ResumeTerminalDispositionOutcome::Continue(29),
                Some(failure),
            ),
            ResumeTerminalDispositionOutcome::Return(29)
        );
        assert_eq!(
            stop_retry_after_confirmed_prompt_acceptance_failure(
                ResumeTerminalDispositionOutcome::Continue(29),
                None,
            ),
            ResumeTerminalDispositionOutcome::Continue(29)
        );
        assert!(
            classify_confirmed_prompt_acceptance_failure(
                29,
                Some(ValidatedPromptAcceptance::PromptSha256),
                true,
            )
            .is_none(),
            "recovered generic nonzero is not a confirmed prompt-acceptance failure"
        );
    }
}
