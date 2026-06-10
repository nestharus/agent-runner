//! ## Declared roles
//!
//! Roles: orchestration.
//!
//! - orchestration: public headless execution entrypoints sequence provider
//!   lookup, input-flag resolution, provider execution, temp cleanup, and
//!   executor result mapping.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/headless.rs
//!     role: adapter
//!     Translates:
//!       - executor-public-entrypoint-contract
//!       - effective-execute-request-contract
//!       - provider-execution-internal-contract
//!       - execution-result-contract
//! ```

use super::super::ExecutionResult;
use super::input_flags::resolve_input_flags;
use super::provider_execution::{
    execute_provider, execute_provider_with_arg_parts_and_supervisor_config,
};
use super::provider_lookup::provider_for_index;
use super::request::EffectiveExecuteRequest;
use super::result::{cleanup_temp_files, execution_result_from_raw};
use super::spawn_identity::{SpawnRuntimeMode, context_from_parent_invocation_env};
use super::supervision::SupervisorConfig;
use oulipoly_config::ModelConfig;
use std::collections::HashMap;
use std::path::Path;

pub fn execute(
    model: &ModelConfig,
    provider_index: usize,
    prompt: &str,
    working_dir: Option<&Path>,
    extra_inputs: &HashMap<String, Vec<String>>,
    parent_invocation_env: Option<&str>,
) -> Result<ExecutionResult, String> {
    let provider = provider_for_index(model, provider_index)?;
    let input_args = resolve_input_flags(model, extra_inputs)?;
    let (result, temp_files) = execute_provider(
        provider,
        model.prompt_mode,
        prompt,
        working_dir,
        &input_args,
        parent_invocation_env,
        None,
        context_from_parent_invocation_env(
            parent_invocation_env,
            &provider.name,
            Some(&model.name),
            None,
            SpawnRuntimeMode::Headless,
            working_dir,
            None,
        ),
    )?;
    cleanup_temp_files(temp_files);

    Ok(execution_result_from_raw(
        result,
        provider_index,
        None,
        None,
    ))
}

pub fn execute_effective(request: EffectiveExecuteRequest<'_>) -> Result<ExecutionResult, String> {
    execute_effective_with_start_known_provider_session_id(request, None)
}

pub fn execute_effective_with_start_known_provider_session_id(
    request: EffectiveExecuteRequest<'_>,
    start_known_provider_session_id: Option<&str>,
) -> Result<ExecutionResult, String> {
    execute_effective_with_optional_supervisor_config(
        request,
        start_known_provider_session_id,
        None,
    )
}

#[cfg(test)]
pub(super) fn execute_effective_with_supervisor_config(
    request: EffectiveExecuteRequest<'_>,
    start_known_provider_session_id: Option<&str>,
    supervisor_config: SupervisorConfig,
) -> Result<ExecutionResult, String> {
    execute_effective_with_optional_supervisor_config(
        request,
        start_known_provider_session_id,
        Some(supervisor_config),
    )
}

fn execute_effective_with_optional_supervisor_config(
    request: EffectiveExecuteRequest<'_>,
    start_known_provider_session_id: Option<&str>,
    supervisor_config: Option<SupervisorConfig>,
) -> Result<ExecutionResult, String> {
    let input_args = resolve_input_flags(request.model, request.extra_inputs)?;
    let (result, temp_files) = execute_provider_with_arg_parts_and_supervisor_config(
        request.provider,
        &request.provider.args,
        &[],
        request.prompt_mode,
        Some(request.prompt),
        request.working_dir,
        &input_args,
        request.parent_invocation_env,
        start_known_provider_session_id,
        context_from_parent_invocation_env(
            request.parent_invocation_env,
            &request.provider.name,
            Some(&request.model.name),
            start_known_provider_session_id,
            SpawnRuntimeMode::Headless,
            request.working_dir,
            request.models_dir,
        ),
        supervisor_config,
    )?;
    cleanup_temp_files(temp_files);

    Ok(execution_result_from_raw(
        result,
        request.provider_index,
        None,
        None,
    ))
}
