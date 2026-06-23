//! Role: formatter.

use super::identity::ExternalSessionIdentity;
use super::replace_input_mapper::PreparedReplaceInput;
use crate::session_replace::ReplaceReceipt;
use std::path::PathBuf;

pub(crate) fn no_change_receipt(
    identity: ExternalSessionIdentity,
    session_id: &str,
    input: PreparedReplaceInput,
) -> ReplaceReceipt {
    ReplaceReceipt {
        session_id: session_id.to_string(),
        provider_name: identity.provider_name,
        storage_type: "external_provider".to_string(),
        operation: "import-replace".to_string(),
        preimage_sha256: input.preimage_sha256_expected.clone().unwrap_or_default(),
        postimage_sha256: input.preimage_sha256_expected.unwrap_or_default(),
        jsonl_path: PathBuf::new(),
        state_updated: false,
        committed_at: chrono::Utc::now().to_rfc3339(),
    }
}
