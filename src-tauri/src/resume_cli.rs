//! Headless resume CLI orchestration helpers.
//!
//! ## Declared roles
//!
//! `orchestration`

use oulipoly_config::ModelConfig;
use oulipoly_runtime::executor;
use std::collections::HashMap;
use std::path::Path;

pub(super) fn resume_result_error_category(
    agent_runtime_services: &super::wiring::AgentRuntimeServices,
    result: &executor::ExecutionResult,
    models: &HashMap<String, ModelConfig>,
    working_dir: Option<&Path>,
) -> Option<String> {
    if super::execution_succeeded(result.exit_code) {
        return None;
    }
    if super::resume_acceptance_adapter::classify(result.resume_acceptance.as_ref())
        == super::resume_acceptance_adapter::ResumeAcceptanceCategory::SessionMismatch
    {
        return Some(super::resume_session_mismatch_category());
    }
    super::diagnose_execution_error(agent_runtime_services, result, models, working_dir)
}
