//! Provider execution, evidence, finalization, and caller-result adapters for
//! the resident session supervisor.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use oulipoly_config::{PromptMode, ProviderConfig, ResumeStrategy};
use oulipoly_state::{
    AcknowledgementWrite, CompositeInvocationId, InvocationStatus, ProviderTurnEffectInput,
    SessionLifecycleRepository, StateDb, TurnFence,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::executor::cli::{self, ResumePayload};
use crate::executor::terminal_signal::TerminalSignalKind;
use crate::executor::{ExecutionResult, ResumeAcceptanceStatus, SubmittedUserTurn};
use crate::services::{ExecutorServicePort, ExecutorServiceRequest, ServiceError};
use crate::session_supervisor::{SupervisorError, TurnRequest};

const MAX_MAILBOX_BATCH_ROWS: usize = 20;

#[derive(Clone, Debug)]
pub struct LegacyCliResumeRequest {
    pub provider: ProviderConfig,
    pub provider_index: usize,
    pub prompt_mode: PromptMode,
    pub prompt: Option<String>,
    pub working_dir: Option<PathBuf>,
    pub parent_invocation_env: Option<String>,
    pub session_id: String,
    pub strategy: ResumeStrategy,
    pub model_name: String,
    pub models_dir: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub enum ProviderTurnExecutionRequest {
    LegacyCliResume(LegacyCliResumeRequest),
    ExternalProvider(ExecutorServiceRequest),
}

impl ProviderTurnExecutionRequest {
    fn target_session_id(&self) -> Option<&str> {
        match self {
            Self::LegacyCliResume(request) => Some(&request.session_id),
            Self::ExternalProvider(
                ExecutorServiceRequest::EffectiveWithStartKnownProviderSessionId {
                    start_known_provider_session_id,
                    ..
                }
                | ExecutorServiceRequest::EffectiveWithCreateKnownProviderSessionId {
                    start_known_provider_session_id,
                    ..
                },
            ) => Some(start_known_provider_session_id),
            Self::ExternalProvider(_) => None,
        }
    }

    fn prompt(&self) -> Option<&str> {
        match self {
            Self::LegacyCliResume(request) => request.prompt.as_deref(),
            Self::ExternalProvider(
                ExecutorServiceRequest::Facade { prompt, .. }
                | ExecutorServiceRequest::Effective { prompt, .. }
                | ExecutorServiceRequest::EffectiveWithStartKnownProviderSessionId { prompt, .. }
                | ExecutorServiceRequest::EffectiveWithCreateKnownProviderSessionId {
                    prompt, ..
                },
            ) => Some(prompt),
        }
    }
}

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
pub enum ProviderExecutionStatus {
    Completed,
    ProviderRejected,
    QuotaExhausted,
    MalformedEvidence,
    AbnormalExit,
    ResumeCompletionUnconfirmed,
    LaunchFailed,
}

impl ProviderExecutionStatus {
    fn success(&self) -> bool {
        matches!(self, Self::Completed)
    }

    fn error_category(&self) -> Option<&'static str> {
        match self {
            Self::Completed => None,
            Self::ProviderRejected => Some("provider_rejected"),
            Self::QuotaExhausted => Some("quota_exhausted"),
            Self::MalformedEvidence => Some("malformed_provider_evidence"),
            Self::AbnormalExit => Some("provider_abnormal_exit"),
            Self::ResumeCompletionUnconfirmed => Some("resume_completion_unconfirmed"),
            Self::LaunchFailed => Some("provider_launch_failed"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderExecutionError {
    LegacyCli(String),
    Service(ServiceError),
}

impl fmt::Display for ProviderExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LegacyCli(message) => formatter.write_str(message),
            Self::Service(error) => write!(formatter, "{error}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProviderExecutionOutcome {
    pub status: ProviderExecutionStatus,
    pub caller_exit_code: i32,
    pub result: Option<ExecutionResult>,
    pub error: Option<ProviderExecutionError>,
}

impl ProviderExecutionOutcome {
    pub fn completed(result: ExecutionResult) -> Self {
        Self::from_result(result)
    }

    pub fn failed(status: ProviderExecutionStatus, error: ProviderExecutionError) -> Self {
        debug_assert!(!status.success());
        Self {
            status,
            caller_exit_code: 1,
            result: None,
            error: Some(error),
        }
    }

    fn from_result(result: ExecutionResult) -> Self {
        let status = classify_execution_result(&result);
        let caller_exit_code = if status.success() {
            result.exit_code
        } else if result.exit_code == 0 {
            1
        } else {
            result.exit_code
        };
        Self {
            status,
            caller_exit_code,
            result: Some(result),
            error: None,
        }
    }
}

pub trait ProviderTurnExecutor {
    fn execute(&self, request: &ProviderTurnExecutionRequest) -> ProviderExecutionOutcome;
}

pub struct ProductionProviderTurnExecutor {
    executor_service: Arc<dyn ExecutorServicePort>,
}

impl ProductionProviderTurnExecutor {
    pub fn new(executor_service: Arc<dyn ExecutorServicePort>) -> Self {
        Self { executor_service }
    }
}

impl ProviderTurnExecutor for ProductionProviderTurnExecutor {
    fn execute(&self, request: &ProviderTurnExecutionRequest) -> ProviderExecutionOutcome {
        match request {
            ProviderTurnExecutionRequest::LegacyCliResume(request) => {
                let result = cli::execute_resume_optional_prompt_with_model_identity(
                    &request.provider,
                    request.provider_index,
                    request.prompt_mode,
                    request.prompt.as_deref(),
                    request.working_dir.as_deref(),
                    request.parent_invocation_env.as_deref(),
                    ResumePayload {
                        session_id: &request.session_id,
                        strategy: &request.strategy,
                    },
                    &request.model_name,
                    request.models_dir.as_deref(),
                );
                match result {
                    Ok(result) => ProviderExecutionOutcome::from_result(result),
                    Err(error) => ProviderExecutionOutcome::failed(
                        ProviderExecutionStatus::LaunchFailed,
                        ProviderExecutionError::LegacyCli(error),
                    ),
                }
            }
            ProviderTurnExecutionRequest::ExternalProvider(request) => {
                match self.executor_service.execute(request.clone()) {
                    Ok(output) => ProviderExecutionOutcome::from_result(output.result),
                    Err(error) => ProviderExecutionOutcome::failed(
                        classify_service_error(&error),
                        ProviderExecutionError::Service(error),
                    ),
                }
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderEvidence {
    ProcessLaunched,
    TransportAccepted,
    SubmittedUserTurn {
        provider_session_id: String,
        prompt_sha256: String,
        delivery_nonce: Option<String>,
        source: Option<String>,
        message_id: Option<String>,
    },
    ResumeAccepted {
        provider_session_id: String,
        evidence: String,
    },
    IngestedUserTurn {
        provider_session_id: String,
        turn_id: String,
    },
    AssistantOutput {
        provider_session_id: String,
    },
    AssistantOutputAbsent,
    IngestedAssistantTurn {
        provider_session_id: String,
        turn_id: String,
    },
    AffirmativeProviderCompletion {
        provider_session_id: String,
        evidence: String,
    },
    TerminalSignal {
        kind: TerminalSignalKind,
        evidence: String,
    },
    ProviderRejected {
        reason: String,
    },
    QuotaExhausted {
        reason: String,
    },
    Malformed {
        reason: String,
    },
    Manual {
        evidence: String,
    },
    ResumeCompletionUnconfirmed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FencedProviderEvidence {
    pub fence: TurnFence,
    pub evidence: ProviderEvidence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvidenceStrength {
    Informational,
    Submitted,
    Confirmed,
}

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
        let (acknowledgement, invocation_finalization) = finalize_invocation_exact(
            state,
            launch,
            &fence,
            execution,
            submitted_evidence.as_deref(),
            confirmed_evidence.as_deref(),
            observed_at,
        )?;
        Ok(ProviderTurnEffectReport {
            acknowledgement,
            invocation_finalization,
        })
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

pub fn classify_provider_evidence(
    expected_fence: &TurnFence,
    expected_session_id: &str,
    expected_prompt_sha256: Option<&str>,
    expected_delivery_nonce: Option<&str>,
    evidence: &FencedProviderEvidence,
) -> Result<EvidenceStrength, ProviderTurnAdapterError> {
    validate_evidence_fence(expected_fence, evidence)?;
    let strength = match &evidence.evidence {
        ProviderEvidence::SubmittedUserTurn {
            provider_session_id,
            prompt_sha256,
            delivery_nonce,
            ..
        } if provider_session_id == expected_session_id
            && submitted_payload_matches(
                prompt_sha256,
                delivery_nonce.as_deref(),
                expected_prompt_sha256,
                expected_delivery_nonce,
            ) =>
        {
            EvidenceStrength::Submitted
        }
        ProviderEvidence::ResumeAccepted {
            provider_session_id,
            evidence,
        } if provider_session_id == expected_session_id && !evidence.trim().is_empty() => {
            EvidenceStrength::Submitted
        }
        ProviderEvidence::IngestedUserTurn {
            provider_session_id,
            turn_id,
        } if provider_session_id == expected_session_id && !turn_id.trim().is_empty() => {
            EvidenceStrength::Submitted
        }
        ProviderEvidence::AssistantOutput {
            provider_session_id,
        } if provider_session_id == expected_session_id => EvidenceStrength::Confirmed,
        ProviderEvidence::IngestedAssistantTurn {
            provider_session_id,
            turn_id,
        } if provider_session_id == expected_session_id && !turn_id.trim().is_empty() => {
            EvidenceStrength::Confirmed
        }
        ProviderEvidence::AffirmativeProviderCompletion {
            provider_session_id,
            evidence,
        } if provider_session_id == expected_session_id && !evidence.trim().is_empty() => {
            EvidenceStrength::Confirmed
        }
        _ => EvidenceStrength::Informational,
    };
    Ok(strength)
}

pub fn prompt_sha256(prompt: &str) -> String {
    let digest = Sha256::digest(prompt.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
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
        || batch.delivery_ids.len() > MAX_MAILBOX_BATCH_ROWS
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

fn submitted_payload_matches(
    prompt_sha256: &str,
    delivery_nonce: Option<&str>,
    expected_prompt_sha256: Option<&str>,
    expected_delivery_nonce: Option<&str>,
) -> bool {
    match (expected_prompt_sha256, expected_delivery_nonce) {
        (Some(expected_prompt), Some(expected_nonce)) => {
            prompt_sha256 == expected_prompt && delivery_nonce == Some(expected_nonce)
        }
        (Some(expected_prompt), None) => prompt_sha256 == expected_prompt,
        (None, Some(expected_nonce)) => delivery_nonce == Some(expected_nonce),
        (None, None) => false,
    }
}

fn validate_evidence_fence(
    expected: &TurnFence,
    evidence: &FencedProviderEvidence,
) -> Result<(), ProviderTurnAdapterError> {
    if evidence.fence != *expected {
        return Err(ProviderTurnAdapterError::InvalidFence("provider evidence"));
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

fn classify_execution_result(result: &ExecutionResult) -> ProviderExecutionStatus {
    if result.terminal_reason.as_deref() == Some("resume_completion_unconfirmed") {
        return ProviderExecutionStatus::ResumeCompletionUnconfirmed;
    }
    if result
        .resume_acceptance
        .as_ref()
        .is_some_and(|acceptance| acceptance.status == ResumeAcceptanceStatus::Rejected)
    {
        return ProviderExecutionStatus::ProviderRejected;
    }
    if result.terminal_signal.as_ref().is_some_and(|signal| {
        matches!(
            signal.kind,
            TerminalSignalKind::QuotaExhaustedInband
                | TerminalSignalKind::MaybeQuotaExhausted
                | TerminalSignalKind::RateLimited
        )
    }) {
        return ProviderExecutionStatus::QuotaExhausted;
    }
    if result.exit_code != 0 {
        return ProviderExecutionStatus::AbnormalExit;
    }
    if result.resume_acceptance.as_ref().is_some_and(|acceptance| {
        acceptance.status == ResumeAcceptanceStatus::Unconfirmed
            && !result.produced_assistant_response
    }) {
        return ProviderExecutionStatus::ResumeCompletionUnconfirmed;
    }
    ProviderExecutionStatus::Completed
}

fn classify_service_error(error: &ServiceError) -> ProviderExecutionStatus {
    let message = error.to_string();
    if message.starts_with("external provider policy rejected launch") {
        ProviderExecutionStatus::ProviderRejected
    } else if message.starts_with("external provider protocol failed:")
        || matches!(error, ServiceError::InvalidRequest { .. })
    {
        ProviderExecutionStatus::MalformedEvidence
    } else {
        ProviderExecutionStatus::LaunchFailed
    }
}

fn evidence_from_execution(
    launch: &ProviderTurnLaunch,
    fence: &TurnFence,
    execution: &ProviderExecutionOutcome,
) -> Vec<FencedProviderEvidence> {
    let mut evidence = Vec::new();
    let Some(result) = execution.result.as_ref() else {
        evidence.push(fenced(fence, failure_evidence(execution)));
        return evidence;
    };
    evidence.push(fenced(fence, ProviderEvidence::ProcessLaunched));
    if let Some(submitted) = result.submitted_user_turn.as_ref() {
        evidence.push(fenced(fence, submitted_user_evidence(submitted)));
    }
    if let Some(acceptance) = result.resume_acceptance.as_ref() {
        let fact = match acceptance.status {
            ResumeAcceptanceStatus::Accepted => ProviderEvidence::ResumeAccepted {
                provider_session_id: launch.mailbox_batch.session_id.clone(),
                evidence: acceptance.evidence.clone().unwrap_or_default(),
            },
            ResumeAcceptanceStatus::Rejected => ProviderEvidence::ProviderRejected {
                reason: acceptance.evidence.clone().unwrap_or_default(),
            },
            ResumeAcceptanceStatus::Unconfirmed => ProviderEvidence::ResumeCompletionUnconfirmed,
        };
        evidence.push(fenced(fence, fact));
    }
    evidence.push(fenced(
        fence,
        if result.produced_assistant_response {
            ProviderEvidence::AssistantOutput {
                provider_session_id: launch.mailbox_batch.session_id.clone(),
            }
        } else {
            ProviderEvidence::AssistantOutputAbsent
        },
    ));
    if let Some(signal) = result.terminal_signal.as_ref() {
        evidence.push(fenced(
            fence,
            ProviderEvidence::TerminalSignal {
                kind: signal.kind,
                evidence: signal.evidence.clone(),
            },
        ));
    }
    if execution.status == ProviderExecutionStatus::ResumeCompletionUnconfirmed {
        evidence.push(fenced(fence, ProviderEvidence::ResumeCompletionUnconfirmed));
    }
    evidence
}

fn fenced(fence: &TurnFence, evidence: ProviderEvidence) -> FencedProviderEvidence {
    FencedProviderEvidence {
        fence: fence.clone(),
        evidence,
    }
}

fn submitted_user_evidence(submitted: &SubmittedUserTurn) -> ProviderEvidence {
    ProviderEvidence::SubmittedUserTurn {
        provider_session_id: submitted.provider_session_id.clone(),
        prompt_sha256: submitted.prompt_sha256.clone(),
        delivery_nonce: submitted.delivery_nonce.clone(),
        source: submitted.source.clone(),
        message_id: submitted.message_id.clone(),
    }
}

fn failure_evidence(execution: &ProviderExecutionOutcome) -> ProviderEvidence {
    let reason = execution
        .error
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("{:?}", execution.status));
    match execution.status {
        ProviderExecutionStatus::ProviderRejected => ProviderEvidence::ProviderRejected { reason },
        ProviderExecutionStatus::QuotaExhausted => ProviderEvidence::QuotaExhausted { reason },
        ProviderExecutionStatus::MalformedEvidence => ProviderEvidence::Malformed { reason },
        ProviderExecutionStatus::ResumeCompletionUnconfirmed => {
            ProviderEvidence::ResumeCompletionUnconfirmed
        }
        ProviderExecutionStatus::Completed
        | ProviderExecutionStatus::AbnormalExit
        | ProviderExecutionStatus::LaunchFailed => ProviderEvidence::TerminalSignal {
            kind: TerminalSignalKind::Unknown,
            evidence: reason,
        },
    }
}

fn acknowledgement_evidence(
    launch: &ProviderTurnLaunch,
    fence: &TurnFence,
    evidence: &[FencedProviderEvidence],
) -> Result<(Option<String>, Option<String>), ProviderTurnAdapterError> {
    if launch.mailbox_batch.delivery_ids.is_empty() {
        return Ok((None, None));
    }
    let prompt_hash = launch.request.prompt().map(prompt_sha256);
    let submitted = select_evidence(
        EvidenceStrength::Submitted,
        fence,
        launch,
        prompt_hash.as_deref(),
        evidence,
    )?;
    let confirmed = select_evidence(
        EvidenceStrength::Confirmed,
        fence,
        launch,
        prompt_hash.as_deref(),
        evidence,
    )?;
    // Confirmation is stronger than submission. Persist the stronger fact at
    // both monotonic stages when the provider exposes no separate submit fact.
    let submission = submitted.as_deref().or(confirmed.as_deref());
    Ok((submission.map(ToOwned::to_owned), confirmed))
}

fn select_evidence(
    strength: EvidenceStrength,
    fence: &TurnFence,
    launch: &ProviderTurnLaunch,
    prompt_hash: Option<&str>,
    evidence: &[FencedProviderEvidence],
) -> Result<Option<String>, ProviderTurnAdapterError> {
    let mut candidates = evidence
        .iter()
        .filter_map(|item| {
            let classified = classify_provider_evidence(
                fence,
                &launch.mailbox_batch.session_id,
                prompt_hash,
                launch.mailbox_batch.delivery_nonce.as_deref(),
                item,
            );
            match classified {
                Ok(actual) if actual == strength => Some(Ok((
                    evidence_rank(&item.evidence),
                    encode_evidence(&item.evidence),
                ))),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    candidates.sort();
    Ok(candidates.into_iter().next().map(|(_, encoded)| encoded))
}

fn evidence_rank(evidence: &ProviderEvidence) -> u8 {
    match evidence {
        ProviderEvidence::SubmittedUserTurn { .. } => 0,
        ProviderEvidence::IngestedUserTurn { .. } => 1,
        ProviderEvidence::ResumeAccepted { .. } => 2,
        ProviderEvidence::IngestedAssistantTurn { .. } => 0,
        ProviderEvidence::AssistantOutput { .. } => 1,
        ProviderEvidence::AffirmativeProviderCompletion { .. } => 2,
        _ => u8::MAX,
    }
}

fn encode_evidence(evidence: &ProviderEvidence) -> String {
    let value = match evidence {
        ProviderEvidence::SubmittedUserTurn {
            provider_session_id,
            prompt_sha256,
            delivery_nonce,
            source,
            message_id,
        } => json!({
            "schema": "oulipoly.provider-turn-evidence/v1",
            "kind": "submitted_user_turn",
            "provider_session_id": provider_session_id,
            "prompt_sha256": prompt_sha256,
            "delivery_nonce": delivery_nonce,
            "source": source,
            "message_id": message_id,
        }),
        ProviderEvidence::ResumeAccepted {
            provider_session_id,
            evidence,
        } => json!({
            "schema": "oulipoly.provider-turn-evidence/v1",
            "kind": "resume_accepted",
            "provider_session_id": provider_session_id,
            "evidence": evidence,
        }),
        ProviderEvidence::IngestedUserTurn {
            provider_session_id,
            turn_id,
        } => turn_evidence("ingested_user_turn", provider_session_id, turn_id),
        ProviderEvidence::AssistantOutput {
            provider_session_id,
        } => json!({
            "schema": "oulipoly.provider-turn-evidence/v1",
            "kind": "assistant_output",
            "provider_session_id": provider_session_id,
        }),
        ProviderEvidence::IngestedAssistantTurn {
            provider_session_id,
            turn_id,
        } => turn_evidence("ingested_assistant_turn", provider_session_id, turn_id),
        ProviderEvidence::AffirmativeProviderCompletion {
            provider_session_id,
            evidence,
        } => json!({
            "schema": "oulipoly.provider-turn-evidence/v1",
            "kind": "affirmative_provider_completion",
            "provider_session_id": provider_session_id,
            "evidence": evidence,
        }),
        _ => json!({
            "schema": "oulipoly.provider-turn-evidence/v1",
            "kind": "informational",
            "evidence": format!("{evidence:?}"),
        }),
    };
    serde_json::to_string(&value).expect("provider evidence JSON serializes")
}

fn turn_evidence(kind: &str, provider_session_id: &str, turn_id: &str) -> Value {
    json!({
        "schema": "oulipoly.provider-turn-evidence/v1",
        "kind": kind,
        "provider_session_id": provider_session_id,
        "turn_id": turn_id,
    })
}

fn finalize_invocation_exact(
    state: &mut StateDb,
    launch: &ProviderTurnLaunch,
    fence: &TurnFence,
    execution: &ProviderExecutionOutcome,
    submitted_evidence: Option<&str>,
    confirmed_evidence: Option<&str>,
    observed_at: i64,
) -> Result<(EffectWrite, EffectWrite), ProviderTurnAdapterError> {
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
        return Ok((acknowledgement, EffectWrite::AlreadyApplied));
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
    Ok((acknowledgement, EffectWrite::Applied))
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
