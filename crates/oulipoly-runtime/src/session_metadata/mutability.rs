//! ## Declared roles
//! predicate
//!
//! Metadata mutability classification.

use super::SessionStorageType;
use oulipoly_config::ProviderConfig;

pub(super) fn is_metadata_mutable(
    storage_type: SessionStorageType,
    provider: &ProviderConfig,
    jsonl_path: &std::path::Path,
    workspace_root: &std::path::Path,
) -> bool {
    storage_type != SessionStorageType::Other
        && provider.resume.is_some()
        && jsonl_path.is_absolute()
        && workspace_root.is_absolute()
}
