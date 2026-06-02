//! ## Declared roles
//! orchestration, accessor, mapper, validator, formatter, predicate

use super::artifact_access::{
    expected_rotation_artifact_sha256, read_artifact_bytes, required_rotation_artifact_path,
};
use super::artifact_digest_mapper::sha256_hex;
use super::artifact_verification::{
    validate_plan_artifact_digest, validate_rotation_artifact_digest,
};
use super::error_formatter;
use super::semantic_host_plan_rejection;
use super::types::PlanArtifact;
use crate::rotation_domain::ExternalRotationError;
use oulipoly_provider::generated::Artifact;

pub(super) fn verify_rotation_artifacts(artifacts: &[Artifact]) -> Result<(), String> {
    for artifact in artifacts {
        verify_rotation_artifact(artifact)?;
    }
    Ok(())
}

pub(super) fn validate_plan_artifact_file(
    artifact: &PlanArtifact,
) -> Result<(), ExternalRotationError> {
    let bytes = read_artifact_bytes(&artifact.path).map_err(|error| {
        semantic_host_plan_rejection(error_formatter::artifact_read_error(&artifact.path, error))
    })?;
    validate_plan_artifact_digest(artifact, &sha256_hex(&bytes))
}

pub(super) fn validate_plan_artifact_files(
    artifacts: &[PlanArtifact],
) -> Result<(), ExternalRotationError> {
    for artifact in artifacts {
        validate_plan_artifact_file(artifact)?;
    }
    Ok(())
}

fn verify_rotation_artifact(artifact: &Artifact) -> Result<(), String> {
    let Some(expected) = expected_rotation_artifact_sha256(artifact) else {
        return Ok(());
    };
    let path = required_rotation_artifact_path(artifact)?;
    let bytes = read_artifact_bytes(path).map_err(error_formatter::artifact_read_failed)?;
    validate_rotation_artifact_digest(&sha256_hex(&bytes), expected)
}
