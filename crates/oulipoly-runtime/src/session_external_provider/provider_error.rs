//! Role: mapper.

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExternalSessionProviderError {
    pub(super) token: &'static str,
    pub(super) detail: String,
}

pub(crate) fn map_registry_missing_error() -> ExternalSessionProviderError {
    map_provider_error(
        "provider_registry_missing",
        "provider registry was not wired",
    )
}

pub(crate) fn map_capability_missing_error() -> ExternalSessionProviderError {
    map_provider_error(
        "session_capability_missing",
        "provider describe did not advertise session capability",
    )
}

pub(crate) fn map_schema_invalid_request_error() -> ExternalSessionProviderError {
    map_provider_error(
        "schema_invalid_request",
        "provider request could not serialize",
    )
}

pub(crate) fn map_invalid_canonical_format_error() -> ExternalSessionProviderError {
    map_provider_error("invalid_canonical_format", "unexpected canonical format")
}

pub(crate) fn map_invalid_base64_error() -> ExternalSessionProviderError {
    map_provider_error("invalid_base64", "provider returned invalid base64")
}

pub(crate) fn map_hash_mismatch_error() -> ExternalSessionProviderError {
    map_provider_error(
        "hash_mismatch",
        "provider canonical hash did not match bytes",
    )
}

pub(crate) fn map_canonical_parse_count_mismatch_error() -> ExternalSessionProviderError {
    map_provider_error(
        "canonical_parse_count_mismatch",
        "canonical JSONL parse/count validation failed",
    )
}

pub(crate) fn map_turn_count_mismatch_error() -> ExternalSessionProviderError {
    map_provider_error(
        "turn_count_mismatch",
        "provider turn_count did not match records",
    )
}

pub(crate) fn map_postimage_hash_mismatch_error() -> ExternalSessionProviderError {
    map_provider_error(
        "canonical_postimage_hash_mismatch",
        "provider postimage hash did not match accepted facts",
    )
}

pub(crate) fn map_provider_owned_token_error(token: &'static str) -> ExternalSessionProviderError {
    map_provider_error(token, token)
}

pub(crate) fn map_invalid_artifact_error() -> ExternalSessionProviderError {
    map_provider_error(
        "invalid_artifact",
        "provider returned invalid artifact facts",
    )
}

pub(crate) fn map_invalid_host_state_plan_error() -> ExternalSessionProviderError {
    map_provider_error(
        "invalid_host_state_plan",
        "provider returned invalid host state plan",
    )
}

fn map_provider_error(
    token: &'static str,
    detail: impl Into<String>,
) -> ExternalSessionProviderError {
    ExternalSessionProviderError {
        token,
        detail: detail.into(),
    }
}
