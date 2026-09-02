//! ## Declared roles
//!
//! `orchestration`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/discovery/orchestration.rs
//!     role: adapter
//!     Translates:
//!       - Tauri IPC discovery command contract
//!       - runtime discovery blocking task contract
//!       - discovery persistence ordering contract
//!       - discovery read filter contract
//! ```
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/src/commands/discovery/orchestration.rs
//!     role: intrinsic-surface
//!     Domain: discovery persistence lifecycle: runtime discovery, GUI DB open, stale delete, model upsert, and parameter upsert are one ordered IPC lifecycle.
//!     Owns:
//!       - src-tauri/src/commands/discovery/accessor.rs
//!       - src-tauri/src/commands/discovery/predicate.rs
//!       - src-tauri/src/commands/discovery/formatter.rs
//!       - src-tauri/src/commands/accessor.rs
//!   - component: src-tauri/src/commands/discovery/orchestration.rs
//!     role: intrinsic-surface
//!     Domain: discovery read lifecycle: provider/model filters and SetupRepository reads are one IPC lifecycle.
//!     Owns:
//!       - src-tauri/src/commands/discovery/accessor.rs
//!       - src-tauri/src/commands/accessor.rs
//! ```

use super::{accessor, formatter};
use crate::AppState;
use oulipoly_runtime::discovery;
use oulipoly_state::repositories::SetupRepository;
use oulipoly_state::{DiscoveredModel, ModelParameter};

#[tauri::command]
pub(crate) async fn discover_models_cmd(
    state: tauri::State<'_, AppState>,
    cli_name: String,
) -> Result<Vec<DiscoveredModel>, String> {
    let db_path = accessor::state_db_path(&state);
    let state_db_opener = accessor::state_db_opener(&state);
    let provider_registry = state.provider_registry.current();

    tauri::async_runtime::spawn_blocking(move || {
        let result = accessor::discover_models_for_family(&provider_registry, &cli_name)?;
        let db = accessor::open_state_db_at(&state_db_opener, &db_path)?;
        accessor::persist_discovery_result(&db, &cli_name, result)
    })
    .await
    .map_err(formatter::discovery_join_error)?
}

pub fn persist_discovery_result(
    repo: &dyn SetupRepository,
    cli_name: &str,
    result: discovery::DiscoveryResult,
) -> Result<Vec<DiscoveredModel>, String> {
    accessor::persist_discovery_result(repo, cli_name, result)
}

#[tauri::command]
pub(crate) fn list_discovered_models(
    state: tauri::State<AppState>,
    provider: Option<String>,
) -> Result<Vec<DiscoveredModel>, String> {
    list_discovered_models_inner(&state, provider)
}

pub fn list_discovered_models_inner(
    state: &AppState,
    provider: Option<String>,
) -> Result<Vec<DiscoveredModel>, String> {
    accessor::list_discovered_models_inner(state, provider.as_deref())
}

#[tauri::command]
pub(crate) fn get_model_parameters(
    state: tauri::State<AppState>,
    model_name: String,
    provider: String,
) -> Result<Vec<ModelParameter>, String> {
    get_model_parameters_inner(&state, model_name, provider)
}

pub fn get_model_parameters_inner(
    state: &AppState,
    model_name: String,
    provider: String,
) -> Result<Vec<ModelParameter>, String> {
    accessor::list_model_parameters_inner(state, &model_name, &provider)
}
