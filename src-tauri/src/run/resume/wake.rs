//! ## Declared roles
//!
//! `accessor`, `formatter`, `mapper`, `orchestration`, `predicate`
//!
//! ## Lifecycle relationship
//!
//! This module currently owns production resume evidence acquisition and
//! mailbox/wake projection. It remains compatible with the target
//! `ProviderTurnAdapter` until AGE-278 performs the joined cutover; the exact
//! domain boundary and retirement criteria are owned by
//! `docs/architecture/provider-turn-lifecycle.md`.

use oulipoly_provider::client::CancellationToken;
use oulipoly_runtime::executor;
use oulipoly_runtime::executor::prompt_acceptance::{
    ExpectedPromptAcceptance, ValidatedPromptAcceptance, promote_prompt_acceptance_attestation,
};
use oulipoly_runtime::services::InvocationLifecycleServicePort;
use oulipoly_runtime::session_provider::{
    SessionProviderIdentity, SessionProviderPageCursor, SessionProviderReadPageRequest,
    SessionProviderTurnProjection, read_turn_page,
};
use oulipoly_state::mailbox::{MailboxDb, MailboxDeliveryObservationAnchor};
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::time::Duration;

use super::lifecycle::ResumeInvocationAttempt;
use super::orchestration::{ResumeAttemptInput, ResumeAttemptLoopControl};
use super::{formatter, mapper};
use crate::zero_turn_orchestration::ZeroTurnAction;

use crate::native_receipt::*;
const OBSERVATION_MAX_PENDING_ATTEMPTS: usize = 4;

#[derive(Clone, Copy)]
pub(super) struct ResumeCompletionEvidence<'a> {
    pub(super) zero_turn_action: ZeroTurnAction,
    pub(super) recovered_generic_nonzero: bool,
    pub(super) prompt_acceptance_confirmation: Option<&'a ValidatedPromptAcceptance>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum MailboxDeliveryOutcome {
    Absent,
    AlreadySettled,
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
    submitted_seq: Option<i64>,
) -> Result<crate::mailbox_delivery::PreparedMailboxDelivery, String> {
    crate::mailbox_delivery::prepare_headless_resume_delivery(
        resolved,
        answer,
        models_dir,
        submitted_seq,
    )
}

pub(super) fn reconcile_pending_headless_delivery_observations(
    resolved: &oulipoly_state::ResolvedResume,
    effective_cwd: &std::path::Path,
    config_root: &std::path::Path,
) -> Result<(), String> {
    let Some(db) = MailboxDb::open_default_if_exists()? else {
        return Ok(());
    };
    ensure_observation_not_stopped(&db, &resolved.active_session_id)?;
    prepare_legacy_observation_recovery(&db, resolved)?;
    let pending = db.pending_delivery_observations(
        &resolved.active_session_id,
        OBSERVATION_MAX_PENDING_ATTEMPTS,
    )?;
    drop(db);
    for pending in pending {
        if pending.anchor.provider_session_id != resolved.active_session_id {
            continue;
        }
        if let Err(error) =
            crate::native_receipt::helper::observe_target(crate::native_receipt::helper::Target {
                attempt_id: pending.attempt_id,
                anchor_identity: crate::native_receipt::helper::anchor_identity(&pending.anchor),
                model_name: resolved.model_name.clone().unwrap_or_default(),
                cwd: effective_cwd.to_path_buf(),
                config_root: config_root.to_path_buf(),
            })
        {
            formatter::emit_stderr(&format!(
                "Warning: Bounded recovery observation failed for {}: {error}",
                pending.anchor.provider_name
            ));
        }
    }
    Ok(())
}

