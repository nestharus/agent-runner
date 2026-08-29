//! ## Declared roles
//!
//! `accessor`, `validator`
//!
//! Config path derivation: models / agents / config-root directories, resolving
//! explicit CLI overrides before the required environment-controlled root.

use crate::usage::cli::Cli;
use std::path::PathBuf;

pub(crate) fn resolve_models_dir(cli: &Cli) -> Result<PathBuf, String> {
    if let Some(ref dir) = cli.models_dir {
        return Ok(dir.clone());
    }
    default_models_dir()
}

pub(crate) fn default_models_dir() -> Result<PathBuf, String> {
    default_config_root().map(|root| root.join("models"))
}

pub(crate) fn default_config_root() -> Result<PathBuf, String> {
    oulipoly_state::paths::config_dir()
}

pub(crate) fn resolve_agents_dir(cli: &Cli) -> Result<PathBuf, String> {
    cli.agents_dir.clone().map_or_else(default_agents_dir, Ok)
}

fn default_agents_dir() -> Result<PathBuf, String> {
    default_config_root().map(|root| root.join("agents"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    // Characterization test for AGE-8 — pins current behavior of resolve_models_dir CLI adapter.
    #[test]
    fn resolve_models_dir_prefers_explicit_override() {
        let cli = Cli::try_parse_from([
            "oulipoly-agent-runner",
            "--models-dir",
            "/tmp/age8-models",
            "--model",
            "fixture",
            "prompt",
        ])
        .unwrap();

        assert_eq!(
            resolve_models_dir(&cli).unwrap(),
            PathBuf::from("/tmp/age8-models")
        );
    }
}
