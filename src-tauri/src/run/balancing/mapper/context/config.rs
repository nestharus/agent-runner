use std::path::PathBuf;

use oulipoly_config::{ProvidersConfig, SessionsConfig};
use oulipoly_state::StateDb;

use super::super::super::accessor::BalancedExecutionEnvironment;

pub(in crate::run::balancing) struct BalancedConfigTomlPaths {
    pub(in crate::run::balancing) providers_path: PathBuf,
    pub(in crate::run::balancing) sessions_path: PathBuf,
}

pub(in crate::run::balancing) fn balanced_config_toml_paths(
    config_root: PathBuf,
) -> BalancedConfigTomlPaths {
    BalancedConfigTomlPaths {
        providers_path: config_root.join("providers.toml"),
        sessions_path: config_root.join("sessions.toml"),
    }
}

pub(in crate::run::balancing) fn balanced_execution_environment(
    state: StateDb,
    providers_cfg: ProvidersConfig,
    sessions_cfg: SessionsConfig,
    models_dir: PathBuf,
) -> BalancedExecutionEnvironment {
    BalancedExecutionEnvironment {
        state,
        providers_cfg,
        sessions_cfg,
        models_dir,
    }
}