fn prepare_legacy_observation_recovery(
    db: &MailboxDb,
    resolved: &oulipoly_state::ResolvedResume,
) -> Result<(), String> {
    let candidates = db.legacy_delivery_observation_candidates(
        &resolved.active_session_id,
        OBSERVATION_MAX_PENDING_ATTEMPTS,
    )?;
    if candidates.is_empty() {
        return Ok(());
    }
    let state = oulipoly_state::StateDb::open_default()?;
    for window in candidates {
        let Some(invocation) = state.get_invocation_by_uuid(&window.delivery_invocation_uuid)?
        else {
            continue;
        };
        if invocation.provider_session_id.as_deref() != Some(window.session_id.as_str()) {
            continue;
        }
        let Some(provider_name) = invocation.provider_name else {
            continue;
        };
        let Some(authority) = state.invocation_provider_session_authority(invocation.id)? else {
            continue;
        };
        let Some(envelope) = crate::mailbox_delivery::legacy_notification_envelope(&window)? else {
            continue;
        };
        db.record_legacy_delivery_observation_identity(
            &window.attempt_id,
            &window.session_id,
            &MailboxDeliveryObservationAnchor {
                provider_name,
                provider_instance_id: authority.provider_instance_id,
                settings_id: authority.settings_id,
                provider_session_id: window.session_id.clone(),
                resume_token: None,
                expected_sha256: normalized_text_sha256(&envelope),
            },
        )?;
    }
    Ok(())
}

pub(super) fn bind_headless_resume_delivery_attempt(
    input: &ResumeAttemptInput<'_>,
    provider: &oulipoly_config::ProviderConfig,
    invocation_uuid: &str,
) -> Result<(), String> {
    crate::mailbox_delivery::bind_headless_resume_delivery_attempt(
        input.mailbox_session_id,
        input.mailbox_delivery_nonce,
        input.mailbox_delivery_seqs,
        invocation_uuid,
    )?;
    if !input.mailbox_delivery_seqs.is_empty() {
        let mut db = MailboxDb::open_default_if_exists()?
            .ok_or("mailbox missing while retaining receipt route")?;
        db.wake_sessions().upsert_session_metadata(
            oulipoly_state::mailbox::SessionMetadataUpsert {
                session_id: input.mailbox_session_id,
                mode: "headless",
                invocation_uuid: Some(invocation_uuid),
                provider_name: Some(&provider.name),
                model_name: input.resolved.model_name.as_deref(),
                models_dir: input.env.models_dir.to_str(),
                effective_cwd: input.effective_spawn_cwd.to_str(),
            },
        )?;
    }
    persist_pre_delivery_observation_anchor(input, provider)
}

pub(super) fn begin_headless_delivery_submission(
    input: &ResumeAttemptInput<'_>,
    invocation_uuid: &str,
) -> Result<(), String> {
    if input.mailbox_delivery_seqs.is_empty() {
        return Ok(());
    }
    let attempt_id = input
        .mailbox_delivery_nonce
        .ok_or_else(|| "headless delivery missing nonce".to_string())?;
    let db = MailboxDb::open_default_if_exists()?
        .ok_or_else(|| "headless delivery sidecar missing".to_string())?;
    db.begin_headless_delivery_submission(
        attempt_id,
        input.mailbox_session_id,
        invocation_uuid,
        input.mailbox_delivery_requires_turn_confirmation,
    )
}

fn persist_pre_delivery_observation_anchor(
    input: &ResumeAttemptInput<'_>,
    provider: &oulipoly_config::ProviderConfig,
) -> Result<(), String> {
    if !input.mailbox_delivery_requires_turn_confirmation || input.mailbox_delivery_seqs.is_empty()
    {
        return Ok(());
    }
    let attempt_id = input
        .mailbox_delivery_nonce
        .ok_or_else(|| "headless mailbox delivery is missing its durable nonce".to_string())?;
    let Some(db) = MailboxDb::open_default_if_exists()? else {
        return Err("mailbox sidecar missing while anchoring headless delivery".to_string());
    };
    ensure_observation_not_stopped(&db, input.mailbox_session_id)?;
    if db.delivery_observation_anchor(attempt_id)?.is_some() {
        return Ok(());
    }
    match capture_pre_delivery_observation_anchor(input, provider) {
        Ok(anchor) => {
            db.record_delivery_observation_anchor(attempt_id, input.mailbox_session_id, &anchor)
        }
        Err(error) => {
            db.record_delivery_observation_anchor_failure(
                attempt_id,
                input.mailbox_session_id,
                &error,
            )?;
            Err(error)
        }
    }
}

