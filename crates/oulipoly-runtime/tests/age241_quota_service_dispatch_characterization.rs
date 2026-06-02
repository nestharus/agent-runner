//! ## Declared roles
//!
//! `mapper`, `orchestration`, `accessor`, `formatter`, `predicate`, `validator`

use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ProviderEntry, ProvidersConfig,
    provider_implementation_ref::ProviderImplementationRef,
};
use oulipoly_runtime::provider_registry::{
    ProviderRegistry, ProviderRegistryHandle, ProviderRegistryOptions,
};
use oulipoly_runtime::quota::{self, InFlight, RefreshOutcome};
use oulipoly_runtime::services::{QuotaServiceOutput, QuotaServicePort, QuotaServiceRequest};
use oulipoly_state::{QuotaRecord, QuotaWindow, StateDb};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const BUILTIN_PROVIDER: &str = "builtin-quota-provider";

struct ScriptFixture {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

struct BuiltinByteIdentityEvidence {
    direct: RefreshOutcome,
    direct_persisted: PersistedWindowEvidence,
    service: RefreshOutcome,
    service_persisted: PersistedWindowEvidence,
    service_in_flight: InFlight,
}

struct PersistedWindowEvidence {
    quota: QuotaRecord,
    windows: Vec<QuotaWindow>,
}

struct BuiltinRunEvidence {
    outcome: RefreshOutcome,
    persisted: PersistedWindowEvidence,
}

struct ServiceRunEvidence {
    run: BuiltinRunEvidence,
    in_flight: InFlight,
}

fn provider_ref_path(path: &Path) -> ProviderImplementationRef {
    ProviderImplementationRef {
        path: Some(path.display().to_string()),
        crate_name: None,
        version: None,
        binary: None,
        script: None,
    }
}

fn model_with_provider_ref(provider_ref: ProviderImplementationRef) -> ModelConfig {
    ModelConfig {
        name: "neutral-external-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::model_provider(
            "neutral-external-provider",
            Vec::new(),
        )],
        inputs: Vec::new(),
        provider: Some(provider_ref),
    }
}

fn unrelated_registry(script: &ScriptFixture) -> ProviderRegistry {
    ProviderRegistry::from_model_configs(
        &[model_with_provider_ref(provider_ref_path(&script.path))],
        ProviderRegistryOptions::default(),
    )
    .expect("unrelated provider registry should construct")
}

fn assert_unrelated_registry_configured(registry: &ProviderRegistry) {
    assert_eq!(
        registry.configured_model_names(),
        ["neutral-external-model"]
    );
    assert_eq!(registry.configured_artifact_keys().len(), 1);
}

fn registry_handle(registry: ProviderRegistry) -> ProviderRegistryHandle {
    ProviderRegistryHandle::new(Arc::new(registry))
}

fn fixture_script(name: &str, body: &str) -> ScriptFixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = script_fixture_path(dir.path(), name);
    write_executable_script(&path, &script_file_contents(body));
    script_fixture(dir, path)
}

fn script_file_contents(body: &str) -> String {
    format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n")
}

fn script_fixture_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(name)
}

fn write_executable_script(path: &Path, contents: &str) {
    fs::write(path, contents).expect("write script");
}

fn script_fixture(dir: tempfile::TempDir, path: PathBuf) -> ScriptFixture {
    ScriptFixture { _dir: dir, path }
}

fn providers_with_builtin_quota_script(script: &str) -> ProvidersConfig {
    let mut providers = ProvidersConfig::default();
    providers.entries.insert(
        BUILTIN_PROVIDER.to_string(),
        ProviderEntry {
            quota_script: Some(script.to_string()),
            ..ProviderEntry::default()
        },
    );
    providers
}

fn builtin_window_script(used_percent: u32) -> String {
    format!(
        "printf '{{\"windows\":[{{\"used_percent\":{used_percent},\"resets_at\":\"2099-01-01T00:00:00Z\"}}]}}'"
    )
}

fn counter_provider_script(counter_path: &Path) -> String {
    format!(
        "count=$(cat {counter}); count=$((count + 1)); printf '%s' \"$count\" > {counter}; printf '{{\"contract\":\"oulipoly.provider/v1\",\"request_id\":\"request-example-001\",\"ok\":true,\"result\":{{}}}}\\n'",
        counter = shell_quote(counter_path)
    )
}

fn in_memory_state(label: &str) -> StateDb {
    StateDb::open(Path::new(":memory:")).unwrap_or_else(|error| panic!("{label}: {error}"))
}

