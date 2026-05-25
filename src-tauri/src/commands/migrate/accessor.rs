//! Declared role: accessor

use oulipoly_state::StateDb;
use std::path::{Path, PathBuf};

pub(super) fn default_state_db_path() -> Result<PathBuf, String> {
    StateDb::default_path()
}

pub(super) fn state_db_parent_dir(db_path: &Path) -> Result<&Path, String> {
    db_path
        .parent()
        .ok_or_else(|| format!("state DB path has no parent: {}", db_path.display()))
}
