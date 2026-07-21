//! ## Declared roles
//!
//! `accessor`, `formatter`, `orchestration`, `predicate`

use std::path::{Path, PathBuf};

use oulipoly_config::{ModelConfig, ProviderConfig};
use oulipoly_runtime::balancer;
use oulipoly_runtime::services::{MigrationServiceOutput, ServiceError};

use super::execution::select_repl_direct_provider;
use super::{formatter, mapper};
use crate::migration_providers::ResumeExecutionEnvironment;
use crate::resume_cli::{
    ResumeExecutionTarget, format_resume_error, interactive_resume_execution_target,
    resume_migration_pool,
};
use crate::spawn_cwd::effective_resume_spawn_cwd;
use crate::wiring;

pub(super) struct ReplProviderSelectionInput<'a, 'ctx> {
    pub(super) agent_runtime_services: &'a wiring::AgentRuntimeServices,
    pub(super) env: &'a ResumeExecutionEnvironment,
    pub(super) model: &'a ModelConfig,
    pub(super) ctx: &'a balancer::BalanceContext<'ctx>,
    pub(super) resolved_resume: &'a mut Option<oulipoly_state::ResolvedResume>,
    pub(super) fallback_target: &'a mut Option<ResumeExecutionTarget>,
    pub(super) resume: Option<&'a str>,
    pub(super) manual_migrate: Option<&'a str>,
    pub(super) working_dir: Option<&'a Path>,
    pub(super) stderr_is_terminal: bool,
    pub(super) resume_spawn_cwd: &'a mut Option<PathBuf>,
}

pub(super) fn select_repl_provider(
    mut input: ReplProviderSelectionInput<'_, '_>,
) -> Result<Option<(usize, ProviderConfig, Option<String>)>, String> {
    if input.resolved_resume.is_none() {
        return select_repl_direct_provider(
            input.agent_runtime_services,
            input.model,
            input.env,
            input.ctx,
        )
        .map(Some);
    }
    let mut resolved = input
        .resolved_resume
        .take()
        .expect("resolved resume checked above");
    let selected = select_repl_resume_provider(&mut input, &mut resolved)?;
    *input.resolved_resume = Some(resolved);
    Ok(selected)
}

fn select_repl_resume_provider(
    input: &mut ReplProviderSelectionInput<'_, '_>,
    resolved: &mut oulipoly_state::ResolvedResume,
) -> Result<Option<(usize, ProviderConfig, Option<String>)>, String> {
    emit_selected_repl_resume_provider(input.stderr_is_terminal, &resolved.active_provider);
    if !validate_repl_resume_target(input.fallback_target.as_ref(), &resolved.active_provider) {
        return Ok(None);
    }
    if !migrate_repl_resume_provider(input, resolved)? {
        return Ok(None);
    }
    selected_repl_resume_provider_tuple(input.fallback_target.as_ref(), resolved)
}

fn emit_selected_repl_resume_provider(stderr_is_terminal: bool, selected_provider: &str) {
    if crate::dispatch::should_emit_resume_short_line(stderr_is_terminal) {
        formatter::emit_stderr(&format!("[resume] -> {selected_provider}"));
    }
}

fn validate_repl_resume_target(
    fallback_target: Option<&ResumeExecutionTarget>,
    selected_provider: &str,
) -> bool {
    if repl_resume_target_missing_resume(fallback_target) {
        emit_repl_missing_resume_block(selected_provider);
        return false;
    }
    true
}

fn repl_resume_target_missing_resume(fallback_target: Option<&ResumeExecutionTarget>) -> bool {
    fallback_target.is_some_and(|target| target.provider.resume.is_none())
}

fn selected_repl_resume_provider_tuple(
    fallback_target: Option<&ResumeExecutionTarget>,
    resolved: &oulipoly_state::ResolvedResume,
) -> Result<Option<(usize, ProviderConfig, Option<String>)>, String> {
    let target = fallback_target.expect("resume target must be resolved before spawn");
    if repl_resume_target_missing_resume(Some(target)) {
        emit_repl_missing_resume_block(&target.provider.name);
        return Ok(None);
    }
    let provider = target.provider.clone();
    Ok(Some(mapper::selected_repl_provider_tuple(
        target.provider_index,
        provider,
        Some(resolved.active_session_id.clone()),
    )))
}

