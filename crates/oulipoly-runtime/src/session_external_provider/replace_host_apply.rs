//! Role: orchestration.

use super::replace_input_mapper::PreparedReplaceInput;
use super::replace_result_mapper::AcceptedReplaceResult;
use crate::session_replace::{ExternalProviderReplaceTransaction, ReplaceError, ReplaceReceipt};

pub(crate) fn verify_provider_transformed_replace(
    session_id: &str,
    input: &PreparedReplaceInput,
    accepted: &AcceptedReplaceResult,
) -> Result<(), ReplaceError> {
    let expected_records =
        crate::session_replace::parse_external_provider_postimage_canonical_input(&input.bytes)?;
    let metadata = crate::session_replace::access_external_provider_replace_metadata(session_id)?;
    let artifact_metadata =
        crate::session_replace::map_external_provider_postimage_artifact_metadata(
            metadata,
            &accepted.artifact_path,
        );
    let actual_postimage_sha256 =
        crate::session_replace::access_external_provider_artifact_canonical_hash(
            &artifact_metadata,
        )?;
    crate::session_replace::validate_external_provider_postimage_hash(
        &actual_postimage_sha256,
        &accepted.postimage_sha256,
    )?;
    let actual_records =
        crate::session_replace::access_external_provider_artifact_canonical_records(
            &artifact_metadata,
        )?;
    crate::session_replace::validate_external_provider_postimage_semantics(
        &expected_records,
        &actual_records,
    )
}

pub(crate) fn apply_verified_provider_transformed_replace(
    transaction: ExternalProviderReplaceTransaction,
    accepted: AcceptedReplaceResult,
) -> Result<ReplaceReceipt, ReplaceError> {
    crate::session_replace::commit_external_provider_replace(
        transaction,
        &accepted.postimage_sha256,
    )
}
