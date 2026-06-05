//! ## Declared roles
//!
//! `mapper`, `orchestration`, `accessor`, `validator`, `formatter`
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/tests/age34_runtime_quota_service_routing.rs
//!     role: intrinsic-surface
//!     Domain: runtime_quota_service_routing_parity_harness
//!     Owns:
//!       - OULIPOLY_DATA_HOME and OULIPOLY_DATA_DIR env override isolation under a process mutex
//!       - provider config, provider-registry handle, and StateDb fixture construction
//!       - direct-vs-service refresh parity evidence mapping
//!       - quota service routing/in-flight/failure assertions
//! ```
//!
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
use std::ffi::OsString;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Points `OULIPOLY_DATA_HOME` at a fresh tempdir for the lifetime of the
/// guard, and scrubs the higher-precedence `OULIPOLY_DATA_DIR`, so the
/// per-account auth-refresh single-flight lock's freshness stamp starts empty
/// and never bleeds across tests or test binaries.
struct DataHomeOverride {
    _home: tempfile::TempDir,
    _lock: MutexGuard<'static, ()>,
    previous_home: Option<OsString>,
    previous_data_dir: Option<OsString>,
}

impl DataHomeOverride {
    fn new() -> Self {
        let lock = env_lock();
        let home = tempfile::tempdir().expect("data home tempdir");
        let previous_home = std::env::var_os("OULIPOLY_DATA_HOME");
        let previous_data_dir = std::env::var_os("OULIPOLY_DATA_DIR");
        // SAFETY: the held ENV_LOCK serializes this process-global mutation.
        unsafe {
            std::env::remove_var("OULIPOLY_DATA_DIR");
            std::env::set_var("OULIPOLY_DATA_HOME", home.path());
        }
        Self {
            _home: home,
            _lock: lock,
            previous_home,
            previous_data_dir,
        }
    }
}

impl Drop for DataHomeOverride {
    fn drop(&mut self) {
        // SAFETY: this guard still holds ENV_LOCK until after the restore.
        unsafe {
            match &self.previous_home {
                Some(previous) => std::env::set_var("OULIPOLY_DATA_HOME", previous),
                None => std::env::remove_var("OULIPOLY_DATA_HOME"),
            }
            match &self.previous_data_dir {
                Some(previous) => std::env::set_var("OULIPOLY_DATA_DIR", previous),
                None => std::env::remove_var("OULIPOLY_DATA_DIR"),
            }
        }
    }
}

fn providers_with_quota_script(script: &str) -> ProvidersConfig {
    providers_with_quota_script_and_refresh(script, None)
}

fn providers_with_quota_script_and_refresh(
    script: &str,
    auth_refresh_command: Option<&str>,
) -> ProvidersConfig {
    let mut providers = ProvidersConfig::default();
    providers.entries.insert(
        "quota-provider".to_string(),
        ProviderEntry {
            quota_script: Some(script.to_string()),
            auth_refresh_command: auth_refresh_command.map(str::to_string),
            ..ProviderEntry::default()
        },
    );
    providers
}

fn providers_without_quota_script() -> ProvidersConfig {
    let mut providers = ProvidersConfig::default();
    providers
        .entries
        .insert("quota-provider".to_string(), ProviderEntry::default());
    providers
}

fn populated_registry_handle() -> ProviderRegistryHandle {
    let registry = ProviderRegistry::from_model_configs(
        &[ModelConfig {
            name: "neutral-quota-model".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig::model_provider(
                "neutral-quota-provider",
                Vec::new(),
            )],
            inputs: Vec::new(),
            provider: Some(ProviderImplementationRef {
                path: Some("/neutral-provider-placeholder".to_string()),
                crate_name: None,
                version: None,
                binary: None,
                script: None,
            }),
        }],
        ProviderRegistryOptions::default(),
    )
    .expect("populated registry");
    ProviderRegistryHandle::new(Arc::new(registry))
}

fn assert_updated_window(outcome: RefreshOutcome, expected: f64) {
    match outcome {
        RefreshOutcome::Updated { windows } => {
            assert_eq!(windows.len(), 1);
            assert!((windows[0].used_percent - expected).abs() < 1e-6);
        }
        other => panic!("expected Updated, got {other:?}"),
    }
}

