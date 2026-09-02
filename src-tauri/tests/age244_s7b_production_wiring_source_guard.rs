#[test]
fn s7b_production_export_and_import_replace_use_external_provider_identity_resolver() {
    let export_source =
        read_source("src-tauri/src/commands/session_locate_export/orchestration.rs");
    let export_body = source_between(
        &export_source,
        "pub(crate) fn run_session_export(",
        "fn unwrap_export_output",
    );
    assert_contains(
        "session_locate_export/orchestration.rs::run_session_export",
        export_body,
        "mapper::session_export_service_request(",
    );
    assert_contains(
        "session_locate_export/orchestration.rs::run_session_export",
        export_body,
        "crate::commands::session_external_provider_identity::resolve_session_external_provider_identity(",
    );
    assert_not_contains_compact(
        "session_locate_export/orchestration.rs::run_session_export",
        export_body,
        "session_export_service_request(session_id,None",
    );

    let replace_source =
        read_source("src-tauri/src/commands/session_import_replace/orchestration.rs");
    let replace_body = source_between(
        &replace_source,
        "pub(crate) fn run_session_import_replace(",
        "super::render_import_replace_output(output.result)",
    );
    assert_contains(
        "session_import_replace/orchestration.rs::run_session_import_replace",
        replace_body,
        "super::import_replace_request(",
    );
    assert_contains(
        "session_import_replace/orchestration.rs::run_session_import_replace",
        replace_body,
        "crate::commands::session_external_provider_identity::resolve_session_external_provider_identity(",
    );
    assert_not_contains_compact(
        "session_import_replace/orchestration.rs::run_session_import_replace",
        replace_body,
        "import_replace_request(session_id,from_file,preimage_sha256,None",
    );
}

#[test]
fn s7b_shared_identity_resolver_requires_persisted_explicit_account_authority() {
    let source = read_source("src-tauri/src/commands/session_external_provider_identity.rs");

    for needle in [
        "pub(crate) fn resolve_session_external_provider_identity(",
        "Result<Option<SessionServiceExternalProviderIdentity>, String>",
        "access_default_state_for_identity().map_err(identity_operational_error)?",
        "StateDb::open_default().map_err(",
        "ProvidersConfig::load(&default_config_root()?.join(\"providers.toml\"))",
        "load_models(&default_models_dir()?, Some(providers))\n        .map_err(",
        "match state.resolve_resume(models, session_id, None)",
        "Err(ResumeError::NoChainFound { .. })",
        "Err(ResumeError::WrongIdKind { .. })",
        "Err(ResumeError::Ambiguous { input, .. })",
        "SessionExternalProviderIdentityError::AmbiguousSession { input }",
        "Err(error) => Err(SessionExternalProviderIdentityError::Operational",
        "validate_external_provider_name(provider_name)?",
        "active_provider_session_authority(&resolved.chain_id)",
        "authority.as_ref()",
        "let Some(provider) = providers.get(provider_name)",
        "if provider.implementation.is_none()",
        "let authority = authority.ok_or_else(",
        "provider session has no persisted endpoint authority",
        "SessionServiceExternalProviderIdentity {",
        "provider_name: provider_name.to_string()",
        "provider_instance_id: Some(authority.provider_instance_id.clone())",
        "settings_id: authority.settings_id.clone()",
    ] {
        assert_contains("session_external_provider_identity.rs", &source, needle);
    }
    for forbidden in [
        "provider_registry_handle",
        "describe_model_provider",
        "DescribeResult",
        "ProviderClient",
        ".ok()",
        "unwrap_or_default()",
    ] {
        assert_not_contains("session_external_provider_identity.rs", &source, forbidden);
    }
}

