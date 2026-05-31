//! ## Declared roles
//!
//! `accessor`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/discovery/accessor.rs
//!     role: adapter
//!     Translates:
//!       - runtime discovery invocation contract
//!       - GUI state.db opener contract
//!       - SetupRepository discovery persistence/read contract
//! ```

use super::predicate;
use crate::AppState;
use crate::commands::accessor as command_accessor;
use oulipoly_runtime::discovery;
use oulipoly_state::repositories::{SetupRepository, StateDbOpener};
use oulipoly_state::{DiscoveredModel, ModelParameter, StateDb};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn state_db_path(state: &AppState) -> PathBuf {
    state.db_path()
}

pub fn state_db_opener(state: &AppState) -> Arc<dyn StateDbOpener + Send + Sync> {
    Arc::clone(&state.state_db_opener)
}

pub fn open_state_db_at(
    opener: &Arc<dyn StateDbOpener + Send + Sync>,
    db_path: &Path,
) -> Result<StateDb, String> {
    opener.open_at(db_path)
}

pub fn discover_models_for_cli(cli_name: &str) -> Result<discovery::DiscoveryResult, String> {
    discovery::discover_models(cli_name)
}

pub fn persist_discovery_result(
    repo: &dyn SetupRepository,
    cli_name: &str,
    result: discovery::DiscoveryResult,
) -> Result<Vec<DiscoveredModel>, String> {
    if predicate::has_discovered_models(&result) {
        delete_stale_models(repo, cli_name, &result.cli_version)?;
    }

    for model in &result.models {
        upsert_discovered_model(repo, model)?;
    }

    for (model_name, param) in &result.parameters {
        upsert_model_parameter(repo, model_name, cli_name, param)?;
    }

    Ok(result.models)
}

pub fn delete_stale_models(
    repo: &dyn SetupRepository,
    cli_name: &str,
    cli_version: &str,
) -> Result<u64, String> {
    repo.delete_stale_models(cli_name, cli_version)
}

pub fn upsert_discovered_model(
    repo: &dyn SetupRepository,
    model: &DiscoveredModel,
) -> Result<(), String> {
    repo.upsert_discovered_model(model)
}

pub fn upsert_model_parameter(
    repo: &dyn SetupRepository,
    model_name: &str,
    cli_name: &str,
    param: &ModelParameter,
) -> Result<(), String> {
    repo.upsert_model_parameter(model_name, cli_name, param)
}

pub fn list_discovered_models_inner(
    state: &AppState,
    provider: Option<&str>,
) -> Result<Vec<DiscoveredModel>, String> {
    command_accessor::with_setup_repository(state, |repo| repo.list_discovered_models(provider))
}

pub fn list_model_parameters_inner(
    state: &AppState,
    model_name: &str,
    provider: &str,
) -> Result<Vec<ModelParameter>, String> {
    command_accessor::with_setup_repository(state, |repo| {
        repo.list_model_parameters(model_name, provider)
    })
}
