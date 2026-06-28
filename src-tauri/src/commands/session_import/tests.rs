use super::formatter::{format_provider_report_line, format_totals_line};
use super::orchestration::session_import_targets;
use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, provider_implementation_ref::ProviderImplementationRef,
};
use oulipoly_runtime::services::{
    SessionImportProviderReport, SessionImportProviderStatus, SessionImportTotals,
};

#[test]
fn target_resolution_deduplicates_provider_accounts_and_supports_provider_filter() {
    let models = vec![
        external_model("model-a", &["provider-a", "provider-b"]),
        external_model("model-b", &["provider-a"]),
        builtin_model("builtin", &["provider-c"]),
    ];

    let all = session_import_targets(&models, None);
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].model_name, "model-a");
    assert_eq!(all[0].provider_name, "provider-a");
    assert_eq!(all[0].settings_id, "provider-a");
    assert_eq!(all[1].provider_name, "provider-b");

    let filtered = session_import_targets(&models, Some("provider-b"));
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].provider_name, "provider-b");
}

#[test]
fn target_resolution_supports_model_filter() {
    let models = vec![
        external_model("model-a", &["provider-a"]),
        external_model("model-b", &["provider-b"]),
    ];

    let filtered = session_import_targets(&models, Some("model-b"));
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
    let mut model = builtin_model(name, providers);
    model.provider = Some(ProviderImplementationRef {
        path: Some("/tmp/provider".to_string()),
        crate_name: None,
        version: None,
        binary: None,
        script: None,
    });
    model
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
