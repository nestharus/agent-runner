//! Declared role: filter

use std::path::PathBuf;

pub(super) fn first_unused_path(candidates: Vec<PathBuf>) -> Option<PathBuf> {
    candidates
        .into_iter()
        .find(|candidate| super::predicate::unused_path(candidate))
}
