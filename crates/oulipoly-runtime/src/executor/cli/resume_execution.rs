//! ## Declared roles
//!
//! Roles: mapper, orchestration.
//!
//! - orchestration: resume execution sequences resume payload translation,
//!   provider-capture disabling, provider execution, temp cleanup, resume
//!   acceptance classification, and executor result mapping.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/resume_execution.rs
//!     role: adapter
//!     Translates:
//!       - executor-resume-entrypoint-contract
//!       - resume-payload-contract
//!       - provider-execution-internal-contract
//!       - resume-acceptance-contract
//!       - execution-result-contract
//! ```

use super::super::{ExecutionResult, SessionCaptureMethod, SessionCaptureResult};
use super::provider_execution::execute_provider_with_arg_parts_and_supervisor_config;
use super::result::{RawResult, cleanup_temp_files, execution_result_from_raw};
use super::resume::{ResumePayload, classify_resume_acceptance, compose_resume_args};
use super::spawn_identity::{
    SpawnIdentityContext, SpawnRuntimeMode, context_from_parent_invocation_env,
};
use super::supervision::SupervisorConfig;
use oulipoly_config::{PromptMode, ProviderConfig};
use std::path::Path;

pub fn execute_resume(
    provider: &ProviderConfig,
    provider_index: usize,
    prompt_mode: PromptMode,
    prompt: &str,
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    resume: ResumePayload<'_>,
) -> Result<ExecutionResult, String> {
    execute_resume_optional_prompt(
        provider,
        provider_index,
        prompt_mode,
        Some(prompt),
        working_dir,
        parent_invocation_env,
        resume,
    )
}

pub fn execute_resume_optional_prompt(
    provider: &ProviderConfig,
    provider_index: usize,
    prompt_mode: PromptMode,
    prompt: Option<&str>,
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    resume: ResumePayload<'_>,
) -> Result<ExecutionResult, String> {
    execute_resume_with_optional_supervisor_config(
        provider,
        provider_index,
        prompt_mode,
        prompt,
        working_dir,
        parent_invocation_env,
        resume,
        None,
        None,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "resume execution facade keeps provider, prompt, cwd, parent marker, resume payload, and spawn model identity explicit"
)]
pub fn execute_resume_optional_prompt_with_model_identity(
    provider: &ProviderConfig,
    provider_index: usize,
    prompt_mode: PromptMode,
    prompt: Option<&str>,
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    resume: ResumePayload<'_>,
    model_name: &str,
) -> Result<ExecutionResult, String> {
    execute_resume_with_optional_supervisor_config(
        provider,
        provider_index,
        prompt_mode,
        prompt,
        working_dir,
        parent_invocation_env,
        resume,
        Some(model_name),
        None,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "resume execution keeps provider, prompt, cwd, parent marker, resume payload, and optional supervisor test injection explicit"
)]
fn execute_resume_with_optional_supervisor_config(
    provider: &ProviderConfig,
    provider_index: usize,
    prompt_mode: PromptMode,
    prompt: Option<&str>,
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    resume: ResumePayload<'_>,
    model_name: Option<&str>,
    supervisor_config: Option<SupervisorConfig>,
) -> Result<ExecutionResult, String> {
    let input = resume_execution_input(
        provider,
        resume,
        parent_invocation_env,
        model_name,
        working_dir,
    )?;
    let (result, temp_files) = execute_provider_with_arg_parts_and_supervisor_config(
        &input.provider_without_capture,
        &provider.args,
        &input.resume_args,
        prompt_mode,
        prompt,
        working_dir,
        &[],
        parent_invocation_env,
        None,
        input.spawn_identity,
        supervisor_config,
    )?;
    cleanup_temp_files(temp_files);
    Ok(resume_execution_result(
        provider,
        provider_index,
        result,
        &input.session_id,
    ))
}

struct ResumeExecutionInput {
    session_id: String,
    resume_args: Vec<String>,
    provider_without_capture: ProviderConfig,
    spawn_identity: Option<SpawnIdentityContext>,
}

fn resume_execution_input(
    provider: &ProviderConfig,
    resume: ResumePayload<'_>,
    parent_invocation_env: Option<&str>,
    model_name: Option<&str>,
    working_dir: Option<&Path>,
) -> Result<ResumeExecutionInput, String> {
    let session_id = resume.session_id.to_string();
    let provider_without_capture = provider_without_capture(provider);
    Ok(ResumeExecutionInput {
        resume_args: compose_resume_args(resume.strategy, resume.session_id)?,
        spawn_identity: resume_spawn_identity(
            parent_invocation_env,
            &provider_without_capture,
            model_name,
            &session_id,
            working_dir,
        ),
        provider_without_capture,
        session_id,
    })
}

fn provider_without_capture(provider: &ProviderConfig) -> ProviderConfig {
    let mut provider = provider.clone();
    provider.session_capture = None;
    provider
}

fn resume_spawn_identity(
    parent_invocation_env: Option<&str>,
    provider: &ProviderConfig,
    model_name: Option<&str>,
    session_id: &str,
    working_dir: Option<&Path>,
) -> Option<SpawnIdentityContext> {
    context_from_parent_invocation_env(
        parent_invocation_env,
        &provider.name,
        model_name,
        Some(session_id),
        SpawnRuntimeMode::Headless,
        working_dir,
    )
}

fn resume_execution_result(
    provider: &ProviderConfig,
    provider_index: usize,
    result: RawResult,
    session_id: &str,
) -> ExecutionResult {
    let resume_acceptance = classify_resume_acceptance(
        provider.resume_acceptance.as_ref(),
        result.exit_code,
        &result.telemetry_stdout,
        result.stderr.as_bytes(),
        session_id,
    );
    execution_result_from_raw(
        result,
        provider_index,
        Some(resume_acceptance),
        Some(no_session_capture_result()),
    )
}

fn no_session_capture_result() -> SessionCaptureResult {
    SessionCaptureResult {
        session_id: None,
        method: SessionCaptureMethod::None,
    }
}