#[test]
fn s7b_runtime_seam_enriches_identity_from_selected_account_endpoint() {
    let adapter = read_source("crates/oulipoly-runtime/src/session_external_provider/mod.rs");
    let identity = read_source("crates/oulipoly-runtime/src/session_external_provider/identity.rs");
    let formatter =
        read_source("crates/oulipoly-runtime/src/session_external_provider/identity_formatter.rs");

    assert_contains(
        "session_external_provider/mod.rs",
        &adapter,
        "provider_registry_accessor::preflight_provider(registry.as_ref(), &identity)",
    );
    assert_contains(
        "session_external_provider/mod.rs",
        &adapter,
        "let describe = endpoint.capabilities();",
    );
    assert_contains(
        "session_external_provider/mod.rs",
        &adapter,
        "let settings_id = endpoint",
    );
    assert_contains(
        "session_external_provider/mod.rs",
        &adapter,
        "identity::map_described_identity(identity, provider_instance_id, settings_id)",
    );
    assert_contains(
        "session_external_provider/identity.rs",
        &identity,
        "pub(crate) fn map_described_identity(",
    );
    assert_contains(
        "session_external_provider/identity_formatter.rs",
        &formatter,
        "format!(\"{provider_id}-instance\")",
    );
}

#[test]
fn s7b_command_surfaces_preserve_builtin_output_and_error_rendering() {
    let export_source =
        read_source("src-tauri/src/commands/session_locate_export/orchestration.rs");
    let export_body = source_between(
        &export_source,
        "pub(crate) fn run_session_export(",
        "fn unwrap_export_output",
    );
    for needle in [
        "SessionExternalProviderIdentityError::AmbiguousSession { input }",
        "SessionExternalProviderIdentityError::Operational { message }",
        "unwrap_export_output(service_output.result)",
        "write_session_export_output(&output)",
    ] {
        assert_contains(
            "session_locate_export/orchestration.rs::run_session_export",
            export_body,
            needle,
        );
    }
    assert_contains(
        "session_locate_export/orchestration.rs::unwrap_export_output",
        &export_source,
        "ExportOutputOutcome::Error(err) => Err(handle_export_error(&err))",
    );

    let replace_source =
        read_source("src-tauri/src/commands/session_import_replace/orchestration.rs");
    assert_contains(
        "session_import_replace/orchestration.rs::run_session_import_replace",
        &replace_source,
        "return super::render_import_replace_output(Err(replace_error_from_identity_error(",
    );
    assert_contains(
        "session_import_replace/orchestration.rs::replace_error_from_identity_error",
        &replace_source,
        "ReplaceError::SchemaIncompatible { reason: message }",
    );
    assert_contains(
        "session_import_replace/orchestration.rs::run_session_import_replace",
        &replace_source,
        "super::render_import_replace_output(output.result)",
    );
}

#[test]
fn s7b_cli_defaults_builds_populated_default_path_provider_registry() {
    let wiring = read_source("src-tauri/src/wiring.rs");
    let cli_defaults = source_between(
        &wiring,
        "pub fn cli_defaults() -> Result<Self, String>",
        "pub fn production(",
    );

    for needle in [
        "let paths = default_cli_runtime_paths()?;",
        "ProviderRegistryOptions::default()",
        ".with_config_root(paths.config_root.clone())",
        ".with_data_root(paths.data_root.clone())",
        "production_provider_registry(",
        "ProductionSessionExportService::with_registry_handle(",
        "ProductionSessionReplaceService::with_registry_handle(",
    ] {
        assert_contains("wiring.rs::cli_defaults", cli_defaults, needle);
    }
    assert_not_contains(
        "wiring.rs::cli_defaults",
        cli_defaults,
        "ProviderRegistry::empty(provider_registry_options.clone())",
    );
}

