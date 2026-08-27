//! Coordinates supervised provider execution, evidence interpretation, durable
//! effect application, and caller completion through their owning boundaries.
//!
//! ## Declared roles
//!
//! `orchestration`, `validator`
//!
//! ## Lifecycle relationship
//!
//! This is the target `SessionSupervisor` adapter for AGE-278's joined cutover,
//! not a second active production resume path. Until that cutover,
//! `src-tauri/src/run/resume` remains authoritative and compatibility changes
//! must assess both paths. The activation conditions, permitted domain
//! differences, and retirement criteria are owned by
//! `docs/architecture/provider-turn-lifecycle.md`.

use std::collections::{HashMap, HashSet};
use std::fmt;

use oulipoly_state::{CompositeInvocationId, StateDb, TurnFence};

use crate::provider_turn_contract::MAILBOX_BATCH_MAX_ROWS;
use crate::provider_turn_effect_application::apply_provider_turn_effects_exact;
use crate::provider_turn_evidence::{
    acknowledgement_evidence, evidence_from_execution, validate_evidence_fence,
};
use crate::session_supervisor::{SupervisorError, TurnRequest};

pub use crate::provider_turn_effect_application::{EffectWrite, ProviderTurnEffectReport};
pub use crate::provider_turn_evidence::{
    EvidenceStrength, FencedProviderEvidence, ProviderEvidence, classify_provider_evidence,
    prompt_sha256,
};
pub use crate::provider_turn_execution::{
    CliResumeRequest, ProductionProviderTurnExecutor, ProviderExecutionError,
    ProviderExecutionOutcome, ProviderExecutionStatus, ProviderTurnExecutionRequest,
    ProviderTurnExecutor,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvocationOwnership {
    pub invocation_row_id: i64,
    pub invocation: CompositeInvocationId,
    pub parent_invocation_id: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MailboxBatchIdentity {
    pub session_id: String,
    pub delivery_ids: Vec<String>,
    pub sequences: Vec<i64>,
    pub delivery_nonce: Option<String>,
}

impl MailboxBatchIdentity {
    pub fn empty(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            delivery_ids: Vec::new(),
            sequences: Vec::new(),
            delivery_nonce: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProviderTurnLaunch {
    pub launch_id: String,
    pub request: ProviderTurnExecutionRequest,
    pub invocation: InvocationOwnership,
    pub mailbox_batch: MailboxBatchIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderTurnEffects {
    Ready(ProviderTurnEffectReport),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProviderTurnCallerResult {
    pub launch_id: String,
    pub invocation: InvocationOwnership,
    pub execution: ProviderExecutionOutcome,
    pub effects: ProviderTurnEffects,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProviderLaunchEffect {
    pub write: EffectWrite,
    pub outcome: ProviderExecutionOutcome,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ProviderTurnAdapterError {
    InvalidFence(&'static str),
    ConflictingReplay,
    State(String),
    Supervisor(String),
}

impl fmt::Display for ProviderTurnAdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFence(field) => write!(formatter, "invalid provider turn fence: {field}"),
            Self::ConflictingReplay => formatter.write_str("conflicting provider turn replay"),
            Self::State(error) => write!(formatter, "provider turn state effect: {error}"),
            Self::Supervisor(error) => write!(formatter, "provider turn completion: {error}"),
        }
    }
}

impl std::error::Error for ProviderTurnAdapterError {}

impl From<SupervisorError> for ProviderTurnAdapterError {
    fn from(value: SupervisorError) -> Self {
        Self::Supervisor(value.to_string())
    }
}

struct CachedExecution {
    launch_id: String,
    outcome: ProviderExecutionOutcome,
}

pub struct ProviderTurnAdapter<Executor> {
    executor: Executor,
    executions: HashMap<TurnFence, CachedExecution>,
}

impl<Executor> ProviderTurnAdapter<Executor>
where
    Executor: ProviderTurnExecutor,
{
    pub fn new(executor: Executor) -> Self {
        Self {
            executor,
            executions: HashMap::new(),
        }
    }

    pub fn execute_once(
        &mut self,
        request: &TurnRequest<ProviderTurnLaunch, ProviderTurnCallerResult>,
    ) -> Result<ProviderLaunchEffect, ProviderTurnAdapterError> {
        validate_turn_request(request)?;
        let fence = turn_fence(request);
        if let Some(cached) = self.executions.get(&fence) {
            if cached.launch_id != request.notification.input.launch_id {
                return Err(ProviderTurnAdapterError::ConflictingReplay);
            }
            return Ok(ProviderLaunchEffect {
                write: EffectWrite::AlreadyApplied,
                outcome: cached.outcome.clone(),
            });
        }
        let outcome = self.executor.execute(&request.notification.input.request);
        self.executions.insert(
            fence,
            CachedExecution {
                launch_id: request.notification.input.launch_id.clone(),
                outcome: outcome.clone(),
            },
        );
        Ok(ProviderLaunchEffect {
            write: EffectWrite::Applied,
            outcome,
        })
    }

    pub fn apply_effects(
        &self,
        request: &TurnRequest<ProviderTurnLaunch, ProviderTurnCallerResult>,
        state: &mut StateDb,
        execution: &ProviderExecutionOutcome,
        observed_evidence: &[FencedProviderEvidence],
        observed_at: i64,
    ) -> Result<ProviderTurnEffectReport, ProviderTurnAdapterError> {
        validate_turn_request(request)?;
        let launch = &request.notification.input;
        let fence = turn_fence(request);
        let mut evidence = evidence_from_execution(launch, &fence, execution);
        evidence.extend_from_slice(observed_evidence);
        for item in &evidence {
            validate_evidence_fence(&fence, item)?;
        }
        let (submitted_evidence, confirmed_evidence) =
            acknowledgement_evidence(launch, &fence, &evidence)?;
        apply_provider_turn_effects_exact(
            state,
            launch,
            &fence,
            execution,
            submitted_evidence.as_deref(),
            confirmed_evidence.as_deref(),
            observed_at,
        )
    }

    pub fn consume(
        &mut self,
        request: TurnRequest<ProviderTurnLaunch, ProviderTurnCallerResult>,
        state: &mut StateDb,
        completed_at: i64,
    ) -> Result<(), ProviderTurnAdapterError> {
        let fence = turn_fence(&request);
        let execution = self.execute_once(&request)?.outcome;
        let effects = match self.apply_effects(&request, state, &execution, &[], completed_at) {
            Ok(report) => ProviderTurnEffects::Ready(report),
            Err(error) => ProviderTurnEffects::Failed(error.to_string()),
        };
        let result = ProviderTurnCallerResult {
            launch_id: request.notification.input.launch_id.clone(),
            invocation: request.notification.input.invocation.clone(),
            execution,
            effects,
        };
        request.completion.complete(result, completed_at)?;
        self.executions.remove(&fence);
        Ok(())
    }
}

fn validate_turn_request(
    request: &TurnRequest<ProviderTurnLaunch, ProviderTurnCallerResult>,
) -> Result<(), ProviderTurnAdapterError> {
    let launch = &request.notification.input;
    if launch.launch_id.trim().is_empty() {
        return Err(ProviderTurnAdapterError::InvalidFence("launch_id"));
    }
    if request.turn.session_id.as_deref() != Some(request.session_id.as_str()) {
        return Err(ProviderTurnAdapterError::InvalidFence("turn.session_id"));
    }
    let Some(target_session_id) = launch.request.target_session_id() else {
        return Err(ProviderTurnAdapterError::InvalidFence(
            "execution session unavailable",
        ));
    };
    if target_session_id != request.session_id {
        return Err(ProviderTurnAdapterError::InvalidFence("execution session"));
    }
    if launch.invocation.invocation.id != request.turn.spawn_invocation_id {
        return Err(ProviderTurnAdapterError::InvalidFence("spawn invocation"));
    }
    validate_mailbox_batch(&request.session_id, &launch.mailbox_batch)
}

fn validate_mailbox_batch(
    session_id: &str,
    batch: &MailboxBatchIdentity,
) -> Result<(), ProviderTurnAdapterError> {
    if batch.session_id != session_id {
        return Err(ProviderTurnAdapterError::InvalidFence("mailbox session"));
    }
    if batch.delivery_ids.len() != batch.sequences.len()
        || batch.delivery_ids.len() > MAILBOX_BATCH_MAX_ROWS
    {
        return Err(ProviderTurnAdapterError::InvalidFence(
            "mailbox batch bounds",
        ));
    }
    if batch.delivery_ids.is_empty() {
        if batch.delivery_nonce.is_some() {
            return Err(ProviderTurnAdapterError::InvalidFence("mailbox nonce"));
        }
        return Ok(());
    }
    if batch.delivery_ids.iter().any(|id| id.trim().is_empty())
        || batch.delivery_nonce.as_deref().is_none_or(str::is_empty)
    {
        return Err(ProviderTurnAdapterError::InvalidFence("mailbox identity"));
    }
    let unique_ids = batch.delivery_ids.iter().collect::<HashSet<_>>();
    if unique_ids.len() != batch.delivery_ids.len()
        || batch.sequences.iter().any(|sequence| *sequence <= 0)
        || batch
            .sequences
            .windows(2)
            .any(|window| window[0] >= window[1])
    {
        return Err(ProviderTurnAdapterError::InvalidFence("mailbox ordering"));
    }
    Ok(())
}

fn turn_fence(request: &TurnRequest<ProviderTurnLaunch, ProviderTurnCallerResult>) -> TurnFence {
    TurnFence {
        session_id: request.session_id.clone(),
        generation_id: request.turn.generation_id.clone(),
        spawn_invocation_id: request.turn.spawn_invocation_id.clone(),
    }
}
