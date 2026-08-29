//! Declared roles: accessor

use agent_runner_lib::load_app_config;
use oulipoly_config::{ModelConfig, ProvidersConfig};
use std::collections::HashMap;

pub(super) fn load_diagnostics_dependencies(
    models: &HashMap<String, ModelConfig>,
) -> Option<super::mapper::DiagnosticsDependencies> {
    let app_config = load_app_config();
    let diag_model_name = app_config.diagnostics_model?;
    let diag_model = models.get(&diag_model_name)?.clone();
    let providers_path = crate::cli::paths::default_config_root()
        .ok()?
        .join("providers.toml");
    let providers_cfg = ProvidersConfig::load(&providers_path).unwrap_or_default();
    Some(super::mapper::diagnostics_dependencies(
        diag_model,
        providers_cfg,
    ))
}
