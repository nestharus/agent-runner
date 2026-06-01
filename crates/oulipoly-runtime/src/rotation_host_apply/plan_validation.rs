//! ## Declared roles
//! validator, accessor, predicate, formatter, mapper, filter

use super::predicates::{find_plan_segment, timestamps_equal};
use super::types::{ChainSegmentSnapshot, PlanArtifact, ValidatedMutationInputs};
use super::{error_formatter, semantic_host_plan_rejection};
use crate::rotation_domain::{ExternalRotationError, ExternalRotationIdentity};
use crate::services::MigrationServiceRequest;
use chrono::{DateTime, Utc};
use oulipoly_provider::generated::Artifact;
use serde_json::Value;
use std::path::PathBuf;

pub(super) fn validate_host_plan_header<'a>(
    host_state_plan: &'a Value,
    request: &MigrationServiceRequest<'_>,
    identity: &ExternalRotationIdentity,
) -> Result<&'a serde_json::Map<String, Value>, ExternalRotationError> {
    let plan = host_state_plan
        .as_object()
        .ok_or_else(|| semantic_host_plan_rejection("host_state_plan must be an object"))?;
    let version = plan
        .get("schema_version")
        .and_then(Value::as_i64)
        .ok_or_else(|| semantic_host_plan_rejection("schema_version is required"))?;
    if version != 1 {
        return Err(semantic_host_plan_rejection(
            "unsupported host_state_plan version",
        ));
    }
    validate_plan_string(plan, "operation", "rotation.materialize")?;
    validate_plan_string(plan, "chain_id", &request.resolved.chain_id)?;
    validate_plan_string(plan, "source_provider", &identity.source_provider)?;
    validate_plan_string(plan, "target_provider", &identity.target_provider)?;
    validate_plan_string(plan, "source_session_id", &identity.source_session_id)?;
    if plan
        .get("target_session_id")
        .and_then(Value::as_str)
        .is_none()
    {
        return Err(semantic_host_plan_rejection(
            "target_session_id is required",
        ));
    }
    let reason = plan
        .get("transition_reason")
        .and_then(Value::as_str)
        .ok_or_else(|| semantic_host_plan_rejection("transition_reason is required"))?;
    validate_transition_reason(reason)?;
    Ok(plan)
}

pub(super) fn validate_host_plan_body(
    plan: &serde_json::Map<String, Value>,
    snapshot: &ChainSegmentSnapshot,
    result_artifacts: &[Artifact],
    identity: &ExternalRotationIdentity,
) -> Result<(), ExternalRotationError> {
    validate_snapshot_identity(snapshot, identity)?;
    validate_plan_segments(plan, snapshot, identity)?;
    validate_plan_artifacts(plan, result_artifacts)?;
    Ok(())
}

pub(super) fn validate_mutation_inputs(
    plan: &serde_json::Map<String, Value>,
    request: &MigrationServiceRequest<'_>,
    identity: &ExternalRotationIdentity,
    result: &oulipoly_provider::generated::RotationMaterializeResult,
) -> Result<ValidatedMutationInputs, ExternalRotationError> {
    let target_session_id = required_plan_string(plan, "target_session_id")?;
    let changed_at = validated_segment_boundary(plan, identity, target_session_id)?;
    let target_provider_index = request
        .migration_model
        .providers
        .iter()
        .position(|provider| provider.name == identity.target_provider)
        .ok_or_else(|| semantic_host_plan_rejection("target provider is not in model pool"))?;
    let target_jsonl_path = result
        .artifacts
        .iter()
        .find_map(|artifact| artifact.path.as_deref())
        .map(PathBuf::from)
        .ok_or_else(|| semantic_host_plan_rejection("materialized artifact path is required"))?;
    Ok(ValidatedMutationInputs {
        target_provider_index,
        target_session_id: target_session_id.to_string(),
        target_jsonl_path,
        reason: super::plan_mapper::transition_reason_for_mapping(required_plan_string(
            plan,
            "transition_reason",
        )?)?,
        changed_at,
    })
}

pub(super) fn validate_plan_artifact_list(
    plan: &serde_json::Map<String, Value>,
) -> Result<Vec<PlanArtifact>, ExternalRotationError> {
    let plan_artifacts = super::plan_mapper::plan_artifacts(plan)?;
    if plan_artifacts.is_empty() {
        return Err(semantic_host_plan_rejection("plan artifacts are required"));
    }
    Ok(plan_artifacts)
}

fn validate_snapshot_identity(
    snapshot: &ChainSegmentSnapshot,
    identity: &ExternalRotationIdentity,
) -> Result<(), ExternalRotationError> {
    if snapshot.active_provider != identity.source_provider {
        return Err(semantic_host_plan_rejection(
            "source provider snapshot mismatch",
        ));
    }
    if snapshot.active_session_id != identity.source_session_id {
        return Err(semantic_host_plan_rejection(
            "source session snapshot mismatch",
        ));
    }
    if snapshot.active_ended_at.is_some() {
        return Err(semantic_host_plan_rejection(
            "source segment snapshot is not active",
        ));
    }
    Ok(())
}

