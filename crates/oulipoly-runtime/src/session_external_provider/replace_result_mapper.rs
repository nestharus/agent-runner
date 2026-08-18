//! Role: validator.

use super::identity::ExternalSessionIdentity;
use super::provider_error::{
    ExternalSessionProviderError, map_invalid_artifact_error, map_invalid_host_state_plan_error,
    map_postimage_hash_mismatch_error, map_provider_owned_token_error,
};
use super::replace_input_mapper::PreparedReplaceInput;
use super::request_builder::{CANONICAL_FORMAT, PROVIDER_OWNED_REPLACE_PROTOCOL};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use oulipoly_provider::generated::{SessionReplaceCanonicalPostimage, SessionReplaceResult};
use serde_json::Value;

pub(crate) const DB_APPLY_CAPABILITY: &str = "replace_session_turns_from_canonical_v1";

#[derive(Debug, Clone)]
pub(crate) struct AcceptedProviderOwnedReplaceEvidence {
    pub(crate) recovery_id: String,
    pub(crate) operation_state: String,
    pub(crate) preimage_sha256_observed: String,
    pub(crate) postimage_sha256: String,
    pub(crate) source_id: String,
    pub(crate) last_turn_id: String,
    pub(crate) last_used_at: String,
    pub(crate) records: Vec<crate::session_replace::CanonicalRecord>,
    pub(crate) plan: Value,
}

pub(crate) fn validate_changed_replace_result(
    identity: &ExternalSessionIdentity,
    session_id: &str,
    input: &PreparedReplaceInput,
    result: &SessionReplaceResult,
) -> Result<AcceptedProviderOwnedReplaceEvidence, ExternalSessionProviderError> {
    validate_replace_result_with_states(
        identity,
        session_id,
        input,
        result,
        &["prepared", "committed", "atomic_committed"],
    )
}

pub(crate) fn validate_recovery_replace_result(
    identity: &ExternalSessionIdentity,
    session_id: &str,
    input: &PreparedReplaceInput,
    result: &SessionReplaceResult,
) -> Result<AcceptedProviderOwnedReplaceEvidence, ExternalSessionProviderError> {
    validate_replace_result_with_states(
        identity,
        session_id,
        input,
        result,
        &["prepared", "committed", "atomic_committed", "rolled_back"],
    )
}

fn validate_replace_result_with_states(
    identity: &ExternalSessionIdentity,
    session_id: &str,
    input: &PreparedReplaceInput,
    result: &SessionReplaceResult,
    allowed_states: &[&str],
) -> Result<AcceptedProviderOwnedReplaceEvidence, ExternalSessionProviderError> {
    let operation_id = required_str(result.operation_id.as_deref(), "missing_operation_id")?;
    if operation_id != input.operation_id {
        return Err(map_provider_owned_token_error("operation_id_mismatch"));
    }
    let recovery_id = required_str(result.recovery_id.as_deref(), "missing_recovery_id")?;
    let operation_state =
        required_str(result.operation_state.as_deref(), "missing_operation_state")?;
    if !allowed_states.contains(&operation_state) {
        return Err(map_provider_owned_token_error("invalid_operation_state"));
    }
    let observed = required_str(
        result.preimage_sha256_observed.as_deref(),
        "missing_preimage_sha256_observed",
    )?;
    validate_hash_shape(observed, "invalid_preimage_sha256_observed")?;
    let postimage = required_str(
        result.postimage_sha256.as_deref(),
        "missing_canonical_postimage",
    )?;
    validate_hash_shape(postimage, "canonical_postimage_hash_mismatch")?;
    let canonical_postimage = result
        .canonical_postimage
        .as_ref()
        .ok_or_else(|| map_provider_owned_token_error("missing_canonical_postimage"))?;
    let (records, source_id) =
        validate_canonical_postimage(session_id, postimage, canonical_postimage)?;
    if canonical_postimage.turn_count != records.len() as u64 {
        return Err(map_provider_owned_token_error(
            "canonical_postimage_turn_count_mismatch",
        ));
    }
    let plan = result
        .host_state_plan
        .as_ref()
        .ok_or_else(|| map_provider_owned_token_error("invalid_host_state_plan"))?;
    let plan_context = HostStatePlanContext {
        identity,
        session_id,
        input,
        operation_id,
        recovery_id,
        observed,
        postimage,
        source_id: &source_id,
        turn_count: records.len() as u64,
    };
    let (last_turn_id, last_used_at) = validate_host_state_plan(plan, &plan_context)?;
    Ok(AcceptedProviderOwnedReplaceEvidence {
        recovery_id: recovery_id.to_string(),
        operation_state: operation_state.to_string(),
        preimage_sha256_observed: observed.to_string(),
        postimage_sha256: postimage.to_string(),
        source_id,
        last_turn_id,
        last_used_at,
        records,
        plan: plan.clone(),
    })
}

