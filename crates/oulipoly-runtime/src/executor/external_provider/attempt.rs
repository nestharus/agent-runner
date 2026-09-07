//! One allocated attempt's transport and evidence. No admission, successor lease,
//! account loop, or lifecycle coordinator lives here. Roles: mapper, orchestration.
use super::context::ExternalProviderDispatchContext;
use crate::executor::cli::spawn_identity::{
    SpawnIdentityContext, exit_runtime_generation_outcome,
    register_allocated_runtime_generation_starting,
};
use crate::executor::{
    CapturedChildInvocation, ExecutionOutputSpool, ExecutionResult, ReturnChannel,
    ReturnChannelSettlement,
};
use crate::provider_registry::ProviderRegistry;
use crate::services::{ExecutorServiceRequest, ServiceError};
use oulipoly_provider::custody::{
    ActorSettlementReceipt, AttemptActorCustody, GeneratedRequestIdentity, ProviderOperation,
};
use oulipoly_provider::error::ProviderClientError;
use oulipoly_provider::stream::DecodedLaunchEvent;
use oulipoly_state::mailbox::{
    ExactProcessEvidence, MailboxDb, RuntimeGenerationId, RuntimeGenerationRow,
    RuntimeLifecycleState, RuntimeTerminalReason,
};
use oulipoly_state::{
    CompletionRegistrationAuthority, InvocationMutationAuthority, ProviderLaunchLease,
    ProviderLaunchOwnerFence, ProviderLaunchPromotion, RotatableLaunchFailureKind, StateDb,
};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Clone)]
pub struct AllocatedProviderLaunchAttempt {
    pub lease: ProviderLaunchLease,
    pub completion_authority: CompletionRegistrationAuthority,
    pub state_db_path: PathBuf,
    pub mailbox_db_path: PathBuf,
    pub channel_root: PathBuf,
    pub parent_invocation_uuid: Uuid,
}
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ProviderLaunchPromotionSummary {
    pub provider_session_observed: bool,
    pub prompt_accepted: bool,
    pub assistant_response_observed: bool,
    pub captured_child: bool,
    pub returned_artifact: bool,
    pub mailbox_submission_accepted: bool,
    pub persistence_failed: bool,
}
impl ProviderLaunchPromotionSummary {
    pub fn transfer_forbidden(&self) -> bool {
        self.provider_session_observed
            || self.prompt_accepted
            || self.assistant_response_observed
            || self.captured_child
            || self.returned_artifact
            || self.mailbox_submission_accepted
            || self.persistence_failed
    }
    fn mark(&mut self, promotion: ProviderLaunchPromotion) {
        match promotion {
            ProviderLaunchPromotion::ProviderSessionObserved => {
                self.provider_session_observed = true
            }
            ProviderLaunchPromotion::PromptAccepted => self.prompt_accepted = true,
            ProviderLaunchPromotion::AssistantResponseObserved => {
                self.assistant_response_observed = true
            }
            ProviderLaunchPromotion::CapturedChild => self.captured_child = true,
            ProviderLaunchPromotion::ReturnedArtifact => self.returned_artifact = true,
            ProviderLaunchPromotion::MailboxSubmissionAccepted => {
                self.mailbox_submission_accepted = true
            }
        }
    }
}
#[derive(Debug, Clone)]
pub enum ProviderLaunchFailure {
    Provider(ProviderClientError),
    Execution(ServiceError),
}
#[derive(Debug, Clone, serde::Serialize)]
pub struct RuntimeSettlementReceipt {
    pub runtime_generation_uuid: Uuid,
    pub spawn_invocation_uuid: Uuid,
    pub row: Option<RuntimeGenerationRow>,
    pub row_sha256: Option<String>,
    pub effect_incapable: bool,
    pub uncertainty: Option<String>,
}
#[derive(Debug)]
pub struct ProviderLaunchAttemptFailure {
    pub owner: ProviderLaunchOwnerFence,
    pub error: ProviderLaunchFailure,
    pub rotatable_kind: Option<RotatableLaunchFailureKind>,
    pub observations: ProviderLaunchPromotionSummary,
    pub actor_settlement: Vec<ActorSettlementReceipt>,
    pub runtime_settlement: RuntimeSettlementReceipt,
    pub return_channel_settlement: ReturnChannelSettlement,
    pub requests: Vec<GeneratedRequestIdentity>,
    pub captured_child_invocations: Vec<CapturedChildInvocation>,
    pub output_spool: Option<ExecutionOutputSpool>,
    pub evidence_retention_failure: Option<String>,
}
#[derive(Debug)]
#[allow(
    clippy::large_enum_variant,
    reason = "LT02 frozen public outcome contract keeps both payloads unboxed"
)]
pub enum ProviderLaunchAttemptOutcome {
    Completed(ExecutionResult),
    Failed(ProviderLaunchAttemptFailure),
}

