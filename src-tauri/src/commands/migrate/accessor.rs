//! Declared role: accessor

use oulipoly_state::StateDb;
use std::path::{Path, PathBuf};

pub(super) fn default_state_db_path() -> Result<PathBuf, String> {
    StateDb::default_path()
}

pub(super) fn state_db_parent_dir(db_path: &Path) -> Result<&Path, String> {
    db_path
        .parent()
        .ok_or_else(|| super::formatter::format_state_db_path_no_parent_error(db_path))
}

pub(super) fn open_default_state_db() -> Result<StateDb, String> {
    StateDb::open_default()
}

pub(super) fn open_state_db(path: &Path) -> Result<StateDb, String> {
    StateDb::open(path)
}
