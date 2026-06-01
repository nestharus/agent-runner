//! ## Declared roles
//! predicate

use std::path::Path;

pub(super) fn journal_path_exists(path: &Path) -> bool {
    path.exists()
}
