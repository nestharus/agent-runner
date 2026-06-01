//! ## Declared roles
//! orchestration
//!
//! Option-A S7c rotation recovery hooks. Durable journal files are owned here;
//! provider dispatch only publishes records and asks this module to recover or
//! clean them up.

mod classifier;
mod error_formatter;
mod journal_predicates;
mod lock_cleanup;
mod lock_cleanup_predicates;
mod phase_mapper;
mod preimage_accessor;
mod preimage_mapper;
mod record_access;
mod record_formatter;
mod record_mapper;
mod record_orchestration;
mod record_parser;
mod recovery_executor;
mod recovery_mapper;
mod state_orchestration;
mod types;

use crate::rotation_domain::{ExternalRotationError, ExternalRotationIdentity};
use crate::services::MigrationServiceRequest;
use oulipoly_provider::generated::RotationMaterializeResult;
use std::path::{Path, PathBuf};

pub use lock_cleanup::{rotation_journal_lock_path, rotation_journal_path};
pub use types::{
    RotationJournalPhase, RotationJournalPreimage, RotationJournalRecord, RotationJournalState,
    RotationRecoveryPlan,
};

pub fn startup_recovery_before_provider_dispatch(
    request: &MigrationServiceRequest<'_>,
) -> Result<(), ExternalRotationError> {
    let path = rotation_journal_path(request.effective_cwd);
    let state = state_orchestration::read_rotation_journal_state(&path);
    let plan = recovery_mapper::build_rotation_recovery_plan(state);
    recovery_executor::execute_rotation_recovery_plan(request, &path, plan)
}

pub fn publish_after_artifact_record(
    request: &MigrationServiceRequest<'_>,
    identity: &ExternalRotationIdentity,
    result: &RotationMaterializeResult,
) -> Result<PathBuf, ExternalRotationError> {
    write_rotation_journal_record(
        request,
        RotationJournalPhase::CrashAfterArtifact,
        identity,
        result,
    )
}

pub fn publish_during_apply_record(
    request: &MigrationServiceRequest<'_>,
    identity: &ExternalRotationIdentity,
    result: &RotationMaterializeResult,
) -> Result<PathBuf, ExternalRotationError> {
    write_rotation_journal_record(
        request,
        RotationJournalPhase::CrashDuringApply,
        identity,
        result,
    )
}

pub fn cleanup_rotation_journal(root: &Path) -> Result<(), ExternalRotationError> {
    lock_cleanup::cleanup_rotation_journal(root)
}

pub fn write_rotation_journal_record(
    request: &MigrationServiceRequest<'_>,
    phase: RotationJournalPhase,
    identity: &ExternalRotationIdentity,
    result: &RotationMaterializeResult,
) -> Result<PathBuf, ExternalRotationError> {
    let path = rotation_journal_path(request.effective_cwd);
    let lock = rotation_journal_lock_path(request.effective_cwd);
    let continuing_locked_journal =
        phase == RotationJournalPhase::CrashDuringApply && path.exists() && lock.exists();
    if !continuing_locked_journal {
        lock_cleanup::acquire_rotation_lock(&lock)?;
    }
    let preimage = preimage_accessor::capture_rotation_preimage(request)?;
    let record = record_mapper::build_rotation_journal_record(phase, identity, preimage, result);
    record_orchestration::write_rotation_journal_record_to_path(&path, &record)?;
    Ok(path)
}

pub fn classify_rotation_journal_state(marker: Option<&str>) -> RotationJournalState {
    classifier::classify_rotation_journal_state(marker)
}

pub fn build_rotation_recovery_plan(state: RotationJournalState) -> RotationRecoveryPlan {
    recovery_mapper::build_rotation_recovery_plan(state)
}

pub fn execute_rotation_recovery_plan(
    request: &MigrationServiceRequest<'_>,
    path: &Path,
    plan: RotationRecoveryPlan,
) -> Result<(), ExternalRotationError> {
    recovery_executor::execute_rotation_recovery_plan(request, path, plan)
}

fn journal_recovery_failure(reason: impl Into<String>) -> ExternalRotationError {
    crate::rotation_domain::journal_recovery_failure(reason)
}