pub(crate) struct AttemptExecution {
    pub allocation: AllocatedProviderLaunchAttempt,
    state: Mutex<StateDb>,
    pub actors: AttemptActorCustody,
    pub spawn: SpawnIdentityContext,
    pub evidence: Mutex<AttemptEvidence>,
}
#[derive(Default)]
pub(crate) struct AttemptEvidence {
    pub promotions: ProviderLaunchPromotionSummary,
    pub channel: Option<ReturnChannel>,
    pub channel_settlement: Option<ReturnChannelSettlement>,
    pub children: Vec<CapturedChildInvocation>,
    pub output: Option<ExecutionOutputSpool>,
    pub prompt: Option<oulipoly_provider::generated::PromptAcceptanceRequestV1>,
    pub verified_session: Option<String>,
    pub stderr_pending: Vec<u8>,
    pub retention_failure: Option<String>,
}
impl AttemptExecution {
    fn refresh_promotions(&self) {
        let observed = self
            .state
            .lock()
            .map_err(|_| "promotion_state_unavailable".to_string())
            .and_then(|db| db.provider_launch_promotions(&self.allocation.lease.owner));
        let mut evidence = self.evidence.lock().unwrap_or_else(|e| e.into_inner());
        match observed {
            Ok(promotions) => {
                for promotion in promotions {
                    evidence.promotions.mark(promotion);
                }
            }
            Err(_) => evidence.promotions.persistence_failed = true,
        }
    }

