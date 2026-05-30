//! ## Declared roles
//!
//! Roles: orchestration.
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
use super::result::{cleanup_temp_files, execution_result_from_raw};
use super::resume::{ResumePayload, classify_resume_acceptance, compose_resume_args};
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
    supervisor_config: Option<SupervisorConfig>,
) -> Result<ExecutionResult, String> {
    let session_id = resume.session_id.to_string();
    let resume_args = compose_resume_args(resume.strategy, resume.session_id)?;
    let mut provider_without_capture = provider.clone();
    provider_without_capture.session_capture = None;
    let (result, temp_files) = execute_provider_with_arg_parts_and_supervisor_config(
        &provider_without_capture,
        &provider.args,
        &resume_args,
        prompt_mode,
        prompt,
        working_dir,
        &[],
        parent_invocation_env,
        None,
        supervisor_config,
    )?;
    cleanup_temp_files(temp_files);
    let resume_acceptance = classify_resume_acceptance(
        provider.resume_acceptance.as_ref(),
        result.exit_code,
        &result.telemetry_stdout,
        result.stderr.as_bytes(),
        &session_id,
    );
    Ok(execution_result_from_raw(
        result,
        provider_index,
        Some(resume_acceptance),
        Some(SessionCaptureResult {
            session_id: None,
            method: SessionCaptureMethod::None,
        }),
    ))
}
