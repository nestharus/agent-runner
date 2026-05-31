//! ## Declared roles
//!
//! `mapper`, `orchestration`, `accessor`, `formatter`, `validator`, `predicate`, `filter`, `parser`

use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ProvidersConfig,
    provider_implementation_ref::ProviderImplementationRef,
};
use oulipoly_runtime::provider_registry::{
    ProviderRegistry, ProviderRegistryHandle, ProviderRegistryOptions,
};
use oulipoly_runtime::quota::{InFlight, InFlightGuard, RefreshOutcome, RuntimeQuotaService};
use oulipoly_runtime::services::{
    QuotaServiceExternalProviderIdentity, QuotaServicePort, QuotaServiceRequest,
};
use oulipoly_state::StateDb;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const PROVIDER_NAME: &str = "neutral-provider";
const MODEL_NAME: &str = "neutral-model";
const SETTINGS_ID: &str = "provider-a-test";

struct FakeProviderFixture {
    _dir: tempfile::TempDir,
    script: PathBuf,
    log: PathBuf,
}

fn provider_ref(path: &Path) -> ProviderImplementationRef {
    ProviderImplementationRef {
        path: None,
        crate_name: None,
        version: None,
        binary: None,
        script: Some(path.display().to_string()),
    }
}

fn model_config(path: &Path) -> ModelConfig {
    ModelConfig {
        name: MODEL_NAME.to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::model_provider(PROVIDER_NAME, Vec::new())],
        inputs: Vec::new(),
        provider: Some(provider_ref(path)),
    }
}

fn quota_identity() -> QuotaServiceExternalProviderIdentity {
    QuotaServiceExternalProviderIdentity {
        model_name: MODEL_NAME.to_string(),
        provider_instance_id: PROVIDER_NAME.to_string(),
        settings_id: SETTINGS_ID.to_string(),
    }
}

fn quota_service(fixture: &FakeProviderFixture) -> RuntimeQuotaService {
    let registry = ProviderRegistry::from_model_configs(
        &[model_config(&fixture.script)],
        ProviderRegistryOptions::default(),
    )
    .expect("registry");
    RuntimeQuotaService::with_registry_handle(ProviderRegistryHandle::new(Arc::new(registry)))
}

fn request<'a>(
    providers_cfg: &'a ProvidersConfig,
    in_flight: &'a InFlight,
    state: &'a StateDb,
) -> QuotaServiceRequest<'a> {
    QuotaServiceRequest {
        provider_name: PROVIDER_NAME.to_string(),
        providers_cfg,
        in_flight,
        state,
        external_provider: Some(quota_identity()),
    }
}

fn refresh_with_fixture(fixture: &FakeProviderFixture) -> (RefreshOutcome, StateDb) {
    let evidence = refresh_fixture_evidence(fixture);
    refresh_fixture_result(evidence)
}

struct RefreshFixtureEvidence {
    outcome: RefreshOutcome,
    state: StateDb,
}

struct RefreshFixtureRuntime {
    providers: ProvidersConfig,
    in_flight: InFlight,
    state: StateDb,
}

fn refresh_fixture_evidence(fixture: &FakeProviderFixture) -> RefreshFixtureEvidence {
    let runtime = refresh_fixture_runtime();
    let outcome = refresh_fixture_outcome(fixture, &runtime);
    refresh_fixture_evidence_from_parts(outcome, runtime.state)
}

fn refresh_fixture_runtime() -> RefreshFixtureRuntime {
    let providers = ProvidersConfig::default();
    let in_flight = InFlight::new();
    let state = StateDb::open(Path::new(":memory:")).expect("state");
    RefreshFixtureRuntime {
        providers,
        in_flight,
        state,
    }
}

fn refresh_fixture_outcome(
    fixture: &FakeProviderFixture,
    runtime: &RefreshFixtureRuntime,
) -> RefreshOutcome {
    let output = quota_service(fixture)
        .refresh_quota(request(
            &runtime.providers,
            &runtime.in_flight,
            &runtime.state,
        ))
        .expect("service output");
    output.outcome
}

fn refresh_fixture_evidence_from_parts(
    outcome: RefreshOutcome,
    state: StateDb,
) -> RefreshFixtureEvidence {
    RefreshFixtureEvidence { outcome, state }
}

fn refresh_fixture_result(evidence: RefreshFixtureEvidence) -> (RefreshOutcome, StateDb) {
    (evidence.outcome, evidence.state)
}

