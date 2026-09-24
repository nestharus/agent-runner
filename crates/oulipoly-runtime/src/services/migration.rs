//! ## Declared roles
//! orchestration, predicate, mapper, formatter
//!
//! Migration service output mapper. It preserves the service-level distinction
//! between nonfatal migration decisions and infrastructure failures.

mod candidate_failure_orchestration;
mod candidate_failure_state_access;
mod error_formatter;
mod external_branch_orchestration;
mod external_identity_accessor;
mod external_target_validator;
mod provider_name_accessor;
mod rejection_mapper;

use super::dtos::{MigrationServiceOutput, MigrationServiceRequest, RotationFailedReason};
use super::error::ServiceError;
use crate::balancer::{
    MigrationDecision, TransitionReason, decide_manual_migration, select_next_working_candidate,
};
use crate::provider_registry::ProviderRegistryHandle;

pub(super) fn migrate(
    request: MigrationServiceRequest<'_>,
    provider_registry: Option<&ProviderRegistryHandle>,
) -> Result<MigrationServiceOutput, ServiceError> {
    let external_declared = external_branch_orchestration::model_declares_external_provider(
        &request,
        provider_registry,
    );
    // A durable journal belongs to an older operation. Recover it under its
    // own record-derived fence before any capability probe or new-operation
    // fence, avoiding both provider-before-recovery and nested file locks.
    if external_declared {
        crate::rotation_journal::startup_recovery_before_provider_dispatch(&request)
            .map_err(error_formatter::construct_migration_service_error)?;
    }

    // A model-level implementation reference is already an authentic external
    // declaration. Select its target from local configuration/State, then hold
    // provider-wide protection before provider identity/capability discovery.
    // The selected target is passed forward unchanged so a fair-cursor target
    // cannot drift between the fence and identity resolution.
    if request.migration_model.provider.is_some() {
        let target_provider = external_identity_accessor::select_external_target_provider(
            &request,
            provider_registry,
        )
        .map_err(error_formatter::construct_migration_service_error)?;
        let migration_fence = request
            .state
            .begin_completed_turn_migration(
                request.resolved,
                &target_provider,
                None,
                oulipoly_state::CompletedTurnMigrationScope::ExternalProviderWide,
                oulipoly_state::CompletedTurnMigrationStage::ExternalBeforeProvider,
            )
            .map_err(|message| ServiceError::Dependency { message })?;
        let identity = external_identity_accessor::resolve_external_provider_identity_for_target(
            &request,
            provider_registry,
            &target_provider,
        )
        .map_err(error_formatter::construct_migration_service_error)?;
        return crate::rotation_external_provider::materialize_rotation_with_fence(
            provider_registry.expect("external identity requires registry"),
            identity,
            &request,
            &migration_fence,
        )
        .map_err(error_formatter::construct_migration_service_error);
    }

    match external_branch_orchestration::select_migration_branch(&request, provider_registry)? {
        external_branch_orchestration::MigrationBranch::BuiltIn => {
            migrate_built_in(request, provider_registry)
        }
        external_branch_orchestration::MigrationBranch::External { identity } => {
            // Account endpoints must first advertise rotation. A legacy endpoint
            // that returns rotation=false reaches built-in exact-session behavior
            // without a false provider-wide refusal. Once external rotation is
            // selected, this fence covers all materialization/journal/host-State
            // effects without retaining a SQLite writer.
            let migration_fence = request
                .state
                .begin_completed_turn_migration(
                    request.resolved,
                    &identity.target_provider,
                    None,
                    oulipoly_state::CompletedTurnMigrationScope::ExternalProviderWide,
                    oulipoly_state::CompletedTurnMigrationStage::ExternalBeforeProvider,
                )
                .map_err(|message| ServiceError::Dependency { message })?;
            crate::rotation_external_provider::materialize_rotation_with_fence(
                provider_registry.expect("external identity requires registry"),
                identity,
                &request,
                &migration_fence,
            )
            .map_err(error_formatter::construct_migration_service_error)
        }
    }
}

fn migrate_built_in(
    request: MigrationServiceRequest<'_>,
    provider_registry: Option<&ProviderRegistryHandle>,
) -> Result<MigrationServiceOutput, ServiceError> {
    // AGE-163 WU-A.5: explicit manual-target requests route through the
    // typed `decide_manual_migration`. Rejections surface as the typed
    // `RotationFailed { ManualTarget* }` variants the caller renders as
    // operator-visible diagnostics.
    if let Some(target) = request.manual_target {
        return migrate_manual(request, target, provider_registry);
    }
    match decide_service_migration(&request) {
        Ok(MigrationDecision::Stay) => Ok(MigrationServiceOutput::Stay),
        Err(err) => Err(error_formatter::migration_dependency_error(err)),
        Ok(MigrationDecision::Migrate {
            target_provider_index,
            reason,
        }) => run_service_migration(request, target_provider_index, reason, provider_registry),
    }
}

