use agent_runner_lib::AppState;
use std::collections::HashMap;

#[test]
fn app_state_db_path_returns_models_parent_state_db() {
    let dir = tempfile::tempdir().unwrap();
    let models_dir = dir.path().join("models");
    let state = AppState::test_default(models_dir, HashMap::new());

    assert_eq!(state.db_path(), dir.path().join("state.db"));
}

#[test]
fn app_state_constructor_and_fallback_surface_remain_anchored() {
    let raw_source = include_str!("../src/app_state.rs");
    let source = compact(raw_source);

    assert!(
        raw_source.contains(
            "const EMPTY_PROVIDER_SETTINGS_HOST_EXPECT_MESSAGE: &str =\n    \"empty provider settings host should build\";"
        ),
        "provider-settings empty-host panic payload must remain static and byte-preserving"
    );
    for required in [
        "fnprovider_settings_host_for_models(models_dir:&Path,models:&HashMap<String,config::ModelConfig>,)->oulipoly_runtime::provider_settings::ProviderSettingsHost",
        "provider_settings::build_host(models_dir,models).unwrap_or_else(|_|",
        "ProviderSettingsHost::from_model_configs(&[],provider_settings::host_options(models_dir),).expect(EMPTY_PROVIDER_SETTINGS_HOST_EXPECT_MESSAGE)",
    ] {
        assert!(
            source.contains(required),
            "provider-settings fallback host construction must stay anchored: {required}"
        );
    }

    for constructor in [
        "pub(crate)fnnew(",
        "pubfntest_default(",
        "pubfnwith_services(",
    ] {
        let body = function_body(&source, constructor);
        assert!(
            body.contains("provider_settings_host_for_models(&models_dir,&models)"),
            "{constructor} must initialize provider settings from the configured model map"
        );
        assert!(
            body.contains("quota_in_flight:oulipoly_runtime::quota::InFlight::new()"),
            "{constructor} must initialize quota in-flight state"
        );
        assert!(
            body.contains("provider_settings:Mutex::new(provider_settings)"),
            "{constructor} must install the provider settings host"
        );
        assert!(
            body.contains("setup_input_tx:Mutex::new(None)"),
            "{constructor} must initialize setup response channel state"
        );
    }

    let new_body = function_body(&source, "pub(crate)fnnew(");
    for required in [
        "state_db_opener:Arc<dynStateDbOpener+Send+Sync>=services.state_db_opener.clone()",
        "providers_config:Arc<dynProvidersConfigRepository+Send+Sync>=services.providers_config.clone()",
        "routing_service:Arc<dynoulipoly_runtime::services::RoutingServicePort>=services.routing_service.clone()",
        "quota_service:Arc::clone(&services.quota_service)",
        "executor_service:Arc::clone(&services.executor_service)",
        "diagnostics_service:Arc::clone(&services.diagnostics_service)",
    ] {
        assert!(
            new_body.contains(required),
            "AppState::new must preserve production service wiring: {required}"
        );
    }

    let test_default_body = function_body(&source, "pubfntest_default(");
    for required in [
        "state_db_opener:Arc::new(ProductionStateDbOpener)",
        "providers_config:Arc::new(FilesystemProvidersConfigRepository)",
        "quota_service:Arc::new(oulipoly_runtime::quota::RuntimeQuotaService)",
        "executor_service:Arc::new(oulipoly_runtime::executor::RuntimeExecutorService::with_registry_handle(provider_registry.clone()",
        "diagnostics_service:Arc::new(oulipoly_runtime::diagnostics::RuntimeDiagnosticsService)",
        "provider_registry",
    ] {
        assert!(
            test_default_body.contains(required),
            "AppState::test_default must preserve default service wiring: {required}"
        );
    }

    let with_services_body = function_body(&source, "pubfnwith_services(");
    for required in [
        "state_db_opener:services.state_db_opener",
        "providers_config:services.providers_config",
        "quota_service:services.quota_service",
        "executor_service:services.executor_service",
        "diagnostics_service:services.diagnostics_service",
        "setup_repository:Some(services.setup_repository)",
    ] {
        assert!(
            with_services_body.contains(required),
            "AppState::with_services must preserve test-double service wiring: {required}"
        );
    }
}

fn compact(source: &str) -> String {
    source.split_whitespace().collect::<String>()
}

fn function_body<'a>(source: &'a str, marker: &str) -> &'a str {
    let start = source
        .find(marker)
        .unwrap_or_else(|| panic!("missing function marker {marker}"));
    let after = &source[start..];
    let next_fn = after[marker.len()..]
        .find("fn")
        .map(|offset| marker.len() + offset)
        .unwrap_or(after.len());
    &after[..next_fn]
}
