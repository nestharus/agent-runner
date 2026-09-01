use super::formatter::{format_provider_report_line, format_totals_line};
use super::orchestration::session_import_targets;
use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ProviderEndpointConfig, ProviderEntry, ProvidersConfig,
};
use oulipoly_runtime::provider_registry::{ProviderRegistry, ProviderRegistryOptions};
use oulipoly_runtime::services::{
    SessionImportProviderReport, SessionImportProviderStatus, SessionImportTotals,
};
use std::collections::HashMap;

#[test]
fn target_resolution_deduplicates_provider_accounts_and_supports_provider_filter() {
    let models = vec![
        external_model("model-a", &["provider-a", "provider-b"]),
        external_model("model-b", &["provider-a"]),
        builtin_model("builtin", &["provider-c"]),
    ];
    let registry = registry_from_models_and_accounts(
        &models,
        &[
            ("provider-a", "/tmp/native-a", "/tmp/provider"),
            ("provider-b", "/tmp/native-b", "/tmp/provider"),
        ],
    );

    let all = session_import_targets(&models, &registry, None);
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].model_name, "model-a");
    assert_eq!(all[0].provider_name, "provider-a");
    assert_eq!(all[0].settings_id, "provider-a");
    assert_eq!(all[1].provider_name, "provider-b");

    let filtered = session_import_targets(&models, &registry, Some("provider-b"));
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].provider_name, "provider-b");
}

#[test]
fn target_resolution_includes_models_without_top_level_provider_refs() {
    let models = vec![builtin_model("opencode-test", &["opencode", "opencode2"])];
    let registry = registry_from_models_and_accounts(
        &models,
        &[
            ("opencode", "opencode1", "/tmp/agent-runner-opencode"),
            ("opencode2", "opencode2", "/tmp/agent-runner-opencode"),
        ],
    );

    let all = session_import_targets(&models, &registry, None);

    assert_eq!(all.len(), 2);
    assert_eq!(all[0].model_name, "opencode-test");
    assert_eq!(all[0].provider_name, "opencode");
    assert_eq!(all[1].provider_name, "opencode2");

    let filtered = session_import_targets(&models, &registry, Some("opencode"));
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].provider_name, "opencode");
}

#[test]
fn target_resolution_maps_binary_instance_slots_to_session_provider_binary() {
    let models = vec![builtin_model("opencode-test", &["opencode", "opencode2"])];
    let registry = registry_from_models_and_accounts(
        &models,
        &[
            ("opencode", "opencode1", "/tmp/agent-runner-opencode"),
            ("opencode2", "opencode2", "/tmp/agent-runner-opencode"),
        ],
    );

    assert_eq!(
        registry.artifact_key_for_model_provider("opencode-test", "opencode"),
        Some("path:/tmp/agent-runner-opencode".to_string())
    );
    assert_eq!(
        registry.artifact_key_for_model_provider("opencode-test", "opencode2"),
        Some("path:/tmp/agent-runner-opencode".to_string())
    );
}

#[test]
fn target_resolution_does_not_treat_path_like_commands_as_endpoint_authority() {
    let models = vec![builtin_model("local-shim", &["provider-a"])];
    let registry = registry_from_models_and_accounts(
        &models,
        &[(
            "provider-a",
            "/tmp/provider-instance-shim",
            "/tmp/explicit-provider-endpoint",
        )],
    );

    assert_eq!(
        registry.artifact_key_for_model_provider("local-shim", "provider-a"),
        Some("path:/tmp/explicit-provider-endpoint".to_string())
    );
}

#[test]
fn target_resolution_supports_model_filter() {
    let models = vec![
        external_model("model-a", &["provider-a"]),
        external_model("model-b", &["provider-b"]),
    ];
    let registry = registry_from_models_and_accounts(
        &models,
        &[
            ("provider-a", "/tmp/native-a", "/tmp/provider"),
            ("provider-b", "/tmp/native-b", "/tmp/provider"),
        ],
    );

    let filtered = session_import_targets(&models, &registry, Some("model-b"));
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].model_name, "model-b");
    assert_eq!(filtered[0].provider_name, "provider-b");
}

#[test]
fn formatter_renders_provider_and_totals_lines() {
    let provider = SessionImportProviderReport {
        model_name: "model\na".to_string(),
        provider_name: "provider\ta".to_string(),
        settings_id: "settings\ra".to_string(),
        status: SessionImportProviderStatus::Skipped {
            reason: "no\ncapability".to_string(),
        },
        discovered: 2,
        imported: 1,
        skipped: 1,
        errors: vec!["err".to_string()],
        warnings: vec!["warn".to_string(), "warn2".to_string()],
        turns_backfilled: 3,
    };

    assert_eq!(
        format_provider_report_line(&provider),
        "provider=provider a model=model a settings_id=settings a status=skipped(no capability) discovered=2 imported=1 skipped=1 errors=1 warnings=2 turns_backfilled=3"
    );
    assert_eq!(
        format_totals_line(&SessionImportTotals {
            providers_total: 2,
            providers_succeeded: 1,
            providers_skipped: 1,
            providers_failed: 0,
            discovered: 2,
            imported: 1,
            skipped: 1,
            errors: 0,
            warnings: 2,
            turns_backfilled: 3,
        }),
        "totals providers=2 succeeded=1 skipped_providers=1 failed=0 discovered=2 imported=1 skipped_sessions=1 errors=0 warnings=2 turns_backfilled=3"
    );
}

fn external_model(name: &str, providers: &[&str]) -> ModelConfig {
    builtin_model(name, providers)
}

fn builtin_model(name: &str, providers: &[&str]) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        prompt_mode: PromptMode::Arg,
        providers: providers
            .iter()
            .map(|provider| ProviderConfig::model_provider(*provider, Vec::new()))
            .collect(),
        inputs: Vec::new(),
        provider: None,
    }
}

fn registry_from_models_and_accounts(
    models: &[ModelConfig],
    accounts: &[(&str, &str, &str)],
) -> ProviderRegistry {
    let providers = providers_config(accounts);
    ProviderRegistry::from_configs(models, &providers, ProviderRegistryOptions::default())
        .expect("registry should construct from explicit account endpoints")
}

fn providers_config(accounts: &[(&str, &str, &str)]) -> ProvidersConfig {
    ProvidersConfig {
        entries: accounts
            .iter()
            .map(|(name, command, executable)| {
                let entry = ProviderEntry {
                    implementation: Some(ProviderEndpointConfig {
                        family: "session-import-fixture".to_string(),
                        executable: (*executable).to_string(),
                    }),
                    command: Some((*command).to_string()),
                    ..Default::default()
                };
                ((*name).to_string(), entry)
            })
            .collect::<HashMap<_, _>>(),
    }
}
