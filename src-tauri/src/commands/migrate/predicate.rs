//! Declared role: predicate

use std::path::Path;

pub(super) fn unused_path(path: &Path) -> bool {
    !path.exists()
}

pub(super) fn missing_state_db(db_path: &Path) -> bool {
    !db_path.exists()
}