fn validate_canonical_postimage(
    session_id: &str,
    postimage: &str,
    canonical_postimage: &SessionReplaceCanonicalPostimage,
) -> Result<(Vec<crate::session_replace::CanonicalRecord>, String), ExternalSessionProviderError> {
    if canonical_postimage.format_id != CANONICAL_FORMAT {
        return Err(map_provider_owned_token_error(
            "missing_canonical_postimage",
        ));
    }
    if canonical_postimage.sha256 != postimage {
        return Err(map_postimage_hash_mismatch_error());
    }
    let Some(data_base64) = canonical_postimage.data_base64.as_deref() else {
        return Err(map_provider_owned_token_error(
            "missing_canonical_postimage",
        ));
    };
    let bytes = BASE64
        .decode(data_base64)
        .map_err(|_| map_provider_owned_token_error("missing_canonical_postimage"))?;
    let actual = sha256_hex(&bytes);
    if actual != canonical_postimage.sha256 {
        return Err(map_postimage_hash_mismatch_error());
    }
    let records = crate::session_replace::parse_provider_owned_canonical_input_for_session(
        session_id, &bytes,
    )
    .map_err(|_| map_invalid_artifact_error())?;
    Ok((records, canonical_postimage.source_id.clone()))
}

struct HostStatePlanContext<'a> {
    identity: &'a ExternalSessionIdentity,
    session_id: &'a str,
    input: &'a PreparedReplaceInput,
    operation_id: &'a str,
    recovery_id: &'a str,
    observed: &'a str,
    postimage: &'a str,
    source_id: &'a str,
    turn_count: u64,
}

fn validate_host_state_plan(
    plan: &Value,
    context: &HostStatePlanContext<'_>,
) -> Result<(String, String), ExternalSessionProviderError> {
    let Some(object) = plan.as_object() else {
        return Err(map_invalid_host_state_plan_error());
    };
    for required in [
        "schema_version",
        "operation",
        "replace_protocol",
        "operation_id",
        "recovery_id",
        "session_id",
        "provider_name",
        "canonical_format",
        "input_sha256",
        "postimage_sha256",
        "preimage_sha256_observed",
        "turn_count",
        "db_apply",
        "source_id",
        "last_turn_id",
        "last_used_at",
    ] {
        if !object.contains_key(required) {
            return Err(map_invalid_host_state_plan_error());
        }
    }
    validate_plan_integer(plan, "schema_version", 2, "invalid_host_state_plan")?;
    validate_plan_scalar(
        plan,
        "operation",
        "session.replace",
        "invalid_host_state_plan",
    )?;
    validate_plan_scalar(
        plan,
        "replace_protocol",
        PROVIDER_OWNED_REPLACE_PROTOCOL,
        "invalid_host_state_plan",
    )?;
    validate_plan_scalar(
        plan,
        "operation_id",
        context.operation_id,
        "host_state_plan_operation_id_mismatch",
    )?;
    validate_plan_scalar(
        plan,
        "recovery_id",
        context.recovery_id,
        "recovery_id_mismatch",
    )?;
    validate_plan_scalar(
        plan,
        "session_id",
        context.session_id,
        "invalid_host_state_plan",
    )?;
    validate_plan_scalar(
        plan,
        "provider_name",
        &context.identity.provider_name,
        "invalid_host_state_plan",
    )?;
    validate_plan_scalar(
        plan,
        "canonical_format",
        CANONICAL_FORMAT,
        "invalid_host_state_plan",
    )?;
    validate_plan_scalar(
        plan,
        "input_sha256",
        &context.input.records_sha256,
        "host_state_plan_input_mismatch",
    )?;
    validate_plan_scalar(
        plan,
        "postimage_sha256",
        context.postimage,
        "host_state_plan_postimage_mismatch",
    )?;
    validate_plan_scalar(
        plan,
        "preimage_sha256_observed",
        context.observed,
        "host_state_plan_preimage_mismatch",
    )?;
    validate_plan_integer(
        plan,
        "turn_count",
        context.turn_count,
        "invalid_host_state_plan",
    )?;
    validate_plan_scalar(
        plan,
        "db_apply",
        DB_APPLY_CAPABILITY,
        "invalid_host_state_plan",
    )?;
    validate_plan_scalar(
        plan,
        "source_id",
        context.source_id,
        "invalid_host_state_plan",
    )?;
    let last_turn_id = plan
        .get("last_turn_id")
        .and_then(Value::as_str)
        .ok_or_else(map_invalid_host_state_plan_error)?
        .to_string();
    let last_used_at = plan
        .get("last_used_at")
        .and_then(Value::as_str)
        .ok_or_else(map_invalid_host_state_plan_error)?
        .to_string();
    Ok((last_turn_id, last_used_at))
}

fn required_str<'a>(
    value: Option<&'a str>,
    token: &'static str,
) -> Result<&'a str, ExternalSessionProviderError> {
    value.ok_or_else(|| map_provider_owned_token_error(token))
}

fn validate_hash_shape(
    hash: &str,
    token: &'static str,
) -> Result<(), ExternalSessionProviderError> {
    if hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(map_provider_owned_token_error(token))
    }
}

fn validate_plan_scalar(
    plan: &Value,
    key: &str,
    expected: &str,
    token: &'static str,
) -> Result<(), ExternalSessionProviderError> {
    if plan.get(key).and_then(Value::as_str) == Some(expected) {
        Ok(())
    } else {
        Err(map_provider_owned_token_error(token))
    }
}

fn validate_plan_integer(
    plan: &Value,
    key: &str,
    expected: u64,
    token: &'static str,
) -> Result<(), ExternalSessionProviderError> {
    if plan.get(key).and_then(Value::as_u64) == Some(expected) {
        Ok(())
    } else {
        Err(map_provider_owned_token_error(token))
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;

    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}
