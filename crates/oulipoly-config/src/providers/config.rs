//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - mapper
//! - orchestration
//!
//! Role set: { accessor, formatter, mapper, orchestration }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-config/src/providers/config.rs
//!     role: intrinsic-surface
//!     Domain: providers-config-collection
//!     Owns:
//!       - ProvidersConfig public provider account collection schema
//!       - provider lookup and runtime ProviderConfig composition entrypoints
//!       - providers.toml file loading orchestration subordinate to provider collection construction
//! ```
//!
use crate::model::{PromptMode, ProviderConfig};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use super::entry::ProviderEntry;
use super::error::LoadError;
use super::loader::{
    apply_defaults_to_raw_providers, parse_providers_toml, raw_entry_to_entry,
    validate_invocation_mode, validate_providers_config,
};
use super::raw_entry::RawProvidersToml;

#[derive(Debug, Clone, Default)]
pub struct ProvidersConfig {
    pub entries: HashMap<String, ProviderEntry>,
}

/// Read a providers.toml file, returning `None` when it does not exist.
fn read_optional_providers_file(path: &Path) -> Result<Option<String>, LoadError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(LoadError::Io(err)),
    }
}

impl ProvidersConfig {
    /// Parse a providers.toml, returning an empty config if the file doesn't exist.
    pub fn load(path: &Path) -> Result<Self, LoadError> {
        let Some(text) = read_optional_providers_file(path)? else {
            return Ok(Self::default());
        };
        let raw = parse_providers_toml(&text)?;
        let raw = apply_defaults_to_raw_providers(raw);
        validate_providers_config(&raw)?;
        Ok(Self::from_validated_raw(raw))
    }

    fn from_validated_raw(raw: RawProvidersToml) -> Self {
        let entries = raw
            .into_iter()
            .map(|(name, raw_entry)| {
                let invocation_mode =
                    validate_invocation_mode(&name, raw_entry.invocation_mode.as_deref())
                        .expect("validated provider invocation_mode");
                (name, raw_entry_to_entry(&raw_entry, invocation_mode))
            })
            .collect();
        Self { entries }
    }

    pub fn get(&self, name: &str) -> Option<&ProviderEntry> {
        self.entries.get(name)
    }

    pub fn effective_provider(
        &self,
        model_provider: &ProviderConfig,
    ) -> Result<(ProviderConfig, PromptMode), String> {
        let runtime = self
            .get(&model_provider.name)
            .ok_or_else(|| format_missing_provider_error(&model_provider.name))?;
        runtime.effective_provider(&model_provider.name, Some(model_provider))
    }

    pub fn runtime_provider(&self, name: &str) -> Result<(ProviderConfig, PromptMode), String> {
        let runtime = self
            .get(name)
            .ok_or_else(|| format_missing_provider_error(name))?;
        runtime.effective_provider(name, None)
    }
}

fn format_missing_provider_error(name: &str) -> String {
    format!("provider {name} is missing from providers.toml")
}
