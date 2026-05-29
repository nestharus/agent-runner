use std::fs;
use std::path::{Path, PathBuf};

// risk: Cross-language IPC drift; level: Tauri command registration; source: contract "Tauri IPC Contract"
#[test]
fn tauri_registers_exact_provider_settings_command_names() {
    let source = production_source("src/lib.rs", &read("src/lib.rs"));
    let missing = provider_settings_commands()
        .into_iter()
        .filter(|command| !source.contains(command))
        .collect::<Vec<_>>();

    assert!(
        missing.is_empty(),
        "missing provider settings Tauri command registrations: {missing:?}"
    );
}

// risk: Broad provider dispatch shortcut; level: Tauri command adapter; source: proposal "Tauri command design"
#[test]
fn tauri_provider_settings_handlers_are_thin_runtime_adapters() {
    let source = provider_settings_command_source();

    for forbidden in [
        "SchemaRequest",
        "SettingsListRequest",
        "SettingsGetRequest",
        "SettingsCreateRequest",
        "SettingsUpdateRequest",
        "SettingsDeleteRequest",
        "SettingsValidateRequest",
        "SettingsMigrateRequest",
        "invoke_typed",
        "ProviderClient::new",
    ] {
        assert!(
            !source.contains(forbidden),
            "Tauri settings handlers must not construct provider envelopes or clients directly: {forbidden}"
        );
    }

    for required in [
        "describe_settings_target",
        "settings_schema",
        "settings_list",
        "settings_get",
        "settings_create",
        "settings_update",
        "settings_delete",
        "settings_validate",
        "settings_migrate",
    ] {
        assert!(
            source.contains(required),
            "Tauri settings handlers must delegate to runtime settings host method {required}"
        );
    }
}

// risk: Registry/model reload staleness; level: Tauri state lifecycle; source: contract "Runtime state signals"
#[test]
fn model_reload_save_and_delete_refresh_configured_provider_settings_registry() {
    let source = read("src/lib.rs");

    for function_name in ["fn reload_models", "fn save_model_inner", "fn delete_model"] {
        let body = function_body(&source, function_name);
        assert!(
            body.contains("provider_settings") || body.contains("provider_registry"),
            "{function_name} must refresh the configured provider settings registry/service"
        );
        assert!(
            body.contains("from_model_configs") || body.contains("refresh"),
            "{function_name} must rebuild/replace registry state from the updated model map"
        );
    }
}

// risk: Migration mutating or interpreting central config; level: Tauri migration packaging; source: contract "Migration Contract"
#[test]
fn settings_migration_packaging_is_read_only_and_separate_from_central_config_migration() {
    let source = provider_settings_command_source();

    assert!(
        source.contains("legacy"),
        "settings.migrate must receive a legacy payload"
    );
    assert!(
        source.contains("dryRun") || source.contains("dry_run"),
        "settings.migrate must expose dry-run state"
    );
    for forbidden in [
        "migrate_config_files",
        "write_model",
        "save_model_inner",
        "delete_model",
        "fs::write",
        "std::fs::write",
    ] {
        assert!(
            !source.contains(forbidden),
            "settings.migrate packaging must not mutate central config or call central migration: {forbidden}"
        );
    }
}

// risk: Migration mutating or interpreting central config; level: source guard; source: contract "S5 must not retire central config"
#[test]
fn s5_does_not_retire_central_provider_or_model_config_parsing() {
    let model_source = read("../crates/oulipoly-config/src/model.rs");
    let providers_source = read("../crates/oulipoly-config/src/providers.rs");
    let tauri_source = read("src/lib.rs");

    for required in [
        "pub struct ModelConfig",
        "pub providers: Vec<ProviderConfig>",
        "pub provider: Option<ProviderImplementationRef>",
        "pub fn load_models",
        "pub fn render_validated_model_toml",
    ] {
        assert!(
            model_source.contains(required),
            "S5 must keep central model config parsing/rendering surface: {required}"
        );
    }

    for required in [
        "pub struct ProviderEntry",
        "pub struct ProvidersConfig",
        "pub fn load",
        "pub fn effective_provider",
        "pub fn runtime_provider",
    ] {
        assert!(
            providers_source.contains(required),
            "S5 must keep central providers.toml parsing/runtime surface: {required}"
        );
    }

    for required in [
        "load_providers_for_models_dir_with",
        "config::load_models",
        "config::render_validated_model_toml",
    ] {
        assert!(
            tauri_source.contains(required),
            "S5 settings CRUD must remain additive and keep central config consumers: {required}"
        );
    }
}

// risk: Provider diagnostics loss; level: Tauri IPC DTOs; source: contract "Structured IPC DTOs must expose"
#[test]
fn tauri_provider_settings_dtos_preserve_success_error_diagnostic_and_version_fields() {
    let source = provider_settings_command_source();

    for required in [
        "ProviderSettingsTarget",
        "ProviderSettingsSchema",
        "ProviderSettingsRecord",
        "ProviderSettingsError",
        "diagnostics",
        "category",
        "code",
        "message",
        "retryable",
        "details",
        "processStatus",
        "version",
        "schemaId",
        "displayName",
    ] {
        assert!(
            source.contains(required),
            "Tauri provider settings DTOs must preserve structured field {required}"
        );
    }
}