fn fixture(
    source_response: &str,
    probe_response: &str,
    refresh_response: &str,
) -> FakeProviderFixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = provider_script_path(&dir);
    let log = provider_log_path(&dir);
    let body = provider_script_body(source_response, probe_response, refresh_response, &log);
    seed_provider_script(&script, &body);
    fake_provider_fixture(dir, script, log)
}

fn script_text(body: &str) -> String {
    format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n")
}

fn make_provider_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("chmod script");
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

fn provider_script_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("neutral-provider.sh")
}

fn provider_log_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("provider.log")
}

fn seed_provider_script(script: &Path, body: &str) {
    fs::write(script, script_text(body)).expect("write script");
    make_provider_executable(script);
}

fn fake_provider_fixture(
    dir: tempfile::TempDir,
    script: PathBuf,
    log: PathBuf,
) -> FakeProviderFixture {
    FakeProviderFixture {
        _dir: dir,
        script,
        log,
    }
}

fn success_fixture() -> FakeProviderFixture {
    success_fixture_with_window(0.42, 4102444800000)
}

fn success_fixture_with_window(
    remaining_ratio: f64,
    resets_at_unix_ms: i64,
) -> FakeProviderFixture {
    fixture(
        &source_true_response(),
        &probe_success_response(remaining_ratio, resets_at_unix_ms),
        &refresh_success_response(),
    )
}

fn source_false_fixture() -> FakeProviderFixture {
    fixture(
        &source_false_response(),
        &probe_success_response(0.42, 4102444800000),
        &refresh_success_response(),
    )
}

fn invalid_window_fixture() -> FakeProviderFixture {
    fixture(
        &source_true_response(),
        &probe_invalid_window_response(),
        &refresh_success_response(),
    )
}

fn retry_fixture() -> FakeProviderFixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = provider_script_path(&dir);
    let log = provider_log_path(&dir);
    let count = provider_probe_count_path(&dir);
    let body = provider_script_body(
        &source_true_response(),
        &probe_retry_response(&count, 0.25, 4102444800000),
        &refresh_success_response(),
        &log,
    );
    seed_provider_script(&script, &body);
    fake_provider_fixture(dir, script, log)
}

fn provider_probe_count_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("probe-count")
}

fn provider_script_body(
    source_response: &str,
    probe_response: &str,
    refresh_response: &str,
    log: &Path,
) -> String {
    format!(
        r#"
subcommand="${{1:?}}"
request="$(cat)"
request_id="$(printf '%s' "$request" | sed -n 's/.*"request_id":"\([^"]*\)".*/\1/p')"
printf '%s\t%s\n' "$subcommand" "$request" >> {log}
describe_result='{{"provider_id":"neutral","display_name":"Neutral","contract_versions":["oulipoly.provider/v1"],"preferred_contract":"oulipoly.provider/v1","capabilities":{{"launch":false,"policy":false,"quota":true,"session":false,"terminal":false,"rotation":false,"discovery":false,"settings":false,"setup_brain":false,"setup":false,"migration":false}}}}'
case "$subcommand" in
  describe)
    printf '{{"contract":"oulipoly.provider/v1","request_id":"%s","ok":true,"result":%s}}\n' "$request_id" "$describe_result"
    ;;
  quota.source)
    {source_response}
    ;;
  quota.probe)
    {probe_response}
    ;;
  quota.refresh_auth)
    {refresh_response}
    ;;
esac
"#,
        log = shell_quote(log)
    )
}

fn source_true_response() -> String {
    "printf '{\"contract\":\"oulipoly.provider/v1\",\"request_id\":\"%s\",\"ok\":true,\"result\":{\"has_source\":true,\"freshness\":\"fresh\",\"source_id\":\"ignored\"}}\\n' \"$request_id\"".to_string()
}

fn source_false_response() -> String {
    "printf '{\"contract\":\"oulipoly.provider/v1\",\"request_id\":\"%s\",\"ok\":true,\"result\":{\"has_source\":false,\"freshness\":\"fresh\"}}\\n' \"$request_id\"".to_string()
}

fn probe_success_response(remaining_ratio: f64, resets_at_unix_ms: i64) -> String {
    format!(
        "printf '{{\"contract\":\"oulipoly.provider/v1\",\"request_id\":\"%s\",\"ok\":true,\"result\":{{\"available\":true,\"checked_at_unix_ms\":0,\"windows\":[{{\"remaining_ratio\":{remaining_ratio},\"resets_at_unix_ms\":{resets_at_unix_ms}}}]}}}}\\n' \"$request_id\""
    )
}

