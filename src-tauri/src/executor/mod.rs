pub mod cli;

use crate::config::ModelConfig;
use std::collections::HashMap;
use std::path::Path;

#[allow(dead_code)]
pub struct ExecutionResult {
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub exit_code: i32,
    pub provider_index: usize,
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