fn assert_persisted_window(state: &StateDb, expected: f64) {
    let evidence = persisted_window_evidence(state);
    assert_persisted_window_evidence(evidence, expected);
}

struct PersistedWindowEvidence {
    quota: QuotaRecord,
    windows: Vec<QuotaWindow>,
}

fn persisted_window_evidence(state: &StateDb) -> PersistedWindowEvidence {
    let quota = state
        .get_quota("quota-provider")
        .expect("get quota")
        .expect("quota row");
    let windows = state.get_windows("quota-provider").expect("get windows");
    PersistedWindowEvidence { quota, windows }
}

fn assert_persisted_window_evidence(evidence: PersistedWindowEvidence, expected: f64) {
    assert_eq!(evidence.quota.provider_name, "quota-provider");
    assert_eq!(evidence.windows.len(), 1);
    assert!((evidence.windows[0].used_percent - expected).abs() < 1e-6);
}

struct DirectAndServiceEvidence {
    direct: RefreshOutcome,
    service: RefreshOutcome,
    service_in_flight: InFlight,
}

struct PersistedDirectAndServiceEvidence {
    direct: RefreshOutcome,
    service: RefreshOutcome,
    direct_state: StateDb,
    service_state: StateDb,
    service_in_flight: InFlight,
}

struct DirectRunEvidence {
    outcome: RefreshOutcome,
    state: StateDb,
}

struct ServiceRunEvidence {
    outcome: RefreshOutcome,
    state: StateDb,
    in_flight: InFlight,
}

fn direct_and_service_outcomes(providers: &ProvidersConfig) -> DirectAndServiceEvidence {
    let evidence = persisted_direct_and_service_outcomes(providers);
    DirectAndServiceEvidence {
        direct: evidence.direct,
        service: evidence.service,
        service_in_flight: evidence.service_in_flight,
    }
}

fn persisted_direct_and_service_outcomes(
    providers: &ProvidersConfig,
) -> PersistedDirectAndServiceEvidence {
    let direct = persisted_direct_run(providers);
    let service = persisted_service_run(providers);
    persisted_direct_and_service_evidence(direct, service)
}

fn persisted_direct_run(providers: &ProvidersConfig) -> DirectRunEvidence {
    let direct_state = StateDb::open(Path::new(":memory:")).expect("direct state");
    let direct_in_flight = InFlight::new();
    let direct = quota::refresh_provider(
        "quota-provider",
        providers,
        &direct_in_flight,
        &direct_state,
    );
    direct_run_evidence(direct, direct_state)
}

fn direct_run_evidence(outcome: RefreshOutcome, state: StateDb) -> DirectRunEvidence {
    DirectRunEvidence { outcome, state }
}

fn persisted_service_run(providers: &ProvidersConfig) -> ServiceRunEvidence {
    let service_state = StateDb::open(Path::new(":memory:")).expect("service state");
    let service_in_flight = InFlight::new();
    let quota_service =
        quota::RuntimeQuotaService::with_registry_handle(populated_registry_handle());
    let service: &dyn QuotaServicePort = &quota_service;
    let QuotaServiceOutput { outcome } = service
        .refresh_quota(quota_service_request(
            providers,
            &service_in_flight,
            &service_state,
        ))
        .expect("service refresh");
    service_run_evidence(outcome, service_state, service_in_flight)
}

fn service_run_evidence(
    outcome: RefreshOutcome,
    state: StateDb,
    in_flight: InFlight,
) -> ServiceRunEvidence {
    ServiceRunEvidence {
        outcome,
        state,
        in_flight,
    }
}

fn quota_service_request<'a>(
    providers: &'a ProvidersConfig,
    in_flight: &'a InFlight,
    state: &'a StateDb,
) -> QuotaServiceRequest<'a> {
    QuotaServiceRequest {
        provider_name: "quota-provider".to_string(),
        providers_cfg: providers,
        in_flight,
        state,
        external_provider: None,
    }
}

