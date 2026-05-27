//! ## Declared roles
//!
//! `accessor`, `validator`
//!
//! Config path derivation: models / agents / config-root directories, resolving
//! explicit CLI overrides before the `dirs::config_dir()` fallback. Relocated
//! byte-identical out of `main.rs` (AGE-207, AGE-183 program slice B12).

use crate::usage::cli::Cli;
use std::path::PathBuf;

pub(crate) fn resolve_models_dir(cli: &Cli) -> PathBuf {
    if let Some(ref dir) = cli.models_dir {
        return dir.clone();
    }
    default_models_dir()
}

pub(crate) fn default_models_dir() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("oulipoly-agent-runner").join("models"))
        .unwrap_or_else(|| PathBuf::from("models"))
}

pub(crate) fn default_config_root() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("oulipoly-agent-runner"))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub(crate) fn resolve_agents_dir(cli: &Cli) -> PathBuf {
    cli.agents_dir.clone().unwrap_or_else(default_agents_dir)
}

fn default_agents_dir() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("oulipoly-agent-runner").join("agents"))
        .unwrap_or_else(|| PathBuf::from("agents"))
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

        assert_eq!(resolve_models_dir(&cli), PathBuf::from("/tmp/age8-models"));
    }
}
