use oulipoly_config::{ProviderEntry, ProvidersConfig};
use oulipoly_runtime::quota::{self, InFlight, RefreshOutcome};
use oulipoly_runtime::services::{QuotaServiceOutput, QuotaServicePort, QuotaServiceRequest};
use oulipoly_state::StateDb;
use std::path::Path;

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
    let quota = state
        .get_quota("quota-provider")
        .expect("get quota")
        .expect("quota row");
    assert_eq!(quota.provider_name, "quota-provider");
    let windows = state.get_windows("quota-provider").expect("get windows");
    assert_eq!(windows.len(), 1);
    assert!((windows[0].used_percent - expected).abs() < 1e-6);
}

fn direct_and_service_outcomes(
    providers: &ProvidersConfig,
) -> (RefreshOutcome, RefreshOutcome, InFlight) {
    let direct_state = StateDb::open(Path::new(":memory:")).expect("direct state");
    let direct_in_flight = InFlight::new();
    let direct = quota::refresh_provider(
        "quota-provider",
        providers,
        &direct_in_flight,
        &direct_state,
    );

    let service_state = StateDb::open(Path::new(":memory:")).expect("service state");
    let service_in_flight = InFlight::new();
    let service: &dyn QuotaServicePort = &quota::RuntimeQuotaService;
    let QuotaServiceOutput { outcome } = service
        .refresh_quota(QuotaServiceRequest {
            provider_name: "quota-provider".to_string(),
            providers_cfg: providers,
            in_flight: &service_in_flight,
            state: &service_state,
        })
        .expect("service refresh");

    (direct, outcome, service_in_flight)
}

fn assert_failed_with_same_message(direct: RefreshOutcome, service: RefreshOutcome) {
    match (direct, service) {
        (RefreshOutcome::Failed(direct), RefreshOutcome::Failed(service)) => {
            assert_eq!(service, direct);
        }
        (direct, service) => {
            panic!("expected matching Failed outcomes, got {direct:?}/{service:?}")
        }
    }
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

    let direct_state = StateDb::open(Path::new(":memory:")).expect("direct state");
    let direct_in_flight = InFlight::new();
    let direct = quota::refresh_provider(
        "quota-provider",
        &providers,
        &direct_in_flight,
        &direct_state,
    );

    let service_state = StateDb::open(Path::new(":memory:")).expect("service state");
    let service_in_flight = InFlight::new();
    let service: &dyn QuotaServicePort = &quota::RuntimeQuotaService;
    let QuotaServiceOutput { outcome } = service
        .refresh_quota(QuotaServiceRequest {
            provider_name: "quota-provider".to_string(),
            providers_cfg: &providers,
            in_flight: &service_in_flight,
            state: &service_state,
        })
        .expect("service refresh");

    assert_updated_window(direct, 0.42);
    assert_updated_window(outcome, 0.42);
    assert_persisted_window(&direct_state, 0.42);
    assert_persisted_window(&service_state, 0.42);
    assert!(
        service_in_flight.try_claim("quota-provider").is_some(),
        "service adapter must not hold the in-flight guard after refresh"
    );
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
    let state = StateDb::open(Path::new(":memory:")).expect("state");
    let in_flight = InFlight::new();
    let guard = in_flight
        .try_claim("quota-provider")
        .expect("pre-claim provider");

    let service: &dyn QuotaServicePort = &quota::RuntimeQuotaService;
    let QuotaServiceOutput { outcome } = service
        .refresh_quota(QuotaServiceRequest {
            provider_name: "quota-provider".to_string(),
            providers_cfg: &providers,
            in_flight: &in_flight,
            state: &state,
        })
        .expect("service refresh");

    assert!(matches!(outcome, RefreshOutcome::AlreadyInFlight));
    drop(guard);
    assert!(
        in_flight.try_claim("quota-provider").is_some(),
        "caller-owned guard release must remain visible after service call"
    );
}

// Risk: R-A5 / proposal T12 - quota service routing must preserve the direct
// NoScript outcome when a provider exists but has no quota_script configured.
// Level: unit.
// Source: AGE-34 contract "Expected observable signals" for Q1 error paths.
#[test]
fn runtime_quota_service_no_script_matches_direct_refresh_provider() {
    let providers = providers_without_quota_script();
    let (direct, service, service_in_flight) = direct_and_service_outcomes(&providers);

    assert!(matches!(direct, RefreshOutcome::NoScript));
    assert!(matches!(service, RefreshOutcome::NoScript));
    assert!(
        service_in_flight.try_claim("quota-provider").is_some(),
        "NoScript path must not retain an in-flight guard"
    );
}

// Risk: R-A5 / proposal T12 - quota service routing must preserve Failed after
// the quota script fails, auth refresh runs, and the retry still fails.
// Level: unit.
// Source: AGE-34 contract "Expected observable signals" for Q1 error paths.
#[test]
fn runtime_quota_service_failed_retry_matches_direct_refresh_provider() {
    let providers = providers_with_quota_script_and_refresh(
        "echo 'quota unavailable' >&2; exit 7",
        Some("true"),
    );
    let (direct, service, service_in_flight) = direct_and_service_outcomes(&providers);

    assert_failed_with_same_message(direct, service);
    assert!(
        service_in_flight.try_claim("quota-provider").is_some(),
        "Failed retry path must release the service in-flight guard"
    );
}

// Risk: R-A5 / proposal T12 - quota service routing must preserve JSON parser
// failures from the underlying refresh_provider path.
// Level: unit.
// Source: AGE-34 contract "Expected observable signals" for Q1 parser errors.
#[test]
fn runtime_quota_service_invalid_json_matches_direct_refresh_provider() {
    let providers = providers_with_quota_script("echo 'not-json'");
    let (direct, service, service_in_flight) = direct_and_service_outcomes(&providers);

    assert_failed_with_same_message(direct, service);
    assert!(
        service_in_flight.try_claim("quota-provider").is_some(),
        "Invalid JSON path must release the service in-flight guard"
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
    let (direct, service, service_in_flight) = direct_and_service_outcomes(&providers);

    assert_failed_with_same_message(direct, service);
    assert!(
        service_in_flight.try_claim("quota-provider").is_some(),
        "Bad resets_at path must release the service in-flight guard"
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
    let (direct, service, service_in_flight) = direct_and_service_outcomes(&providers);

    assert_failed_with_same_message(direct, service);
    assert!(
        service_in_flight.try_claim("quota-provider").is_some(),
        "Validation-failure path must release the service in-flight guard"
    );
}