fn persisted_direct_and_service_evidence(
    direct: DirectRunEvidence,
    service: ServiceRunEvidence,
) -> PersistedDirectAndServiceEvidence {
    PersistedDirectAndServiceEvidence {
        direct: direct.outcome,
        service: service.outcome,
        direct_state: direct.state,
        service_state: service.state,
        service_in_flight: service.in_flight,
    }
}

fn assert_failed_outcomes(direct: RefreshOutcome, service: RefreshOutcome) {
    match (direct, service) {
        (RefreshOutcome::Failed(direct), RefreshOutcome::Failed(service)) => {
            assert_eq!(service, direct);
        }
        (direct, service) => {
            panic!("expected matching Failed outcomes, got {direct:?}/{service:?}")
        }
    }
}

fn assert_no_script_outcomes(evidence: &DirectAndServiceEvidence) {
    assert!(matches!(evidence.direct, RefreshOutcome::NoScript));
    assert!(matches!(evidence.service, RefreshOutcome::NoScript));
}

fn assert_service_in_flight_released(in_flight: &InFlight, message: &str) {
    assert!(in_flight.try_claim("quota-provider").is_some(), "{message}");
}

fn assert_persisted_success_evidence(evidence: PersistedDirectAndServiceEvidence, expected: f64) {
    assert_updated_window(evidence.direct, expected);
    assert_updated_window(evidence.service, expected);
    assert_persisted_window(&evidence.direct_state, expected);
    assert_persisted_window(&evidence.service_state, expected);
    assert_service_in_flight_released(
        &evidence.service_in_flight,
        "service adapter must not hold the in-flight guard after refresh",
    );
}

struct PreclaimedInFlightEvidence {
    outcome: RefreshOutcome,
    in_flight: InFlight,
}

fn preclaimed_in_flight_outcome(providers: &ProvidersConfig) -> PreclaimedInFlightEvidence {
    let state = StateDb::open(Path::new(":memory:")).expect("state");
    let in_flight = InFlight::new();
    let guard = in_flight
        .try_claim("quota-provider")
        .expect("pre-claim provider");
    let outcome = preclaimed_service_outcome(providers, &in_flight, &state);
    drop(guard);
    preclaimed_in_flight_evidence(outcome, in_flight)
}

fn preclaimed_service_outcome(
    providers: &ProvidersConfig,
    in_flight: &InFlight,
    state: &StateDb,
) -> RefreshOutcome {
    let quota_service =
        quota::RuntimeQuotaService::with_registry_handle(populated_registry_handle());
    let service: &dyn QuotaServicePort = &quota_service;
    let QuotaServiceOutput { outcome } = service
        .refresh_quota(quota_service_request(providers, in_flight, state))
        .expect("service refresh");
    outcome
}

fn preclaimed_in_flight_evidence(
    outcome: RefreshOutcome,
    in_flight: InFlight,
) -> PreclaimedInFlightEvidence {
    PreclaimedInFlightEvidence { outcome, in_flight }
}

fn assert_preclaimed_in_flight_evidence(evidence: PreclaimedInFlightEvidence) {
    assert!(matches!(evidence.outcome, RefreshOutcome::AlreadyInFlight));
    assert_service_in_flight_released(
        &evidence.in_flight,
        "caller-owned guard release must remain visible after service call",
    );
}

fn assert_failure_and_guard_release(evidence: DirectAndServiceEvidence, message: &str) {
    let DirectAndServiceEvidence {
        direct,
        service,
        service_in_flight,
    } = evidence;
    assert_failed_outcomes(direct, service);
    assert_service_in_flight_released(&service_in_flight, message);
}

// Risk: R-A5 / proposal T12 - quota service routing must borrow caller-owned
// ProvidersConfig, InFlight, and StateDb and delegate to refresh_provider with
// identical outcome, persisted windows, and guard release behavior.
// Level: unit.
// Source: AGE-34 contract "QuotaServiceRequest / QuotaServiceOutput";
// assumption A5.
#[test]
fn runtime_quota_service_matches_direct_refresh_provider_and_persists_windows() {
    let script =
        "echo '{\"windows\":[{\"used_percent\":42,\"resets_at\":\"2099-01-01T00:00:00Z\"}]}'";
    let providers = providers_with_quota_script(script);
    let evidence = persisted_direct_and_service_outcomes(&providers);
    assert_persisted_success_evidence(evidence, 0.42);
}

