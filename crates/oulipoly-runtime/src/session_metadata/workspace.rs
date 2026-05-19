//! ## Declared roles
//! accessor, mapper
//!
//! Workspace-root resolution facade over cwd adapter execution.

use super::MetadataError;
use super::errors::unsupported_storage_error;
use oulipoly_config::SessionStorage;
use std::path::PathBuf;

pub(super) fn resolve_cwd_from_session_storage(
    session_storage: Option<&SessionStorage>,
    provider_name: &str,
    session_id: &str,
) -> Result<PathBuf, MetadataError> {
    super::cwd::resolve_workspace_root(session_storage, session_id)
        .map_err(|reason| unsupported_storage_error(provider_name, reason))
}
