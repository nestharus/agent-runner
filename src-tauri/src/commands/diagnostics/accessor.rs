//! Declared roles: accessor

use agent_runner_lib::load_app_config;
use oulipoly_config::{ModelConfig, ProvidersConfig};
use std::collections::HashMap;

pub(super) struct DiagnosticsDependencies {
    pub(super) diag_model: ModelConfig,
    pub(super) providers_cfg: ProvidersConfig,
}

pub(super) fn load_diagnostics_dependencies(
    models: &HashMap<String, ModelConfig>,
) -> Option<DiagnosticsDependencies> {
    let app_config = load_app_config();
    let diag_model_name = app_config.diagnostics_model?;
    let diag_model = models.get(&diag_model_name)?.clone();
    let providers_path = crate::default_config_root().join("providers.toml");
    let providers_cfg = ProvidersConfig::load(&providers_path).unwrap_or_default();
    Some(DiagnosticsDependencies {
        diag_model,
        providers_cfg,
    })
}
