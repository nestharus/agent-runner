//! Provider process execution and outcome classification for supervised turns.
//!
//! ## Declared roles
//!
//! `mapper`, `orchestration`

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use oulipoly_config::{PromptMode, ProviderConfig, ResumeStrategy};

use crate::executor::cli::{self, ResumePayload};
use crate::executor::terminal_signal::TerminalSignalKind;
use crate::executor::{ExecutionResult, ResumeAcceptanceStatus};
use crate::services::{ExecutorServicePort, ExecutorServiceRequest, ServiceError};

#[derive(Clone)]
pub struct CliResumeRequest {
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

impl fmt::Debug for CliResumeRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CliResumeRequest")
            .field("provider", &self.provider)
            .field("provider_index", &self.provider_index)
            .field("prompt_mode", &self.prompt_mode)
            .field("working_dir", &self.working_dir)
            .field(
                "parent_invocation_env",
                &self.parent_invocation_env.as_ref().map(|_| "[REDACTED]"),
            )
            .field("session_id", &self.session_id)
            .field("strategy", &self.strategy)
            .field("model_name", &self.model_name)
            .field("models_dir", &self.models_dir)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub enum ProviderTurnExecutionRequest {
    CliResume(CliResumeRequest),
    ExternalProvider(ExecutorServiceRequest),
}

impl ProviderTurnExecutionRequest {
    pub(crate) fn target_session_id(&self) -> Option<&str> {
        match self {
            Self::CliResume(request) => Some(&request.session_id),
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

    pub(crate) fn prompt(&self) -> Option<&str> {
        match self {
            Self::CliResume(request) => request.prompt.as_deref(),
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
    pub(crate) fn success(&self) -> bool {
        matches!(self, Self::Completed)
    }

    pub(crate) fn error_category(&self) -> Option<&'static str> {
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
    Cli(String),
    Service(ServiceError),
}

impl fmt::Display for ProviderExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cli(message) => formatter.write_str(message),
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
            ProviderTurnExecutionRequest::CliResume(request) => {
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
                        ProviderExecutionError::Cli(error),
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
