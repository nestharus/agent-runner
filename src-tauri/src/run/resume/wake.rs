//! ## Declared roles
//!
//! `accessor`, `formatter`, `mapper`, `orchestration`, `predicate`

use oulipoly_runtime::executor;
use oulipoly_runtime::executor::prompt_acceptance::{
    ExpectedPromptAcceptance, ValidatedPromptAcceptance, promote_prompt_acceptance_attestation,
};
use oulipoly_runtime::services::InvocationLifecycleServicePort;
use oulipoly_runtime::sessions;
use sha2::{Digest, Sha256};

use super::lifecycle::ResumeInvocationAttempt;
use super::orchestration::{ResumeAttemptInput, ResumeAttemptLoopControl};
use super::{formatter, mapper};
use crate::zero_turn_orchestration::ZeroTurnAction;

#[derive(Clone, Copy)]
pub(super) struct ResumeCompletionEvidence<'a> {
    pub(super) zero_turn_action: ZeroTurnAction,
    pub(super) recovered_generic_nonzero: bool,
    pub(super) prompt_acceptance_confirmation: Option<&'a ValidatedPromptAcceptance>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum MailboxDeliveryOutcome {
    Absent,
    Confirmed,
    ConfirmedPromptAcceptance(ValidatedPromptAcceptance),
    Unconfirmed,
}

pub(super) fn validate_auto_wake_child(session_id: &str) -> Result<Option<i32>, String> {
    crate::wake_coordinator::validate_auto_wake_child(session_id)
}

pub(super) fn reset_manual_resume_wake_claim(session_id: &str) -> Result<(), String> {
    if crate::wake_coordinator::is_auto_wake_invocation() {
        return Ok(());
    }
    crate::wake_coordinator::reset_manual_resume_wake_claim(session_id)
}

pub(super) fn release_current_auto_wake_claim(session_id: &str) {
    crate::wake_coordinator::release_current_auto_wake_claim_for_session(session_id);
}

pub(super) fn release_claim_after_wake_preparation_error(session_id: &str) {
    if crate::wake_coordinator::is_auto_wake_invocation() {
        release_current_auto_wake_claim(session_id);
    }
}

pub(super) fn recheck_after_failed_auto_wake(session_id: &str, result: &Result<i32, String>) {
    if failed_auto_wake_needs_recheck(result) {
        let _ = crate::wake_coordinator::recheck_after_failed_auto_wake(session_id);
    }
}

fn failed_auto_wake_needs_recheck(result: &Result<i32, String>) -> bool {
    crate::wake_coordinator::is_auto_wake_invocation() && !matches!(result, Ok(0))
}

pub(super) fn prepare_headless_resume_delivery(
    resolved: &oulipoly_state::ResolvedResume,
    answer: Option<String>,
    models_dir: Option<&std::path::Path>,
) -> Result<crate::mailbox_delivery::PreparedMailboxDelivery, String> {
    crate::mailbox_delivery::prepare_headless_resume_delivery(resolved, answer, models_dir)
}

pub(super) fn ingest_mailbox_delivery_confirmation_turn_if_needed(
    input: &ResumeAttemptInput<'_>,
    provider: &oulipoly_config::ProviderConfig,
    result: &executor::ExecutionResult,
    completion_evidence: ResumeCompletionEvidence<'_>,
) {
    let errors = ingest_mailbox_delivery_confirmation_turn_silently_if_needed(
        input,
        provider,
        result,
        completion_evidence,
    );
    emit_session_ingest_warnings(&provider.name, &errors);
}

fn ingest_mailbox_delivery_confirmation_turn_silently_if_needed(
    input: &ResumeAttemptInput<'_>,
    provider: &oulipoly_config::ProviderConfig,
    result: &executor::ExecutionResult,
    completion_evidence: ResumeCompletionEvidence<'_>,
) -> Vec<String> {
    if !mailbox_delivery_requires_turn_confirmation(
        input,
        result,
        completion_evidence.recovered_generic_nonzero,
    ) || mailbox_delivery_turn_confirmed(input, &provider.name, completion_evidence)
    {
        return Vec::new();
    }
    let report = sessions::scan_provider_session(
        &provider.name,
        &input.env.sessions_cfg,
        &input.env.state,
        &input.resolved.active_session_id,
    );
    report.errors
}

