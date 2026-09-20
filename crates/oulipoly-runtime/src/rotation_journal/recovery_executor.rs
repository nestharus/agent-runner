//! ## Declared roles
//! orchestration, accessor, validator, predicate, formatter

use super::journal_recovery_failure;
use super::lock_cleanup::{cleanup_journal_and_lock, remove_record_artifacts};
use super::record_orchestration::read_rotation_journal_record;
use super::types::RotationRecoveryPlan;
use crate::rotation_domain::ExternalRotationError;
use crate::services::MigrationServiceRequest;
use std::path::Path;

pub(super) fn execute_rotation_recovery_plan(
    request: &MigrationServiceRequest<'_>,
    path: &Path,
    plan: RotationRecoveryPlan,
) -> Result<(), ExternalRotationError> {
    match plan {
        RotationRecoveryPlan::Noop => Ok(()),
        RotationRecoveryPlan::RollBack => recover_after_artifact(request, path),
        RotationRecoveryPlan::RollForward => recover_during_apply(request, path),
        RotationRecoveryPlan::Quarantine { reason } => Err(journal_recovery_failure(reason)),
    }
}

fn recovery_target_session(
    record: &crate::rotation_journal::RotationJournalRecord,
) -> Option<&str> {
    record
        .result
        .host_state_plan
        .get("target_session_id")
        .and_then(serde_json::Value::as_str)
}

fn recover_after_artifact(
    request: &MigrationServiceRequest<'_>,
    path: &Path,
) -> Result<(), ExternalRotationError> {
    let record = read_rotation_journal_record(path)?;
    let _migration_fence = request
        .state
        .begin_completed_turn_journal_recovery(
            &record.preimage.chain_id,
            &record.identity.target_provider,
            recovery_target_session(&record),
        )
        .map_err(journal_recovery_failure)?;
    let Some(root) = path.parent() else {
        return cleanup_journal_and_lock(path);
    };
    remove_record_artifacts(root, &record.result.artifacts)?;
    cleanup_journal_and_lock(path)
}

fn recover_during_apply(
    request: &MigrationServiceRequest<'_>,
    path: &Path,
) -> Result<(), ExternalRotationError> {
    let record = read_rotation_journal_record(path)?;
    let migration_fence = request
        .state
        .begin_completed_turn_journal_recovery(
            &record.preimage.chain_id,
            &record.identity.target_provider,
            recovery_target_session(&record),
        )
        .map_err(journal_recovery_failure)?;
    crate::rotation_host_apply::verify_rotation_artifacts(&record.result.artifacts)
        .map_err(crate::rotation_domain::artifact_verification_failure)?;
    if crate::rotation_host_apply::rotation_already_applied(
        request,
        &record.identity,
        &record.result,
    )?
    .is_none()
    {
        crate::rotation_host_apply::validate_host_state_plan_with_fence(
            &record.result.host_state_plan,
            &record.result.artifacts,
            request,
            &record.identity,
            &migration_fence,
        )?;
        crate::rotation_host_apply::apply_chain_segment_transaction(
            request,
            &record.identity,
            &record.result,
        )?;
    }
    cleanup_journal_and_lock(path)
}
