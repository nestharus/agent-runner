pub mod cli;

use oulipoly_config::ModelConfig;
use std::collections::HashMap;
use std::path::Path;

#[allow(dead_code)]
pub struct ExecutionResult {
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub exit_code: i32,
    pub provider_index: usize,
    pub session_capture: SessionCaptureResult,
    pub resume_acceptance: Option<ResumeAcceptanceResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeAcceptanceResult {
    pub status: ResumeAcceptanceStatus,
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeAcceptanceStatus {
    Accepted,
    Rejected,
    Unconfirmed,
}

impl ResumeAcceptanceStatus {
    pub fn db_value(self) -> &'static str {
        match self {
            ResumeAcceptanceStatus::Accepted => "accepted",
            ResumeAcceptanceStatus::Rejected => "rejected",
            ResumeAcceptanceStatus::Unconfirmed => "unconfirmed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionCaptureResult {
    pub session_id: Option<String>,
    pub method: SessionCaptureMethod,
}

#[derive(Debug, Clone)]
pub enum SessionCaptureMethod {
    None,
    ForcedFlagVerified,
    StdoutJsonEvent,
    Failed(String),
}

impl SessionCaptureMethod {
    pub fn db_value(&self) -> &'static str {
        match self {
            SessionCaptureMethod::None => "none",
            SessionCaptureMethod::ForcedFlagVerified => "forced_flag_verified",
            SessionCaptureMethod::StdoutJsonEvent => "stdout_json_event",
            SessionCaptureMethod::Failed(_) => "failed",
        }
    }
}

pub use cli::provider_name;

/// Execute a model with the original prompt-only interface (backwards compat).
pub fn execute(
    model: &ModelConfig,
    provider_index: usize,
    prompt: &str,
    working_dir: Option<&Path>,
) -> Result<ExecutionResult, String> {
    cli::execute(
        model,
        provider_index,
        prompt,
        working_dir,
        &HashMap::new(),
        None,
    )
}

/// Execute a model with extra inputs mapped to CLI flags.
pub fn execute_with_inputs(
    model: &ModelConfig,
    provider_index: usize,
    prompt: &str,
    working_dir: Option<&Path>,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> Result<ExecutionResult, String> {
    cli::execute(
        model,
        provider_index,
        prompt,
        working_dir,
        extra_inputs,
        None,
    )
}

/// Execute a model with extra inputs and an explicit parent-invocation env payload.
pub fn execute_with_inputs_and_env(
    model: &ModelConfig,
    provider_index: usize,
    prompt: &str,
    working_dir: Option<&Path>,
    extra_inputs: &HashMap<String, Vec<String>>,
    parent_invocation_env: Option<&str>,
) -> Result<ExecutionResult, String> {
    cli::execute(
        model,
        provider_index,
        prompt,
        working_dir,
        extra_inputs,
        parent_invocation_env,
    )
}

pub fn execute_effective_with_inputs_and_env(
    request: cli::EffectiveExecuteRequest<'_>,
) -> Result<ExecutionResult, String> {
    cli::execute_effective(request)
}
