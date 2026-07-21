//! ## Declared roles
//!
//! `filter`, `mapper`, `orchestration`, `predicate`

use std::path::Path;

use oulipoly_runtime::services::{MigrationServiceOutput, ServiceError};

use super::execution::{
    provider_ref_resume_block_exit_code, resolved_uses_provider_ref,
    validate_provider_ref_headless_resume_target,
};
use super::{filter, formatter, mapper};
use crate::migration_providers::ResumeExecutionEnvironment;
use crate::quota_zero_turn::filter_quota_exhausted_migration_candidates;
use crate::resume_cli::{
    ResumeExecutionTarget, renderable_resume_execution_target, resume_migration_pool,
};
use crate::wiring;

pub(super) fn migrate_resume_target(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    env: &ResumeExecutionEnvironment,
    resolved: &mut oulipoly_state::ResolvedResume,
    target: &mut ResumeExecutionTarget,
    manual_migrate: Option<&str>,
    attempts: usize,
    effective_spawn_cwd: &Path,
) -> Result<(), i32> {
    if validate_provider_ref_default_migration_skip(resolved, target, manual_migrate)? {
        return Ok(());
    }
    let migration_model = migration_model_for_attempt(env, resolved, manual_migrate, attempts);
    let migration_result = dispatch_resume_migration(
        agent_runtime_services,
        env,
        resolved,
        manual_migrate,
        attempts,
        effective_spawn_cwd,
        &migration_model,
    );
    apply_resume_migration_result(env, resolved, target, migration_result)
}

fn validate_provider_ref_default_migration_skip(
    resolved: &oulipoly_state::ResolvedResume,
    target: &ResumeExecutionTarget,
    manual_migrate: Option<&str>,
) -> Result<bool, i32> {
    if !should_skip_provider_ref_default_migration(resolved, manual_migrate) {
        return Ok(false);
    }
    validate_provider_ref_headless_resume_target(resolved, target, &resolved.active_provider)
        .map_err(|message| provider_ref_resume_block_exit_code(&message))?;
    Ok(true)
}

fn should_skip_provider_ref_default_migration(
    resolved: &oulipoly_state::ResolvedResume,
    manual_migrate: Option<&str>,
) -> bool {
    manual_migrate.is_none() && resolved_uses_provider_ref(resolved)
}

fn migration_model_for_attempt(
    env: &ResumeExecutionEnvironment,
    resolved: &oulipoly_state::ResolvedResume,
    manual_migrate: Option<&str>,
    attempts: usize,
) -> oulipoly_config::ModelConfig {
    let mut migration_model = resume_migration_pool(resolved, &env.providers_cfg);
    if manual_migrate.is_none() || attempts > 1 {
        filter_quota_exhausted_migration_candidates(
            &env.state,
            &mut migration_model,
            &resolved.active_provider,
        );
    }
    migration_model
}

fn dispatch_resume_migration(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    env: &ResumeExecutionEnvironment,
    resolved: &oulipoly_state::ResolvedResume,
    manual_migrate: Option<&str>,
    attempts: usize,
    effective_spawn_cwd: &Path,
    migration_model: &oulipoly_config::ModelConfig,
) -> Result<MigrationServiceOutput, ServiceError> {
    let mut migration_stderr = std::io::stderr();
    agent_runtime_services
        .migration_service
        .migrate(mapper::migration_service_request(
            mapper::ResumeMigrationRequestInput {
                env,
                resolved,
                manual_target: filter::first_attempt_manual_migrate(attempts, manual_migrate),
                active_exhausted: false,
                migration_model,
                effective_cwd: effective_spawn_cwd,
                stderr: &mut migration_stderr,
            },
        ))
}

fn apply_resume_migration_result(
    env: &ResumeExecutionEnvironment,
    resolved: &mut oulipoly_state::ResolvedResume,
    target: &mut ResumeExecutionTarget,
    migration_result: Result<MigrationServiceOutput, ServiceError>,
) -> Result<(), i32> {
    match migration_result {
        Ok(MigrationServiceOutput::Migrated { segment: migrated })
        | Ok(MigrationServiceOutput::AutoRotated {
            segment: migrated, ..
        }) => {
            resolved.active_provider = migrated.target_provider;
            resolved.active_session_id = migrated.target_session_id;
            *target = renderable_resume_execution_target(resolved, &env.providers_cfg)?;
            Ok(())
        }
        Ok(MigrationServiceOutput::Stay) => Ok(()),
        Ok(MigrationServiceOutput::RotationFailed { reason }) => {
            formatter::emit_stderr(&formatter::rotation_failed_reason(&reason));
            Err(1)
        }
        Err(ServiceError::Dependency { message }) => {
            formatter::emit_migration_dependency_failure(&message);
            Err(1)
        }
        Err(err) => {
            formatter::emit_migration_service_failure(&err);
            Err(1)
        }
    }
}
