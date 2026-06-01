//! ## Declared roles
//! predicate

use std::path::Path;

pub(super) fn artifact_path_is_inside_rotation_root(root: &Path, path: &Path) -> bool {
    path.canonicalize()
        .ok()
        .zip(root.canonicalize().ok())
        .is_some_and(|(path, root)| path.starts_with(root))
}