fn capture_pre_delivery_observation_anchor(
    input: &ResumeAttemptInput<'_>,
    provider: &oulipoly_config::ProviderConfig,
) -> Result<MailboxDeliveryObservationAnchor, String> {
    let attempt_id = input
        .mailbox_delivery_nonce
        .ok_or_else(|| "headless mailbox delivery is missing its durable nonce".to_string())?;
    let answer = input
        .answer
        .filter(|answer| !answer.trim().is_empty())
        .ok_or_else(|| "mailbox_delivery_observation_answer_missing".to_string())?;
    let identity = observation_identity(input, provider)?;
    let provider_instance_id = identity
        .provider_instance_id
        .clone()
        .ok_or_else(|| "session_provider_instance_identity_missing".to_string())?;
    let registry = input
        .agent_runtime_services
        .provider_registry_handle
        .current();
    let cancellation = CancellationToken::new();
    let page = read_turn_page(SessionProviderReadPageRequest {
        registry: &registry,
        identity: identity.clone(),
        session_id: input.mailbox_session_id,
        effective_cwd: Some(input.effective_spawn_cwd),
        projection: SessionProviderTurnProjection::UserObservation,
        expected_delivery_nonce: Some(attempt_id),
        cursor: SessionProviderPageCursor::Tail,
        expected_page_index: 0,
        expected_turn_sequence: 0,
        max_turns: OBSERVATION_MAX_TURNS,
        max_response_bytes: OBSERVATION_MAX_RESPONSE_BYTES,
        max_source_bytes: OBSERVATION_MAX_SOURCE_BYTES,
        max_inline_body_bytes: 0,
        cancellation: &cancellation,
        timeout: OBSERVATION_TIMEOUT,
    })
    .map_err(|error| {
        let db = MailboxDb::open_default_if_exists();
        match db {
            Ok(Some(db)) => {
                retain_observation_failure(&db, input.mailbox_session_id, attempt_id, error)
            }
            Ok(None) => {
                format!("{error}; mailbox sidecar missing while retaining observation failure")
            }
            Err(storage) => format!("{error}; {storage}"),
        }
    })?;
    let resume_token = page
        .resume_token
        .ok_or_else(|| "mailbox_delivery_observation_anchor_missing".to_string())?;
    Ok(MailboxDeliveryObservationAnchor {
        provider_name: identity.provider_name,
        provider_instance_id,
        settings_id: identity.settings_id,
        provider_session_id: input.mailbox_session_id.to_string(),
        resume_token: Some(resume_token),
        expected_sha256: normalized_text_sha256(answer),
    })
}

fn observation_identity(
    input: &ResumeAttemptInput<'_>,
    provider: &oulipoly_config::ProviderConfig,
) -> Result<SessionProviderIdentity, String> {
    let identity = crate::session_ingest_cli::session_external_provider_identity(
        input.agent_runtime_services,
        input.resolved.model.as_ref(),
        &provider.name,
    )
    .ok_or_else(|| "mailbox_delivery_observation_provider_unavailable".to_string())?;
    Ok(SessionProviderIdentity {
        model_name: identity.model_name,
        provider_name: identity.provider_name,
        provider_instance_id: identity.provider_instance_id,
        settings_id: identity.settings_id,
    })
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
    let _ = (input, provider, result, completion_evidence);
    Vec::new()
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
) -> Result<MailboxDeliveryOutcome, String> {
    if input.mailbox_delivery_seqs.is_empty() {
        return Ok(MailboxDeliveryOutcome::Absent);
    }
    if mailbox_delivery_already_settled(input)? {
        return Ok(MailboxDeliveryOutcome::AlreadySettled);
    }
    let errors = ingest_mailbox_delivery_confirmation_turn_silently_if_needed(
        input,
        provider,
        result,
        completion_evidence,
    );
    emit_session_ingest_warnings(&provider.name, &errors);
    Ok(
        if mailbox_delivery_unconfirmed(input, provider, result, completion_evidence) {
            MailboxDeliveryOutcome::Unconfirmed
        } else if let Some(acceptance) = completion_evidence.prompt_acceptance_confirmation {
            MailboxDeliveryOutcome::ConfirmedPromptAcceptance(acceptance.clone())
        } else {
            MailboxDeliveryOutcome::Confirmed
        },
    )
}

