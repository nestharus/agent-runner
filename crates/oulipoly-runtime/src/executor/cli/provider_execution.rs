//! ## Declared roles
//!
//! Roles: orchestration.
//!
//! - orchestration: provider execution sequences launch assembly, supervisor
//!   execution, return-channel cleanup, and raw executor result mapping.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/provider_execution.rs
//!     role: adapter
//!     Translates:
//!       - provider-launch-contract
//!       - provider-supervisor-output-contract
//!       - return-channel-ipc-contract
//!       - raw-execution-result-contract
//! ```

use super::ipc;
use super::launch::{ProviderLaunch, ProviderLaunchRequest, assemble_provider_launch};
use super::result::{RawResult, raw_result_from_supervised_output};
use super::supervision::{SupervisorConfig, run_provider_supervisor};
use oulipoly_config::{PromptMode, ProviderConfig};
use std::path::{Path, PathBuf};

pub(super) fn execute_provider(
    provider: &ProviderConfig,
    prompt_mode: PromptMode,
    prompt: &str,
    working_dir: Option<&Path>,
    input_args: &[String],
    parent_invocation_env: Option<&str>,
    start_known_provider_session_id: Option<&str>,
) -> Result<(RawResult, Vec<PathBuf>), String> {
    execute_provider_with_arg_parts_and_supervisor_config(
        provider,
        &provider.args,
        &[],
        prompt_mode,
        Some(prompt),
        working_dir,
        input_args,
        parent_invocation_env,
        start_known_provider_session_id,
        None,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "executor call shape keeps base args, lifecycle tail args, prompt mode, cwd, input flags, parent marker, optional start-bound session, and optional supervisor test injection explicit"
)]
pub(super) fn execute_provider_with_arg_parts_and_supervisor_config(
    provider: &ProviderConfig,
    provider_args: &[String],
    tail_args: &[String],
    prompt_mode: PromptMode,
    prompt: Option<&str>,
    working_dir: Option<&Path>,
    input_args: &[String],
    parent_invocation_env: Option<&str>,
    start_known_provider_session_id: Option<&str>,
    supervisor_config: Option<SupervisorConfig>,
) -> Result<(RawResult, Vec<PathBuf>), String> {
    let launch = assemble_provider_launch(
        ProviderLaunchRequest {
            provider,
            provider_args,
            tail_args,
            prompt_mode,
            prompt,
            working_dir,
            input_args,
            parent_invocation_env,
            start_known_provider_session_id,
        },
        supervisor_config,
    )?;
    let ProviderLaunch {
        cmd,
        supervisor_config,
        capture_plan,
        return_channel,
        temp_files,
    } = launch;
    let output = run_provider_supervisor(cmd, provider, supervisor_config)?;
    let returned_artifacts = ipc::read_and_cleanup_return_channel(&return_channel);
    let result = raw_result_from_supervised_output(&capture_plan, output, returned_artifacts);

    Ok((result, temp_files))
}