// risk: Production routing/quota/session cutover; level: source guard; source: contract "Out of scope"
#[test]
fn production_runtime_paths_do_not_route_through_provider_settings_dispatch() {
    let guarded_sources = [
        "../crates/oulipoly-runtime/src/balancer/mod.rs",
        "../crates/oulipoly-runtime/src/executor/mod.rs",
        "../crates/oulipoly-runtime/src/executor/cli.rs",
        "../crates/oulipoly-runtime/src/quota/in_flight.rs",
        "../crates/oulipoly-runtime/src/quota/mod.rs",
        "../crates/oulipoly-runtime/src/quota/outcome.rs",
        "../crates/oulipoly-runtime/src/quota/parse.rs",
        "../crates/oulipoly-runtime/src/quota/process.rs",
        "../crates/oulipoly-runtime/src/session_metadata/mod.rs",
        "../crates/oulipoly-runtime/src/sessions/mod.rs",
        "../crates/oulipoly-runtime/src/migration/mod.rs",
        "src/setup/flow.rs",
    ];
    let forbidden_terms = [
        "ProviderSettingsHost",
        "provider_settings",
        "settings_schema(",
        "settings_list(",
        "settings_get(",
        "settings_create(",
        "settings_update(",
        "settings_delete(",
        "settings_validate(",
        "settings_migrate(",
    ];

    let violations = guarded_sources
        .into_iter()
        .flat_map(|relative| source_term_violations(relative, &forbidden_terms))
        .collect::<Vec<_>>();
    assert!(
        violations.is_empty(),
        "production paths must not switch to provider settings dispatch: {violations:?}"
    );
}

// risk: Provider-specific vocabulary drift; level: source guard; source: contract "Out of scope"
#[test]
fn new_s5_provider_settings_surfaces_use_neutral_vocabulary() {
    let candidate_files = [
        "src/commands/provider_settings.rs",
        "src/commands/provider_settings/mod.rs",
        "../src/lib/providerSettings.ts",
        "../src/components/provider-settings/ProviderSettingsPanel.tsx",
        "../src/components/provider-settings/JsonSchemaRenderer.tsx",
        "../src/views/ProviderSettingsView.tsx",
        "../e2e/provider-settings.spec.ts",
    ];
    let forbidden_terms = [
        "claude",
        "codex",
        "gemini",
        "opencode",
        "anthropic",
        "openai",
    ];

    let violations = candidate_files
        .into_iter()
        .filter_map(optional_source)
        .flat_map(|(relative, source)| {
            lowercase_term_violations(relative, source, &forbidden_terms)
        })
        .collect::<Vec<_>>();
    assert!(
        violations.is_empty(),
        "new S5 provider settings fixtures/copy must remain neutral: {violations:?}"
    );
}

// risk: oulipoly-provider dependency independence; level: source guard; source: proposal "Forbidden behavior"
#[test]
fn oulipoly_provider_dependency_independence_remains_intact_for_s5() {
    let manifest = read("../crates/oulipoly-provider/Cargo.toml");
    for forbidden in [
        "oulipoly-runtime",
        "oulipoly-config",
        "oulipoly-state",
        "oulipoly-agent-runner",
    ] {
        assert!(
            !manifest.contains(forbidden),
            "oulipoly-provider must remain independent of host/runtime crates for S5: {forbidden}"
        );
    }
}

fn provider_settings_commands() -> Vec<&'static str> {
    vec![
        "list_provider_settings_targets",
        "get_provider_settings_schema",
        "list_provider_settings",
        "get_provider_settings",
        "create_provider_settings",
        "update_provider_settings",
        "delete_provider_settings",
        "validate_provider_settings",
        "migrate_provider_settings",
    ]
}

fn provider_settings_command_source() -> String {
    let candidates = [
        "src/commands/provider_settings.rs",
        "src/commands/provider_settings/mod.rs",
        "src/lib.rs",
    ];
    candidates
        .into_iter()
        .find_map(|relative| {
            let path = manifest_path(relative);
            path.exists()
                .then(|| production_source(relative, &fs::read_to_string(path).unwrap()))
        })
        .expect("provider settings Tauri command source should exist")
}

fn production_source(relative: &str, source: &str) -> String {
    if relative == "src/lib.rs" {
        source
            .split("\n#[cfg(test)]\nmod tests")
            .next()
            .unwrap_or(source)
            .to_string()
    } else {
        source.to_string()
    }
}

fn source_term_violations(relative: &'static str, forbidden_terms: &[&'static str]) -> Vec<String> {
    let source = read(relative);
    forbidden_terms
        .iter()
        .copied()
        .filter(|term| source.contains(term))
        .map(|term| format!("{relative}:{term}"))
        .collect()
}

fn optional_source(relative: &'static str) -> Option<(&'static str, String)> {
    let path = manifest_path(relative);
    path.exists()
        .then(|| (relative, fs::read_to_string(path).unwrap().to_lowercase()))
}

fn lowercase_term_violations(
    relative: &'static str,
    source: String,
    forbidden_terms: &[&'static str],
) -> Vec<String> {
    forbidden_terms
        .iter()
        .copied()
        .filter(|term| source.contains(term))
        .map(|term| format!("{relative}:{term}"))
        .collect()
}

fn function_body<'a>(source: &'a str, marker: &str) -> &'a str {
    let start = source
        .find(marker)
        .unwrap_or_else(|| panic!("missing function marker {marker}"));
    let after = &source[start..];
    let next_fn = after[marker.len()..]
        .find("\nfn ")
        .map(|offset| marker.len() + offset)
        .unwrap_or(after.len());
    &after[..next_fn]
}

fn read(relative: &str) -> String {
    fs::read_to_string(manifest_path(relative))
        .unwrap_or_else(|error| panic!("failed to read {relative}: {error}"))
}

fn manifest_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}