pub(super) fn handle_unconfirmed_mailbox_delivery_if_needed(
    input: &ResumeAttemptInput<'_>,
    attempt: &mut ResumeInvocationAttempt<'_>,
    provider: &oulipoly_config::ProviderConfig,
    provider_session_id: &str,
    result: &executor::ExecutionResult,
    completion_evidence: ResumeCompletionEvidence<'_>,
) -> Result<Option<ResumeAttemptLoopControl>, String> {
    if mailbox_delivery_already_settled(input)?
        || !mailbox_delivery_unconfirmed(input, provider, result, completion_evidence)
    {
        return Ok(None);
    }
    #[cfg(test)]
    super::terminal::races_tests::checkpoint("before_failure_reconcile");
    if reconcile_failed_mailbox_delivery_attempt(input, "mailbox_delivery_unconfirmed")? {
        return Ok(None);
    }
    finalize_unconfirmed_mailbox_delivery(input, attempt, result)?;
    mark_resume_attempt_idle(provider_session_id, &attempt.invocation.id, Some(1))?;
    Ok(Some(ResumeAttemptLoopControl::Return(1)))
}

fn mailbox_delivery_unconfirmed(
    input: &ResumeAttemptInput<'_>,
    provider: &oulipoly_config::ProviderConfig,
    result: &executor::ExecutionResult,
    completion_evidence: ResumeCompletionEvidence<'_>,
) -> bool {
    mailbox_delivery_requires_turn_confirmation(
        input,
        result,
        completion_evidence.recovered_generic_nonzero,
    ) && !mailbox_delivery_turn_confirmed(input, provider, completion_evidence)
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
    provider: &oulipoly_config::ProviderConfig,
    completion_evidence: ResumeCompletionEvidence<'_>,
) -> bool {
    if completion_evidence.prompt_acceptance_confirmation.is_some() {
        return true;
    }
    match confirm_mailbox_delivery_from_anchor(input, provider) {
        Ok(confirmed) => confirmed,
        Err(error) => {
            formatter::emit_stderr(&format!(
                "Warning: Bounded mailbox delivery observation failed for {}: {error}",
                provider.name
            ));
            false
        }
    }
}

fn confirm_mailbox_delivery_from_anchor(
    input: &ResumeAttemptInput<'_>,
    provider: &oulipoly_config::ProviderConfig,
) -> Result<bool, String> {
    let Some(attempt_id) = input.mailbox_delivery_nonce else {
        return Ok(false);
    };
    let Some(db) = MailboxDb::open_default_if_exists()? else {
        return Ok(false);
    };
    if db.delivery_observation_confirmation(attempt_id)?.is_some() {
        return Ok(true);
    }
    let Some(anchor) = db.delivery_observation_anchor(attempt_id)? else {
        return Ok(false);
    };
    let answer = input.answer.unwrap_or_default();
    if anchor.provider_name != provider.name
        || anchor.provider_session_id != input.mailbox_session_id
        || anchor.expected_sha256 != normalized_text_sha256(answer)
    {
        return Ok(false);
    }
    drop(db);
    crate::native_receipt::helper::observe_target(crate::native_receipt::helper::Target {
        attempt_id: attempt_id.to_string(),
        anchor_identity: crate::native_receipt::helper::anchor_identity(&anchor),
        model_name: input.resolved.model_name.clone().unwrap_or_default(),
        cwd: input.effective_spawn_cwd.to_path_buf(),
        config_root: input.env.config_root.clone(),
    })?;
    // Only committed exact native evidence counts, never helper exit status.
    let Some(db) = MailboxDb::open_default_if_exists()? else {
        return Ok(false);
    };
    Ok(db.delivery_observation_confirmation(attempt_id)?.is_some())
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

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn normalized_text_sha256(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    sha256_hex(normalized.trim().as_bytes())
}

// Exit 1 records this invocation's inability to establish confirmation, not
// provider failure, non-submission, or resend permission. The pending attempt
// and anchor remain eligible for later observation; history need not be rewritten.
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
        .finalize_invocation(
            oulipoly_state::InvocationMutationAuthority::Standalone,
            mapper::finalize_request(
                &input.env.state,
                attempt.invocation_row_id,
                false,
                1,
                Some("mailbox_delivery_unconfirmed"),
                result.terminal_reason.as_deref(),
            ),
        )
        .map_err(|err| err.to_string())?;
    attempt.guard.mark_finalized();
    Ok(())
}