    pub(crate) fn promote(&self, promotion: ProviderLaunchPromotion) -> Result<(), String> {
        // Memory also locks transfer if persistence fails. Never clear this bit.
        self.evidence
            .lock()
            .map_err(|_| "promotion_evidence_unavailable")?
            .promotions
            .mark(promotion);
        let result = self
            .state
            .lock()
            .map_err(|_| "promotion_state_unavailable".into())
            .and_then(|db| {
                db.record_promotion(&self.allocation.lease.owner, Uuid::new_v4(), promotion)
            });
        if result.is_err() {
            self.evidence
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .promotions
                .persistence_failed = true;
        }
        result
    }
    pub(crate) fn bind_endpoint(
        &self,
        endpoint: &crate::provider_registry::PinnedProviderEndpoint,
    ) -> Result<(), String> {
        self.state
            .lock()
            .map_err(|_| "endpoint_state_unavailable")?
            .bind_launch_endpoint(&self.allocation.lease.owner, &endpoint.endpoint_identity()?)
    }
    pub(crate) fn observe(
        &self,
        context: &ExternalProviderDispatchContext,
        event: &DecodedLaunchEvent,
    ) -> Result<(), String> {
        use oulipoly_provider::generated::PROMPT_ACCEPTED_MARKER_V1;
        if let DecodedLaunchEvent::Stderr { data, .. } = event {
            let mut evidence = self
                .evidence
                .lock()
                .map_err(|_| "child_evidence_unavailable")?;
            evidence.stderr_pending.extend(data);
            if evidence.stderr_pending.len() > 1024 * 1024 {
                evidence.promotions.persistence_failed = true;
                return Err("child_marker_line_unbounded".into());
            }
            let end = evidence
                .stderr_pending
                .iter()
                .rposition(|b| *b == b'\n')
                .map(|i| i + 1)
                .unwrap_or(0);
            let lines: Vec<_> = evidence.stderr_pending.drain(..end).collect();
            drop(evidence);
            self.retain_children(&String::from_utf8_lossy(&lines))?;
        }
        let session = match event {
            DecodedLaunchEvent::Exit(exit) => exit
                .session
                .as_ref()
                .and_then(super::launch_result_mapper::marker_provider_session_id),
            _ => super::dispatch::provider_session_id_from_launch_event(event),
        };
        if let Some(session) = session {
            crate::session_authority::verify_session_authority(
                crate::session_authority::SessionAuthorityExpectation {
                    account_name: &context.provider.name,
                    provider_session_id: context.start_known_provider_session_id.as_deref(),
                },
                Some(crate::session_authority::AuthoritativeSessionObservation {
                    account_name: &self.allocation.lease.candidate.account_name,
                    provider_session_id: &session,
                }),
            )
            .map_err(|e| e.to_string())?;
            self.promote(ProviderLaunchPromotion::ProviderSessionObserved)?;
            self.evidence
                .lock()
                .map_err(|_| "session_evidence_unavailable")?
                .verified_session = Some(session);
        }
        if let DecodedLaunchEvent::Marker { name, value, .. } = event {
            if name == "oulipoly.produced_assistant_response" && value.as_bool() == Some(true) {
                self.promote(ProviderLaunchPromotion::AssistantResponseObserved)?;
            }
            if name == PROMPT_ACCEPTED_MARKER_V1 {
                let observed =
                    super::launch_result_mapper::parse_prompt_acceptance_attestation_marker(value)
                        .ok_or("prompt_acceptance_invalid")?;
                let evidence = self
                    .evidence
                    .lock()
                    .map_err(|_| "prompt_evidence_unavailable")?;
                let expected = evidence
                    .prompt
                    .as_ref()
                    .ok_or("prompt_acceptance_not_negotiated")?;
                if expected.prompt_sha256 != observed.prompt_sha256
                    || expected.delivery_nonce != observed.delivery_nonce
                    || evidence.verified_session.as_deref() != Some(&observed.provider_session_id)
                {
                    return Err("prompt_acceptance_mismatch".into());
                }
                drop(evidence);
                self.promote(ProviderLaunchPromotion::PromptAccepted)?;
            }
        }
        Ok(())
    }
    pub(crate) fn retain_children(&self, stderr: &str) -> Result<(), String> {
        let children = crate::executor::cli::captured_child_invocations_from_stderr(stderr);
        if children.is_empty() {
            return Ok(());
        }
        // Preserve custody even when the promotion write fails.
        let mut evidence = self
            .evidence
            .lock()
            .map_err(|_| "child_evidence_unavailable")?;
        for child in children {
            if !evidence
                .children
                .iter()
                .any(|c| c.composite_id == child.composite_id)
            {
                evidence.children.push(child);
            }
        }
        drop(evidence);
        self.promote(ProviderLaunchPromotion::CapturedChild)
    }
    pub(crate) fn seal_channel(&self) -> Vec<crate::executor::ReturnedArtifactRef> {
        let channel = self
            .evidence
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .channel
            .take();
        let actors = self.actors.receipts();
        let settlement = channel
            .map(|channel| {
                channel.seal(&actors, |refs| {
                    self.promote(ProviderLaunchPromotion::ReturnedArtifact)?;
                    self.state
                        .lock()
                        .map_err(|_| "artifact_state_unavailable")?
                        .record_returned_artifacts(
                            InvocationMutationAuthority::ProviderLaunch(
                                &self.allocation.lease.owner,
                            ),
                            self.allocation.lease.owner.invocation_row_id,
                            refs,
                        )
                })
            })
            .unwrap_or(ReturnChannelSettlement::NotCreated);
        let refs = settlement.artifacts().to_vec();
        self.evidence
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .channel_settlement = Some(settlement);
        refs
    }
}

