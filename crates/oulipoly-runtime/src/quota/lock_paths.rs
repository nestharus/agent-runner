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
//!       - process data-home environment contract (`OULIPOLY_DATA_DIR`, `OULIPOLY_DATA_HOME`, `XDG_DATA_HOME`, `dirs::data_dir`)
//!       - lock-name sanitization contract (`[A-Za-z0-9_-]`)
//! ```
//!
//! The marker-verification refresh lock (`usage-refresh-locks`) and the
//! auth-refresh single-flight lock (`auth-refresh-locks`) both live under the
//! same process data home and share one sanitized key space, so a given
//! account resolves to the same lock identity from every call site.

use std::ffi::OsString;
use std::path::PathBuf;

/// Process data home for runtime lock files: `OULIPOLY_DATA_HOME`, then
/// `XDG_DATA_HOME`, then the platform data dir, then the current directory.
/// Public so the usage CLI (a separate crate) can resolve the same lock root
/// instead of re-deriving the env layout.
pub fn data_home() -> PathBuf {
    env_path("OULIPOLY_DATA_HOME")
        .or_else(|| env_path("XDG_DATA_HOME"))
        .unwrap_or_else(default_data_home)
}

pub fn app_data_dir() -> PathBuf {
    env_path(oulipoly_state::paths::DATA_DIR_ENV)
        .unwrap_or_else(|| data_home().join(oulipoly_state::paths::APP_DATA_DIR_NAME))
}

fn env_path(name: &str) -> Option<PathBuf> {
    env_os(name).map(PathBuf::from)
}

fn env_os(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

fn default_data_home() -> PathBuf {
    dirs::data_dir().unwrap_or_else(current_dir_path)
}

fn current_dir_path() -> PathBuf {
    PathBuf::from(".")
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
