//! Role: orchestration.

use super::identity::ExternalSessionIdentity;
use super::replace_result_mapper::AcceptedProviderOwnedReplaceEvidence;
use crate::session_replace::{ProviderReplaceDbTarget, ReplaceError, ReplaceReceipt};
use std::path::PathBuf;

pub(crate) fn apply_provider_owned_replace_to_target(
    identity: &ExternalSessionIdentity,
    session_id: &str,
    accepted: &AcceptedProviderOwnedReplaceEvidence,
    target: &ProviderReplaceDbTarget,
) -> Result<ReplaceReceipt, ReplaceError> {
    validate_provider_owned_db_target(identity, session_id, accepted, target)?;
    let mut apply_target = target.clone();
    apply_target.source_file = accepted.source_id.clone();
    crate::session_replace::apply_provider_owned_replace_sqlite(&apply_target, &accepted.records)?;
    Ok(provider_owned_receipt(identity, session_id, accepted))
}

fn validate_provider_owned_db_target(
    identity: &ExternalSessionIdentity,
    session_id: &str,
    accepted: &AcceptedProviderOwnedReplaceEvidence,
    target: &ProviderReplaceDbTarget,
) -> Result<(), ReplaceError> {
    if target.provider_name != identity.provider_name || target.session_id != session_id {
        return Err(ReplaceError::OperationalError {
            message: "provider_db_identity_mismatch".to_string(),
        });
    }
    if accepted
        .plan
        .get("chain_id")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|chain_id| chain_id != target.chain_id)
    {
        return Err(ReplaceError::OperationalError {
            message: "provider_db_identity_mismatch".to_string(),
        });
    }
    if accepted
        .plan
        .get("active_segment_id")
        .and_then(serde_json::Value::as_i64)
        .is_some_and(|segment_id| segment_id != target.active_segment_id)
    {
        return Err(ReplaceError::OperationalError {
            message: "provider_db_identity_mismatch".to_string(),
        });
    }
    if accepted
        .plan
        .get("active_segment_started_at")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|started_at| started_at != target.active_segment_started_at)
    {
        return Err(ReplaceError::OperationalError {
            message: "provider_db_identity_mismatch".to_string(),
        });
    }
    Ok(())
}

pub(crate) fn provider_owned_receipt(
    identity: &ExternalSessionIdentity,
    session_id: &str,
    accepted: &AcceptedProviderOwnedReplaceEvidence,
) -> ReplaceReceipt {
    ReplaceReceipt {
        session_id: session_id.to_string(),
        provider_name: identity.provider_name.clone(),
        storage_type: "external_provider".to_string(),
        operation: "import-replace".to_string(),
        preimage_sha256: accepted.preimage_sha256_observed.clone(),
        postimage_sha256: accepted.postimage_sha256.clone(),
        jsonl_path: PathBuf::from(&accepted.source_id),
        state_updated: true,
        committed_at: chrono::Utc::now().to_rfc3339(),
    }
}
