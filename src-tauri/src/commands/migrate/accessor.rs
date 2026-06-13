//! ## Declared roles
//!
//! - orchestration
//! - validator
//! - accessor
//!
//! Role set: { orchestration, validator, accessor }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/src/commands/migrate/accessor.rs
//!     role: intrinsic-surface
//!     Domain: migrate-db-open-seam
//!     Owns:
//!       - the migrate-db StateDb open seam this accessor owns: StateDb::default_path, StateDb::open, and StateDb::open_with_legacy_provider_names (the PP-001 pushed-lookup entrypoint)
//!       - the parent-dir resolution + error formatting this seam owns: std::path::Path/PathBuf parent access and super::formatter::format_state_db_path_no_parent_error
//!       - the PP-001 app-side legacy provider-name lookup pushed into the open seam: crate::migration_providers::legacy_invocation_provider_names
//!       - Intrinsic carrier subordinate to this domain: oulipoly_state::StateDb
//! ```

use oulipoly_state::StateDb;
use std::path::{Path, PathBuf};

pub(super) fn default_state_db_path() -> Result<PathBuf, String> {
    StateDb::default_path()
}

pub(super) fn state_db_parent_dir(db_path: &Path) -> Result<&Path, String> {
    db_path_parent(db_path)
        .ok_or_else(|| super::formatter::format_state_db_path_no_parent_error(db_path))
}

fn db_path_parent(db_path: &Path) -> Option<&Path> {
    db_path.parent()
}

pub(super) fn open_default_state_db() -> Result<StateDb, String> {
    // PP-001: push the app-resolved legacy provider-name lookup into the open so
    // a pre-UUID invocation migration maps provider names without StateDb itself
    // discovering the app config layout.
    StateDb::open_with_legacy_provider_names(
        &StateDb::default_path()?,
        &crate::migration_providers::legacy_invocation_provider_names(),
    )
}

pub(super) fn open_state_db(path: &Path) -> Result<StateDb, String> {
    StateDb::open(path)
}