fn quota_service_request<'a>(
    providers_cfg: &'a ProvidersConfig,
    in_flight: &'a InFlight,
    state: &'a StateDb,
) -> QuotaServiceRequest<'a> {
    QuotaServiceRequest {
        provider_name: BUILTIN_PROVIDER.to_string(),
        providers_cfg,
        in_flight,
        state,
        external_provider: None,
    }
}

fn refresh_builtin_direct(
    providers: &ProvidersConfig,
    in_flight: &InFlight,
    state: &StateDb,
) -> RefreshOutcome {
    quota::refresh_provider(BUILTIN_PROVIDER, providers, in_flight, state)
}

fn refresh_builtin_through_service(
    providers: &ProvidersConfig,
    in_flight: &InFlight,
    state: &StateDb,
    registry: ProviderRegistryHandle,
) -> RefreshOutcome {
    let quota_service = quota::RuntimeQuotaService::with_registry_handle(registry);
    let service: &dyn QuotaServicePort = &quota_service;
    let QuotaServiceOutput { outcome } = service
        .refresh_quota(quota_service_request(providers, in_flight, state))
        .expect("service refresh");
    outcome
}

fn builtin_byte_identity_evidence(
    used_percent: u32,
    registry: ProviderRegistryHandle,
) -> BuiltinByteIdentityEvidence {
    let providers = providers_with_builtin_quota_script(&builtin_window_script(used_percent));
    let direct = builtin_direct_run_evidence(&providers);
    let service = builtin_service_run_evidence(&providers, registry);
    builtin_byte_identity_from_runs(direct, service)
}

fn builtin_direct_run_evidence(providers: &ProvidersConfig) -> BuiltinRunEvidence {
    let direct_state = in_memory_state("direct state");
    let direct_in_flight = InFlight::new();
    let direct = refresh_builtin_direct(providers, &direct_in_flight, &direct_state);
    let direct_persisted = persisted_window_evidence(&direct_state);
    builtin_run_evidence(direct, direct_persisted)
}

fn builtin_run_evidence(
    outcome: RefreshOutcome,
    persisted: PersistedWindowEvidence,
) -> BuiltinRunEvidence {
    BuiltinRunEvidence { outcome, persisted }
}

fn builtin_service_run_evidence(
    providers: &ProvidersConfig,
    registry: ProviderRegistryHandle,
) -> ServiceRunEvidence {
    let service_state = in_memory_state("service state");
    let service_in_flight = InFlight::new();
    let service =
        refresh_builtin_through_service(providers, &service_in_flight, &service_state, registry);
    let service_persisted = persisted_window_evidence(&service_state);
    service_run_evidence(service, service_persisted, service_in_flight)
}

fn service_run_evidence(
    outcome: RefreshOutcome,
    persisted: PersistedWindowEvidence,
    in_flight: InFlight,
) -> ServiceRunEvidence {
    ServiceRunEvidence {
        run: builtin_run_evidence(outcome, persisted),
        in_flight,
    }
}

fn builtin_byte_identity_from_runs(
    direct: BuiltinRunEvidence,
    service: ServiceRunEvidence,
) -> BuiltinByteIdentityEvidence {
    BuiltinByteIdentityEvidence {
        direct: direct.outcome,
        direct_persisted: direct.persisted,
        service: service.run.outcome,
        service_persisted: service.run.persisted,
        service_in_flight: service.in_flight,
    }
}

fn assert_updated_window(outcome: &RefreshOutcome, expected: f64, expected_resets_at: &str) {
    match outcome {
        RefreshOutcome::Updated { windows } => {
            assert_eq!(windows.len(), 1);
            assert!((windows[0].used_percent - expected).abs() < 1e-9);
            assert_eq!(windows[0].resets_at.to_rfc3339(), expected_resets_at);
        }
        other => panic!("expected Updated, got {other:?}"),
    }
}

fn assert_matching_updated_outcomes(
    direct: &RefreshOutcome,
    service: &RefreshOutcome,
    expected: f64,
    expected_resets_at: &str,
) {
    assert_updated_window(direct, expected, expected_resets_at);
    assert_updated_window(service, expected, expected_resets_at);
}

fn persisted_window_evidence(state: &StateDb) -> PersistedWindowEvidence {
    let quota = state
        .get_quota(BUILTIN_PROVIDER)
        .expect("get quota")
        .expect("quota row");
    let windows = state.get_windows(BUILTIN_PROVIDER).expect("get windows");
    PersistedWindowEvidence { quota, windows }
}

fn assert_persisted_window_evidence(
    evidence: &PersistedWindowEvidence,
    expected: f64,
    expected_resets_at: &str,
) {
    assert_eq!(evidence.quota.provider_name, BUILTIN_PROVIDER);
    assert_eq!(evidence.windows.len(), 1);
    assert!((evidence.windows[0].used_percent - expected).abs() < 1e-9);
    assert_eq!(
        evidence.windows[0].resets_at.to_rfc3339(),
        expected_resets_at
    );
}