#[test]
fn production_provider_registries_use_only_account_configured_endpoint_authority() {
    let wiring = read_source("src-tauri/src/wiring.rs");
    let cli_defaults = source_between(
        &wiring,
        "pub fn cli_defaults() -> Result<Self, String>",
        "pub fn production(",
    );
    let production = source_between(
        &wiring,
        "pub fn production(",
        "fn default_cli_runtime_paths()",
    );

    for (context, source) in [
        ("wiring.rs::cli_defaults", cli_defaults),
        ("wiring.rs::production", production),
    ] {
        assert_not_contains(context, source, ".with_path_entries_from_process_path()");
    }

    let production_registry = source_between(
        &wiring,
        "fn production_provider_registry(",
        "fn load_registry_providers(",
    );
    assert_contains(
        "wiring.rs::production_provider_registry",
        production_registry,
        "let providers = load_registry_providers(paths)?;",
    );
    assert_contains(
        "wiring.rs::production_provider_registry",
        production_registry,
        "registry_from_configs(&models, &providers, options)",
    );
    assert_not_contains(
        "wiring.rs::production_provider_registry",
        production_registry,
        "ProviderRegistry::from_model_configs(",
    );

    let locate_export =
        read_source("src-tauri/src/commands/session_locate_export/orchestration.rs");
    assert_not_contains(
        "session_locate_export/orchestration.rs::build_session_locate_provider_registry",
        &locate_export,
        ".with_path_entries_from_process_path()",
    );
    assert_contains(
        "session_locate_export/orchestration.rs::build_session_locate_provider_registry",
        &locate_export,
        "ProviderRegistry::from_configs(",
    );

    let app_state = read_source("src-tauri/src/app_state.rs");
    assert_not_contains(
        "app_state.rs::provider_registry_options",
        &app_state,
        ".with_path_entries_from_process_path()",
    );
    assert_not_contains(
        "app_state.rs::provider settings construction",
        &app_state,
        "ProviderSettingsHost::from_model_configs",
    );
    assert_not_contains(
        "app_state.rs::provider settings construction",
        &app_state,
        "EMPTY_PROVIDER_SETTINGS_HOST_EXPECT_MESSAGE",
    );

    let provider_settings = read_source("src-tauri/src/commands/provider_settings.rs");
    assert_not_contains(
        "provider_settings.rs::host_options",
        &provider_settings,
        ".with_path_entries_from_process_path()",
    );
}

fn read_source(relative: &str) -> String {
    std::fs::read_to_string(workspace_root().join(relative))
        .unwrap_or_else(|error| panic!("failed to read {relative}: {error}"))
}

fn source_between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start_index = source_marker_index(source, start);
    let end_index = source_end_marker_index(source, start_index, start, end);
    source_slice_between(source, start_index, end_index)
}

fn source_marker_index(source: &str, marker: &str) -> usize {
    source
        .find(marker)
        .unwrap_or_else(|| panic!("missing start marker {marker:?}"))
}

fn source_end_marker_index(source: &str, start_index: usize, start: &str, end: &str) -> usize {
    source[start_index..]
        .find(end)
        .map(|offset| start_index + offset)
        .unwrap_or_else(|| panic!("missing end marker {end:?} after {start:?}"))
}

fn source_slice_between(source: &str, start_index: usize, end_index: usize) -> &str {
    &source[start_index..end_index]
}

fn assert_contains(context: &str, source: &str, needle: &str) {
    assert!(
        source.contains(needle),
        "{context} must keep S7b production external-provider identity wiring: missing {needle:?}"
    );
}

fn assert_not_contains(context: &str, source: &str, needle: &str) {
    assert!(
        !source.contains(needle),
        "{context} must not hard-code missing external-provider identity: found {needle:?}"
    );
}

fn assert_not_contains_compact(context: &str, source: &str, needle: &str) {
    let (compact_source, compact_needle) = compact_source_and_needle(source, needle);
    assert_compact_absent(context, &compact_source, &compact_needle, needle);
}

fn compact_source_and_needle(source: &str, needle: &str) -> (String, String) {
    (compact_whitespace(source), compact_whitespace(needle))
}

fn assert_compact_absent(context: &str, compact_source: &str, compact_needle: &str, needle: &str) {
    assert!(
        !compact_source.contains(compact_needle),
        "{context} must not hard-code missing external-provider identity: found {needle:?}"
    );
}

fn compact_whitespace(input: &str) -> String {
    input.split_whitespace().collect()
}

fn workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}
