//! Atomic persistence for one provider-turn effect set.

use super::session_lifecycle::{
    acknowledgement_with_fence, validate_acknowledgement_fence, validate_nonempty,
};
use super::*;
use crate::diagnostic_recorder::{FlightRecorder, process_recorder};
use oulipoly_agent_messenger::ReturnedArtifactRef;

pub struct ProviderTurnEffectInput<'a> {
    pub invocation_row_id: i64,
    pub delivery_ids: &'a [String],
    pub accept_delivery_if_missing: bool,
    pub session_id: &'a str,
    pub turn_generation_id: &'a str,
    pub submitted_evidence: Option<&'a str>,
    pub confirmed_evidence: Option<&'a str>,
    pub observed_at: i64,
    pub returned_artifacts: &'a [ReturnedArtifactRef],
    pub resume_acceptance_status: Option<&'a str>,
    pub resume_acceptance_evidence: Option<&'a str>,
    pub success: bool,
    pub exit_code: i32,
    pub error_category: Option<&'a str>,
    pub terminal_reason: Option<&'a str>,
}

pub struct ProviderTurnEffectWrite {
    pub acknowledgement: AcknowledgementWrite,
}

impl StateDb {
    pub fn apply_provider_turn_effects(
        &self,
        mutation_authority: crate::InvocationMutationAuthority<'_>,
        input: ProviderTurnEffectInput<'_>,
    ) -> Result<ProviderTurnEffectWrite, String> {
        self.apply_provider_turn_effects_inner(mutation_authority, input, None)
    }

    pub(super) fn apply_completed_turn_effects(
        &self,
        mutation_authority: crate::InvocationMutationAuthority<'_>,
        input: ProviderTurnEffectInput<'_>,
        settlement: &str,
    ) -> Result<(), String> {
        self.apply_provider_turn_effects_inner(mutation_authority, input, Some(settlement))
            .map(|_| ())
    }

    fn apply_provider_turn_effects_inner(
        &self,
        mutation_authority: crate::InvocationMutationAuthority<'_>,
        input: ProviderTurnEffectInput<'_>,
        settlement: Option<&str>,
    ) -> Result<ProviderTurnEffectWrite, String> {
        Self::prepare_returned_artifacts_table(&self.conn)?;
        let identity = Self::load_invocation_identity_for_returned_artifacts(
            &self.conn,
            input.invocation_row_id,
        )?;
        Self::validate_returned_artifact_refs(&identity, input.returned_artifacts)?;

        let lifecycle_row = self.lifecycle_context_for_row_or_none(input.invocation_row_id);
        let timer = lc_log_adapter::start_timer();
        let finished_at = Self::current_rfc3339_timestamp();
        // Select or initialize the recorder before State writer authority. Any
        // nested sidecar-open evidence must only use its deferred handoff.
        let recorder = process_recorder();
        let transaction_result = self.apply_provider_turn_effects_transaction(
            mutation_authority,
            &input,
            &finished_at,
            settlement,
            &recorder,
        );

        match transaction_result {
            Ok((invocation, acknowledgement)) => {
                let authoritative_finished_at = invocation
                    .finished_at
                    .clone()
                    .unwrap_or_else(|| finished_at.clone());
                self.report_finalize_invocation(
                    input.invocation_row_id,
                    input.success,
                    input.exit_code,
                    input.error_category,
                    input.terminal_reason,
                    &authoritative_finished_at,
                    lifecycle_row.as_ref(),
                    timer,
                    Ok(invocation),
                )?;
                Ok(ProviderTurnEffectWrite { acknowledgement })
            }
            Err(error) => {
                let report_error = self.report_finalize_invocation(
                    input.invocation_row_id,
                    input.success,
                    input.exit_code,
                    input.error_category,
                    input.terminal_reason,
                    &finished_at,
                    lifecycle_row.as_ref(),
                    timer,
                    Err(error.clone()),
                );
                match report_error {
                    Err(report_error) => Err(report_error),
                    Ok(()) => Err(error),
                }
            }
        }
    }

