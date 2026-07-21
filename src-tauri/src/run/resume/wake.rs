//! ## Declared roles
//!
//! `accessor`, `formatter`, `mapper`, `orchestration`, `predicate`

use oulipoly_runtime::executor;
use oulipoly_runtime::services::InvocationLifecycleServicePort;
use oulipoly_runtime::sessions;
use sha2::{Digest, Sha256};

use super::lifecycle::ResumeInvocationAttempt;
use super::orchestration::{ResumeAttemptInput, ResumeAttemptLoopControl};
use super::{formatter, mapper};
use crate::zero_turn_orchestration::ZeroTurnAction;

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
    zero_turn_action: ZeroTurnAction,
) {
    if !mailbox_delivery_requires_turn_confirmation(input, &provider.name, result)
        || mailbox_delivery_turn_confirmed(input, &provider.name, result, zero_turn_action)
    {
        return;
    }
    let report = sessions::scan_provider_session(
        &provider.name,
        &input.env.sessions_cfg,
        &input.env.state,
        &input.resolved.active_session_id,
    );
    emit_session_ingest_warnings(&provider.name, &report.errors);
}

fn emit_session_ingest_warnings(provider_name: &str, errors: &[String]) {
    for error in errors {
        formatter::emit_stderr(&format!(
            "Warning: Session ingest failed for {}: {error}",
            provider_name
        ));
    }
}

pub(super) fn handle_unconfirmed_mailbox_delivery_if_needed(
    input: &ResumeAttemptInput<'_>,
    attempt: &mut ResumeInvocationAttempt<'_>,
    provider: &oulipoly_config::ProviderConfig,
    provider_session_id: &str,
    result: &executor::ExecutionResult,
    zero_turn_action: ZeroTurnAction,
) -> Result<Option<ResumeAttemptLoopControl>, String> {
    if !mailbox_delivery_unconfirmed(input, &provider.name, result, zero_turn_action) {
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
    zero_turn_action: ZeroTurnAction,
) -> bool {
    mailbox_delivery_requires_turn_confirmation(input, provider_name, result)
        && !mailbox_delivery_turn_confirmed(input, provider_name, result, zero_turn_action)
}

fn mailbox_delivery_requires_turn_confirmation(
    input: &ResumeAttemptInput<'_>,
    provider_name: &str,
    result: &executor::ExecutionResult,
) -> bool {
    result.exit_code == 0
        && !input.mailbox_delivery_seqs.is_empty()
        && input.answer.is_some_and(|answer| !answer.trim().is_empty())
        && input.env.sessions_cfg.get(provider_name).is_some()
}

fn mailbox_delivery_turn_confirmed(
    input: &ResumeAttemptInput<'_>,
    provider_name: &str,
    result: &executor::ExecutionResult,
    zero_turn_action: ZeroTurnAction,
) -> bool {
    matches!(zero_turn_action, ZeroTurnAction::Continue)
        || submitted_user_turn_confirms_mailbox_delivery(input, result)
        || ingested_user_turn_confirms_mailbox_delivery(input, provider_name)
}

fn submitted_user_turn_confirms_mailbox_delivery(
    input: &ResumeAttemptInput<'_>,
    result: &executor::ExecutionResult,
) -> bool {
    let Some(submitted) = result.submitted_user_turn.as_ref() else {
        return false;
    };
    let Some(answer) = input.answer else {
        return false;
    };
    submitted.provider_session_id == input.resolved.active_session_id
        && submitted_user_turn_payload_confirms_mailbox_delivery(input, submitted, answer)
}

fn submitted_user_turn_payload_confirms_mailbox_delivery(
    input: &ResumeAttemptInput<'_>,
    submitted: &executor::SubmittedUserTurn,
    answer: &str,
) -> bool {
    if let Some(delivery_nonce) = input.mailbox_delivery_nonce {
        return submitted.delivery_nonce.as_deref() == Some(delivery_nonce);
    }
    submitted.prompt_sha256 == sha256_hex(answer.as_bytes())
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