fn migrate_manual(
    mut request: MigrationServiceRequest<'_>,
    target: &str,
    provider_registry: Option<&ProviderRegistryHandle>,
) -> Result<MigrationServiceOutput, ServiceError> {
    match decide_manual_migration(request.migration_model, request.resolved, target) {
        Ok(MigrationDecision::Stay) => Ok(MigrationServiceOutput::Stay),
        Ok(MigrationDecision::Migrate {
            target_provider_index,
            reason,
        }) => match attempt_migration(
            &mut request,
            target_provider_index,
            reason,
            provider_registry,
        ) {
            Ok(segment) => Ok(MigrationServiceOutput::Migrated { segment }),
            Err(err) => Err(error_formatter::migration_dependency_error(err)),
        },
        Err(rejection) => Ok(MigrationServiceOutput::RotationFailed {
            reason: rejection_mapper::rejection_to_rotation_failed(rejection),
        }),
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
    provider_registry: Option<&ProviderRegistryHandle>,
) -> Result<MigrationServiceOutput, ServiceError> {
    let auto_rotate_path =
        request.manual_target.is_none() && reason == TransitionReason::QuotaThreshold;
    let first = attempt_migration(
        &mut request,
        initial_target_index,
        reason,
        provider_registry,
    );
    match first {
        Ok(segment) => Ok(MigrationServiceOutput::Migrated { segment }),
        Err(
            err @ (crate::migration::MigrationError::SourceMissingStorage { .. }
            | crate::migration::MigrationError::SourceMissing { .. }),
        ) if auto_rotate_path => iterate_working_set_candidates(
            &mut request,
            initial_target_index,
            reason,
            err,
            provider_registry,
        ),
        Err(err) => Err(error_formatter::migration_dependency_error(err)),
    }
}

fn attempt_migration(
    request: &mut MigrationServiceRequest<'_>,
    target_provider_index: usize,
    reason: TransitionReason,
    provider_registry: Option<&ProviderRegistryHandle>,
) -> Result<crate::migration::MigratedSegment, crate::migration::MigrationError> {
    let target = &request.migration_model.providers[target_provider_index].name;
    let authority =
        target_provider_authority(provider_registry, &request.resolved.active_provider, target)?;
    crate::migration::migrate_chain_segment_with_target_authority(
        request.state,
        request.sessions_cfg,
        request.migration_model,
        request.resolved,
        request.effective_cwd,
        target_provider_index,
        reason,
        authority.as_ref(),
        request.stderr,
    )
}

fn target_provider_authority(
    provider_registry: Option<&ProviderRegistryHandle>,
    source: &str,
    target: &str,
) -> Result<Option<oulipoly_state::StoredProviderSessionAuthority>, crate::migration::MigrationError>
{
    let Some(registry) = provider_registry.map(ProviderRegistryHandle::current) else {
        return Ok(None);
    };
    if !registry.has_account_endpoint(source) && !registry.has_account_endpoint(target) {
        return Ok(None);
    }
    if !registry.has_account_endpoint(target) {
        return Err(
            crate::migration::MigrationError::TargetAuthorityUnavailable {
                provider: target.into(),
                message: "selected target has no configured provider endpoint".into(),
            },
        );
    }
    let endpoint = registry.preflight_account(target).map_err(|error| {
        crate::migration::MigrationError::TargetAuthorityUnavailable {
            provider: target.into(),
            message: error.to_string(),
        }
    })?;
    let identity = endpoint.endpoint_identity().map_err(|message| {
        crate::migration::MigrationError::TargetAuthorityUnavailable {
            provider: target.into(),
            message,
        }
    })?;
    Ok(Some(oulipoly_state::StoredProviderSessionAuthority {
        provider_instance_id: identity.provider_instance_id,
        settings_id: identity.settings_id,
    }))
}

fn iterate_working_set_candidates(
    request: &mut MigrationServiceRequest<'_>,
    initial_target_index: usize,
    reason: TransitionReason,
    initial_error: crate::migration::MigrationError,
    provider_registry: Option<&ProviderRegistryHandle>,
) -> Result<MigrationServiceOutput, ServiceError> {
    let mut candidates_tried = Vec::new();
    candidate_failure_orchestration::record_failed_candidate(
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
        .map_err(error_formatter::migration_dependency_error)?;
        let Some(candidate_index) = next else {
            return Ok(MigrationServiceOutput::RotationFailed {
                reason: RotationFailedReason::WorkingSetExhausted { candidates_tried },
            });
        };
        match attempt_migration(request, candidate_index, reason, provider_registry) {
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
                candidate_failure_orchestration::record_failed_candidate(
                    request,
                    candidate_index,
                    &err,
                    &mut candidates_tried,
                );
                last_failure_index = candidate_index;
            }
            Err(err) => {
                return Err(error_formatter::migration_dependency_error(err));
            }
        }
    }
}
