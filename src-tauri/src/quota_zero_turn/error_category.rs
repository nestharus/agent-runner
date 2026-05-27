//! Quota and execution-result error-category decisioning helpers.
//!
//! Relocated from `src-tauri/src/main.rs` by AGE-204 (map row H13). Output-preserving.
//!
//! ## Declared roles
//!
//! `mapper`, `predicate`, `formatter`
//!
//! - `mapper`: `resume_result_error_category` / `balanced_result_error_category` delegate to
//!   the resume/balanced CLI diagnostics dispatchers; `quota_exhausted_category` maps to the
//!   typed `diagnostics::ErrorCategory::QuotaExhausted` string.
//! - `predicate`: `error_category_is_quota_exhausted`.
//! - `formatter`: `format_quota_retry_budget_exhausted`.

use std::collections::HashMap;
use std::path::Path;

use oulipoly_config::ModelConfig;
use oulipoly_runtime::{diagnostics, executor};

use crate::{resume_cli, run, wiring};

pub(crate) fn resume_result_error_category(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    result: &executor::ExecutionResult,
    models: &HashMap<String, ModelConfig>,
    working_dir: Option<&Path>,
) -> Option<String> {
    resume_cli::resume_result_error_category(agent_runtime_services, result, models, working_dir)
}

pub(crate) fn balanced_result_error_category(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    result: &executor::ExecutionResult,
    models: &HashMap<String, ModelConfig>,
    working_dir: Option<&Path>,
) -> Option<String> {
    run::balancing::balanced_result_error_category(
        agent_runtime_services,
        result,
        models,
        working_dir,
    )
}

pub(crate) fn quota_exhausted_category() -> String {
    diagnostics::ErrorCategory::QuotaExhausted
        .as_str()
        .to_string()
}

pub(crate) fn error_category_is_quota_exhausted(error_category: Option<&str>) -> bool {
    error_category == Some(diagnostics::ErrorCategory::QuotaExhausted.as_str())
}

pub(crate) fn format_quota_retry_budget_exhausted(model_name: &str, max_attempts: usize) -> String {
    format!(
        "quota-exhausted retry budget exhausted for pool {model_name} after {max_attempts} attempts"
    )
}