fn emit_repl_missing_resume_block(provider_name: &str) {
    formatter::emit_stderr(&format!(
        "provider {provider_name} has no [providers.resume] block; cannot resume"
    ));
}

fn migrate_repl_resume_provider(
    input: &mut ReplProviderSelectionInput<'_, '_>,
    resolved: &mut oulipoly_state::ResolvedResume,
) -> Result<bool, String> {
    if should_skip_repl_provider_ref_default_migration(resolved, input.manual_migrate) {
        return Ok(true);
    }
    let migration = prepare_repl_migration(input, resolved)?;
    *input.resume_spawn_cwd = Some(migration.effective_spawn_cwd.clone());
    match dispatch_repl_migration(
        input,
        resolved,
        &migration.model,
        &migration.effective_spawn_cwd,
    ) {
        Ok(MigrationServiceOutput::Migrated { segment: migrated })
        | Ok(MigrationServiceOutput::AutoRotated {
            segment: migrated, ..
        }) => apply_repl_migrated_segment(input, resolved, migrated)?,
        Ok(MigrationServiceOutput::Stay) => {}
        Ok(MigrationServiceOutput::RotationFailed { reason }) => {
            return Ok(render_repl_rotation_failed(&reason));
        }
        Err(ServiceError::Dependency { message }) => {
            return Ok(render_repl_migration_dependency_failure(&message));
        }
        Err(err) => return Err(formatter::migration_service_failure(&err)),
    }
    Ok(true)
}

fn should_skip_repl_provider_ref_default_migration(
    resolved: &oulipoly_state::ResolvedResume,
    manual_migrate: Option<&str>,
) -> bool {
    manual_migrate.is_none()
        && resolved
            .model
            .as_ref()
            .is_some_and(|model| model.provider.is_some())
}

struct PreparedReplMigration {
    model: ModelConfig,
    effective_spawn_cwd: PathBuf,
}

fn prepare_repl_migration(
    input: &ReplProviderSelectionInput<'_, '_>,
    resolved: &oulipoly_state::ResolvedResume,
) -> Result<PreparedReplMigration, String> {
    Ok(PreparedReplMigration {
        model: resume_migration_pool(resolved, &input.env.providers_cfg),
        effective_spawn_cwd: effective_resume_spawn_cwd(
            &input.env.state,
            &input.env.providers_cfg,
            &input.env.sessions_cfg,
            resolved,
            input
                .resume
                .expect("resume input must exist for resolved resume"),
            input.working_dir,
        )?,
    })
}

fn dispatch_repl_migration(
    input: &mut ReplProviderSelectionInput<'_, '_>,
    resolved: &oulipoly_state::ResolvedResume,
    migration_model: &ModelConfig,
    effective_spawn_cwd: &Path,
) -> Result<MigrationServiceOutput, ServiceError> {
    let mut migration_stderr = std::io::stderr();
    input
        .agent_runtime_services
        .migration_service
        .migrate(mapper::migration_service_request(
            mapper::ReplMigrationRequestInput {
                env: input.env,
                resolved,
                manual_target: input.manual_migrate,
                migration_model,
                effective_cwd: effective_spawn_cwd,
                stderr: &mut migration_stderr,
            },
        ))
}

fn apply_repl_migrated_segment(
    input: &mut ReplProviderSelectionInput<'_, '_>,
    resolved: &mut oulipoly_state::ResolvedResume,
    migrated: oulipoly_runtime::migration::MigratedSegment,
) -> Result<(), String> {
    resolved.active_provider = migrated.target_provider;
    resolved.active_session_id = migrated.target_session_id;
    *input.fallback_target = Some(
        interactive_resume_execution_target(resolved, &input.env.providers_cfg)
            .map_err(format_resume_error)?,
    );
    Ok(())
}

fn render_repl_rotation_failed(reason: &oulipoly_runtime::services::RotationFailedReason) -> bool {
    formatter::emit_stderr(&formatter::rotation_failed_reason(reason));
    false
}

fn render_repl_migration_dependency_failure(message: &str) -> bool {
    formatter::emit_stderr(&formatter::migration_dependency_failure(message));
    false
}
