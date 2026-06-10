//! ## Declared roles
//!
//! `orchestration`, `mapper`, `validator`.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/balancer/migration/tests.rs::migration_decision_fixture_adapter
//!     role: adapter
//!     Translates:
//!       - migration decision fixtures into config, projection, and state assertions
//! ```

use super::super::projection::WindowProjection;
use super::*;
use oulipoly_config::{SessionStorage, model::PromptMode};
use std::path::PathBuf;

#[test]
fn decide_manual_migration_emits_target_not_in_pool_rejection() {
    let model = migratable_model(&[("alpha", "project_storage"), ("beta", "project_storage")]);
    let resolved = resolved_for(&model, 0);
    let result = decide_manual_migration(&model, &resolved, "alpha-missing");
    match result {
        Err(ManualMigrationRejection::TargetNotInPool { target, pool }) => {
            assert_eq!(target, "alpha-missing");
            assert_eq!(pool, vec!["alpha".to_string(), "beta".to_string()]);
        }
        other => panic!("expected TargetNotInPool, got {other:?}"),
    }
}

#[test]
fn decide_manual_migration_emits_single_provider_pool_rejection() {
    let model = migratable_model(&[("alpha", "project_storage")]);
    let resolved = resolved_for(&model, 0);
    let result = decide_manual_migration(&model, &resolved, "alpha");
    assert!(matches!(
        result,
        Err(ManualMigrationRejection::SingleProviderPool { ref provider })
            if provider == "alpha"
    ));
}

#[test]
fn decide_manual_migration_emits_active_not_in_pool_rejection() {
    let model = migratable_model(&[("alpha", "project_storage"), ("beta", "project_storage")]);
    let mut resolved = resolved_for(&model, 0);
    resolved.active_provider = "archived".to_string();
    let result = decide_manual_migration(&model, &resolved, "beta");
    assert!(matches!(
        result,
        Err(ManualMigrationRejection::ActiveProviderNotInPool { ref active })
            if active == "archived"
    ));
}

#[test]
fn decide_manual_migration_emits_not_migratable_pair_rejection() {
    let model = migratable_model(&[("alpha", "project_storage"), ("beta", "none")]);
    let resolved = resolved_for(&model, 0);
    let result = decide_manual_migration(&model, &resolved, "beta");
    match result {
        Err(ManualMigrationRejection::NotMigratablePair { source, target }) => {
            assert_eq!(source, "alpha");
            assert_eq!(target, "beta");
        }
        other => panic!("expected NotMigratablePair, got {other:?}"),
    }
}

#[test]
fn decide_manual_migration_rejects_unparseable_script_storage_target() {
    let model = migratable_model(&[("alpha", "project_storage"), ("beta", "custom_script")]);
    let resolved = resolved_for(&model, 0);
    let result = decide_manual_migration(&model, &resolved, "beta");

    assert!(matches!(
        result,
        Err(ManualMigrationRejection::NotMigratablePair { ref source, ref target })
            if source == "alpha" && target == "beta"
    ));
}

#[test]
fn provider_load_falls_back_to_zero_without_finite_window_projection() {
    let projection = ProviderProjection {
        provider_index: 0,
        projections_per_window: vec![
            WindowProjection {
                window_id: 0,
                projected_used: f64::NAN,
                hours_until_reset: 2.0,
                remaining_headroom: 0.0,
            },
            WindowProjection {
                window_id: 1,
                projected_used: f64::INFINITY,
                hours_until_reset: 4.0,
                remaining_headroom: 0.0,
            },
        ],
        binding_score: Some(1.0),
        recent_error_count: 0,
    };

    assert_eq!(provider_load(&projection), 0.0);
}

fn migratable_model(provider_names: &[(&str, &str)]) -> ModelConfig {
    let providers = provider_names
        .iter()
        .map(|(name, storage_kind)| migratable_provider(name, storage_kind))
        .collect();
    ModelConfig {
        name: "migration-fixture".to_string(),
        prompt_mode: PromptMode::Arg,
        providers,
        inputs: Vec::new(),
        provider: None,
    }
}

fn migratable_provider(name: &str, storage_kind: &str) -> ProviderConfig {
    ProviderConfig {
        name: name.to_string(),
        command: name.to_string(),
        args: Vec::new(),
        interactive_args: Some(vec!["launch".to_string()]),
        resume: Some(resume_strategy_for_test()),
        session_capture: None,
        resume_acceptance: None,
        session_storage: session_storage_for_test(name, storage_kind),
        system_prompt_override: None,
        tool_restrictions: None,
        invocation_mode: Default::default(),
    }
}

fn resume_strategy_for_test() -> oulipoly_config::ResumeStrategy {
    oulipoly_config::ResumeStrategy {
        kind: oulipoly_config::ResumeKind::Flag,
        flag: Some("--resume".to_string()),
        subcommand: None,
    }
}

fn session_storage_for_test(name: &str, storage_kind: &str) -> Option<SessionStorage> {
    match storage_kind {
        "project_storage" => Some(project_storage_storage_for_test(name)),
        "custom_script" => Some(custom_script_storage_for_test()),
        "none" => None,
        other => panic!("unknown storage kind fixture {other}"),
    }
}

fn project_storage_storage_for_test(name: &str) -> SessionStorage {
    SessionStorage::ClaudeCode {
        projects_dir: PathBuf::from(format!("/tmp/{name}/projects")),
    }
}

fn custom_script_storage_for_test() -> SessionStorage {
    SessionStorage::Script {
        cwd_script: "custom-cwd /tmp/custom/projects".to_string(),
        transcript_script: Some("custom-locate-transcript /tmp/custom/projects".to_string()),
        storage_type: Some(oulipoly_config::ScriptSessionStorageType::ClaudeCode),
    }
}

fn resolved_for(model: &ModelConfig, provider_index: usize) -> ResolvedResume {
    let provider = &model.providers[provider_index];
    ResolvedResume {
        chain_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string(),
        model_name: Some(model.name.clone()),
        model: Some(model.clone()),
        active_provider: provider.name.clone(),
        active_session_id: "5169694d-de0f-40d1-890c-6e28e55bab27".to_string(),
    }
}
