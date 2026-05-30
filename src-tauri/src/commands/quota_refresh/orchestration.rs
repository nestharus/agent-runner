//! ## Declared roles
//!
//! `orchestration`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/quota_refresh/orchestration.rs
//!     role: adapter
//!     Translates:
//!       - Tauri IPC quota-refresh command contract
//!       - AppState quota-refresh command contract
//!       - Result<String> error projection contract
//!       - runtime quota service orchestration contract
//! ```
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/src/commands/quota_refresh/orchestration.rs
//!     role: intrinsic-surface
//!     Domain: quota-refresh lifecycle: candidate discovery, providers.toml loading, state DB opening, stale gating, runtime quota service refresh, and DTO mapping are one output-preserving command lifecycle.
//!     Owns:
//!       - src-tauri/src/commands/quota_refresh/candidates.rs
//!       - src-tauri/src/commands/quota_refresh/accessor.rs
//!       - src-tauri/src/commands/quota_refresh/mapper.rs
//! ```

use super::{QuotaRefreshEntry, accessor, candidates, mapper};
use crate::AppState;
use oulipoly_config::ProvidersConfig;
use oulipoly_state::StateDb;

/// Refresh quotas for every distinct provider that participates in at least
/// one multi-provider model. Single-provider models are skipped since there's
/// no load-balancing decision to inform.
#[tauri::command]
pub(crate) async fn refresh_quotas(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<QuotaRefreshEntry>, String> {
    refresh_quotas_inner(&state)
}

pub fn refresh_quotas_inner(state: &AppState) -> Result<Vec<QuotaRefreshEntry>, String> {
    let candidates = refresh_candidate_provider_names(state)?;
    let providers_path = accessor::providers_config_path(&state.models_dir);
    let providers_cfg = accessor::load_providers_config_or_default(state, &providers_path);
    let db_path = accessor::state_db_path(&state.models_dir);
    let db = accessor::open_state_db(state, &db_path)?;

    candidates
        .into_iter()
        .map(|provider_name| refresh_entry_for_provider(state, &providers_cfg, &db, provider_name))
        .collect()
}

fn refresh_candidate_provider_names(state: &AppState) -> Result<Vec<String>, String> {
    let models = accessor::lock_models(state)?;
    Ok(candidates::provider_names_for_multi_provider_models(
        &models,
    ))
}

fn refresh_entry_for_provider(
    state: &AppState,
    providers_cfg: &ProvidersConfig,
    db: &StateDb,
    provider_name: String,
) -> Result<QuotaRefreshEntry, String> {
    if !accessor::provider_is_stale(db, &provider_name) {
        return Ok(mapper::fresh_entry(provider_name));
    }

    let outcome = accessor::refresh_provider_quota(state, &provider_name, providers_cfg, db)?;
    Ok(mapper::entry_from_refresh_outcome(provider_name, outcome))
}
