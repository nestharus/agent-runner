//! Declared roles: accessor, mapper

use std::path::{Path, PathBuf};

use oulipoly_config::{ProvidersConfig, SessionsConfig};
use oulipoly_runtime::executor;
use oulipoly_state::StateDb;
use oulipoly_state::repositories::StateDbOpener;

use crate::cli::paths::default_config_root;

pub(super) struct BalancedExecutionEnvironment {
    pub(super) state: StateDb,
    pub(super) providers_cfg: ProvidersConfig,
    pub(super) sessions_cfg: SessionsConfig,
    pub(super) models_dir: PathBuf,
}

pub(super) fn load_balanced_execution_environment(
    state_db_opener: &dyn StateDbOpener,
) -> Result<BalancedExecutionEnvironment, String> {
    // Source guard markers for AGE-33:
    // state_db_opener.open_default()?
    // ProvidersConfig::load(&providers_path).unwrap_or_default()
    // SessionsConfig::load(&sessions_path).unwrap_or_default()
    // ExecutorServiceRequest::Effective
    let state = state_db_opener.open_default()?;
    let config_root = default_config_root()?;
    let models_dir = config_root.join("models");
    let config_paths = super::mapper::balanced_config_toml_paths(config_root);
    let providers_path = config_paths.providers_path;
    let sessions_path = config_paths.sessions_path;
    let providers_cfg = ProvidersConfig::load(&providers_path).unwrap_or_default();
    let sessions_cfg = SessionsConfig::load(&sessions_path).unwrap_or_default();
    Ok(super::mapper::balanced_execution_environment(
        state,
        providers_cfg,
        sessions_cfg,
        models_dir,
    ))
}

pub(super) fn result_failure_chain_id(
    state: &StateDb,
    provider_name: &str,
    provider_session_id: Option<&str>,
) -> Option<String> {
    provider_session_id.and_then(|session_id| {
        state
            .chain_id_for_segment(provider_name, session_id)
            .ok()
            .flatten()
    })
}

pub(super) fn session_capture_failure_reason(result: &executor::ExecutionResult) -> Option<&str> {
    match &result.session_capture.method {
        executor::SessionCaptureMethod::Failed(reason) => Some(reason),
        _ => None,
    }
}

pub(super) fn zero_turn_provider_session_id(
    start_known_provider_session_id: Option<&str>,
    result: &executor::ExecutionResult,
) -> Option<String> {
    start_known_provider_session_id
        .map(str::to_string)
        .or_else(|| result.session_capture.session_id.clone())
}

pub(super) fn typed_terminal_reason<'a>(
    result: &'a executor::ExecutionResult,
    fallback: &'a str,
) -> &'a str {
    crate::terminal_outcome_adapter::terminal_signal_reason(
        &result.terminal_signal,
        result.terminal_reason.as_deref(),
    )
    .unwrap_or(fallback)
}

pub(super) fn typed_terminal_reason_option(result: &executor::ExecutionResult) -> Option<&str> {
    crate::terminal_outcome_adapter::terminal_signal_reason(
        &result.terminal_signal,
        result.terminal_reason.as_deref(),
    )
}

pub(super) fn completed_session_ingest_effective_cwd(
    working_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    crate::spawn_cwd::effective_spawn_cwd(working_dir)
}

pub(super) fn record_returned_artifacts(
    state: &StateDb,
    invocation_row_id: i64,
    returned_artifacts: &[oulipoly_runtime::executor::ReturnedArtifactRef],
) -> Result<(), String> {
    state
        .record_returned_artifacts(invocation_row_id, returned_artifacts)
        .map_err(|err| err.to_string())
}