pub(super) fn mailbox_delivery_already_settled(
    input: &ResumeAttemptInput<'_>,
) -> Result<bool, String> {
    if input.mailbox_delivery_seqs.is_empty() {
        return Ok(false);
    }
    let attempt_id = input
        .mailbox_delivery_nonce
        .ok_or_else(|| "mailbox reconciliation missing exact attempt nonce".to_string())?;
    let db = MailboxDb::open_default_if_exists()?
        .ok_or_else(|| "mailbox reconciliation missing sidecar".to_string())?;
    db.delivery_attempt_fully_settled(
        attempt_id,
        input.mailbox_session_id,
        Some(&input.resolved.chain_id),
        input.mailbox_delivery_seqs,
    )
}

fn reconcile_failed_mailbox_delivery_attempt(
    input: &ResumeAttemptInput<'_>,
    delivery_error: &str,
) -> Result<bool, String> {
    if input.mailbox_delivery_seqs.is_empty() {
        return Ok(false);
    }
    let attempt_id = input
        .mailbox_delivery_nonce
        .ok_or_else(|| "mailbox failure reconciliation missing exact attempt nonce".to_string())?;
    let mut db = MailboxDb::open_default_if_exists()?
        .ok_or_else(|| "mailbox failure reconciliation missing sidecar".to_string())?;
    db.mark_delivery_attempt_failed(
        attempt_id,
        input.mailbox_session_id,
        Some(&input.resolved.chain_id),
        input.mailbox_delivery_seqs,
        delivery_error,
    )
}

pub(super) fn record_failed_mailbox_delivery_attempt(
    input: &ResumeAttemptInput<'_>,
    delivery_error: &str,
) -> Result<(), String> {
    reconcile_failed_mailbox_delivery_attempt(input, delivery_error).map(|_| ())
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

pub(super) fn settle_accepted_mailbox_delivery_and_recheck(
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
    let _ = crate::wake_coordinator::mark_terminal_attempt_idle_and_recheck(
        provider_session_id,
        invocation_uuid,
        exit_code,
    )?;
    Ok(())
}

pub(super) fn settle_clean_exit_mailbox_delivery_outcome(
    input: &ResumeAttemptInput<'_>,
    provider_session_id: &str,
    invocation_uuid: &str,
    physical_exit_code: i32,
    shell_exit_code: i32,
    outcome: MailboxDeliveryOutcome,
) -> Result<(), String> {
    match outcome {
        MailboxDeliveryOutcome::Absent | MailboxDeliveryOutcome::AlreadySettled => {
            mark_resume_attempt_idle(provider_session_id, invocation_uuid, Some(shell_exit_code))
        }
        MailboxDeliveryOutcome::Confirmed => settle_accepted_mailbox_delivery_and_recheck(
            input,
            provider_session_id,
            invocation_uuid,
            physical_exit_code,
        ),
        MailboxDeliveryOutcome::ConfirmedPromptAcceptance(acceptance) => {
            settle_accepted_mailbox_delivery_and_recheck(
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

#[cfg(test)]
#[path = "observation_tests.rs"]
mod observation_tests;
