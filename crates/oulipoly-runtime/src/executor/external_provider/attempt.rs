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
    ExactProcessEvidence, MailboxDb, RuntimeGenerationRow, RuntimeLifecycleState,
    RuntimeTerminalReason,
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
/// A live published row observation plus independent actor evidence. A digest
/// does not extend its lifetime into a mutation capability: State revalidates
/// native facts/claims under its authority fence before retention or settlement.
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct OriginalRuntimeExitAttempt {
    terminal_code: String,
    exit_code: Option<i32>,
    site: String,
    projection_result: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct RetainedAttemptCustody {
    lease: ProviderLaunchLease,
    actors: Vec<ActorSettlementReceipt>,
    channel: ReturnChannelSettlement,
    retention_failure: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    runtime_exit_attempts: Vec<OriginalRuntimeExitAttempt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    runtime_exit_operations: Vec<super::super::cli::runtime_exit_journal::ExitObservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recovery_evidence: Option<serde_json::Value>,
}

/// Original producer receipts remain the authority even when outer activation
/// custody later drains. Outer ECHILD never substitutes for missing receipts.
pub fn settle_retained_native_cancellation(
    state: &StateDb,
    generation: Uuid,
    invocation: Uuid,
) -> Result<(), String> {
    let raw = state
        .native_recovered_attempt_custody(generation, invocation)?
        .or(state.native_attempt_custody(generation, invocation)?);
    let mut retained: RetainedAttemptCustody = match raw {
        Some(raw) => serde_json::from_value(raw).map_err(|e| e.to_string())?,
        None => serde_json::from_value(recover_native_custody(state, generation, invocation)?)
            .map_err(|e| e.to_string())?,
    };
    if retained.retention_failure.is_some() || !complete_actor_receipts(&retained.actors) {
        // Incomplete aggregate observations remain immutable. Stronger original
        // waits may supply a separate recovery observation, never a fabricated
        // rewrite of what the original executor reported.
        retained = serde_json::from_value(recover_native_custody(state, generation, invocation)?)
            .map_err(|e| e.to_string())?;
    }
    if retained.lease.runtime_generation_uuid != generation
        || retained.lease.owner.invocation_uuid != invocation
    {
        return Err("native_custody_identity_mismatch".into());
    }
    // Reusing an old recovered aggregate requires the actual published drain,
    // even if its immutable replay was originally retained from detached bytes.
    if retained.recovery_evidence.is_some() {
        state.retain_native_recovered_attempt_custody(
            &retained.lease.owner,
            &serde_json::to_value(&retained).map_err(|e| e.to_string())?,
        )?;
    }
    // Aggregate retention is independent of artifact persistence, too. A cached
    // CleanupFailed cannot suppress this retry after State storage recovers.
    if !retained.channel.artifacts().is_empty() {
        state.record_promotion(
            &retained.lease.owner,
            retained.lease.owner.attempt_id,
            ProviderLaunchPromotion::ReturnedArtifact,
        )?;
        state.record_returned_artifacts(
            InvocationMutationAuthority::ProviderLaunch(&retained.lease.owner),
            retained.lease.owner.invocation_row_id,
            retained.channel.artifacts(),
        )?;
    }
    let mailbox_path = MailboxDb::path_for_state_db(state.path());
    let mut runtime = runtime_receipt_for(&retained.lease, &mailbox_path, &retained.actors);
    // Require the authored logical cancellation independently of when original
    // custody observed it. A later request never changes that immutable drain.
    let _ = logical_cancellation_observation(state, &retained.lease)?;
    // A complete original aggregate can precede native runtime finalization.
    // Its existence must not suppress projection from the later original drain.
    // The mailbox transitions require integrated original custody. Uninvoked
    // Launch retains startup failure; spawned Launch keeps the original executor's
    // attempted runtime outcome. Raw status and late cancellation cannot relabel it.
    if !runtime.effect_incapable
        && complete_actor_receipts(&retained.actors)
        && retained
            .actors
            .iter()
            .all(|actor| actor.attempt_id == retained.lease.owner.attempt_id)
    {
        let launch = retained
            .actors
            .iter()
            .find(|a| a.operation == ProviderOperation::Launch)
            .ok_or("native_launch_receipt_absent")?;
        let mut mailbox = MailboxDb::open(&mailbox_path)?;
        if launch.spawned {
            let process = launch
                .exact_process_identity
                .as_ref()
                .ok_or("native_launch_identity_absent")?;
            let drain = mailbox
                .native_original_drain(&generation.to_string(), &invocation.to_string())?
                .ok_or("native_original_drain_absent")?;
            if drain["receipt"]["accepted_cancellation"].is_string() {
                // Keep the pre-existing original-cancellation projection. It is
                // never used to fill an uncancelled original observation.
                mailbox.exit_native_cancelled_after_original_drain(
                    &generation.to_string(),
                    &invocation.to_string(),
                    false,
                )?;
            } else {
                let (reason, code, drain_request) = original_runtime_exit_outcome(
                    &retained,
                    runtime.row.as_ref().ok_or("native_runtime_absent")?,
                )?;
                mailbox.exit_native_launched_after_original_drain(
                    &generation.to_string(),
                    &invocation.to_string(),
                    &oulipoly_state::completion_continuation::SourceProcessIdentity {
                        pid: process.os_pid,
                        boot_id: process.os_boot_id.clone(),
                        starttime_ticks: process.os_pid_starttime_ticks,
                    },
                    reason,
                    code,
                    drain_request.as_deref(),
                )?;
            }
        } else {
            mailbox.exit_native_cancelled_after_original_drain(
                &generation.to_string(),
                &invocation.to_string(),
                true,
            )?;
        }
        runtime = runtime_receipt_for(&retained.lease, &mailbox_path, &retained.actors);
    }
    let continuing = match &retained.channel {
        ReturnChannelSettlement::Quarantined {
            path, artifacts, ..
        }
        | ReturnChannelSettlement::CleanupFailed { path, artifacts } => {
            let drain = MailboxDb::read_native_publication(
                &mailbox_path,
                &generation.to_string(),
                &invocation.to_string(),
            )?
            .original_drain
            .ok_or("native_channel_continuing_domain_owner_absent")?;
            let duty = oulipoly_state::ProviderLaunchChannelSettlement::ContinuingCustody {
                domain_id: drain["domain_id"]
                    .as_str()
                    .ok_or("native_channel_domain_absent")?
                    .to_string(),
                original_owner: retained.lease.owner.clone(),
                disposition: if matches!(
                    retained.channel,
                    ReturnChannelSettlement::Quarantined { .. }
                ) {
                    "quarantined"
                } else {
                    "cleanup_failed"
                }
                .into(),
                path: path.to_string_lossy().into_owned(),
                artifacts: artifacts.clone(),
            };
            state.retain_native_channel_duty(&retained.lease.owner, &duty)?;
            Some(duty)
        }
        _ => None,
    };
    let supplement = retain_recovered_dead_cancellation(state, &retained, &runtime)?;
    let proof = cancellation_proof(&retained, &runtime, continuing, supplement.as_ref())?;
    state.settle_cancel(&retained.lease.owner, &proof)
}

/// Generic Starting recovery is an honest `recovered_dead` observation, not
/// proof of an uninvoked launch. Keep that row unchanged. Only complete original
/// operation receipts and the exact original activation drain can additionally
/// establish settlement for an independently accepted cancellation.
fn retain_recovered_dead_cancellation(
    state: &StateDb,
    retained: &RetainedAttemptCustody,
    runtime: &RuntimeSettlementReceipt,
) -> Result<Option<serde_json::Value>, String> {
    if runtime.effect_incapable {
        return Ok(None);
    }
    let Some(row) = &runtime.row else {
        return Ok(None);
    };
    if row.terminal_reason != Some(RuntimeTerminalReason::RecoveredDead)
        || !complete_actor_receipts(&retained.actors)
        || retained
            .actors
            .iter()
            .any(|actor| actor.attempt_id != retained.lease.owner.attempt_id)
    {
        return Ok(None);
    }
    let launch = retained
        .actors
        .iter()
        .find(|a| a.operation == ProviderOperation::Launch)
        .ok_or("native_launch_receipt_absent")?;
    let published = MailboxDb::read_native_publication(
        &MailboxDb::path_for_state_db(state.path()),
        &retained.lease.runtime_generation_uuid.to_string(),
        &retained.lease.owner.invocation_uuid.to_string(),
    )?;
    let drain = published
        .original_drain
        .ok_or("native_original_drain_absent")?;
    if published.runtime.as_ref() != Some(row) {
        return Err("native_publication_runtime_changed".into());
    }
    let mut interpretation = row.clone();
    let (classification, reason) = if launch.spawned {
        (
            "original_launched_outcome_after_recovered_dead",
            if drain["receipt"]["accepted_cancellation"].is_string() {
                RuntimeTerminalReason::Cancelled
            } else {
                MailboxDb::open(&MailboxDb::path_for_state_db(state.path()))?
                    .require_native_runtime_quiescent(
                        &retained.lease.runtime_generation_uuid.to_string(),
                        &retained.lease.owner.invocation_uuid.to_string(),
                    )?;
                original_runtime_exit_outcome(retained, row)?.0
            },
        )
    } else {
        (
            "original_never_invoked_after_recovered_dead",
            RuntimeTerminalReason::StartupFailed,
        )
    };
    interpretation.terminal_reason = Some(reason);
    // Existing exact-process/claim validator applied only to this separate
    // interpretation. Generic recovered_dead history and transfer stay unchanged.
    if !runtime_row_effect_incapable(&retained.lease, &interpretation, &retained.actors) {
        return Ok(None);
    }
    let cancellation = logical_cancellation_observation(state, &retained.lease)?;
    let mut evidence = serde_json::json!({
        "classification":classification,
        "lease":retained.lease,"original_runtime_row":row,
        "original_drain":drain,"original_actor_receipts":retained.actors,
        "cancellation_terminal_code":reason
    });
    if !drain["receipt"]["accepted_cancellation"].is_string() {
        // Keep already-authored cancelled-drain supplements byte-semantically
        // stable on replay; only the distinct uncancelled ordering needs this
        // separate persisted acceptance observation.
        evidence["logical_cancellation"] = cancellation;
    }
    if launch.spawned && !retained.runtime_exit_attempts.is_empty() {
        evidence["original_runtime_exit_attempts"] =
            serde_json::to_value(&retained.runtime_exit_attempts).map_err(|e| e.to_string())?;
    }
    if launch.spawned && !retained.runtime_exit_operations.is_empty() {
        evidence["original_runtime_exit_operations"] =
            serde_json::to_value(&retained.runtime_exit_operations).map_err(|e| e.to_string())?;
    }
    state.retain_native_runtime_cancellation(&retained.lease.owner, &evidence)?;
    Ok(Some(evidence))
}

/// Select compatible actual operations, never the last outer helper label.
/// Contradictory eligible requests stay unresolved; rejected requests remain in
/// the retained history. Explicit custody refusals can supply a request for a
/// NEW recovery operation, not proof that the rejected transition completed.
/// Both that request and absent-result requests require fresh exact Q at the
/// authority consumer. Unclassified historical rejections remain unknown.
fn original_runtime_exit_outcome(
    retained: &RetainedAttemptCustody,
    row: &RuntimeGenerationRow,
) -> Result<(RuntimeTerminalReason, Option<i32>, Option<String>), String> {
    use super::super::cli::runtime_exit_journal::{ExitDisposition, ExitOperation};
    let mut selected = None;
    for observation in &retained.runtime_exit_operations {
        let intent = &observation.intent;
        if intent.generation != retained.lease.runtime_generation_uuid.to_string()
            || intent.invocation != retained.lease.owner.invocation_uuid.to_string()
        {
            return Err("native_exit_journal_identity_conflict".into());
        }
        if observation.result.as_ref().is_some_and(|r| {
            (r.rejected && !r.custody_refused) || r.disposition == ExitDisposition::AlreadyExited
        }) {
            continue;
        }
        if row.terminal_reason == Some(RuntimeTerminalReason::RecoveredDead)
            && observation.result.as_ref().is_some_and(|r| {
                matches!(
                    r.disposition,
                    ExitDisposition::Applied
                        | ExitDisposition::AlreadyApplied
                        | ExitDisposition::Finished
                )
            })
        {
            return Err("native_original_durable_runtime_outcome_conflict".into());
        }
        let Some(before) = &observation.before else {
            continue;
        };
        if before["spawn_invocation_uuid"] != intent.invocation
            || before["generation_id"] != intent.generation
            || before["exact_process_evidence"]
                != serde_json::to_value(&row.exact_process_evidence).map_err(|e| e.to_string())?
        {
            return Err("native_exit_predecessor_identity_conflict".into());
        }
        let recovered = row.terminal_reason == Some(RuntimeTerminalReason::RecoveredDead);
        let reason = match intent.operation {
            ExitOperation::FinalizeDrain => continue, // helper may branch; NOT an SQL outcome
            ExitOperation::FinishDrain
                if intent.reason == "orderly_completion"
                    && before["lifecycle_state"] == "draining"
                    && intent.drain_request.is_some()
                    && before["drain_request_id"].as_str() == intent.drain_request.as_deref()
                    && row.drain_request_id.as_ref().map(ToString::to_string)
                        == intent.drain_request
                    && (row.lifecycle_state == RuntimeLifecycleState::Draining || recovered) =>
            {
                RuntimeTerminalReason::OrderlyCompletion
            }
            ExitOperation::NonOrderly
                if intent.reason == "abnormal_termination"
                    && matches!(
                        before["lifecycle_state"].as_str(),
                        Some("starting" | "running")
                    )
                    && (matches!(
                        row.lifecycle_state,
                        RuntimeLifecycleState::Starting | RuntimeLifecycleState::Running
                    ) || recovered) =>
            {
                RuntimeTerminalReason::AbnormalTermination
            }
            _ => continue,
        };
        let candidate = (reason, intent.exit_code, intent.drain_request.clone());
        if selected.as_ref().is_some_and(|prior| prior != &candidate) {
            return Err("native_original_runtime_operations_conflict".into());
        }
        selected = Some(candidate);
    }
    selected.ok_or("native_original_applicable_runtime_operation_absent".into())
}

/// The request's persisted timestamp/token is separate from the original drain
/// observer's cancellation field. NULL there is not backdated into an acceptance.
fn logical_cancellation_observation(
    state: &StateDb,
    lease: &ProviderLaunchLease,
) -> Result<serde_json::Value, String> {
    let (launch, requested): (String, String) = state.connection().query_row(
        "SELECT l.logical_launch_id,l.cancel_requested_at FROM provider_logical_launches l JOIN provider_launch_attempts a ON a.attempt_id=l.current_attempt_id WHERE l.logical_launch_id=?1 AND a.attempt_id=?2 AND a.runtime_generation_uuid=?3 AND a.invocation_uuid=?4 AND l.status IN ('cancelling','cancelled') AND l.cancel_requested_at IS NOT NULL",
        rusqlite::params![lease.owner.logical_launch_id.to_string(), lease.owner.attempt_id.to_string(), lease.runtime_generation_uuid.to_string(), lease.owner.invocation_uuid.to_string()],
        |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| format!("native_logical_cancellation_absent: {e}"))?;
    Ok(
        serde_json::json!({"logical_launch_id":launch,"requested_at":requested,
        "token":format!("{launch}:{requested}")}),
    )
}

fn complete_actor_receipts(actors: &[ActorSettlementReceipt]) -> bool {
    actors.iter().all(ActorSettlementReceipt::effect_incapable)
        && [
            ProviderOperation::Describe,
            ProviderOperation::Policy,
            ProviderOperation::Launch,
        ]
        .iter()
        .all(|required| actors.iter().any(|a| &a.operation == required))
}

fn cancellation_proof(
    retained: &RetainedAttemptCustody,
    runtime: &RuntimeSettlementReceipt,
    continuing: Option<oulipoly_state::ProviderLaunchChannelSettlement>,
    runtime_supplement: Option<&serde_json::Value>,
) -> Result<oulipoly_state::ProviderLaunchCustodyProof, String> {
    use oulipoly_state::{
        ProviderLaunchActorSettlement as Actor, ProviderLaunchChannelSettlement as Channel,
    };
    if retained.retention_failure.is_some()
        || (!runtime.effect_incapable && runtime_supplement.is_none())
    {
        return Err("native_runtime_or_evidence_custody_unsettled".into());
    }
    let mut actors = Vec::new();
    let mut launch_identity = None;
    for receipt in &retained.actors {
        if receipt.attempt_id != retained.lease.owner.attempt_id || !receipt.effect_incapable() {
            return Err("native_actor_custody_unsettled".into());
        }
        let operation = match receipt.operation {
            ProviderOperation::Describe => "describe",
            ProviderOperation::Policy => "policy",
            ProviderOperation::Launch => "launch",
            ProviderOperation::TerminalClassify => "terminal_classify",
            _ => return Err("native_actor_operation_unrepresented".into()),
        }
        .to_string();
        actors.push(if let Some(identity) = &receipt.exact_process_identity {
            let digest = format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(identity).map_err(|e| e.to_string())?)
            );
            if receipt.operation == ProviderOperation::Launch {
                launch_identity = Some(digest.clone());
            }
            Actor::Reaped {
                operation,
                process_identity_sha256: digest,
                process_tree_terminated: receipt.process_tree_terminated,
                leader_reaped: receipt.leader_reaped,
            }
        } else {
            Actor::NeverSpawned { operation }
        });
    }
    let channel = match &retained.channel {
        ReturnChannelSettlement::NotCreated => Channel::NotCreated,
        ReturnChannelSettlement::EmptyRemoved => Channel::EmptyRemoved,
        ReturnChannelSettlement::ArtifactsCommitted(refs) => {
            Channel::ArtifactsCommitted(refs.clone())
        }
        ReturnChannelSettlement::Quarantined { .. }
        | ReturnChannelSettlement::CleanupFailed { .. } => {
            continuing.ok_or("native_channel_continuing_owner_unassigned")?
        }
    };
    let row = runtime
        .row
        .as_ref()
        .ok_or("native_runtime_receipt_absent")?;
    let reason = runtime_supplement
        .map(|value| value["cancellation_terminal_code"].clone())
        .map(Ok)
        .unwrap_or_else(|| serde_json::to_value(row.terminal_reason))
        .map_err(|e| e.to_string())?
        .as_str()
        .ok_or("native_runtime_terminal_reason_absent")?
        .to_string();
    Ok(oulipoly_state::ProviderLaunchCustodyProof {
        attempt_id: retained.lease.owner.attempt_id,
        runtime_generation_uuid: retained.lease.runtime_generation_uuid,
        spawn_invocation_uuid: retained.lease.owner.invocation_uuid,
        actors,
        runtime_terminal_code: reason,
        runtime_never_bound: launch_identity.is_none(),
        runtime_process_identity_sha256: launch_identity,
        runtime_exited: row.lifecycle_state == RuntimeLifecycleState::Exited,
        active_delivery_claim: row.active_delivery_claim_id.is_some(),
        runtime_settlement_sha256: match runtime_supplement {
            Some(value) => format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(value).map_err(|e| e.to_string())?)
            ),
            None => runtime
                .row_sha256
                .clone()
                .ok_or("native_runtime_digest_absent")?,
        },
        return_channel_id: retained.lease.return_channel_id.clone(),
        channel,
        return_channel_settlement_sha256: format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&retained.channel).map_err(|e| e.to_string())?)
        ),
    })
}