fn probe_invalid_window_response() -> String {
    "printf '{\"contract\":\"oulipoly.provider/v1\",\"request_id\":\"%s\",\"ok\":true,\"result\":{\"available\":true,\"checked_at_unix_ms\":0,\"windows\":[{\"remaining_ratio\":1.4,\"resets_at_unix_ms\":0}]}}\\n' \"$request_id\"".to_string()
}

fn probe_retry_response(count_path: &Path, remaining_ratio: f64, resets_at_unix_ms: i64) -> String {
    format!(
        r#"count="$(cat {count} 2>/dev/null || printf 0)"
count="$((count + 1))"
printf '%s' "$count" > {count}
if [ "$count" = "1" ]; then
  printf '{{"contract":"oulipoly.provider/v1","request_id":"%s","ok":false,"error":{{"code":"probe_failed","category":"failed","message":"probe failed","retryable":true}}}}\n' "$request_id"
else
  {success}
fi"#,
        count = shell_quote(count_path),
        success = probe_success_response(remaining_ratio, resets_at_unix_ms)
    )
}

fn refresh_success_response() -> String {
    "printf '{\"contract\":\"oulipoly.provider/v1\",\"request_id\":\"%s\",\"ok\":true,\"result\":{\"refreshed\":true,\"available\":true,\"checked_at_unix_ms\":0}}\\n' \"$request_id\"".to_string()
}

fn provider_log(fixture: &FakeProviderFixture) -> String {
    fs::read_to_string(&fixture.log).unwrap_or_default()
}

fn logged_request(fixture: &FakeProviderFixture, subcommand: &str) -> Value {
    let log = provider_log(fixture);
    let line = logged_subcommand_line(&log, subcommand);
    parse_logged_request(line)
}

fn logged_subcommand_line<'a>(log: &'a str, subcommand: &str) -> &'a str {
    log.lines()
        .find(|line| line.starts_with(subcommand))
        .expect("logged request")
}

fn parse_logged_request(line: &str) -> Value {
    let payload = logged_request_payload(line);
    serde_json::from_str(payload).expect("json request")
}

fn logged_request_payload(line: &str) -> &str {
    line.split_once('\t').expect("tab").1
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn refresh_with_preclaimed_inflight(fixture: &FakeProviderFixture) -> RefreshOutcome {
    let context = preclaimed_inflight_context();
    refresh_with_inflight_context(fixture, &context)
}

struct PreclaimedInFlightContext {
    providers: ProvidersConfig,
    in_flight: InFlight,
    state: StateDb,
}

fn preclaimed_inflight_context() -> PreclaimedInFlightContext {
    let providers = ProvidersConfig::default();
    let in_flight = InFlight::new();
    let state = StateDb::open(Path::new(":memory:")).expect("state");
    PreclaimedInFlightContext {
        providers,
        in_flight,
        state,
    }
}

fn refresh_with_inflight_context(
    fixture: &FakeProviderFixture,
    context: &PreclaimedInFlightContext,
) -> RefreshOutcome {
    let _guard = claim_provider_in_flight(&context.in_flight);
    quota_service(fixture)
        .refresh_quota(request(
            &context.providers,
            &context.in_flight,
            &context.state,
        ))
        .expect("service output")
        .outcome
}

fn claim_provider_in_flight(in_flight: &InFlight) -> InFlightGuard<'_> {
    in_flight.try_claim(PROVIDER_NAME).expect("claim")
}

fn assert_success_projection(outcome: RefreshOutcome) {
    assert_updated_window_projection(outcome, 0.58, "2100-01-01T00:00:00+00:00");
}

fn assert_updated_window_projection(
    outcome: RefreshOutcome,
    expected_used: f64,
    expected_reset: &str,
) {
    match outcome {
        RefreshOutcome::Updated { windows } => {
            assert_eq!(windows.len(), 1);
            assert!((windows[0].used_percent - expected_used).abs() < 1e-9);
            assert_eq!(windows[0].resets_at.to_rfc3339(), expected_reset);
        }
        other => panic!("expected Updated, got {other:?}"),
    }
}

fn assert_host_quota_timestamp(state: &StateDb) {
    assert_host_quota_record(host_quota_record(state));
}

fn host_quota_record(state: &StateDb) -> oulipoly_state::QuotaRecord {
    state
        .get_quota(PROVIDER_NAME)
        .expect("get quota")
        .expect("quota row")
}

