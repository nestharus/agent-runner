//! Exact durable provider-turn effect application and replay validation.
//!
//! ## Declared roles
//!
//! `mapper`, `orchestration`, `validator`

use oulipoly_state::{
    AcknowledgementWrite, InvocationStatus, ProviderTurnEffectInput, SessionLifecycleRepository,
    StateDb, TurnFence,
};

use crate::provider_turn_adapter::{ProviderTurnAdapterError, ProviderTurnLaunch};
use crate::provider_turn_execution::ProviderExecutionOutcome;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectWrite {
    Applied,
    AlreadyApplied,
    NotApplicable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderTurnEffectReport {
    pub acknowledgement: EffectWrite,
    pub invocation_finalization: EffectWrite,
}

pub(crate) fn apply_provider_turn_effects_exact(
    state: &mut StateDb,
    launch: &ProviderTurnLaunch,
    fence: &TurnFence,
    execution: &ProviderExecutionOutcome,
    submitted_evidence: Option<&str>,
    confirmed_evidence: Option<&str>,
    observed_at: i64,
) -> Result<ProviderTurnEffectReport, ProviderTurnAdapterError> {
    let invocation = state
        .get_invocation_by_uuid(&launch.invocation.invocation.id)
        .map_err(ProviderTurnAdapterError::State)?
        .ok_or_else(|| ProviderTurnAdapterError::State("invocation not found".to_string()))?;
    if invocation.id != launch.invocation.invocation_row_id
        || invocation.parent_invocation_id != launch.invocation.parent_invocation_id
    {
        return Err(ProviderTurnAdapterError::InvalidFence(
            "invocation ownership",
        ));
    }
    let (success, exit_code, error_category, terminal_reason) = finalization_fields(execution);
    if invocation.status != InvocationStatus::Running {
        validate_acknowledgement_replay(
            state,
            launch,
            fence,
            submitted_evidence,
            confirmed_evidence,
        )?;
        validate_finalized_replay(
            state,
            &invocation,
            launch,
            execution,
            success,
            exit_code,
            error_category,
            terminal_reason,
        )?;
        let acknowledgement = if submitted_evidence.is_some() || confirmed_evidence.is_some() {
            EffectWrite::AlreadyApplied
        } else {
            EffectWrite::NotApplicable
        };
        return Ok(ProviderTurnEffectReport {
            acknowledgement,
            invocation_finalization: EffectWrite::AlreadyApplied,
        });
    }
    let artifacts = execution
        .result
        .as_ref()
        .map(|result| result.returned_artifacts.as_slice())
        .unwrap_or_default();
    let acceptance = execution
        .result
        .as_ref()
        .and_then(|result| result.resume_acceptance.as_ref());
    let write = state
        .apply_provider_turn_effects(ProviderTurnEffectInput {
            invocation_row_id: invocation.id,
            delivery_ids: &launch.mailbox_batch.delivery_ids,
            accept_delivery_if_missing: false,
            session_id: &launch.mailbox_batch.session_id,
            turn_generation_id: &fence.generation_id,
            submitted_evidence,
            confirmed_evidence,
            observed_at,
            returned_artifacts: artifacts,
            resume_acceptance_status: acceptance.map(|value| value.status.db_value()),
            resume_acceptance_evidence: acceptance.and_then(|value| value.evidence.as_deref()),
            success,
            exit_code,
            error_category,
            terminal_reason,
        })
        .map_err(ProviderTurnAdapterError::State)?;
    let acknowledgement = if submitted_evidence.is_none() && confirmed_evidence.is_none() {
        EffectWrite::NotApplicable
    } else if write.acknowledgement == AcknowledgementWrite::Advanced {
        EffectWrite::Applied
    } else {
        EffectWrite::AlreadyApplied
    };
    Ok(ProviderTurnEffectReport {
        acknowledgement,
        invocation_finalization: EffectWrite::Applied,
    })
}

fn validate_acknowledgement_replay(
    state: &StateDb,
    launch: &ProviderTurnLaunch,
    fence: &TurnFence,
    submitted_evidence: Option<&str>,
    confirmed_evidence: Option<&str>,
) -> Result<(), ProviderTurnAdapterError> {
    if submitted_evidence.is_none() && confirmed_evidence.is_none() {
        return Ok(());
    }
    for delivery_id in &launch.mailbox_batch.delivery_ids {
        let recorded = state
            .acknowledgement(delivery_id)
            .map_err(|error| ProviderTurnAdapterError::State(error.to_string()))?
            .ok_or_else(|| {
                ProviderTurnAdapterError::State(format!(
                    "delivery acknowledgement {delivery_id} not found"
                ))
            })?;
        if recorded.session_id != launch.mailbox_batch.session_id
            || recorded.turn_generation_id != fence.generation_id
            || submitted_evidence
                .is_some_and(|evidence| recorded.submitted_evidence.as_deref() != Some(evidence))
            || confirmed_evidence
                .is_some_and(|evidence| recorded.confirmed_evidence.as_deref() != Some(evidence))
        {
            return Err(ProviderTurnAdapterError::State(format!(
                "delivery acknowledgement {delivery_id} conflicts with provider-turn replay"
            )));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_finalized_replay(
    state: &StateDb,
    invocation: &oulipoly_state::InvocationRecord,
    launch: &ProviderTurnLaunch,
    execution: &ProviderExecutionOutcome,
    success: bool,
    exit_code: i32,
    error_category: Option<&str>,
    terminal_reason: Option<&str>,
) -> Result<(), ProviderTurnAdapterError> {
    let expected_status = if success {
        InvocationStatus::Succeeded
    } else {
        InvocationStatus::Failed
    };
    let acceptance = execution
        .result
        .as_ref()
        .and_then(|result| result.resume_acceptance.as_ref());
    let artifacts = execution
        .result
        .as_ref()
        .map(|result| result.returned_artifacts.as_slice())
        .unwrap_or_default();
    let persisted_artifacts = state
        .list_returned_artifacts(invocation.id)
        .map_err(ProviderTurnAdapterError::State)?;
    let exact = invocation.status == expected_status
        && invocation.success == Some(success)
        && invocation.exit_code == Some(exit_code)
        && invocation.error_category.as_deref() == error_category
        && invocation.terminal_reason.as_deref() == terminal_reason
        && invocation.resume_acceptance_status.as_deref()
            == acceptance.map(|acceptance| acceptance.status.db_value())
        && invocation.resume_acceptance_evidence.as_deref()
            == acceptance.and_then(|acceptance| acceptance.evidence.as_deref())
        && persisted_artifacts == artifacts
        && invocation.invocation_uuid == launch.invocation.invocation.id;
    if exact {
        Ok(())
    } else {
        Err(ProviderTurnAdapterError::ConflictingReplay)
    }
}

fn finalization_fields(
    execution: &ProviderExecutionOutcome,
) -> (bool, i32, Option<&str>, Option<&str>) {
    let terminal_reason = execution
        .result
        .as_ref()
        .and_then(|result| result.terminal_reason.as_deref())
        .or_else(|| execution.status.error_category());
    (
        execution.status.success(),
        execution
            .result
            .as_ref()
            .map(|result| result.exit_code)
            .unwrap_or(execution.caller_exit_code),
        execution.status.error_category(),
        terminal_reason,
    )
}