/// Executes exactly one already-active allocated account. This function never
/// activates a lease and has no successor/coordinator or account-selection path.
pub fn execute_allocated_provider_attempt(
    registry: &ProviderRegistry,
    request: ExecutorServiceRequest,
    allocation: AllocatedProviderLaunchAttempt,
) -> ProviderLaunchAttemptOutcome {
    let owner = allocation.lease.owner.clone();
    let setup = || -> Result<(ExternalProviderDispatchContext, Arc<AttemptExecution>), String> {
        let mut context = crate::executor::external_provider_context_from_request(request)
            .map_err(|e| e.to_string())?;
        if context.provider.name != allocation.lease.candidate.account_name
            || context.provider_index != allocation.lease.candidate.provider_index
            || allocation.lease.return_channel_id
                != format!("{}/{}", owner.logical_launch_id, owner.attempt_id)
        {
            return Err("allocated_attempt_context_mismatch".into());
        }
        let state = StateDb::open(&allocation.state_db_path)?;
        state
            .validate_active_launch_attempt(&allocation.lease, &allocation.completion_authority)?;
        let spawn = SpawnIdentityContext::for_allocated_attempt(
            &allocation.lease,
            allocation.mailbox_db_path.clone(),
            context.model.name.clone(),
            context.working_dir.as_deref(),
            context.models_dir.as_deref(),
        )?;
        let mut identity = serde_json::json!({"source":context.provider.name,"id":owner.invocation_uuid.to_string()});
        identity[oulipoly_state::COMPLETION_REGISTRATION_AUTHORITY_LAUNCH_FIELD] = allocation
            .completion_authority
            .process_environment_value()
            .into();
        context.parent_invocation_env = Some(identity.to_string());
        let attempt = Arc::new(AttemptExecution {
            allocation: allocation.clone(),
            state: Mutex::new(state),
            actors: AttemptActorCustody::new(owner.attempt_id),
            spawn,
            evidence: Mutex::new(AttemptEvidence::default()),
        });
        context.attempt = Some(attempt.clone());
        Ok((context, attempt))
    };
    let (context, attempt) = match setup() {
        Ok(value) => value,
        Err(message) => return setup_failure(allocation, message),
    };
    if let Err(message) = register_allocated_runtime_generation_starting(&attempt.spawn) {
        // Do not exit or otherwise mutate a generation owned by an earlier call.
        return setup_failure(allocation, message);
    }
    let result = super::dispatch::attempt_account_dispatch(registry, &context);
    // Paths rejected before an operation call are explicitly accounted by the
    // single dispatch owner. A spawned/missing/uncertain receipt is never replaced.
    for operation in ["describe", "policy.evaluate", "launch"] {
        if !attempt
            .actors
            .receipts()
            .iter()
            .any(|r| r.operation == ProviderOperation::from_subcommand(operation))
        {
            attempt.actors.record_not_invoked(operation);
        }
    }
    if attempt
        .evidence
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .channel_settlement
        .is_none()
    {
        attempt.seal_channel();
    }
    let actors = attempt.actors.receipts();
    let launch_spawned = actors
        .iter()
        .any(|r| r.operation == ProviderOperation::Launch && r.spawned);
    if result.is_err() || !launch_spawned {
        let reason = if launch_spawned {
            RuntimeTerminalReason::AbnormalTermination
        } else {
            RuntimeTerminalReason::StartupFailed
        };
        let _ = exit_runtime_generation_outcome(Some(&attempt.spawn), reason, None);
    }
    attempt.refresh_promotions();
    match result {
        Ok(mut result) => {
            // Terminal/result effects precede all outer orchestration attachment.
            let retained = attempt.retain_children(&result.stderr);
            if result.produced_assistant_response {
                let _ = attempt.promote(ProviderLaunchPromotion::AssistantResponseObserved);
            }
            let evidence = attempt.evidence.lock().unwrap_or_else(|e| e.into_inner());
            result.captured_child_invocations = evidence.children.clone();
            result.returned_artifacts = evidence
                .channel_settlement
                .as_ref()
                .map(|s| s.artifacts().to_vec())
                .unwrap_or_default();
            if retained.is_err() || evidence.promotions.persistence_failed {
                result.exit_code = -1;
                result.terminal_reason = Some("promotion_persistence_failed".into());
            }
            ProviderLaunchAttemptOutcome::Completed(result)
        }
        Err(error) => {
            if let ProviderLaunchFailure::Provider(provider) = error.failure.as_ref() {
                let _ = attempt.retain_children(&provider.diagnostics().stderr_text());
            }
            let mut evidence = attempt.evidence.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(output) = &evidence.output {
                output.mark_incomplete();
                if output
                    .persist_for_invocation(
                        &attempt.state.lock().unwrap_or_else(|e| e.into_inner()),
                        owner.invocation_row_id,
                        &owner.invocation_uuid.to_string(),
                    )
                    .is_err()
                {
                    evidence.retention_failure = Some("failed_output_retention".into());
                }
            }
            ProviderLaunchAttemptOutcome::Failed(ProviderLaunchAttemptFailure {
                owner,
                rotatable_kind: rotatable_kind(&error.failure),
                error: *error.failure,
                observations: evidence.promotions.clone(),
                runtime_settlement: runtime_receipt(&allocation, &actors),
                actor_settlement: actors,
                return_channel_settlement: evidence
                    .channel_settlement
                    .clone()
                    .unwrap_or(ReturnChannelSettlement::NotCreated),
                requests: attempt.actors.requests(),
                captured_child_invocations: evidence.children.clone(),
                output_spool: evidence.output.clone(),
                evidence_retention_failure: evidence.retention_failure.clone(),
            })
        }
    }
}
fn setup_failure(
    allocation: AllocatedProviderLaunchAttempt,
    message: String,
) -> ProviderLaunchAttemptOutcome {
    ProviderLaunchAttemptOutcome::Failed(ProviderLaunchAttemptFailure {
        owner: allocation.lease.owner.clone(),
        error: ProviderLaunchFailure::Execution(ServiceError::InvalidRequest { message }),
        rotatable_kind: None,
        observations: ProviderLaunchPromotionSummary::default(),
        actor_settlement: vec![],
        runtime_settlement: runtime_receipt(&allocation, &[]),
        return_channel_settlement: ReturnChannelSettlement::NotCreated,
        requests: vec![],
        captured_child_invocations: vec![],
        output_spool: None,
        evidence_retention_failure: None,
    })
}
fn rotatable_kind(error: &ProviderLaunchFailure) -> Option<RotatableLaunchFailureKind> {
    let ProviderLaunchFailure::Provider(error) = error else {
        return None;
    };
    if !super::error_mapper::provider_client_error_is_rotatable(error) {
        return None;
    }
    if error.transport_kind() == "host_timeout" {
        Some(RotatableLaunchFailureKind::HostTimeout)
    } else if error.provider_category()
        == Some(oulipoly_provider::generated::ErrorCategory::Unavailable)
    {
        Some(RotatableLaunchFailureKind::ProviderUnavailable)
    } else {
        Some(RotatableLaunchFailureKind::ProviderTimeout)
    }
}
fn runtime_receipt(
    allocation: &AllocatedProviderLaunchAttempt,
    actors: &[ActorSettlementReceipt],
) -> RuntimeSettlementReceipt {
    let mut receipt = RuntimeSettlementReceipt {
        runtime_generation_uuid: allocation.lease.runtime_generation_uuid,
        spawn_invocation_uuid: allocation.lease.owner.invocation_uuid,
        row: None,
        row_sha256: None,
        effect_incapable: false,
        uncertainty: None,
    };
    let read = || -> Result<RuntimeGenerationRow, String> {
        let db = MailboxDb::open_read_only(&allocation.mailbox_db_path)?;
        let generation =
            RuntimeGenerationId::parse(&allocation.lease.runtime_generation_uuid.to_string())
                .map_err(|e| e.to_string())?;
        db.runtime_lifecycle_reader()
            .runtime_generation(&generation)
            .map_err(|e| e.to_string())?
            .ok_or("runtime_generation_absent".into())
    };
    match read() {
        Err(_) => receipt.uncertainty = Some("runtime_row_unreadable_or_absent".into()),
        Ok(row) => {
            receipt.row_sha256 = serde_json::to_vec(&row)
                .ok()
                .map(|b| format!("{:x}", Sha256::digest(b)));
            receipt.effect_incapable =
                runtime_row_effect_incapable(&allocation.lease, &row, actors)
                    && receipt.row_sha256.is_some();
            if !receipt.effect_incapable {
                receipt.uncertainty = Some("runtime_custody_not_proven".into());
            }
            receipt.row = Some(row);
        }
    }
    receipt
}

