//! Migration provider resolution helpers and resume-execution environment loader.
//!
//! Relocated from `src-tauri/src/main.rs` by AGE-203 (slice B8 of the AGE-183
//! main.rs decomposition program; map row H12). Output-preserving: bodies
//! byte-identical to the pre-AGE-203 main.rs definitions; only visibility +
//! `crate::` import targets change.
//!
//! ## Declared roles
//!
//! `orchestration`, `accessor`, `mapper`, `formatter`
//!
//! - `orchestration`: `load_resume_execution_environment` — loads the four-piece
//!   resume environment (state db + providers config + models map + sessions config).
//! - `accessor`: `provider_session_resolved_account` — reads the provider's
//!   `session_storage` and returns the resolved account-path display string;
//!   `legacy_invocation_provider_names` — reads the installed models config.
//! - `mapper`: the `SessionStorage` → `Option<String>` match inside
//!   `provider_session_resolved_account`; the models → `(model, provider_index)
//!   -> provider_name` projection in `legacy_invocation_provider_names`.
//! - `formatter`: the stderr degradation warning emitted by
//!   `legacy_invocation_provider_names` when the models config cannot be loaded.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/migration_providers.rs
//!     role: adapter
//!     Translates:
//!       - AGE-203 H12: ProviderConfig session_storage -> resolved-account display string
//!       - AGE-203 H12: models_dir_override + config root -> ResumeExecutionEnvironment
//!       - PP-001: installed models config -> StateDb LegacyProviderNames lookup pushed
//!         into the legacy invocation-row migration (StateDb no longer discovers it)
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use oulipoly_config::{ModelConfig, ProviderConfig, ProvidersConfig, SessionStorage, load_models};
use oulipoly_runtime::session_metadata::resolve_workspace_root_for_provider_session;
use oulipoly_state::{LegacyProviderNames, StateDb};

use crate::cli::paths::{default_config_root, default_models_dir};

/// Resolve the `(model_name, provider_index) -> provider_name` lookup the
/// legacy invocation-row migration needs, from the app's installed models
/// config. PP-001 inversion: the app (which owns its config layout) computes
/// this and pushes it into `StateDb::open_with_legacy_provider_names`, rather
/// than StateDb discovering and parsing the config during DB open. A missing or
/// corrupt models config is non-fatal: warn and return an empty lookup so
/// unmappable rows degrade to `status='legacy'` (per V10 — observable, not
/// silent).
pub(crate) fn legacy_invocation_provider_names() -> LegacyProviderNames {
    // Provider-unaware load (matching the pre-PP-001 in-StateDb behavior): the
    // legacy lookup only needs each model's provider names, and the migrate path
    // must not surface providers.toml root-arg overlap diagnostics.
    match load_models(&default_models_dir(), None) {
        Ok(models) => provider_names_from_models(models),
        Err(error) => {
            warn_legacy_provider_lookup_unavailable(&error.to_string());
            LegacyProviderNames::new()
        }
    }
}

fn provider_names_from_models(models: HashMap<String, ModelConfig>) -> LegacyProviderNames {
    let mut lookup = LegacyProviderNames::new();
    for (model_name, model) in models {
        for (provider_index, provider) in model.providers.iter().enumerate() {
            lookup.insert((model_name.clone(), provider_index), provider.name.clone());
        }
    }
    lookup
}

fn warn_legacy_provider_lookup_unavailable(error: &str) {
    eprintln!(
        "Warning: failed to load models config for legacy invocation migration ({error}); \
         pre-existing invocation rows will migrate as status='legacy'."
    );
}

pub(crate) fn provider_session_resolved_account(
    provider: &ProviderConfig,
    provider_session_id: &str,
) -> Option<String> {
    resolved_account_from_session_storage(
        provider_session_storage(provider)?,
        &provider.name,
        provider_session_id,
    )
}

fn provider_session_storage(provider: &ProviderConfig) -> Option<&SessionStorage> {
    provider.session_storage.as_ref()
}

fn resolved_account_from_session_storage(
    session_storage: &SessionStorage,
    provider_name: &str,
    provider_session_id: &str,
) -> Option<String> {
    match session_storage {
        SessionStorage::ClaudeCode { projects_dir } => Some(display_path(projects_dir)),
        SessionStorage::Codex { sessions_dir } => Some(display_path(sessions_dir)),
        SessionStorage::Script { .. } => {
            script_storage_resolved_account(session_storage, provider_name, provider_session_id)
        }
    }
}

fn script_storage_resolved_account(
    session_storage: &SessionStorage,
    provider_name: &str,
    provider_session_id: &str,
) -> Option<String> {
    resolve_workspace_root_for_provider_session(
        Some(session_storage),
        provider_name,
        provider_session_id,
    )
    .ok()
    .map(|path| display_path(&path))
}

fn display_path(path: &Path) -> String {
    path.display().to_string()
}

pub(crate) struct ResumeExecutionEnvironment {
    pub(crate) state: StateDb,
    pub(crate) providers_cfg: ProvidersConfig,
    pub(crate) models: HashMap<String, ModelConfig>,
    pub(crate) sessions_cfg: oulipoly_config::SessionsConfig,
    pub(crate) config_root: PathBuf,
    pub(crate) models_dir: PathBuf,
}

pub(crate) fn load_resume_execution_environment(
    models_dir_override: Option<&Path>,
) -> Result<ResumeExecutionEnvironment, String> {
    let state = StateDb::open_default()?;
    let models_dir = resume_execution_models_dir(models_dir_override);
    let config_root = resume_execution_config_root(models_dir_override, &models_dir);
    let providers_cfg = oulipoly_config::ProvidersConfig::load(&config_root.join("providers.toml"))
        .unwrap_or_default();
    let models = load_models(&models_dir, Some(&providers_cfg))?;
    let sessions_cfg = oulipoly_config::SessionsConfig::load(&config_root.join("sessions.toml"))
        .unwrap_or_default();
    Ok(resume_execution_environment(
        state,
        providers_cfg,
        models,
        sessions_cfg,
        config_root,
        models_dir,
    ))
}

fn resume_execution_models_dir(models_dir_override: Option<&Path>) -> std::path::PathBuf {
    models_dir_override
        .map(Path::to_path_buf)
        .unwrap_or_else(default_models_dir)
}

fn resume_execution_config_root(models_dir_override: Option<&Path>, models_dir: &Path) -> PathBuf {
    if models_dir_override.is_some() {
        return models_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(default_config_root);
    }
    default_config_root()
}

fn resume_execution_environment(
    state: StateDb,
    providers_cfg: ProvidersConfig,
    models: HashMap<String, ModelConfig>,
    sessions_cfg: oulipoly_config::SessionsConfig,
    config_root: PathBuf,
    models_dir: PathBuf,
) -> ResumeExecutionEnvironment {
    ResumeExecutionEnvironment {
        state,
        providers_cfg,
        models,
        sessions_cfg,
        config_root,
        models_dir,
    }
}