    fn apply_provider_turn_effects_transaction(
        &self,
        mutation_authority: crate::InvocationMutationAuthority<'_>,
        input: &ProviderTurnEffectInput<'_>,
        finished_at: &str,
        settlement: Option<&str>,
        recorder: &FlightRecorder,
    ) -> Result<(FinalizeInvocationRow, AcknowledgementWrite), String> {
        let tx =
            sqlite::Transaction::new_unchecked(&self.conn, sqlite::TransactionBehavior::Immediate)
                .map_err(|error| {
                    Self::format_finalize_begin_transaction_error(input.invocation_row_id, error)
                })?;
        if let Some(id) = settlement {
            if super::completed_turns::validate_settlement(
                &tx,
                input.invocation_row_id,
                id,
                mutation_authority,
            )? {
                let invocation = Self::load_invocation_for_finalize(&tx, input.invocation_row_id)?;
                return Ok((invocation, AcknowledgementWrite::AlreadyRecorded));
            }
        } else {
            super::completed_turns::refuse_pending_turn(&tx, input.invocation_row_id)?;
        }
        super::provider_launch_lifecycle::validate_invocation_mutation_authority(
            &tx,
            input.invocation_row_id,
            mutation_authority,
        )?;
        let invocation = Self::load_invocation_for_finalize(&tx, input.invocation_row_id)?;
        Self::validate_invocation_is_running(input.invocation_row_id, &invocation.status)?;
        let acknowledgement = self.with_finalization_completion_authority(
            tx,
            &invocation.invocation_uuid,
            input.success,
            recorder,
            |tx| {
                super::provider_launch_lifecycle::promote_invocation_effect(
                    &tx,
                    mutation_authority,
                    crate::ProviderLaunchPromotion::ReturnedArtifact,
                    input.returned_artifacts.len(),
                )?;
                if input.resume_acceptance_status == Some("accepted") {
                    super::provider_launch_lifecycle::promote_invocation_effect(
                        &tx,
                        mutation_authority,
                        crate::ProviderLaunchPromotion::PromptAccepted,
                        1,
                    )?;
                }
                if input.submitted_evidence.is_some() {
                    super::provider_launch_lifecycle::promote_invocation_effect(
                        &tx,
                        mutation_authority,
                        crate::ProviderLaunchPromotion::MailboxSubmissionAccepted,
                        1,
                    )?;
                }
                let mut acknowledgement = AcknowledgementWrite::AlreadyRecorded;

                for delivery_id in input.delivery_ids {
                    validate_acknowledgement_fence(
                        delivery_id,
                        input.session_id,
                        input.turn_generation_id,
                    )
                    .map_err(|error| error.to_string())?;
                    if input.accept_delivery_if_missing {
                        tx.execute(
                            "INSERT OR IGNORE INTO session_delivery_acknowledgements (
                        delivery_id, session_id, turn_generation_id, accepted_at
                     ) VALUES (?1, ?2, ?3, ?4)",
                            params![
                                delivery_id,
                                input.session_id,
                                input.turn_generation_id,
                                input.observed_at
                            ],
                        )
                        .map_err(|error| error.to_string())?;
                    }
                    let existing = acknowledgement_with_fence(
                        &tx,
                        delivery_id,
                        input.session_id,
                        input.turn_generation_id,
                    )
                    .map_err(|error| error.to_string())?;
                    if let Some(evidence) = input.submitted_evidence {
                        validate_nonempty(evidence, "submission evidence")
                            .map_err(|error| error.to_string())?;
                        if let Some(recorded) = existing.submitted_evidence.as_deref() {
                            if recorded != evidence {
                                return Err(SessionLifecycleError::Conflict("submission evidence")
                                    .to_string());
                            }
                        } else {
                            tx.execute(
                                "UPDATE session_delivery_acknowledgements
                         SET submitted_at = ?, submitted_evidence = ?
                         WHERE delivery_id = ? AND session_id = ? AND turn_generation_id = ?
                           AND submitted_at IS NULL",
                                params![
                                    input.observed_at,
                                    evidence,
                                    delivery_id,
                                    input.session_id,
                                    input.turn_generation_id
                                ],
                            )
                            .map_err(|error| error.to_string())?;
                            acknowledgement = AcknowledgementWrite::Advanced;
                        }
                    }
                    if let Some(evidence) = input.confirmed_evidence {
                        validate_nonempty(evidence, "confirmation evidence")
                            .map_err(|error| error.to_string())?;
                        if existing.submitted_at.is_none() && input.submitted_evidence.is_none() {
                            return Err(SessionLifecycleError::InvalidTransition.to_string());
                        }
                        if let Some(recorded) = existing.confirmed_evidence.as_deref() {
                            if recorded != evidence {
                                return Err(SessionLifecycleError::Conflict(
                                    "confirmation evidence",
                                )
                                .to_string());
                            }
                        } else {
                            tx.execute(
                                "UPDATE session_delivery_acknowledgements
                         SET confirmed_at = ?, confirmed_evidence = ?
                         WHERE delivery_id = ? AND session_id = ? AND turn_generation_id = ?
                           AND confirmed_at IS NULL",
                                params![
                                    input.observed_at,
                                    evidence,
                                    delivery_id,
                                    input.session_id,
                                    input.turn_generation_id
                                ],
                            )
                            .map_err(|error| error.to_string())?;
                            acknowledgement = AcknowledgementWrite::Advanced;
                        }
                    }
                }

                tx.execute(
                    "DELETE FROM invocation_returned_artifacts WHERE invocation_id = ?1",
                    params![input.invocation_row_id],
                )
                .map_err(Self::format_reset_returned_artifacts_error)?;
                for (ordinal, reference) in input.returned_artifacts.iter().enumerate() {
                    Self::insert_returned_artifact_row(
                        &tx,
                        input.invocation_row_id,
                        ordinal,
                        reference,
                    )?;
                }
                if let Some(status) = input.resume_acceptance_status {
                    tx.execute(
                        "UPDATE invocations
                 SET resume_acceptance_status = ?1, resume_acceptance_evidence = ?2
                 WHERE id = ?3",
                        params![
                            status,
                            input.resume_acceptance_evidence,
                            input.invocation_row_id
                        ],
                    )
                    .map_err(|error| {
                        Self::format_resume_acceptance_update_error(input.invocation_row_id, error)
                    })?;
                }

                Self::write_invocation_final_row(
                    &tx,
                    input.invocation_row_id,
                    input.success,
                    input.exit_code,
                    input.error_category,
                    input.terminal_reason,
                    finished_at,
                )?;
                Self::upsert_provider_finalize_aggregate(
                    &tx,
                    &invocation.model_name,
                    invocation.provider_name.as_deref(),
                    input.success,
                    input.terminal_reason,
                    finished_at,
                )?;
                if let Some(id) = settlement {
                    let changed = tx
                        .execute(
                            "UPDATE completed_turns
                         SET committed_at=?2, updated_at=?2
                         WHERE settlement_id=?1 AND committed_at IS NULL",
                            params![id, finished_at],
                        )
                        .map_err(|e| e.to_string())?;
                    if changed != 1 {
                        return Err("completed_turn_settlement_conflict".into());
                    }
                }
                tx.commit().map_err(|error| {
                    Self::format_finalize_commit_transaction_error(input.invocation_row_id, error)
                })?;
                Ok(acknowledgement)
            },
        )?;
        Ok((invocation, acknowledgement))
    }
}
