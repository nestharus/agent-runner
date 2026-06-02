//! ## Declared roles
//!
//! `orchestration`, `accessor`, `mapper`, `formatter`
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
//!     Domain: quota_refresh_orchestration
//!     Owns:
//!       - refresh_quotas
//!       - refresh_quotas_inner
//!       - refresh_models
//!       - refresh_candidate_provider_names
//!       - refresh_entry_for_provider
//!       - failed_refresh_entry
//!       - identity::quota_service_external_identity_for_provider
//! ```

use super::{QuotaRefreshEntry, accessor, candidates, identity, mapper};
use crate::AppState;
use oulipoly_config::ModelConfig;
use oulipoly_config::ProvidersConfig;
use oulipoly_runtime::quota::RefreshOutcome;
use oulipoly_state::StateDb;
use std::collections::HashMap;

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
    let models = refresh_models(state)?;
    let candidates = refresh_candidate_provider_names(&models);
    let providers_path = accessor::providers_config_path(&state.models_dir);
    let providers_cfg = accessor::load_providers_config_or_default(state, &providers_path);
    let db_path = accessor::state_db_path(&state.models_dir);
    let db = accessor::open_state_db(state, &db_path)?;

    candidates
        .into_iter()
        .map(|provider_name| {
            refresh_entry_for_provider(state, &models, &providers_cfg, &db, provider_name)
        })
        .collect()
}

fn refresh_models(state: &AppState) -> Result<HashMap<String, ModelConfig>, String> {
    let models = accessor::lock_models(state)?;
    Ok(models.clone())
}

fn refresh_candidate_provider_names(models: &HashMap<String, ModelConfig>) -> Vec<String> {
    candidates::provider_names_for_multi_provider_models(models)
}

fn refresh_entry_for_provider(
    state: &AppState,
    models: &HashMap<String, ModelConfig>,
    providers_cfg: &ProvidersConfig,
    db: &StateDb,
    provider_name: String,
) -> Result<QuotaRefreshEntry, String> {
    if !accessor::provider_is_stale(db, &provider_name) {
        return Ok(mapper::fresh_entry(provider_name));
    }

    let external_identity =
        match identity::quota_service_external_identity_for_provider(models, &provider_name) {
            Ok(identity) => identity,
            Err(message) => return Ok(failed_refresh_entry(provider_name, message)),
        };
    let outcome = accessor::refresh_provider_quota(
        state,
        &provider_name,
        providers_cfg,
        db,
        external_identity,
    )?;
    Ok(mapper::entry_from_refresh_outcome(provider_name, outcome))
}

fn failed_refresh_entry(provider_name: String, message: String) -> QuotaRefreshEntry {
    mapper::entry_from_refresh_outcome(provider_name, RefreshOutcome::Failed(message))
}
