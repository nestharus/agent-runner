//! Role: validator.

use super::identity::ExternalSessionIdentity;
use super::provider_error::{
    ExternalSessionProviderError, map_invalid_artifact_error, map_invalid_host_state_plan_error,
    map_postimage_hash_mismatch_error,
};
use super::replace_input_mapper::PreparedReplaceInput;
use super::request_builder::CANONICAL_FORMAT;
use oulipoly_provider::generated::{Artifact, SessionReplaceResult};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub(crate) struct AcceptedReplaceResult {
    pub(crate) postimage_sha256: String,
    pub(crate) artifact_path: PathBuf,
}

pub(crate) fn validate_changed_replace_result(
    identity: &ExternalSessionIdentity,
    session_id: &str,
    input: &PreparedReplaceInput,
    result: &SessionReplaceResult,
) -> Result<AcceptedReplaceResult, ExternalSessionProviderError> {
    let postimage = required_postimage(result)?;
    validate_hash_shape(postimage)?;
    let artifact_path = validate_artifacts(&result.artifacts)?;
    validate_artifact_hashes(&result.artifacts, postimage)?;
    validate_host_state_plan(identity, session_id, input, postimage, result)?;
    Ok(AcceptedReplaceResult {
        postimage_sha256: postimage.to_string(),
        artifact_path,
    })
}

fn required_postimage(result: &SessionReplaceResult) -> Result<&str, ExternalSessionProviderError> {
    result
        .postimage_sha256
        .as_deref()
        .ok_or_else(map_postimage_hash_mismatch_error)
}

fn validate_hash_shape(hash: &str) -> Result<(), ExternalSessionProviderError> {
    if hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(map_postimage_hash_mismatch_error())
    }
}

fn validate_artifacts(artifacts: &[Artifact]) -> Result<PathBuf, ExternalSessionProviderError> {
    let mut file_artifact_path = None;
    for artifact in artifacts {
        if let Some(path) = validate_artifact(artifact)? {
            file_artifact_path.get_or_insert(path);
        }
    }
    file_artifact_path.ok_or_else(map_invalid_artifact_error)
}

fn validate_artifact(artifact: &Artifact) -> Result<Option<PathBuf>, ExternalSessionProviderError> {
    if artifact.kind != "file" {
        return Ok(None);
    }
    let Some(path) = artifact.path.as_deref() else {
        return Err(map_invalid_artifact_error());
    };
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err(map_invalid_artifact_error());
    }
    let Some(sha256) = artifact.sha256.as_deref() else {
        return Err(map_invalid_artifact_error());
    };
    if validate_hash_shape(sha256).is_err() {
        return Err(map_invalid_artifact_error());
    }
    Ok(Some(path))
}

fn validate_artifact_hashes(
    artifacts: &[Artifact],
    postimage: &str,
) -> Result<(), ExternalSessionProviderError> {
    for artifact in artifacts {
        if artifact.kind == "file" && artifact.sha256.as_deref() != Some(postimage) {
            return Err(map_postimage_hash_mismatch_error());
        }
    }
    Ok(())
}

fn validate_host_state_plan(
    identity: &ExternalSessionIdentity,
    session_id: &str,
    input: &PreparedReplaceInput,
    postimage: &str,
    result: &SessionReplaceResult,
) -> Result<(), ExternalSessionProviderError> {
    let plan = result
        .host_state_plan
        .as_ref()
        .ok_or_else(map_invalid_host_state_plan_error)?;
    validate_plan_keys(plan)?;
    validate_plan_scalar(plan, "operation", "session.replace")?;
    validate_plan_scalar(plan, "session_id", session_id)?;
    validate_plan_scalar(plan, "provider_name", &identity.provider_name)?;
    validate_plan_scalar(plan, "canonical_format", CANONICAL_FORMAT)?;
    validate_plan_scalar(plan, "records_sha256", &input.records_sha256)?;
    validate_plan_scalar(plan, "postimage_sha256", postimage)?;
    validate_plan_integer(plan, "schema_version", 1)?;
    validate_plan_integer(plan, "turn_count", input.turn_count)?;
    validate_plan_artifacts(plan, &result.artifacts)
}

fn validate_plan_keys(plan: &Value) -> Result<(), ExternalSessionProviderError> {
    let Some(object) = plan.as_object() else {
        return Err(map_invalid_host_state_plan_error());
    };
    let allowed = [
        "schema_version",
        "operation",
        "session_id",
        "provider_name",
        "canonical_format",
        "turn_count",
        "records_sha256",
        "postimage_sha256",
        "artifacts",
    ];
    if object.keys().all(|key| allowed.contains(&key.as_str())) {
        Ok(())
    } else {
        Err(map_invalid_host_state_plan_error())
    }
}

fn validate_plan_scalar(
    plan: &Value,
    key: &str,
    expected: &str,
) -> Result<(), ExternalSessionProviderError> {
    if plan.get(key).and_then(Value::as_str) == Some(expected) {
        Ok(())
    } else {
        Err(map_invalid_host_state_plan_error())
    }
}

fn validate_plan_integer(
    plan: &Value,
    key: &str,
    expected: u64,
) -> Result<(), ExternalSessionProviderError> {
    if plan.get(key).and_then(Value::as_u64) == Some(expected) {
        Ok(())
    } else {
        Err(map_invalid_host_state_plan_error())
    }
}

fn validate_plan_artifacts(
    plan: &Value,
    artifacts: &[Artifact],
) -> Result<(), ExternalSessionProviderError> {
    let expected =
        serde_json::to_value(artifacts).map_err(|_| map_invalid_host_state_plan_error())?;
    if plan.get("artifacts") == Some(&expected) {
        Ok(())
    } else {
        Err(map_invalid_host_state_plan_error())
    }
}