fn validate_plan_segments(
    plan: &serde_json::Map<String, Value>,
    snapshot: &ChainSegmentSnapshot,
    identity: &ExternalRotationIdentity,
) -> Result<(), ExternalRotationError> {
    let segments = plan_array(plan, "segments")?;
    let source = find_plan_segment(
        segments,
        &identity.source_provider,
        &identity.source_session_id,
    )
    .ok_or_else(|| semantic_host_plan_rejection("source segment snapshot is missing"))?;
    let target = find_plan_segment(
        segments,
        &identity.target_provider,
        required_plan_string(plan, "target_session_id")?,
    )
    .ok_or_else(|| semantic_host_plan_rejection("target segment snapshot is missing"))?;
    let source_ended_at = required_value_string(source, "ended_at")?;
    if let Some(latest_turn_at) = snapshot.latest_turn_at.as_deref()
        && !timestamps_equal(source_ended_at, latest_turn_at)
    {
        return Err(semantic_host_plan_rejection(
            "source segment snapshot is stale",
        ));
    }
    let target_started_at = required_value_string(target, "started_at")?;
    if !timestamps_equal(target_started_at, source_ended_at) {
        return Err(semantic_host_plan_rejection(
            "target segment snapshot does not align with source boundary",
        ));
    }
    Ok(())
}

fn validated_segment_boundary(
    plan: &serde_json::Map<String, Value>,
    identity: &ExternalRotationIdentity,
    target_session_id: &str,
) -> Result<DateTime<Utc>, ExternalRotationError> {
    let segments = plan_array(plan, "segments")?;
    let source = find_plan_segment(
        segments,
        &identity.source_provider,
        &identity.source_session_id,
    )
    .ok_or_else(|| semantic_host_plan_rejection("source segment snapshot is missing"))?;
    let target = find_plan_segment(segments, &identity.target_provider, target_session_id)
        .ok_or_else(|| semantic_host_plan_rejection("target segment snapshot is missing"))?;
    let source_ended_at = required_value_string(source, "ended_at")?;
    let target_started_at = required_value_string(target, "started_at")?;
    if !timestamps_equal(target_started_at, source_ended_at) {
        return Err(semantic_host_plan_rejection(
            "target segment snapshot does not align with source boundary",
        ));
    }
    DateTime::parse_from_rfc3339(source_ended_at)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|_| semantic_host_plan_rejection("segment boundary timestamp is invalid"))
}

fn validate_plan_artifacts(
    plan: &serde_json::Map<String, Value>,
    result_artifacts: &[Artifact],
) -> Result<(), ExternalRotationError> {
    let plan_artifacts = validate_plan_artifact_list(plan)?;
    let result_artifacts = super::plan_mapper::result_artifacts_with_hashes(result_artifacts)?;
    if plan_artifacts != result_artifacts {
        return Err(semantic_host_plan_rejection(
            "host_state_plan artifacts do not match materialize result artifacts",
        ));
    }
    Ok(())
}

pub(super) fn plan_array<'a>(
    plan: &'a serde_json::Map<String, Value>,
    field: &str,
) -> Result<&'a Vec<Value>, ExternalRotationError> {
    plan.get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| semantic_host_plan_rejection(error_formatter::required_field(field)))
}

pub(super) fn required_plan_string<'a>(
    plan: &'a serde_json::Map<String, Value>,
    field: &str,
) -> Result<&'a str, ExternalRotationError> {
    plan.get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| semantic_host_plan_rejection(error_formatter::required_field(field)))
}

pub(super) fn required_value_string<'a>(
    value: &'a serde_json::Map<String, Value>,
    field: &str,
) -> Result<&'a str, ExternalRotationError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| semantic_host_plan_rejection(error_formatter::required_field(field)))
}

fn validate_plan_string(
    plan: &serde_json::Map<String, Value>,
    field: &str,
    expected: &str,
) -> Result<(), ExternalRotationError> {
    match plan.get(field).and_then(Value::as_str) {
        Some(actual) if actual == expected => Ok(()),
        Some(_) => Err(semantic_host_plan_rejection(
            error_formatter::field_mismatch(field),
        )),
        None => Err(semantic_host_plan_rejection(
            error_formatter::required_field(field),
        )),
    }
}

fn validate_transition_reason(reason: &str) -> Result<(), ExternalRotationError> {
    match reason {
        "manual" | "quota_threshold" | "exhausted" => Ok(()),
        _ => Err(semantic_host_plan_rejection("invalid transition_reason")),
    }
}