fn emit_session_ingest_warnings(provider_name: &str, errors: &[String]) {
    for error in errors {
        formatter::emit_stderr(&format!(
            "Warning: Session ingest failed for {}: {error}",
            provider_name
        ));
    }
}

pub(super) fn resolve_mailbox_delivery_outcome(
    input: &ResumeAttemptInput<'_>,
    provider: &oulipoly_config::ProviderConfig,
    result: &executor::ExecutionResult,
    completion_evidence: ResumeCompletionEvidence<'_>,
) -> MailboxDeliveryOutcome {
    if input.mailbox_delivery_seqs.is_empty() {
        return MailboxDeliveryOutcome::Absent;
    }
    let errors = ingest_mailbox_delivery_confirmation_turn_silently_if_needed(
        input,
        provider,
        result,
        completion_evidence,
    );
    emit_session_ingest_warnings(&provider.name, &errors);
    if mailbox_delivery_unconfirmed(input, &provider.name, result, completion_evidence) {
        MailboxDeliveryOutcome::Unconfirmed
    } else if let Some(acceptance) = completion_evidence.prompt_acceptance_confirmation {
        MailboxDeliveryOutcome::ConfirmedPromptAcceptance(acceptance.clone())
    } else {
        MailboxDeliveryOutcome::Confirmed
    }
}

pub(super) fn handle_unconfirmed_mailbox_delivery_if_needed(
    input: &ResumeAttemptInput<'_>,
    attempt: &mut ResumeInvocationAttempt<'_>,
    provider: &oulipoly_config::ProviderConfig,
    provider_session_id: &str,
    result: &executor::ExecutionResult,
    completion_evidence: ResumeCompletionEvidence<'_>,
) -> Result<Option<ResumeAttemptLoopControl>, String> {
    if !mailbox_delivery_unconfirmed(input, &provider.name, result, completion_evidence) {
        return Ok(None);
    }
    record_failed_mailbox_delivery_attempt(input, "mailbox_delivery_unconfirmed")?;
    finalize_unconfirmed_mailbox_delivery(input, attempt, result)?;
    mark_resume_attempt_idle(provider_session_id, &attempt.invocation.id, Some(1))?;
    Ok(Some(ResumeAttemptLoopControl::Return(1)))
}

fn mailbox_delivery_unconfirmed(
    input: &ResumeAttemptInput<'_>,
    provider_name: &str,
    result: &executor::ExecutionResult,
    completion_evidence: ResumeCompletionEvidence<'_>,
) -> bool {
    mailbox_delivery_requires_turn_confirmation(
        input,
        result,
        completion_evidence.recovered_generic_nonzero,
    ) && !mailbox_delivery_turn_confirmed(input, provider_name, completion_evidence)
}

fn mailbox_delivery_requires_turn_confirmation(
    input: &ResumeAttemptInput<'_>,
    result: &executor::ExecutionResult,
    recovered_generic_nonzero: bool,
) -> bool {
    input.mailbox_delivery_requires_turn_confirmation
        && (result.exit_code == 0 || recovered_generic_nonzero)
        && !input.mailbox_delivery_seqs.is_empty()
        && input.answer.is_some_and(|answer| !answer.trim().is_empty())
}

fn mailbox_delivery_turn_confirmed(
    input: &ResumeAttemptInput<'_>,
    provider_name: &str,
    completion_evidence: ResumeCompletionEvidence<'_>,
) -> bool {
    matches!(
        completion_evidence.zero_turn_action,
        ZeroTurnAction::Continue
    ) || completion_evidence.prompt_acceptance_confirmation.is_some()
        || ingested_user_turn_confirms_mailbox_delivery(input, provider_name)
}

pub(super) fn validated_prompt_acceptance_for_resume(
    input: &ResumeAttemptInput<'_>,
    result: &executor::ExecutionResult,
) -> Option<ValidatedPromptAcceptance> {
    let attestation = result.prompt_acceptance_attestation.as_ref()?;
    let answer = input.answer?;
    let prompt_sha256 = sha256_hex(answer.as_bytes());
    promote_prompt_acceptance_attestation(
        ExpectedPromptAcceptance {
            provider_session_id: &input.resolved.active_session_id,
            prompt_sha256: &prompt_sha256,
            delivery_nonce: input.mailbox_delivery_nonce,
        },
        attestation,
    )
}

