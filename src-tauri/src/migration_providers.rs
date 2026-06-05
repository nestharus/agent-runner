//! Migration provider resolution helpers and resume-execution environment loader.
//!
//! Relocated from `src-tauri/src/main.rs` by AGE-203 (slice B8 of the AGE-183
//! main.rs decomposition program; map row H12). Output-preserving: bodies
//! byte-identical to the pre-AGE-203 main.rs definitions; only visibility +
//! `crate::` import targets change.
//!
//! ## Declared roles
//!
//! `orchestration`, `accessor`, `mapper`
//!
//! - `orchestration`: `load_resume_execution_environment` — loads the four-piece
//!   resume environment (state db + providers config + models map + sessions config).
//! - `accessor`: `provider_session_resolved_account` — reads the provider's
//!   `session_storage` and returns the resolved account-path display string.
//! - `mapper`: the `SessionStorage` → `Option<String>` match inside
//!   `provider_session_resolved_account`.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/migration_providers.rs
//!     role: adapter
//!     Translates:
//!       - AGE-203 H12: ProviderConfig session_storage -> resolved-account display string
//!       - AGE-203 H12: models_dir_override + default config root -> ResumeExecutionEnvironment
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use oulipoly_config::{ModelConfig, ProviderConfig, ProvidersConfig, SessionStorage, load_models};
use oulipoly_runtime::session_metadata::resolve_workspace_root_for_provider_session;
use oulipoly_state::StateDb;

use crate::cli::paths::{default_config_root, default_models_dir};

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
        SessionStorage::ClaudeCode { projects_dir } => Some(projects_dir.display().to_string()),
        SessionStorage::Codex { sessions_dir } => Some(sessions_dir.display().to_string()),
        SessionStorage::Script { .. } => resolve_workspace_root_for_provider_session(
            Some(session_storage),
            provider_name,
            provider_session_id,
        )
        .ok()
        .map(|path| path.display().to_string()),
    }
}

pub(crate) struct ResumeExecutionEnvironment {
    pub(crate) state: StateDb,
    pub(crate) providers_cfg: ProvidersConfig,
    pub(crate) models: HashMap<String, ModelConfig>,
    pub(crate) sessions_cfg: oulipoly_config::SessionsConfig,
    pub(crate) models_dir: PathBuf,
}

pub(crate) fn load_resume_execution_environment(
    models_dir_override: Option<&Path>,
) -> Result<ResumeExecutionEnvironment, String> {
    let state = StateDb::open_default()?;
    let models_dir = resume_execution_models_dir(models_dir_override);
    let config_root = default_config_root();
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
        models_dir,
    ))
}

fn resume_execution_models_dir(models_dir_override: Option<&Path>) -> std::path::PathBuf {
    models_dir_override
        .map(Path::to_path_buf)
        .unwrap_or_else(default_models_dir)
}

fn resume_execution_environment(
    state: StateDb,
    providers_cfg: ProvidersConfig,
    models: HashMap<String, ModelConfig>,
    sessions_cfg: oulipoly_config::SessionsConfig,
    models_dir: PathBuf,
) -> ResumeExecutionEnvironment {
    ResumeExecutionEnvironment {
        state,
        providers_cfg,
        models,
        sessions_cfg,
        models_dir,
    }
}
