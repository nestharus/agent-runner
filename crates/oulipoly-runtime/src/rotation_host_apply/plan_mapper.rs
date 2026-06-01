//! ## Declared roles
//! mapper, accessor, validator, formatter

use super::plan_validation::{plan_array, required_value_string};
use super::semantic_host_plan_rejection;
use super::types::PlanArtifact;
use crate::rotation_domain::ExternalRotationError;
use oulipoly_provider::generated::Artifact;
use serde_json::Value;

pub(super) fn plan_artifacts(
    plan: &serde_json::Map<String, Value>,
) -> Result<Vec<PlanArtifact>, ExternalRotationError> {
    let artifacts = plan_array(plan, "artifacts")?;
    artifacts
        .iter()
        .map(|artifact| {
            let artifact = artifact
                .as_object()
                .ok_or_else(|| semantic_host_plan_rejection("artifact must be an object"))?;
            Ok(PlanArtifact {
                path: required_value_string(artifact, "path")?.to_string(),
                sha256: required_value_string(artifact, "sha256")?.to_string(),
            })
        })
        .collect()
}

pub(super) fn result_artifacts_with_hashes(
    artifacts: &[Artifact],
) -> Result<Vec<PlanArtifact>, ExternalRotationError> {
    artifacts
        .iter()
        .map(|artifact| {
            Ok(PlanArtifact {
                path: artifact.path.clone().ok_or_else(|| {
                    semantic_host_plan_rejection("result artifact path is required")
                })?,
                sha256: artifact.sha256.clone().ok_or_else(|| {
                    semantic_host_plan_rejection("result artifact sha256 is required")
                })?,
            })
        })
        .collect()
}

pub(super) fn transition_reason_for_mapping(
    reason: &str,
) -> Result<crate::balancer::TransitionReason, ExternalRotationError> {
    match reason {
        "manual" => Ok(crate::balancer::TransitionReason::Manual),
        "quota_threshold" => Ok(crate::balancer::TransitionReason::QuotaThreshold),
        "exhausted" => Ok(crate::balancer::TransitionReason::Exhausted),
        _ => Err(semantic_host_plan_rejection("invalid transition_reason")),
    }
}
