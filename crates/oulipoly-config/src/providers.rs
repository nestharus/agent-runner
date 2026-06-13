//! ## Declared roles
//!
//! - accessor
//! - orchestration
//!
//! Role set: { accessor, orchestration }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-config/src/providers.rs
//!     role: intrinsic-surface
//!     Domain: providers-config-facade
//!     Owns:
//!       - public providers module facade and frozen re-export surface
//!       - submodule declaration hub for provider account configuration concerns
//!       - crate-internal test support aliases for moved providers loader helpers
//! ```

mod config;
mod entry;
mod error;
mod loader;
mod raw_entry;

pub use self::config::ProvidersConfig;
pub use self::entry::ProviderEntry;
pub use self::error::LoadError;
pub use self::loader::{migrate_legacy_session_storage_file, parse_prompt_mode};

pub(crate) use self::entry::shell_split;

#[cfg(test)]
use self::loader::{
    apply_defaults_to_raw_providers, parse_providers_toml, validate_providers_config,
};

#[cfg(test)]
use self::raw_entry::{RawEntry, RawProvidersToml};

#[cfg(test)]
mod tests;
