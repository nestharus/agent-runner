//! ## Declared roles
//! orchestration, mapper, formatter
//!
//! Migration service output mapper. It preserves the service-level distinction
//! between nonfatal migration decisions and infrastructure failures.

use super::dtos::{MigrationServiceOutput, MigrationServiceRequest, RotationFailedReason};
use super::error::ServiceError;
use crate::balancer::{
    FailureClass, ManualMigrationRejection, MigrationDecision, TransitionReason,
    apply_post_failure_forensics, decide_manual_migration, select_next_working_candidate,
};

pub(super) fn migrate(
    request: MigrationServiceRequest<'_>,
) -> Result<MigrationServiceOutput, ServiceError> {
    // AGE-163 WU-A.5: explicit manual-target requests route through the
    // typed `decide_manual_migration`. Rejections surface as the typed
    // `RotationFailed { ManualTarget* }` variants the caller renders as
    // operator-visible diagnostics.
    if let Some(target) = request.manual_target {
        return migrate_manual(request, target);
    }
    match decide_service_migration(&request) {
        Ok(MigrationDecision::Stay) => Ok(MigrationServiceOutput::Stay),
        Err(err) => Err(ServiceError::Dependency {
            message: format!("{err:?}"),
        }),
        Ok(MigrationDecision::Migrate {
            target_provider_index,
            reason,
        }) => run_service_migration(request, target_provider_index, reason),
    }
}

fn migrate_manual(
    mut request: MigrationServiceRequest<'_>,
    target: &str,
) -> Result<MigrationServiceOutput, ServiceError> {
    match decide_manual_migration(request.migration_model, request.resolved, target) {
        Ok(MigrationDecision::Stay) => Ok(MigrationServiceOutput::Stay),
        Ok(MigrationDecision::Migrate {
            target_provider_index,
            reason,
        }) => match attempt_migration(&mut request, target_provider_index, reason) {
            Ok(segment) => Ok(MigrationServiceOutput::Migrated { segment }),
            Err(err) => Err(ServiceError::Dependency {
                message: format!("{err:?}"),
            }),
        },
        Err(rejection) => Ok(MigrationServiceOutput::RotationFailed {
            reason: rejection_to_rotation_failed(rejection),
        }),
    }
}

fn rejection_to_rotation_failed(rejection: ManualMigrationRejection) -> RotationFailedReason {
    match rejection {
        ManualMigrationRejection::SingleProviderPool { provider } => {
            RotationFailedReason::ManualTargetIsSingleProviderPool { provider }
        }
        ManualMigrationRejection::ActiveProviderNotInPool { active } => {
            RotationFailedReason::ManualTargetActiveNotInPool { active }
        }
        ManualMigrationRejection::TargetNotInPool { target, pool } => {
            RotationFailedReason::ManualTargetNotInPool { target, pool }
        }
        ManualMigrationRejection::NotMigratablePair { source, target } => {
            RotationFailedReason::ManualTargetNotMigratable { source, target }
        }
    }
}

fn decide_service_migration(
    request: &MigrationServiceRequest<'_>,
) -> Result<MigrationDecision, crate::migration::MigrationError> {
    crate::balancer::decide_migration(
        request.state,
        request.migration_model,
        request.resolved,
        request.manual_target,
    )
}

fn run_service_migration(
    mut request: MigrationServiceRequest<'_>,
    initial_target_index: usize,
    reason: TransitionReason,
) -> Result<MigrationServiceOutput, ServiceError> {
    let auto_rotate_path =
        request.manual_target.is_none() && reason == TransitionReason::QuotaThreshold;
    let first = attempt_migration(&mut request, initial_target_index, reason);
    match first {
        Ok(segment) => Ok(MigrationServiceOutput::Migrated { segment }),
        Err(
            err @ (crate::migration::MigrationError::SourceMissingStorage { .. }
            | crate::migration::MigrationError::SourceMissing { .. }),
        ) if auto_rotate_path => {
            iterate_working_set_candidates(&mut request, initial_target_index, reason, err)
        }
        Err(err) => Err(ServiceError::Dependency {
            message: format!("{err:?}"),
        }),
    }
}

fn attempt_migration(
    request: &mut MigrationServiceRequest<'_>,
    target_provider_index: usize,
    reason: TransitionReason,
) -> Result<crate::migration::MigratedSegment, crate::migration::MigrationError> {
    crate::migration::migrate_chain_segment(
        request.state,
        request.sessions_cfg,
        request.migration_model,
        request.resolved,
        request.effective_cwd,
        target_provider_index,
        reason,
        request.stderr,
    )
}

fn iterate_working_set_candidates(
    request: &mut MigrationServiceRequest<'_>,
    initial_target_index: usize,
    reason: TransitionReason,
    initial_error: crate::migration::MigrationError,
) -> Result<MigrationServiceOutput, ServiceError> {
    let mut candidates_tried = Vec::new();
    record_failed_candidate(
        request,
        initial_target_index,
        &initial_error,
        &mut candidates_tried,
    );
    let now = chrono::Utc::now();
    let mut last_failure_index = initial_target_index;
    loop {
        let next = select_next_working_candidate(
            request.state,
            request.migration_model,
            now,
            Some(last_failure_index),
        )
        .map_err(|err| ServiceError::Dependency {
            message: format!("{err:?}"),
        })?;
        let Some(candidate_index) = next else {
            return Ok(MigrationServiceOutput::RotationFailed {
                reason: RotationFailedReason::WorkingSetExhausted { candidates_tried },
            });
        };
        match attempt_migration(request, candidate_index, reason) {
            Ok(segment) => {
                return Ok(MigrationServiceOutput::AutoRotated {
                    segment,
                    candidates_tried,
                });
            }
            Err(
                err @ (crate::migration::MigrationError::SourceMissingStorage { .. }
                | crate::migration::MigrationError::SourceMissing { .. }),
            ) => {
                record_failed_candidate(request, candidate_index, &err, &mut candidates_tried);
                last_failure_index = candidate_index;
            }
            Err(err) => {
                return Err(ServiceError::Dependency {
                    message: format!("{err:?}"),
                });
            }
        }
    }
}

fn record_failed_candidate(
    request: &MigrationServiceRequest<'_>,
    candidate_index: usize,
    err: &crate::migration::MigrationError,
    candidates_tried: &mut Vec<String>,
) {
    let provider_name = request.migration_model.providers[candidate_index]
        .name
        .clone();
    let _ = apply_post_failure_forensics(
        request.state,
        &provider_name,
        FailureClass::UpstreamApiDown,
        chrono::Utc::now(),
    );
    candidates_tried.push(provider_name);
    let _ = err;
}
