//! ## Declared roles
//!
//! `accessor`, `mapper`, `validator`, `orchestration`, `parser`
//!
use agent_runner_lib::AppState;
use std::collections::HashMap;

#[test]
fn app_state_db_path_returns_models_parent_state_db() {
    let (state, expected) = db_path_fixture();
    assert_state_db_path(&state, expected);
}

fn db_path_fixture() -> (AppState, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let models_dir = dir.path().join("models");
    let state = AppState::test_default(models_dir, HashMap::new());
    (state, dir.keep().join("state.db"))
}

fn assert_state_db_path(state: &AppState, expected: std::path::PathBuf) {
    assert_eq!(state.db_path(), expected);
}

#[test]
fn app_state_constructors_share_the_account_authoritative_registry() {
    let raw_source = include_str!("../src/app_state.rs");
    let source = compact(raw_source);
    assert_no_provider_settings_fallback(raw_source);
    assert_constructor_common_fields(&source);
    assert_new_service_wiring(&source);
    assert_test_default_service_wiring(&source);
    assert_with_services_wiring(&source);
}

fn assert_no_provider_settings_fallback(source: &str) {
    for forbidden in [
        "EMPTY_PROVIDER_SETTINGS_HOST_EXPECT_MESSAGE",
        "provider_settings_host_for_models",
        "ProviderSettingsHost::from_model_configs",
    ] {
        assert!(
            !source.contains(forbidden),
            "provider settings must not fall back to model or empty registry authority: {forbidden}"
        );
    }
}

fn assert_constructor_common_fields(source: &str) {
    for constructor in [
        "pub(crate)fnnew(",
        "pubfntest_default(",
        "pubfnwith_services(",
    ] {
        let body = function_body(source, constructor);
        assert!(
            body.contains("ProviderSettingsHost::with_registry_handle("),
            "{constructor} must initialize provider settings from the shared provider registry"
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
}

fn assert_new_service_wiring(source: &str) {
    let new_body = function_body(source, "pub(crate)fnnew(");
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
}

fn assert_test_default_service_wiring(source: &str) {
    let test_default_body = function_body(source, "pubfntest_default(");
    for required in [
        "state_db_opener:Arc::new(ProductionStateDbOpener)",
        "providers_config:Arc::new(FilesystemProvidersConfigRepository)",
        "quota_service:Arc::new(oulipoly_runtime::quota::RuntimeQuotaService::with_registry_handle(provider_registry.clone()",
        "executor_service:Arc::new(oulipoly_runtime::executor::RuntimeExecutorService::with_registry_handle(provider_registry.clone()",
        "diagnostics_service:Arc::new(oulipoly_runtime::diagnostics::RuntimeDiagnosticsService::with_registry_handle(provider_registry.clone()",
        "provider_registry",
    ] {
        assert!(
            test_default_body.contains(required),
            "AppState::test_default must preserve default service wiring: {required}"
        );
    }
}

fn assert_with_services_wiring(source: &str) {
    let with_services_body = function_body(source, "pubfnwith_services(");
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
    let start = function_marker_index(source, marker);
    let end = function_body_end(&source[start..], marker);
    &source[start..start + end]
}

fn function_marker_index(source: &str, marker: &str) -> usize {
    source
        .find(marker)
        .unwrap_or_else(|| panic!("missing function marker {marker}"))
}

fn function_body_end(after: &str, marker: &str) -> usize {
    after[marker.len()..]
        .find("fn")
        .map(|offset| marker.len() + offset)
        .unwrap_or(after.len())
}