fn assert_in_flight_released(in_flight: &InFlight) {
    assert!(
        in_flight.try_claim(BUILTIN_PROVIDER).is_some(),
        "service no-ref path must release the caller-owned in-flight guard"
    );
}

fn assert_builtin_byte_identity_evidence(
    evidence: &BuiltinByteIdentityEvidence,
    expected: f64,
    expected_resets_at: &str,
) {
    assert_matching_updated_outcomes(
        &evidence.direct,
        &evidence.service,
        expected,
        expected_resets_at,
    );
    assert_persisted_window_evidence(&evidence.direct_persisted, expected, expected_resets_at);
    assert_persisted_window_evidence(&evidence.service_persisted, expected, expected_resets_at);
    assert_in_flight_released(&evidence.service_in_flight);
}

fn initialize_counter_file() -> tempfile::NamedTempFile {
    let counter = tempfile::NamedTempFile::new().expect("counter");
    fs::write(counter.path(), "0").expect("initialize counter");
    counter
}

fn counter_value(counter_path: &Path) -> String {
    fs::read_to_string(counter_path).expect("counter readable")
}

fn assert_counter_unchanged(counter_path: &Path) {
    assert_eq!(
        counter_value(counter_path),
        "0",
        "no-ref quota service refresh must not describe, construct, or invoke the unrelated provider client"
    );
}

fn runtime_source(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

fn contains_provider_registry_dispatch(source: &str) -> bool {
    source.contains("provider_registry")
        || source.contains("ProviderRegistry")
        || source.contains("describe_model_provider")
        || source.contains("enabled_artifact_for_model")
}

fn contains_direct_routing_quota_helpers(source: &str) -> bool {
    source.contains("refresh_provider_for_routing") && source.contains("verify_or_clear_marker")
}

fn contains_direct_marker_quota_helper(source: &str) -> bool {
    source.contains("refresh_provider_for_routing")
}

fn assert_source_outside_provider_registry_dispatch(relative: &str) {
    let source = runtime_source(relative);
    assert!(
        !contains_provider_registry_dispatch(&source),
        "{relative} must stay outside the provider-registry quota dispatch boundary"
    );
}

fn assert_routing_refresh_input_wiring_unchanged() {
    assert!(
        contains_direct_routing_quota_helpers(&runtime_source("balancer/refresh_inputs.rs")),
        "routing refresh input wiring should remain on the direct quota helpers"
    );
}

fn assert_marker_refresh_wiring_unchanged() {
    assert!(
        contains_direct_marker_quota_helper(&runtime_source("quota/marker_verification/mod.rs")),
        "marker verification should keep using the direct routing quota refresh helper"
    );
}

#[test]
fn runtime_quota_service_no_ref_preserves_builtin_bytes_with_unrelated_registry() {
    let unrelated = fixture_script(
        "neutral-provider.sh",
        "printf 'unrelated external provider should not run\\n'",
    );
    let registry = unrelated_registry(&unrelated);
    assert_unrelated_registry_configured(&registry);

    let evidence = builtin_byte_identity_evidence(37, registry_handle(registry));
    assert_builtin_byte_identity_evidence(&evidence, 0.37, "2099-01-01T00:00:00+00:00");
}

#[test]
fn runtime_quota_service_no_ref_does_not_describe_or_invoke_provider_client() {
    let counter = initialize_counter_file();
    let external = fixture_script(
        "neutral-provider.sh",
        &counter_provider_script(counter.path()),
    );
    let registry = unrelated_registry(&external);
    assert_unrelated_registry_configured(&registry);

    let providers = providers_with_builtin_quota_script(&builtin_window_script(12));
    let state = in_memory_state("state");
    let in_flight = InFlight::new();

    let outcome =
        refresh_builtin_through_service(&providers, &in_flight, &state, registry_handle(registry));
    assert_updated_window(&outcome, 0.12, "2099-01-01T00:00:00+00:00");
    assert_counter_unchanged(counter.path());
}

#[test]
fn quota_service_boundary_does_not_route_routing_or_marker_paths_through_provider_registry() {
    for relative in [
        "balancer/refresh_inputs.rs",
        "balancer/topology.rs",
        "quota/marker_verification/mod.rs",
        "quota/marker_verification/config.rs",
        "quota/marker_verification/health.rs",
        "quota/marker_verification/lock.rs",
    ] {
        assert_source_outside_provider_registry_dispatch(relative);
    }

    assert_routing_refresh_input_wiring_unchanged();
    assert_marker_refresh_wiring_unchanged();
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}
