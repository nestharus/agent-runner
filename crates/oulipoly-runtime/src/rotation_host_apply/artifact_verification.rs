//! ## Declared roles
//! validator

use super::error_formatter;
use super::semantic_host_plan_rejection;
use super::types::PlanArtifact;
use crate::rotation_domain::ExternalRotationError;

pub(super) fn validate_plan_artifact_digest(
    artifact: &PlanArtifact,
    actual: &str,
) -> Result<(), ExternalRotationError> {
    if actual == artifact.sha256 {
        Ok(())
    } else {
        Err(semantic_host_plan_rejection(
            error_formatter::host_state_plan_artifact_sha256_mismatch(),
        ))
    }
}

pub(super) fn validate_rotation_artifact_digest(
    actual: &str,
    expected: &str,
) -> Result<(), String> {
    if actual == expected {
        Ok(())
    } else {
        Err(error_formatter::artifact_sha256_mismatch())
    }
}