fn runtime_row_effect_incapable(
    lease: &ProviderLaunchLease,
    row: &RuntimeGenerationRow,
    actors: &[ActorSettlementReceipt],
) -> bool {
    let process_safe = match &row.exact_process_evidence {
        ExactProcessEvidence::NotRecorded => {
            row.spawned_os_pid.is_none()
                && row.terminal_reason == Some(RuntimeTerminalReason::StartupFailed)
                && actors.iter().any(|a| {
                    a.operation == ProviderOperation::Launch && !a.spawned && a.effect_incapable()
                })
        }
        ExactProcessEvidence::Recorded(identity) => {
            actors.iter().any(|a| {
                a.operation == ProviderOperation::Launch
                    && a.effect_incapable()
                    && a.exact_process_identity.as_ref().is_some_and(|p| {
                        p.os_pid == identity.os_pid
                            && p.os_boot_id == identity.os_boot_id
                            && p.os_pid_starttime_ticks == identity.os_pid_starttime_ticks
                    })
            }) && row.spawned_os_pid == Some(identity.os_pid)
                && row.terminal_reason == Some(RuntimeTerminalReason::AbnormalTermination)
        }
    };
    row.generation_id.to_string() == lease.runtime_generation_uuid.to_string()
        && row.spawn_invocation_uuid == lease.owner.invocation_uuid.to_string()
        && row.lifecycle_state == RuntimeLifecycleState::Exited
        && row.exited_at.is_some()
        && row.active_delivery_claim_id.is_none()
        && row.active_delivery_claimed_at.is_none()
        && row.active_delivery_seqs.is_empty()
        && process_safe
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_state::{
        BeginProviderLaunchRequest, ProviderLaunchAttemptAllocation, ProviderLaunchCandidate,
        ProviderLaunchStartMode,
    };
    fn fixture() -> (
        tempfile::TempDir,
        AttemptExecution,
        ExternalProviderDispatchContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("state.db");
        let state = StateDb::open(&state_path).unwrap();
        let allocated = ProviderLaunchAttemptAllocation::allocate().unwrap();
        let request = BeginProviderLaunchRequest {
            logical_launch_id: Uuid::new_v4(),
            request_identity_sha256: "a".repeat(64),
            model_name: "test".into(),
            start_mode: ProviderLaunchStartMode::Create,
            expected_provider_session_id: None,
            candidates: vec![ProviderLaunchCandidate {
                provider_index: 0,
                account_name: "account".into(),
            }],
            parent_invocation_id: None,
            allocation: allocated.clone(),
        };
        let lease = state.begin_launch(&request).unwrap();
        state
            .activate_attempt(&lease, &allocated.completion_authority)
            .unwrap();
        let allocation = AllocatedProviderLaunchAttempt {
            lease: lease.clone(),
            completion_authority: allocated.completion_authority,
            state_db_path: state_path,
            mailbox_db_path: dir.path().join("pid-identity.db"),
            channel_root: dir.path().to_path_buf(),
            parent_invocation_uuid: Uuid::new_v4(),
        };
        let spawn = SpawnIdentityContext::for_allocated_attempt(
            &lease,
            allocation.mailbox_db_path.clone(),
            "test".into(),
            None,
            None,
        )
        .unwrap();
        let attempt = AttemptExecution {
            allocation,
            state: Mutex::new(state),
            actors: AttemptActorCustody::new(lease.owner.attempt_id),
            spawn,
            evidence: Mutex::new(AttemptEvidence::default()),
        };
        let mut provider = oulipoly_config::ProviderConfig::new("/unused", vec![]);
        provider.name = "account".into();
        let model = oulipoly_config::ModelConfig {
            name: "test".into(),
            prompt_mode: oulipoly_config::PromptMode::Arg,
            providers: vec![provider],
            inputs: vec![],
            provider: None,
        };
        let context = crate::executor::external_provider_context_from_request(
            ExecutorServiceRequest::Facade {
                model,
                provider_index: 0,
                prompt: "prompt".into(),
                working_dir: None,
                models_dir: None,
                extra_inputs: Default::default(),
                parent_invocation_env: None,
            },
        )
        .unwrap();
        (dir, attempt, context)
    }
    #[test]
    fn every_closed_source_independently_promotes_and_durable_mailbox_acceptance_is_read_back() {
        for promotion in [
            ProviderLaunchPromotion::ProviderSessionObserved,
            ProviderLaunchPromotion::PromptAccepted,
            ProviderLaunchPromotion::AssistantResponseObserved,
            ProviderLaunchPromotion::CapturedChild,
            ProviderLaunchPromotion::ReturnedArtifact,
            ProviderLaunchPromotion::MailboxSubmissionAccepted,
        ] {
            let (_dir, attempt, _) = fixture();
            assert!(
                !attempt
                    .evidence
                    .lock()
                    .unwrap()
                    .promotions
                    .transfer_forbidden()
            );
            // Mailbox/effect writers can accept under the same State fence without
            // calling an in-memory observer. The terminal readback must join them.
            attempt
                .state
                .lock()
                .unwrap()
                .record_promotion(&attempt.allocation.lease.owner, Uuid::new_v4(), promotion)
                .unwrap();
            attempt.refresh_promotions();
            assert!(
                attempt
                    .evidence
                    .lock()
                    .unwrap()
                    .promotions
                    .transfer_forbidden()
            );
            assert!(
                !attempt
                    .evidence
                    .lock()
                    .unwrap()
                    .promotions
                    .persistence_failed
            );
        }
    }
    #[test]
    fn heartbeat_route_and_unpublished_bytes_are_not_promotion_but_split_child_markers_are() {
        let (_dir, attempt, context) = fixture();
        for event in [
            DecodedLaunchEvent::Heartbeat {
                seq: 1,
                detail: None,
            },
            DecodedLaunchEvent::Stdout {
                seq: 2,
                data: b"partial".to_vec(),
            },
            DecodedLaunchEvent::Marker {
                seq: 3,
                name: "route".into(),
                value: serde_json::json!({"account":"metadata"}),
            },
        ] {
            attempt.observe(&context, &event).unwrap();
        }
        assert!(
            !attempt
                .evidence
                .lock()
                .unwrap()
                .promotions
                .transfer_forbidden()
        );
        let id = Uuid::new_v4();
        let marker = format!("OULIPOLY_INVOCATION={{\"source\":\"fixture\",\"id\":\"{id}\"}}\n");
        for chunk in marker.as_bytes().chunks(7) {
            attempt
                .observe(
                    &context,
                    &DecodedLaunchEvent::Stderr {
                        seq: 4,
                        data: chunk.to_vec(),
                    },
                )
                .unwrap();
        }
        let evidence = attempt.evidence.lock().unwrap();
        assert!(evidence.promotions.captured_child);
        assert_eq!(evidence.children.len(), 1);
        assert_eq!(evidence.children[0].composite_id.id, id.to_string());
    }
    #[test]
    fn prompt_and_session_verification_precede_promotion_and_stale_write_fails_closed() {
        let (_dir, mut attempt, mut context) = fixture();
        let session = DecodedLaunchEvent::Marker {
            seq: 1,
            name: super::super::launch_result_mapper::PROVIDER_SESSION_MARKER.into(),
            value: serde_json::json!({"provider_session_id":"actual"}),
        };
        context.start_known_provider_session_id = Some("expected".into());
        assert!(attempt.observe(&context, &session).is_err());
        assert!(
            !attempt
                .evidence
                .lock()
                .unwrap()
                .promotions
                .provider_session_observed
        );
        context.start_known_provider_session_id = None;
        attempt.observe(&context, &session).unwrap();
        let expected = oulipoly_provider::generated::PromptAcceptanceRequestV1 {
            protocol: oulipoly_provider::generated::PROMPT_ACCEPTANCE_V1.into(),
            prompt_sha256: "a".repeat(64),
            delivery_nonce: Some("nonce".into()),
        };
        attempt.evidence.lock().unwrap().prompt = Some(expected);
        let marker = |hash: &str| DecodedLaunchEvent::Marker {
            seq: 2,
            name: oulipoly_provider::generated::PROMPT_ACCEPTED_MARKER_V1.into(),
            value: serde_json::json!({"protocol":oulipoly_provider::generated::PROMPT_ACCEPTANCE_V1,"provider_session_id":"actual","prompt_sha256":hash,"delivery_nonce":"nonce"}),
        };
        assert!(attempt.observe(&context, &marker(&"b".repeat(64))).is_err());
        assert!(!attempt.evidence.lock().unwrap().promotions.prompt_accepted);
        attempt.observe(&context, &marker(&"a".repeat(64))).unwrap();
        assert!(attempt.evidence.lock().unwrap().promotions.prompt_accepted);
        attempt.allocation.lease.owner.owner_epoch += 1;
        assert!(
            attempt
                .promote(ProviderLaunchPromotion::AssistantResponseObserved)
                .is_err()
        );
        assert!(
            attempt
                .evidence
                .lock()
                .unwrap()
                .promotions
                .persistence_failed
        );
    }
    #[test]
    fn runtime_receipt_rejects_wrong_generation_invocation_status_claim_and_unreadable_row() {
        let (_dir, attempt, _) = fixture();
        let allocation = &attempt.allocation;
        assert!(!runtime_receipt(allocation, &[]).effect_incapable);
        register_allocated_runtime_generation_starting(&attempt.spawn).unwrap();
        attempt.actors.record_not_invoked("launch");
        let actors = attempt.actors.receipts();
        assert!(!runtime_receipt(allocation, &actors).effect_incapable);
        exit_runtime_generation_outcome(
            Some(&attempt.spawn),
            RuntimeTerminalReason::StartupFailed,
            None,
        )
        .unwrap();
        let receipt = runtime_receipt(allocation, &actors);
        assert!(receipt.effect_incapable, "{receipt:?}");
        let row = receipt.row.unwrap();
        assert!(runtime_row_effect_incapable(
            &allocation.lease,
            &row,
            &actors
        ));
        let mut wrong = row.clone();
        wrong.generation_id = RuntimeGenerationId::new();
        assert!(!runtime_row_effect_incapable(
            &allocation.lease,
            &wrong,
            &actors
        ));
        let mut wrong = row.clone();
        wrong.spawn_invocation_uuid = Uuid::new_v4().to_string();
        assert!(!runtime_row_effect_incapable(
            &allocation.lease,
            &wrong,
            &actors
        ));
        let mut wrong = row.clone();
        wrong.lifecycle_state = RuntimeLifecycleState::Starting;
        assert!(!runtime_row_effect_incapable(
            &allocation.lease,
            &wrong,
            &actors
        ));
        let mut wrong = row.clone();
        wrong.active_delivery_claim_id = Some(oulipoly_state::mailbox::DeliveryClaimId::new());
        assert!(!runtime_row_effect_incapable(
            &allocation.lease,
            &wrong,
            &actors
        ));
        let mut wrong = row.clone();
        wrong.active_delivery_seqs = vec![1];
        assert!(!runtime_row_effect_incapable(
            &allocation.lease,
            &wrong,
            &actors
        ));
        let mut wrong = row.clone();
        wrong.terminal_reason = Some(RuntimeTerminalReason::AbnormalTermination);
        assert!(!runtime_row_effect_incapable(
            &allocation.lease,
            &wrong,
            &actors
        ));
        assert!(!runtime_row_effect_incapable(&allocation.lease, &row, &[]));
    }
}