fn assert_host_quota_record(quota: oulipoly_state::QuotaRecord) {
    assert_eq!(quota.provider_name, PROVIDER_NAME);
    assert!(
        quota
            .refreshed_at
            .expect("host timestamp")
            .timestamp_millis()
            > 0
    );
}

fn assert_source_false_without_probe(outcome: RefreshOutcome, fixture: &FakeProviderFixture) {
    assert!(matches!(outcome, RefreshOutcome::NoScript));
    let log = provider_log(fixture);
    assert!(log.contains("quota.source"));
    assert!(!log.contains("quota.probe"));
}

fn assert_already_in_flight_before_provider_calls(
    outcome: RefreshOutcome,
    fixture: &FakeProviderFixture,
) {
    assert!(matches!(outcome, RefreshOutcome::AlreadyInFlight));
    assert_eq!(provider_log(fixture), "");
}

fn assert_logged_request_identity(fixture: &FakeProviderFixture, subcommand: &str) {
    let request = logged_request(fixture, subcommand);
    assert!(
        request["request_id"]
            .as_str()
            .unwrap()
            .starts_with("external-quota-")
    );
    assert_eq!(request["provider_instance_id"], PROVIDER_NAME);
    assert_eq!(request["params"]["settings_id"], SETTINGS_ID);
    assert_eq!(request["params"]["model_name"], MODEL_NAME);
}

fn assert_projection_failed(outcome: RefreshOutcome) {
    match outcome {
        RefreshOutcome::Failed(message) => {
            assert!(message.contains("external provider quota projection failed"));
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

fn assert_retry_updated(outcome: RefreshOutcome) {
    match outcome {
        RefreshOutcome::Updated { windows } => {
            assert_eq!(windows.len(), 1);
            assert!((windows[0].used_percent - 0.75).abs() < 1e-9);
        }
        other => panic!("expected Updated, got {other:?}"),
    }
}

fn assert_retry_call_counts(fixture: &FakeProviderFixture) {
    let log = provider_log(fixture);
    assert_eq!(log.matches("quota.probe").count(), 2);
    assert_eq!(log.matches("quota.refresh_auth").count(), 1);
}

fn refresh_window_projection(remaining_ratio: f64) -> RefreshOutcome {
    let fixture = success_fixture_with_window(remaining_ratio, 0);
    let (outcome, _state) = refresh_with_fixture(&fixture);
    outcome
}

#[test]
fn external_probe_success_projects_and_persists_provider_name() {
    let fixture = success_fixture();
    let (outcome, state) = refresh_with_fixture(&fixture);
    assert_success_projection(outcome);
    assert_host_quota_timestamp(&state);
}

#[test]
fn external_window_projection_maps_remaining_ratio_boundaries() {
    assert_updated_window_projection(
        refresh_window_projection(1.0),
        0.0,
        "1970-01-01T00:00:00+00:00",
    );
    assert_updated_window_projection(
        refresh_window_projection(0.0),
        1.0,
        "1970-01-01T00:00:00+00:00",
    );
}

#[test]
fn external_source_false_maps_to_no_script_without_probe() {
    let fixture = source_false_fixture();
    let (outcome, _state) = refresh_with_fixture(&fixture);
    assert_source_false_without_probe(outcome, &fixture);
}

#[test]
fn external_in_flight_returns_before_registry_or_provider_calls() {
    let fixture = success_fixture();
    let outcome = refresh_with_preclaimed_inflight(&fixture);
    assert_already_in_flight_before_provider_calls(outcome, &fixture);
}

#[test]
fn external_request_envelopes_include_neutral_settings_identity() {
    let fixture = success_fixture();
    let (_outcome, _state) = refresh_with_fixture(&fixture);
    for subcommand in ["quota.source", "quota.probe"] {
        assert_logged_request_identity(&fixture, subcommand);
    }
}

#[test]
fn external_invalid_window_maps_to_failed_without_builtin_fallback() {
    let fixture = invalid_window_fixture();
    let (outcome, _state) = refresh_with_fixture(&fixture);
    assert_projection_failed(outcome);
}

#[test]
fn external_probe_error_refreshes_auth_and_retries_once() {
    let fixture = retry_fixture();
    let (outcome, _state) = refresh_with_fixture(&fixture);
    assert_retry_updated(outcome);
    assert_retry_call_counts(&fixture);
}
