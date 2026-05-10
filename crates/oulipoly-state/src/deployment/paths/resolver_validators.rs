use crate::deployment::metadata::store::rows::DeploymentSnapshot;

use super::types::{DbRole, ResolveError};
use std::path::PathBuf;

pub(super) fn validate_inputs(
    deployment: &DeploymentSnapshot,
    current_schema_version: u32,
) -> Result<(DbRole, u32), ResolveError> {
    let primary = &deployment.primary;
    if primary.schema_version == 0 {
        return Err(ResolveError::NoPrimary);
    }

    let expected_version = expected_version_for_role(deployment);
    if current_schema_version != expected_version {
        return Err(ResolveError::RoleMismatch);
    }

    Ok((primary.role, expected_version))
}

pub(super) fn validate_versioned_path(
    path: Option<PathBuf>,
    schema_version: u32,
) -> Result<PathBuf, ResolveError> {
    path.ok_or(ResolveError::UnknownVersion(schema_version))
}

fn expected_version_for_role(deployment: &DeploymentSnapshot) -> u32 {
    let primary = &deployment.primary;
    match primary.role {
        DbRole::Steady => primary.schema_version,
        DbRole::PreCutoverPrimary => deployment
            .active_deployment
            .as_ref()
            .map(|row| row.from_schema_version)
            .unwrap_or(primary.schema_version),
        DbRole::PostCutoverPrimary => deployment
            .active_deployment
            .as_ref()
            .map(|row| row.to_schema_version)
            .unwrap_or(primary.schema_version),
        DbRole::RetentionSecondary => primary.schema_version,
    }
}
