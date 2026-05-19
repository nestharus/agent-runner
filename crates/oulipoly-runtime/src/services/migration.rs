//! ## Declared roles
//! orchestration, mapper, formatter
//!
//! Migration service output mapper. It preserves the service-level distinction
//! between nonfatal migration decisions and infrastructure failures.

use super::dtos::{MigrationServiceOutput, MigrationServiceRequest};
use super::error::ServiceError;
use crate::balancer::MigrationDecision;

pub(super) fn migrate(
    request: MigrationServiceRequest<'_>,
) -> Result<MigrationServiceOutput, ServiceError> {
    match decide_service_migration(&request) {
        Ok(MigrationDecision::Stay) => Ok(MigrationServiceOutput::Stay),
        Err(err) => Ok(migration_decision_failed_output(err)),
        Ok(MigrationDecision::Migrate {
            target_provider_index,
            reason,
        }) => run_service_migration(request, target_provider_index, reason),
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

fn migration_decision_failed_output(
    err: crate::migration::MigrationError,
) -> MigrationServiceOutput {
    MigrationServiceOutput::DecisionFailed {
        warning: format!("{err:?}"),
    }
}

fn run_service_migration(
    request: MigrationServiceRequest<'_>,
    target_provider_index: usize,
    reason: crate::balancer::TransitionReason,
) -> Result<MigrationServiceOutput, ServiceError> {
    let result = crate::migration::migrate_chain_segment(
        request.state,
        request.sessions_cfg,
        request.migration_model,
        request.resolved,
        request.effective_cwd,
        target_provider_index,
        reason,
        request.stderr,
    );
    match result {
        Ok(segment) => Ok(MigrationServiceOutput::Migrated { segment }),
        Err(
            err @ (crate::migration::MigrationError::SourceMissingStorage { .. }
            | crate::migration::MigrationError::SourceMissing { .. }),
        ) if request.manual_target.is_none()
            && reason == crate::balancer::TransitionReason::QuotaThreshold =>
        {
            Ok(MigrationServiceOutput::DecisionFailed {
                warning: format!("{err:?}"),
            })
        }
        Err(err) => Err(ServiceError::Dependency {
            message: format!("{err:?}"),
        }),
    }
}