pub(crate) struct AttemptExecution {
    pub allocation: AllocatedProviderLaunchAttempt,
    state: Mutex<StateDb>,
    pub actors: AttemptActorCustody,
    pub cancellation: oulipoly_core::CancellationToken,
    pub spawn: SpawnIdentityContext,
    pub evidence: Mutex<AttemptEvidence>,
}
#[derive(Default)]
pub(crate) struct AttemptEvidence {
    pub promotions: ProviderLaunchPromotionSummary,
    pub endpoint: Option<oulipoly_state::ProviderLaunchEndpoint>,
    pub channel: Option<ReturnChannel>,
    pub channel_settlement: Option<ReturnChannelSettlement>,
    pub children: Vec<CapturedChildInvocation>,
    pub output: Option<ExecutionOutputSpool>,
    pub prompt: Option<oulipoly_provider::generated::PromptAcceptanceRequestV1>,
    pub verified_session: Option<String>,
    pub stderr_pending: Vec<u8>,
    pub retention_failure: Option<String>,
    runtime_exit_attempts: Vec<OriginalRuntimeExitAttempt>,
}
impl AttemptExecution {
    pub(crate) fn record_runtime_exit_attempt(
        &self,
        reason: RuntimeTerminalReason,
        exit_code: Option<i32>,
        site: &str,
        result: &impl std::fmt::Debug,
    ) {
        let terminal_code = match reason {
            RuntimeTerminalReason::OrderlyCompletion => "orderly_completion",
            RuntimeTerminalReason::AbnormalTermination => "abnormal_termination",
            RuntimeTerminalReason::StartupFailed => "startup_failed",
            RuntimeTerminalReason::Cancelled => "cancelled",
            RuntimeTerminalReason::RecoveredDead => "recovered_dead",
        };
        self.evidence
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .runtime_exit_attempts
            .push(OriginalRuntimeExitAttempt {
                terminal_code: terminal_code.into(),
                exit_code,
                site: site.into(),
                projection_result: format!("{result:?}"),
            });
    }

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
        let identity = endpoint.endpoint_identity()?;
        self.state
            .lock()
            .map_err(|_| "endpoint_state_unavailable")?
            .bind_launch_endpoint(&self.allocation.lease.owner, &identity)?;
        self.evidence
            .lock()
            .map_err(|_| "endpoint_evidence_unavailable")?
            .endpoint = Some(identity);
        Ok(())
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
            DecodedLaunchEvent::Marker { name, value, .. } if name == PROMPT_ACCEPTED_MARKER_V1 => {
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
                let session = evidence
                    .verified_session
                    .as_deref()
                    .or(context.start_known_provider_session_id.as_deref());
                if expected.prompt_sha256 != observed.prompt_sha256
                    || expected.delivery_nonce != observed.delivery_nonce
                    || session != Some(observed.provider_session_id.as_str())
                {
                    return Err("prompt_acceptance_mismatch".into());
                }
                Some(observed.provider_session_id)
            }
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
            if let Some(endpoint) = self
                .evidence
                .lock()
                .map_err(|_| "endpoint_evidence_unavailable")?
                .endpoint
                .clone()
            {
                self.state
                    .lock()
                    .map_err(|_| "session_state_unavailable")?
                    .commit_invocation_provider_session_authority(
                        InvocationMutationAuthority::ProviderLaunch(&self.allocation.lease.owner),
                        self.allocation.lease.owner.invocation_row_id,
                        &oulipoly_state::ProviderSessionAuthorityCommit {
                            invocation_uuid: &self
                                .allocation
                                .lease
                                .owner
                                .invocation_uuid
                                .to_string(),
                            provider_name: &context.provider.name,
                            provider_instance_id: &endpoint.provider_instance_id,
                            settings_id: &endpoint.settings_id,
                            binding: &oulipoly_state::ProviderSessionBinding {
                                provider_session_id: session.clone(),
                                capture_method: "external_provider_launch",
                                resume_input_id: context.start_known_provider_session_id.clone(),
                                provider_session_resolved_account: None,
                            },
                        },
                    )?;
            }
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
    fn finish_child_markers(&self) {
        // Dispatch has joined the event workers. A transport record's newline
        // does not imply a newline in its decoded stderr payload; parse the
        // remaining final line with the same marker grammar as complete lines.
        let pending = std::mem::take(
            &mut self
                .evidence
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .stderr_pending,
        );
        if self
            .retain_children(&String::from_utf8_lossy(&pending))
            .is_err()
        {
            self.evidence
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .promotions
                .persistence_failed = true;
        }
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
    fn retain_custody(&self) -> Result<(), String> {
        let evidence = self
            .evidence
            .lock()
            .map_err(|_| "attempt_evidence_unavailable")?;
        let retained = RetainedAttemptCustody {
            lease: self.allocation.lease.clone(),
            actors: self.actors.receipts(),
            channel: evidence
                .channel_settlement
                .clone()
                .ok_or("channel_settlement_absent")?,
            retention_failure: evidence.retention_failure.clone(),
            runtime_exit_attempts: evidence.runtime_exit_attempts.clone(),
            runtime_exit_operations: super::super::cli::runtime_exit_journal::read(
                &native_journal(&self.allocation).join("runtime-exit"),
                &self.allocation.lease.runtime_generation_uuid.to_string(),
                &self.allocation.lease.owner.invocation_uuid.to_string(),
            )?,
            recovery_evidence: None,
        };
        self.state
            .lock()
            .map_err(|_| "attempt_state_unavailable")?
            .retain_native_attempt_custody(
                &self.allocation.lease.owner,
                &serde_json::to_value(retained).map_err(|e| e.to_string())?,
            )
    }
    pub(crate) fn retain_channel(&self, channel: &mut ReturnChannel) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        if self
            .state
            .lock()
            .map_err(|_| "state unavailable")?
            .native_attempt_recovery(
                self.allocation.lease.runtime_generation_uuid,
                self.allocation.lease.owner.invocation_uuid,
            )?
            .is_some()
        {
            channel.retain_for_recovery(native_journal(&self.allocation).join("channel"))?;
        }
        Ok(())
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
    execute_allocated_attempt(registry, request, allocation, false)
}
pub fn execute_native_allocated_provider_attempt(
    registry: &ProviderRegistry,
    request: ExecutorServiceRequest,
    allocation: AllocatedProviderLaunchAttempt,
) -> ProviderLaunchAttemptOutcome {
    execute_allocated_attempt(registry, request, allocation, true)
}
fn execute_allocated_attempt(
    registry: &ProviderRegistry,
    request: ExecutorServiceRequest,
    allocation: AllocatedProviderLaunchAttempt,
    original_tree: bool,
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
        state.retain_launch_owner(&allocation.lease.owner)?;
        state
            .validate_active_launch_attempt(&allocation.lease, &allocation.completion_authority)?;
        let mut spawn = SpawnIdentityContext::for_allocated_attempt(
            &allocation.lease,
            allocation.mailbox_db_path.clone(),
            context.model.name.clone(),
            context.working_dir.as_deref(),
            context.models_dir.as_deref(),
        )?
        .with_start_known_session(context.start_known_provider_session_id.clone());
        state.validate_launch_start_session(
            &owner,
            context.start_known_provider_session_id.as_deref(),
        )?;
        let mut identity = serde_json::json!({"source":context.provider.name,"id":owner.invocation_uuid.to_string()});
        identity[oulipoly_state::COMPLETION_REGISTRATION_AUTHORITY_LAUNCH_FIELD] = allocation
            .completion_authority
            .process_environment_value()
            .into();
        context.parent_invocation_env = Some(identity.to_string());
        if original_tree {
            let journal = native_journal(&allocation);
            spawn = spawn.with_native_exit_journal(journal.join("runtime-exit"));
            std::fs::create_dir_all(journal.join("actors")).map_err(|e| e.to_string())?;
            oulipoly_provider::custody::durable::initialize_admissions(
                &journal.join("actors"),
                owner.attempt_id,
            )?;
            state.retain_native_attempt_recovery(&owner, &serde_json::json!({
                "lease":allocation.lease,"journal":journal,
                "channel_path":allocation.channel_root.join(allocation.parent_invocation_uuid.to_string())
                    .join(owner.logical_launch_id.to_string()).join(owner.attempt_id.to_string()).join("returns.jsonl")
            }))?;
        }
        let attempt = Arc::new(AttemptExecution {
            allocation: allocation.clone(),
            state: Mutex::new(state),
            actors: if original_tree {
                AttemptActorCustody::original_tree(owner.attempt_id)
                    .with_journal(native_journal(&allocation).join("actors"))
            } else {
                AttemptActorCustody::new(owner.attempt_id)
            },
            cancellation: oulipoly_core::CancellationToken::new(),
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
    let _cancel_watch = NativeCancellationWatch::start(&allocation, attempt.cancellation.clone());
    let result = super::dispatch::attempt_account_dispatch(registry, &context);
    attempt.finish_child_markers();
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
        let _projection = exit_runtime_generation_outcome(Some(&attempt.spawn), reason, None);
        attempt.record_runtime_exit_attempt(reason, None, "outer_attempt", &_projection);
        #[cfg(feature = "age360-fault-fixtures")]
        if _projection.is_err() {
            oulipoly_state::completion_continuation::age360_fault_barrier(
                "native-outer-exit-projection-failed",
            );
        }
    }
    attempt.refresh_promotions();
    let mut outcome = match result {
        Ok(mut result) => {
            // Missing-final is a mapped failed execution, not the Err arm below.
            // Persist its incomplete prefix before this attempt owner is dropped.
            if result.terminal_reason.as_deref() == Some("external_provider_missing_final_exit")
                && let Err(error) = result.persist_output_for_invocation(
                    &attempt.state.lock().unwrap_or_else(|e| e.into_inner()),
                    owner.invocation_row_id,
                    &owner.invocation_uuid.to_string(),
                )
            {
                result.exit_code = -1;
                result
                    .stderr
                    .push_str(&format!("\nmissing_final_output_retention_failed: {error}"));
                if let Some(signal) = &mut result.terminal_signal {
                    signal.evidence.push_str(";output_retention=failed");
                }
            }
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
    };
    #[cfg(feature = "age360-fault-fixtures")]
    if original_tree {
        oulipoly_state::completion_continuation::age360_fault_barrier(
            "native-before-custody-retention",
        );
    }
    if let Err(error) = attempt.retain_custody() {
        tracing::error!(%error, "native attempt custody retention failed; settlement unavailable");
        match &mut outcome {
            ProviderLaunchAttemptOutcome::Completed(result) => {
                result.exit_code = -1;
                result.terminal_reason = Some("native_custody_retention_failed".into());
                result
                    .stderr
                    .push_str(&format!("\nnative_custody_retention_failed: {error}"));
            }
            ProviderLaunchAttemptOutcome::Failed(failure) => {
                failure.evidence_retention_failure = Some(error);
                failure.observations.persistence_failed = true;
            }
        }
        #[cfg(feature = "age360-fault-fixtures")]
        oulipoly_state::completion_continuation::age360_fault_barrier(
            "native-custody-retention-failed",
        );
    }
    #[cfg(feature = "age360-fault-fixtures")]
    if original_tree {
        oulipoly_state::completion_continuation::age360_fault_barrier(
            "native-after-custody-retention",
        );
    }
    outcome
}

struct NativeCancellationWatch {
    stop: std::sync::mpsc::Sender<()>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl NativeCancellationWatch {
    fn start(
        allocation: &AllocatedProviderLaunchAttempt,
        token: oulipoly_core::CancellationToken,
    ) -> Self {
        let (stop, receiver) = std::sync::mpsc::channel();
        let path = allocation.state_db_path.clone();
        let owner = allocation.lease.owner.clone();
        let worker = std::thread::spawn(move || {
            // Read failures are uncertainty, never cancellation or proof of drain.
            while receiver.try_recv().is_err() {
                if let Ok(state) = StateDb::open(&path)
                    && state.launch_cancellation_requested(&owner).unwrap_or(false)
                {
                    token.cancel();
                    return;
                }
                if receiver.recv_timeout(std::time::Duration::from_millis(25))
                    != Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                {
                    return;
                }
            }
        });
        Self {
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for NativeCancellationWatch {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
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
    runtime_receipt_for(&allocation.lease, &allocation.mailbox_db_path, actors)
}
/// Private fault-catalog observation of the real receipt consumer, not a fake
/// receipt constructor. Normal images do not export this fixture entry point.
#[cfg(feature = "age360-fault-fixtures")]
pub fn age360_observe_native_runtime(
    lease: &ProviderLaunchLease,
    mailbox_path: &std::path::Path,
    actors: &[ActorSettlementReceipt],
) -> serde_json::Value {
    serde_json::to_value(runtime_receipt_for(lease, mailbox_path, actors))
        .expect("receipt serialization")
}

fn runtime_receipt_for(
    lease: &ProviderLaunchLease,
    mailbox_path: &std::path::Path,
    actors: &[ActorSettlementReceipt],
) -> RuntimeSettlementReceipt {
    let mut receipt = RuntimeSettlementReceipt {
        runtime_generation_uuid: lease.runtime_generation_uuid,
        spawn_invocation_uuid: lease.owner.invocation_uuid,
        row: None,
        row_sha256: None,
        effect_incapable: false,
        uncertainty: None,
    };
    let read = || -> Result<RuntimeGenerationRow, String> {
        MailboxDb::read_native_publication(
            mailbox_path,
            &lease.runtime_generation_uuid.to_string(),
            &lease.owner.invocation_uuid.to_string(),
        )?
        .runtime
        .ok_or("runtime_generation_absent".into())
    };
    match read() {
        Err(_) => receipt.uncertainty = Some("runtime_row_unreadable_or_absent".into()),
        Ok(row) => {
            receipt.row_sha256 = serde_json::to_vec(&row)
                .ok()
                .map(|b| format!("{:x}", Sha256::digest(b)));
            receipt.effect_incapable =
                runtime_row_effect_incapable(lease, &row, actors) && receipt.row_sha256.is_some();
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
                && matches!(
                    row.terminal_reason,
                    Some(
                        RuntimeTerminalReason::AbnormalTermination
                            | RuntimeTerminalReason::OrderlyCompletion
                            | RuntimeTerminalReason::Cancelled
                    )
                )
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

fn native_journal(allocation: &AllocatedProviderLaunchAttempt) -> PathBuf {
    allocation
        .state_db_path
        .with_extension("native-producer-custody")
        .join(allocation.lease.owner.attempt_id.to_string())
}

#[cfg(target_os = "linux")]
fn recover_native_custody(
    state: &StateDb,
    generation: Uuid,
    invocation: Uuid,
) -> Result<serde_json::Value, String> {
    use oulipoly_provider::custody::{ProcessIdentity, durable};
    let source = state
        .native_attempt_recovery(generation, invocation)?
        .ok_or("native_recovery_intent_absent")?;
    let lease: ProviderLaunchLease =
        serde_json::from_value(source["lease"].clone()).map_err(|e| e.to_string())?;
    if lease.runtime_generation_uuid != generation || lease.owner.invocation_uuid != invocation {
        return Err("native_recovery_identity_conflict".into());
    }
    let drain = MailboxDb::read_native_publication(
        &MailboxDb::path_for_state_db(state.path()),
        &generation.to_string(),
        &invocation.to_string(),
    )?
    .original_drain
    .ok_or("original_native_boundary_not_drained")?;
    let journal = PathBuf::from(source["journal"].as_str().ok_or("native_journal_absent")?);
    let waits = PathBuf::from(
        drain["result_path"]
            .as_str()
            .ok_or("original_wait_path_absent")?,
    )
    .with_file_name("owned-waits");
    let mut actors = Vec::new();
    let mut evidence = Vec::new();
    for entry in std::fs::read_dir(journal.join("actors")).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path
            .file_name()
            .is_some_and(|name| name == "admissions" || name == "not-invoked")
        {
            continue;
        }
        let finished: Result<ActorSettlementReceipt, _> =
            durable::read_json(&path.join("finished.json"));
        if let Ok(receipt) = &finished
            && receipt.attempt_id == lease.owner.attempt_id
            && receipt.effect_incapable()
        {
            actors.push(receipt.clone());
            continue;
        }
        let mut actor = durable::intent(&path)?;
        if actor.attempt_id != lease.owner.attempt_id {
            return Err("native_actor_intent_conflict".into());
        }
        let identity = durable::attributed_proxy(&path)?;
        let terminal: Result<(ProcessIdentity, i32), _> =
            durable::read_json(&path.join("terminal.json"));
        let (status, observation) = if let Ok((observed, status)) = terminal {
            if observed != identity {
                return Err("native_terminal_identity_conflict".into());
            }
            (
                status,
                serde_json::json!({"producer_terminal":path.join("terminal.json"), "process":identity,"status":status}),
            )
        } else {
            let mut observed = None;
            for entry in std::fs::read_dir(&waits).map_err(|e| e.to_string())? {
                let value: serde_json::Value =
                    durable::read_json(&entry.map_err(|e| e.to_string())?.path())?;
                if value["attempt_id"] == drain["attempt_id"]
                    && value["observation"] == "waitid_wnowait"
                    && (value["owner"] == drain["custodian"] || value["owner"] == drain["adopter"])
                    && value["process"]["pid"] == identity.os_pid
                    && value["process"]["boot_id"] == identity.os_boot_id
                    && value["process"]["starttime_ticks"] == identity.os_pid_starttime_ticks
                {
                    observed = Some(value);
                    break;
                }
            }
            let value = observed.ok_or("original_native_actor_wait_absent")?;
            (
                value["status"]
                    .as_i64()
                    .ok_or("native_wait_status_absent")? as i32,
                value,
            )
        };
        if !libc::WIFEXITED(status) && !libc::WIFSIGNALED(status) {
            return Err("nonterminal_native_wait".into());
        }
        actor.spawned = true;
        actor.exact_process_identity = Some(identity);
        actor.process_status = Some(if libc::WIFEXITED(status) {
            oulipoly_provider::generated::ProcessStatus::Exited {
                code: libc::WEXITSTATUS(status),
            }
        } else {
            oulipoly_provider::generated::ProcessStatus::SignalTerminated {
                signal: libc::WTERMSIG(status),
            }
        });
        // Attribution precedes effects; the genuine terminal wait accounts for
        // this actor. The enclosing original owner's integrated drain accounts
        // for descendants after adoption, not an invented WCD1 producer receipt.
        actor.leader_reaped = true;
        actor.process_tree_terminated = true;
        actor.operation_finished = true;
        actor.host_cancellation_requested = true;
        actor.uncertain = false;
        evidence.push(serde_json::json!({"intent":path,"terminal":observation}));
        actors.push(actor);
    }
    // Only explicit original dispatch admissions supply never-invoked proof;
    // absence of an actor directory cannot do so.
    for receipt in durable::unadmitted_operations(&journal.join("actors"), lease.owner.attempt_id)?
    {
        if !actors.iter().any(|a| a.operation == receipt.operation) {
            actors.push(receipt);
        }
    }
    if actors.is_empty() {
        return Err("native_actor_journal_empty".into());
    }
    let channel = if journal.join("channel/channel.json").exists() {
        ReturnChannel::recover_original(&journal.join("channel"), &actors, |refs| {
            state.record_promotion(
                &lease.owner,
                lease.owner.attempt_id,
                ProviderLaunchPromotion::ReturnedArtifact,
            )?;
            state.record_returned_artifacts(
                InvocationMutationAuthority::ProviderLaunch(&lease.owner),
                lease.owner.invocation_row_id,
                refs,
            )
        })?
    } else {
        let path = PathBuf::from(
            source["channel_path"]
                .as_str()
                .ok_or("native_channel_intent_absent")?,
        );
        if path.parent().is_some_and(|p| p.exists()) {
            ReturnChannelSettlement::CleanupFailed {
                path,
                artifacts: vec![],
            }
        } else {
            ReturnChannelSettlement::NotCreated
        }
    };
    let runtime_exit_attempts = state
        .native_attempt_custody(generation, invocation)?
        .map(serde_json::from_value::<RetainedAttemptCustody>)
        .transpose()
        .map_err(|e| e.to_string())?
        .map(|original| {
            if original.lease != lease {
                return Err("native_original_runtime_exit_lease_conflict".to_string());
            }
            Ok(original.runtime_exit_attempts)
        })
        .transpose()?
        .unwrap_or_default();
    let retained = RetainedAttemptCustody {
        lease,
        runtime_exit_attempts,
        runtime_exit_operations: super::super::cli::runtime_exit_journal::read(
            &journal.join("runtime-exit"),
            &generation.to_string(),
            &invocation.to_string(),
        )?,
        actors,
        channel,
        retention_failure: None,
        recovery_evidence: Some(
            serde_json::json!({"original_drain":drain,"actor_observations":evidence}),
        ),
    };
    let value = serde_json::to_value(&retained).map_err(|e| e.to_string())?;
    if !complete_actor_receipts(&retained.actors) {
        return Err("native_recovered_actor_custody_unsettled".into());
    }
    state.retain_native_recovered_attempt_custody(&retained.lease.owner, &value)?;
    Ok(value)
}
#[cfg(not(target_os = "linux"))]
fn recover_native_custody(_: &StateDb, _: Uuid, _: Uuid) -> Result<serde_json::Value, String> {
    Err("native_original_owner_recovery_unsupported".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_state::mailbox::RuntimeGenerationId;
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
            cancellation: oulipoly_core::CancellationToken::new(),
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
    fn final_child_marker_survives_promotion_write_failure_and_finish_does_not_replay() {
        let (_dir, mut attempt, context) = fixture();
        let id = Uuid::new_v4();
        let marker = format!("OULIPOLY_INVOCATION={{\"source\":\"fixture\",\"id\":\"{id}\"}}");
        attempt
            .observe(
                &context,
                &DecodedLaunchEvent::Stderr {
                    seq: 1,
                    data: marker.into_bytes(),
                },
            )
            .unwrap();
        assert!(attempt.evidence.lock().unwrap().children.is_empty());
        attempt.allocation.lease.owner.owner_epoch += 1;
        attempt.finish_child_markers();
        attempt.finish_child_markers();
        let evidence = attempt.evidence.lock().unwrap();
        assert!(evidence.stderr_pending.is_empty());
        assert!(evidence.promotions.captured_child && evidence.promotions.persistence_failed);
        assert!(evidence.promotions.transfer_forbidden());
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
    fn runtime_exit_selection_preserves_rejections_pending_and_conflicts() {
        use super::super::super::cli::runtime_exit_journal::{
            ExitDisposition, ExitIntent, ExitObservation, ExitOperation, ExitResult,
        };
        let (_dir, attempt, _) = fixture();
        register_allocated_runtime_generation_starting(&attempt.spawn).unwrap();
        let mut row = runtime_receipt(&attempt.allocation, &[]).row.unwrap();
        row.lifecycle_state = RuntimeLifecycleState::Draining;
        row.drain_request_id = Some(oulipoly_state::mailbox::DrainRequestId::new());
        let mut retained = RetainedAttemptCustody {
            lease: attempt.allocation.lease.clone(),
            actors: vec![],
            channel: ReturnChannelSettlement::NotCreated,
            retention_failure: None,
            runtime_exit_attempts: vec![],
            runtime_exit_operations: vec![],
            recovery_evidence: None,
        };
        assert!(original_runtime_exit_outcome(&retained, &row).is_err());
        let finish = ExitObservation {
            intent: ExitIntent {
                generation: retained.lease.runtime_generation_uuid.to_string(),
                invocation: retained.lease.owner.invocation_uuid.to_string(),
                operation: ExitOperation::FinishDrain,
                reason: "orderly_completion".into(),
                exit_code: Some(7),
                drain_request: row.drain_request_id.as_ref().map(ToString::to_string),
            },
            before: Some(serde_json::to_value(&row).unwrap()),
            result: Some(ExitResult {
                disposition: ExitDisposition::Failed,
                rejected: false,
                custody_refused: false,
                returned: "Err(StorageFailure)".into(),
            }),
        };
        retained.runtime_exit_operations.push(finish.clone());
        let mut rejected = finish.clone();
        rejected.intent.operation = ExitOperation::NonOrderly;
        rejected.intent.reason = "abnormal_termination".into();
        rejected.intent.drain_request = None;
        rejected.intent.exit_code = None;
        rejected.result = Some(ExitResult {
            disposition: ExitDisposition::Rejected,
            rejected: true,
            custody_refused: false,
            returned: "Err(Rejected(IllegalPredecessor))".into(),
        });
        retained.runtime_exit_operations.push(rejected);
        assert_eq!(
            original_runtime_exit_outcome(&retained, &row).unwrap().0,
            RuntimeTerminalReason::OrderlyCompletion
        );
        // Lost result is a pending operation, not rejection erasure or LWW. The
        // abnormal predecessor still fails even without its returned result.
        retained.runtime_exit_operations[1].result = None;
        assert_eq!(
            original_runtime_exit_outcome(&retained, &row).unwrap().1,
            Some(7)
        );
        retained.runtime_exit_operations[0].result = None;
        assert_eq!(
            original_runtime_exit_outcome(&retained, &row).unwrap().1,
            Some(7)
        );
        let mut conflict = finish.clone();
        conflict.intent.exit_code = Some(8);
        retained.runtime_exit_operations.push(conflict);
        assert_eq!(
            original_runtime_exit_outcome(&retained, &row).unwrap_err(),
            "native_original_runtime_operations_conflict"
        );
        retained.runtime_exit_operations = vec![finish.clone()];
        // No retrospective classification of an old generic invariant refusal.
        retained.runtime_exit_operations[0].result = Some(serde_json::from_value(serde_json::json!({
            "disposition":"Rejected", "rejected":true, "returned":"Ok(Rejected(InvariantViolation))"
        })).unwrap());
        assert!(original_runtime_exit_outcome(&retained, &row).is_err());
        // Explicit refusal and missing result expose the same original request,
        // not a completed transition. Real Q admission is tested at its consumer.
        retained.runtime_exit_operations[0]
            .result
            .as_mut()
            .unwrap()
            .custody_refused = true;
        let original_refusal = serde_json::to_value(&retained.runtime_exit_operations).unwrap();
        let retry = original_runtime_exit_outcome(&retained, &row).unwrap();
        assert_eq!(
            serde_json::to_value(&retained.runtime_exit_operations).unwrap(),
            original_refusal
        );
        retained.runtime_exit_operations[0].result = None;
        assert_eq!(
            original_runtime_exit_outcome(&retained, &row).unwrap(),
            retry
        );
        retained.runtime_exit_operations = vec![finish];
        row.drain_request_id = Some(oulipoly_state::mailbox::DrainRequestId::new());
        assert!(original_runtime_exit_outcome(&retained, &row).is_err());
        row.drain_request_id = retained.runtime_exit_operations[0]
            .intent
            .drain_request
            .as_ref()
            .map(|id| oulipoly_state::mailbox::DrainRequestId::parse(id).unwrap());
        retained.runtime_exit_operations[0].result = Some(ExitResult {
            disposition: ExitDisposition::AlreadyExited,
            rejected: false,
            custody_refused: false,
            returned: "Ok(AlreadyExited(original row))".into(),
        });
        assert!(original_runtime_exit_outcome(&retained, &row).is_err());
        // Historical outer helper labels cannot replace absent actual operations.
        retained.runtime_exit_operations.clear();
        retained
            .runtime_exit_attempts
            .push(OriginalRuntimeExitAttempt {
                terminal_code: "orderly_completion".into(),
                exit_code: Some(0),
                site: "classified_dispatch".into(),
                projection_result: "Ok(())".into(),
            });
        assert!(original_runtime_exit_outcome(&retained, &row).is_err());
    }

    #[test]
    fn logical_cancellation_keeps_actual_acceptance_separate_and_exact() {
        let (_dir, attempt, _) = fixture();
        let state = attempt.state.lock().unwrap();
        let lease = &attempt.allocation.lease;
        assert!(logical_cancellation_observation(&state, lease).is_err());
        state.request_cancel(lease.owner.logical_launch_id).unwrap();
        let accepted = logical_cancellation_observation(&state, lease).unwrap();
        assert!(accepted["requested_at"].is_string());
        assert_eq!(
            accepted["logical_launch_id"],
            lease.owner.logical_launch_id.to_string()
        );
        state.request_cancel(lease.owner.logical_launch_id).unwrap();
        assert_eq!(
            logical_cancellation_observation(&state, lease).unwrap(),
            accepted
        );
        let mut wrong = lease.clone();
        wrong.runtime_generation_uuid = Uuid::new_v4();
        assert!(logical_cancellation_observation(&state, &wrong).is_err());
        let mut wrong = lease.clone();
        wrong.owner.invocation_uuid = Uuid::new_v4();
        assert!(logical_cancellation_observation(&state, &wrong).is_err());
        let mut wrong = lease.clone();
        wrong.owner.attempt_id = Uuid::new_v4();
        assert!(logical_cancellation_observation(&state, &wrong).is_err());
    }

    #[test]
    fn recovered_dead_prelaunch_requires_original_drain() {
        let (_dir, attempt, _) = fixture();
        register_allocated_runtime_generation_starting(&attempt.spawn).unwrap();
        for operation in ["describe", "policy.evaluate", "launch"] {
            attempt.actors.record_not_invoked(operation);
        }
        exit_runtime_generation_outcome(
            Some(&attempt.spawn),
            RuntimeTerminalReason::StartupFailed,
            None,
        )
        .unwrap();
        let retained = RetainedAttemptCustody {
            lease: attempt.allocation.lease.clone(),
            actors: attempt.actors.receipts(),
            channel: ReturnChannelSettlement::NotCreated,
            retention_failure: None,
            runtime_exit_attempts: vec![],
            runtime_exit_operations: vec![],
            recovery_evidence: None,
        };
        let mut runtime = runtime_receipt(&attempt.allocation, &retained.actors);
        let row = runtime.row.as_mut().unwrap();
        row.terminal_reason = Some(RuntimeTerminalReason::RecoveredDead);
        assert!(!runtime_row_effect_incapable(
            &retained.lease,
            row,
            &retained.actors
        ));
        runtime.effect_incapable = false;
        let state = attempt.state.lock().unwrap();
        assert_eq!(
            retain_recovered_dead_cancellation(&state, &retained, &runtime).unwrap_err(),
            "native_original_drain_absent"
        );
        runtime.row.as_mut().unwrap().terminal_reason =
            Some(RuntimeTerminalReason::AbnormalTermination);
        assert!(
            retain_recovered_dead_cancellation(&state, &retained, &runtime)
                .unwrap()
                .is_none()
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
