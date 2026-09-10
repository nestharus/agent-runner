//! Shared lock-directory resolution for quota refresh locks.
//!
//! ## Declared roles
//! accessor, mapper, predicate
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/quota/lock_paths.rs
//!     role: adapter
//!     Translates:
//!       - explicit process data-home environment contract (`OULIPOLY_DATA_DIR`, `OULIPOLY_DATA_HOME`)
//!       - lock-name sanitization contract (`[A-Za-z0-9_-]`)
//! ```
//!
//! The marker-verification refresh lock (`usage-refresh-locks`) and the
//! auth-refresh single-flight lock (`auth-refresh-locks`) both live under the
//! same process data home and share one sanitized key space, so a given
//! account resolves to the same lock identity from every call site.

use std::path::PathBuf;

/// Explicit process data home for callers that own non-application data.
pub fn data_home() -> Result<PathBuf, String> {
    required_env_path("OULIPOLY_DATA_HOME")
}

pub fn app_data_dir() -> Result<PathBuf, String> {
    oulipoly_state::paths::data_dir()
}

fn required_env_path(name: &str) -> Result<PathBuf, String> {
    std::env::var_os(name).map(PathBuf::from).ok_or_else(|| {
        format!(
            "{name} is not set; set it to an explicit data directory, for example: export {name}=/path/to/data"
        )
    })
}

/// Sanitize an account/provider key into a single safe lock-file stem,
/// mapping every byte outside `[A-Za-z0-9_-]` to `_`. Public so every lock
/// call site (including the usage CLI crate) shares one sanitized key space.
pub fn sanitize_lock_name(name: &str) -> String {
    name.chars().map(sanitize_lock_char).collect()
}

fn sanitize_lock_char(ch: char) -> char {
    if lock_char_is_safe(ch) { ch } else { '_' }
}

fn lock_char_is_safe(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'
}