fn ingested_user_turn_confirms_mailbox_delivery(
    input: &ResumeAttemptInput<'_>,
    provider_name: &str,
) -> bool {
    if let Some(delivery_nonce) = input.mailbox_delivery_nonce {
        return input
            .env
            .state
            .has_session_user_turn_containing(
                provider_name,
                &input.resolved.active_session_id,
                delivery_nonce,
            )
            .unwrap_or(false);
    }
    let Some(answer) = input.answer else {
        return false;
    };
    input
        .env
        .state
        .has_session_user_text_turn(provider_name, &input.resolved.active_session_id, answer)
        .unwrap_or(false)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn finalize_unconfirmed_mailbox_delivery(
    input: &ResumeAttemptInput<'_>,
    attempt: &mut ResumeInvocationAttempt<'_>,
    result: &executor::ExecutionResult,
) -> Result<(), String> {
    formatter::emit_stderr(
        "resume mailbox delivery was not confirmed by a new session turn; leaving mailbox rows pending",
    );
    input
        .agent_runtime_services
        .invocation_lifecycle_service
        .finalize_invocation(mapper::finalize_request(
            &input.env.state,
            attempt.invocation_row_id,
            false,
            1,
            Some("mailbox_delivery_unconfirmed"),
            result.terminal_reason.as_deref(),
        ))
        .map_err(|err| err.to_string())?;
    attempt.guard.mark_finalized();
    Ok(())
}

pub(super) fn record_failed_mailbox_delivery_attempt(
    input: &ResumeAttemptInput<'_>,
    delivery_error: &str,
) -> Result<(), String> {
    crate::mailbox_delivery::mark_headless_resume_delivery_failed(
        input.mailbox_session_id,
        Some(&input.resolved.chain_id),
        input.mailbox_delivery_seqs,
        delivery_error,
    )
}

pub(super) fn failed_delivery_error(result: &executor::ExecutionResult, fallback: &str) -> String {
    crate::terminal_outcome_adapter::terminal_signal_reason(
        &result.terminal_signal,
        result.terminal_reason.as_deref(),
    )
    .or(result.terminal_reason.as_deref())
    .unwrap_or(fallback)
    .to_string()
}

pub(super) fn mark_resume_attempt_idle(
    provider_session_id: &str,
    invocation_uuid: &str,
    exit_code: Option<i32>,
) -> Result<(), String> {
    crate::wake_coordinator::mark_session_idle_after_turn(
        provider_session_id,
        invocation_uuid,
        exit_code,
    )
}

pub(super) fn complete_successful_mailbox_delivery(
    input: &ResumeAttemptInput<'_>,
    provider_session_id: &str,
    invocation_uuid: &str,
    exit_code: i32,
) -> Result<(), String> {
    crate::mailbox_delivery::mark_headless_resume_delivered(
        input.mailbox_session_id,
        Some(&input.resolved.chain_id),
        input.mailbox_delivery_seqs,
        invocation_uuid,
    )?;
    let _ = crate::wake_coordinator::mark_successful_turn_idle_and_recheck(
        provider_session_id,
        invocation_uuid,
        exit_code,
    )?;
    Ok(())
}

pub(super) fn settle_age270_mailbox_delivery_outcome(
    input: &ResumeAttemptInput<'_>,
    provider_session_id: &str,
    invocation_uuid: &str,
    physical_exit_code: i32,
    shell_exit_code: i32,
    outcome: MailboxDeliveryOutcome,
) -> Result<(), String> {
    match outcome {
        MailboxDeliveryOutcome::Absent => {
            mark_resume_attempt_idle(provider_session_id, invocation_uuid, Some(shell_exit_code))
        }
        MailboxDeliveryOutcome::Confirmed => complete_successful_mailbox_delivery(
            input,
            provider_session_id,
            invocation_uuid,
            physical_exit_code,
        ),
        MailboxDeliveryOutcome::ConfirmedPromptAcceptance(acceptance) => {
            complete_successful_mailbox_delivery(
                input,
                acceptance.provider_session_id(),
                invocation_uuid,
                physical_exit_code,
            )
        }
        MailboxDeliveryOutcome::Unconfirmed => {
            record_failed_mailbox_delivery_attempt(input, "mailbox_delivery_unconfirmed")?;
            mark_resume_attempt_idle(provider_session_id, invocation_uuid, Some(shell_exit_code))
        }
    }
}