// Risk: R-A5 / proposal T12 - quota service routing must preserve the
// caller-owned in-flight collapse semantics instead of creating or retaining
// its own InFlight.
// Level: unit.
// Source: AGE-34 contract risk annotation R-A5.
#[test]
fn runtime_quota_service_preserves_caller_owned_in_flight_already_claimed_state() {
    let providers = providers_with_quota_script(
        "echo '{\"windows\":[{\"used_percent\":1,\"resets_at\":\"2099-01-01T00:00:00Z\"}]}'",
    );
    let evidence = preclaimed_in_flight_outcome(&providers);
    assert_preclaimed_in_flight_evidence(evidence);
}

// Risk: R-A5 / proposal T12 - quota service routing must preserve the direct
// NoScript outcome when a provider exists but has no quota_script configured.
// Level: unit.
// Source: AGE-34 contract "Expected observable signals" for Q1 error paths.
#[test]
fn runtime_quota_service_no_script_matches_direct_refresh_provider() {
    let providers = providers_without_quota_script();
    let evidence = direct_and_service_outcomes(&providers);
    assert_no_script_outcomes(&evidence);
    assert_service_in_flight_released(
        &evidence.service_in_flight,
        "NoScript path must not retain an in-flight guard",
    );
}

// Risk: R-A5 / proposal T12 - quota service routing must preserve Failed after
// the quota script fails, auth refresh runs, and the retry still fails.
// Level: unit.
// Source: AGE-34 contract "Expected observable signals" for Q1 error paths.
#[test]
fn runtime_quota_service_failed_retry_matches_direct_refresh_provider() {
    let _data_home = DataHomeOverride::new();
    let providers = providers_with_quota_script_and_refresh(
        "echo 'quota unavailable' >&2; exit 7",
        Some("true"),
    );
    let evidence = direct_and_service_outcomes(&providers);
    assert_failure_and_guard_release(
        evidence,
        "Failed retry path must release the service in-flight guard",
    );
}

// Risk: R-A5 / proposal T12 - quota service routing must preserve JSON parser
// failures from the underlying refresh_provider path.
// Level: unit.
// Source: AGE-34 contract "Expected observable signals" for Q1 parser errors.
#[test]
fn runtime_quota_service_invalid_json_matches_direct_refresh_provider() {
    let providers = providers_with_quota_script("echo 'not-json'");
    let evidence = direct_and_service_outcomes(&providers);
    assert_failure_and_guard_release(
        evidence,
        "Invalid JSON path must release the service in-flight guard",
    );
}

// Risk: R-A5 / proposal T12 - quota service routing must preserve RFC3339
// parser failures from quota windows.
// Level: unit.
// Source: AGE-34 contract "Expected observable signals" for Q1 parser errors.
#[test]
fn runtime_quota_service_bad_resets_at_matches_direct_refresh_provider() {
    let providers = providers_with_quota_script(
        "echo '{\"windows\":[{\"used_percent\":42,\"resets_at\":\"not-a-date\"}]}'",
    );
    let evidence = direct_and_service_outcomes(&providers);
    assert_failure_and_guard_release(
        evidence,
        "Bad resets_at path must release the service in-flight guard",
    );
}

// Risk: R-A5 / proposal T12 - quota service routing must preserve the
// underlying 0..100 used_percent validation.
// Level: unit.
// Source: AGE-34 contract "Expected observable signals" for Q1 validation.
#[test]
fn runtime_quota_service_used_percent_validation_matches_direct_refresh_provider() {
    let providers = providers_with_quota_script(
        "echo '{\"windows\":[{\"used_percent\":150,\"resets_at\":\"2099-01-01T00:00:00Z\"}]}'",
    );
    let evidence = direct_and_service_outcomes(&providers);
    assert_failure_and_guard_release(
        evidence,
        "Validation-failure path must release the service in-flight guard",
    );
}
