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
use crate::resume_cli::{ResumeExecutionTarget, resume_migration_pool};
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
    if resume_notification_must_stay_bound(crate::wake_coordinator::is_auto_wake_invocation()) {
        return Ok(());
    }
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

fn resume_notification_must_stay_bound(is_auto_wake: bool) -> bool {
    is_auto_wake
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
    if should_filter_migration_candidates(manual_migrate, attempts) {
        filter_quota_exhausted_migration_candidates(
            &env.state,
            &mut migration_model,
            &resolved.active_provider,
        );
    }
    migration_model
}

fn should_filter_migration_candidates(manual_migrate: Option<&str>, attempts: usize) -> bool {
    manual_migrate.is_none() || attempts > 1
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
        }) => mapper::apply_migrated_resume_segment(resolved, target, migrated, &env.providers_cfg),
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

#[cfg(test)]
mod tests {
    use super::resume_notification_must_stay_bound;

    #[test]
    fn notification_wake_stays_bound_while_manual_resume_can_migrate() {
        assert!(resume_notification_must_stay_bound(true));
        assert!(!resume_notification_must_stay_bound(false));
    }

    // Relocated AGE123 policy obligation. This calls the real migration entry
    // through its existing service port; no claim or native authority is seeded.
    // Environment is per-child so parallel tests never share auto-wake markers.
    #[test]
    fn notification_auto_wake_stays_bound_despite_explicit_rotation_target() {
        const MODE: &str = "AGE123_POLICY_CHILD";
        if let Ok(mode) = std::env::var(MODE) {
            check_explicit_rotation_policy(mode == "auto");
            return;
        }
        for mode in ["auto", "manual"] {
            let root = tempfile::tempdir().unwrap();
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env(MODE, mode);
            for (key, sub) in [
                ("HOME", "home"),
                ("CODEX_HOME", "codex"),
                ("XDG_CONFIG_HOME", "config"),
                ("XDG_DATA_HOME", "xdg-data"),
                ("XDG_STATE_HOME", "state"),
                ("XDG_CACHE_HOME", "cache"),
                ("XDG_RUNTIME_DIR", "runtime"),
                ("OULIPOLY_DATA_DIR", "data"),
                ("TMPDIR", "tmp"),
            ] {
                let path = root.path().join(sub);
                std::fs::create_dir_all(&path).unwrap();
                command.env(key, path);
            }
            if mode == "auto" {
                command.env("OULIPOLY_AUTO_WAKE", "1");
            }
            let output = command.args([
                "--exact",
                "run::resume::migration::tests::notification_auto_wake_stays_bound_despite_explicit_rotation_target",
                "--nocapture",
            ]).output().unwrap();
            assert!(output.status.success(), "{mode}: {output:?}");
            println!(
                "policy child mode={mode} status={} stdout={:?} stderr={:?}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let stdout = String::from_utf8(output.stdout).unwrap();
            assert!(
                stdout.contains("test result: ok. 1 passed; 0 failed"),
                "{stdout}"
            );
            assert!(
                stdout.contains(&format!(
                    "policy auto={} calls={}",
                    mode == "auto",
                    usize::from(mode == "manual")
                )),
                "{stdout}"
            );
            println!("isolated {mode}: real migration-entry service discrimination passed");
        }
    }

    fn check_explicit_rotation_policy(auto: bool) {
        use oulipoly_runtime::services::{
            MigrationServiceOutput, MigrationServicePort, MigrationServiceRequest, ServiceError,
        };
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        struct MustNotMigrate(AtomicUsize);
        impl MigrationServicePort for MustNotMigrate {
            fn migrate(
                &self,
                request: MigrationServiceRequest<'_>,
            ) -> Result<MigrationServiceOutput, ServiceError> {
                assert_eq!(request.manual_target, Some("provider-b"));
                assert_eq!(request.resolved.active_provider, "provider-a");
                self.0.fetch_add(1, Ordering::SeqCst);
                Err(ServiceError::Dependency {
                    message: "recording migration service reached".into(),
                })
            }
        }
        let root = tempfile::tempdir().unwrap();
        let env = super::ResumeExecutionEnvironment {
            state: oulipoly_state::StateDb::open(&root.path().join("state.db")).unwrap(),
            providers_cfg: Default::default(),
            models: Default::default(),
            sessions_cfg: Default::default(),
            config_root: root.path().join("config"),
            models_dir: root.path().join("models"),
        };
        let mut services = crate::wiring::AgentRuntimeServices::cli_defaults().unwrap();
        let recorder = Arc::new(MustNotMigrate(AtomicUsize::new(0)));
        services.migration_service = recorder.clone();
        let mut resolved = oulipoly_state::ResolvedResume {
            chain_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into(),
            model_name: None,
            model: None,
            active_provider: "provider-a".into(),
            active_session_id: "5169694d-de0f-40d1-890c-6e28e55bab27".into(),
        };
        let mut provider =
            oulipoly_config::ProviderConfig::new("/never-executed-policy-fixture", vec![]);
        provider.name = "provider-a".into();
        let mut target = super::ResumeExecutionTarget {
            model: None,
            provider_index: 0,
            provider,
            prompt_mode: oulipoly_config::PromptMode::Arg,
        };
        let result = super::migrate_resume_target(
            &services,
            &env,
            &mut resolved,
            &mut target,
            Some("provider-b"),
            1,
            root.path(),
        );
        assert_eq!(result, if auto { Ok(()) } else { Err(1) });
        assert_eq!(recorder.0.load(Ordering::SeqCst), usize::from(!auto));
        assert_eq!(resolved.active_provider, "provider-a");
        assert_eq!(
            resolved.active_session_id,
            "5169694d-de0f-40d1-890c-6e28e55bab27"
        );
        assert_eq!(target.provider.name, "provider-a");
        println!(
            "policy auto={auto} calls={}",
            recorder.0.load(Ordering::SeqCst)
        );
    }
}
