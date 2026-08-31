//! ## Declared roles
//!
//! `orchestration`, `mapper`, `accessor`, `formatter`, `predicate`, `validator`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/quota/refresh.rs
//!     role: adapter
//!     Translates:
//!       - quota service port contract (`QuotaServicePort`, `QuotaServiceRequest`, `QuotaServiceOutput`, `QuotaServiceExternalProviderIdentity`, `ServiceError`)
//!       - quota refresh primitive contract (`InFlight`, `RefreshOutcome`, `QuotaScriptWindow`, `parse_output`, `run_script`, `run_auth_refresh_command_coalesced`, `AuthRefreshAttempt`)
//!       - quota state persistence contract (`StateDb`, `QuotaWindowInput`, `upsert_quota_refresh`, `get_windows`)
//!       - provider config and external-provider registry contract (`ProvidersConfig`, `ProviderRegistryHandle`)
//!       - in-file test harness contract (`EnvGuard`, `chrono::Utc`, `tempfile::TempDir`, `ProviderEntry`, in-memory `StateDb` fixtures)
//! ```
use super::{
    AuthRefreshAttempt, InFlight, QuotaScriptWindow, RefreshOutcome, parse_output,
    run_auth_refresh_command_coalesced, run_script,
};
use crate::provider_registry::ProviderRegistryHandle;
use crate::services::{
    QuotaServiceExternalProviderIdentity, QuotaServiceOutput, QuotaServicePort,
    QuotaServiceRequest, ServiceError,
};
use oulipoly_config::ProvidersConfig;
use oulipoly_state::{QuotaWindowInput, StateDb};

#[derive(Debug, Clone, Default)]
pub struct RuntimeQuotaService {
    provider_registry: Option<ProviderRegistryHandle>,
}

#[allow(non_upper_case_globals)]
pub const RuntimeQuotaService: RuntimeQuotaService = RuntimeQuotaService {
    provider_registry: None,
};

impl RuntimeQuotaService {
    pub fn with_registry_handle(provider_registry: ProviderRegistryHandle) -> Self {
        Self {
            provider_registry: Some(provider_registry),
        }
    }
}

impl QuotaServicePort for RuntimeQuotaService {
    fn refresh_quota(
        &self,
        request: QuotaServiceRequest<'_>,
    ) -> Result<QuotaServiceOutput, ServiceError> {
        let outcome = match request.external_provider.clone() {
            Some(identity) => self.refresh_external(request, identity),
            None => refresh_provider(
                &request.provider_name,
                request.providers_cfg,
                request.in_flight,
                request.state,
            ),
        };

        Ok(QuotaServiceOutput { outcome })
    }
}

impl RuntimeQuotaService {
    fn refresh_external(
        &self,
        request: QuotaServiceRequest<'_>,
        identity: QuotaServiceExternalProviderIdentity,
    ) -> RefreshOutcome {
        let registry = match external_provider_registry(&self.provider_registry) {
            Ok(registry) => registry,
            Err(outcome) => return outcome,
        };
        super::external_provider::refresh_external_provider_quota(registry, request, identity)
    }
}

fn external_provider_registry(
    registry: &Option<ProviderRegistryHandle>,
) -> Result<&ProviderRegistryHandle, RefreshOutcome> {
    registry
        .as_ref()
        .ok_or_else(external_provider_registry_unavailable)
}

fn external_provider_registry_unavailable() -> RefreshOutcome {
    RefreshOutcome::Failed(external_provider_registry_unavailable_message())
}

fn external_provider_registry_unavailable_message() -> String {
    "external provider quota registry is unavailable".to_string()
}

/// Refresh one provider's quota. Caller is responsible for checking staleness.
///
/// If the script fails or returns an empty windows list on a provider that
/// previously had non-empty windows (the typical signature of an expired
/// OAuth token where the bundled script silently degrades to empty output),
/// and an `auth_refresh_command` is configured, the runner shells out to it
/// -- letting each CLI's own OAuth code handle token refresh -- and retries
/// the quota script once. Refresh-command failure is non-fatal on its own;
/// only the retry's outcome is recorded.
pub fn refresh_provider(
    provider_name: &str,
    providers_cfg: &ProvidersConfig,
    in_flight: &InFlight,
    state: &StateDb,
) -> RefreshOutcome {
    let Some(entry) = providers_cfg.get(provider_name) else {
        return RefreshOutcome::NoScript;
    };
    let Some(script) = entry.quota_script.as_deref() else {
        return RefreshOutcome::NoScript;
    };
    refresh_provider_from_script(
        provider_name,
        script,
        entry.auth_refresh_command.as_deref(),
        in_flight,
        state,
    )
}

/// Refresh a provider for routing from its provider-owned quota source.
pub fn refresh_provider_for_routing(
    provider_name: &str,
    providers_cfg: &ProvidersConfig,
    in_flight: &InFlight,
    state: &StateDb,
) -> RefreshOutcome {
    let Some(source) = super::source::refresh_source(provider_name, providers_cfg) else {
        return RefreshOutcome::NoScript;
    };
    refresh_provider_from_script(
        provider_name,
        &source.script,
        source.auth_refresh_command.as_deref(),
        in_flight,
        state,
    )
}

fn refresh_provider_from_script(
    provider_name: &str,
    script: &str,
    auth_refresh_command: Option<&str>,
    in_flight: &InFlight,
    state: &StateDb,
) -> RefreshOutcome {
    let Some(_guard) = in_flight.try_claim(provider_name) else {
        return RefreshOutcome::AlreadyInFlight;
    };

    let first = run_quota_script(script);
    if let (true, Some(refresh_cmd)) = (
        should_attempt_auth_refresh(provider_name, &first, state),
        auth_refresh_command,
    ) {
        return refresh_after_auth_command(provider_name, script, refresh_cmd, state);
    }
    persist_script_result(provider_name, first, state)
}

fn run_quota_script(script: &str) -> Result<Vec<QuotaScriptWindow>, String> {
    run_script(script).and_then(|raw| parse_output(&raw))
}

fn refresh_after_auth_command(
    provider_name: &str,
    script: &str,
    refresh_cmd: &str,
    state: &StateDb,
) -> RefreshOutcome {
    let refresh_err = auth_refresh_error(provider_name, refresh_cmd);
    match run_quota_script(script) {
        Ok(windows) => persist_windows(provider_name, windows, state),
        Err(retry_err) => RefreshOutcome::Failed(combine_retry_error(retry_err, refresh_err)),
    }
}

/// Run `auth_refresh_command` under the per-account single-flight lock. Only a
/// shell-out we actually performed can contribute an error to the combined
/// retry message; a coalesced or lock-unavailable attempt is a clean skip
/// (another holder owns the rotation, or we fail closed), so there is nothing
/// to combine and the quota script is simply retried with the current token.
fn auth_refresh_error(provider_name: &str, refresh_cmd: &str) -> Option<String> {
    match run_auth_refresh_command_coalesced(provider_name, refresh_cmd) {
        AuthRefreshAttempt::Ran(result) => result.err(),
        AuthRefreshAttempt::Coalesced | AuthRefreshAttempt::LockUnavailable => None,
    }
}

fn persist_script_result(
    provider_name: &str,
    result: Result<Vec<QuotaScriptWindow>, String>,
    state: &StateDb,
) -> RefreshOutcome {
    match result {
        Ok(windows) => persist_windows(provider_name, windows, state),
        Err(e) => RefreshOutcome::Failed(e),
    }
}

fn persist_windows(
    provider_name: &str,
    windows: Vec<QuotaScriptWindow>,
    state: &StateDb,
) -> RefreshOutcome {
    let routing_windows = quota_window_inputs(&windows);
    let persist_result = upsert_quota_windows(state, provider_name, &routing_windows);
    refresh_outcome_from_persist_result(persist_result, routing_windows)
}

fn quota_window_inputs(windows: &[QuotaScriptWindow]) -> Vec<QuotaWindowInput> {
    windows
        .iter()
        .map(QuotaScriptWindow::to_quota_window_input)
        .collect()
}

fn upsert_quota_windows(
    state: &StateDb,
    provider_name: &str,
    windows: &[QuotaWindowInput],
) -> Result<(), String> {
    state.upsert_quota_refresh(provider_name, windows)
}

fn refresh_outcome_from_persist_result(
    result: Result<(), String>,
    routing_windows: Vec<QuotaWindowInput>,
) -> RefreshOutcome {
    if let Err(e) = result {
        return RefreshOutcome::Failed(e);
    }
    RefreshOutcome::Updated {
        windows: routing_windows,
    }
}

fn combine_retry_error(retry_err: String, refresh_err: Option<String>) -> String {
    match refresh_err {
        Some(r) => format!("{retry_err} (auth_refresh_command also failed: {r})"),
        None => retry_err,
    }
}

/// Decide whether to invoke `auth_refresh_command` after a quota script run.
/// Triggers on hard failure or on an empty-windows result for a provider that
/// previously had non-empty windows (transient empty on first contact is not
/// a refresh signal -- there's no stale token to repair).
fn should_attempt_auth_refresh(
    provider_name: &str,
    result: &Result<Vec<QuotaScriptWindow>, String>,
    state: &StateDb,
) -> bool {
    match result {
        Err(_) => true,
        Ok(windows) if windows.is_empty() => has_prior_windows(state, provider_name),
        Ok(_) => false,
    }
}

fn has_prior_windows(state: &StateDb, provider_name: &str) -> bool {
    state
        .get_windows(provider_name)
        .map(|prior| !prior.is_empty())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota::marker_verification::test_support::EnvGuard;
    use chrono::Utc;
    use oulipoly_config::ProviderEntry;

    const TEST_PROVIDER: &str = "p";

    struct RefreshFixture {
        _dir: Option<tempfile::TempDir>,
        // Per-test data home so the auth-refresh single-flight lock's freshness
        // stamp starts empty and never coalesces one test's shell-out against
        // another's. The EnvGuard also holds the process-wide env lock for the
        // fixture lifetime, serializing these tests with the marker tests.
        _lock_home: tempfile::TempDir,
        _env: EnvGuard,
        marker: Option<std::path::PathBuf>,
        providers: ProvidersConfig,
        state: StateDb,
        in_flight: InFlight,
    }

    /// Build a ProvidersConfig with one provider, optionally configured
    /// with an auth_refresh_command.
    fn cfg(name: &str, quota_script: &str, refresh: Option<&str>) -> ProvidersConfig {
        let mut c = ProvidersConfig::default();
        c.entries.insert(
            name.to_string(),
            ProviderEntry {
                quota_script: Some(quota_script.to_string()),
                auth_refresh_command: refresh.map(|s| s.to_string()),
                ..ProviderEntry::default()
            },
        );
        c
    }

    /// Pre-populate one window so `should_attempt_auth_refresh` sees prior data.
    fn seed_prior_windows(state: &StateDb, provider: &str) {
        state
            .upsert_quota_refresh(
                provider,
                &[QuotaWindowInput {
                    used_percent: 0.50,
                    resets_at: hours_from_now(48),
                }],
            )
            .unwrap();
    }

    fn hours_from_now(h: i64) -> chrono::DateTime<Utc> {
        Utc::now() + chrono::Duration::hours(h)
    }

    fn refresh_fixture(
        providers: ProvidersConfig,
        marker: Option<std::path::PathBuf>,
        dir: Option<tempfile::TempDir>,
    ) -> RefreshFixture {
        let lock_home = tempfile::tempdir().unwrap();
        let env = EnvGuard::set_many(vec![
            (
                "OULIPOLY_DATA_DIR",
                Some(
                    lock_home
                        .path()
                        .join(oulipoly_state::paths::APP_DATA_DIR_NAME)
                        .into_os_string(),
                ),
            ),
            (
                "OULIPOLY_DATA_HOME",
                Some(lock_home.path().as_os_str().to_os_string()),
            ),
        ]);
        RefreshFixture {
            _dir: dir,
            _lock_home: lock_home,
            _env: env,
            marker,
            providers,
            state: StateDb::open(std::path::Path::new(":memory:")).unwrap(),
            in_flight: InFlight::new(),
        }
    }

    fn empty_then_success_fixture() -> RefreshFixture {
        marker_refresh_fixture("refreshed", empty_then_success_quota_script)
    }

    fn first_empty_fixture() -> RefreshFixture {
        marker_refresh_fixture("ran", |_| empty_windows_quota_script())
    }

    fn failure_then_success_fixture() -> RefreshFixture {
        marker_refresh_fixture("refreshed", failure_then_success_quota_script)
    }

    fn marker_refresh_fixture(
        marker_name: &str,
        quota_script_for_marker: fn(&std::path::Path) -> String,
    ) -> RefreshFixture {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join(marker_name);
        let quota_script = quota_script_for_marker(&marker);
        let refresh_cmd = touch_marker_command(&marker);
        let providers = providers_for_quota_fixture(&quota_script, &refresh_cmd);
        refresh_fixture(providers, Some(marker), Some(dir))
    }

    fn empty_then_success_quota_script(marker: &std::path::Path) -> String {
        format!(
            "if [ -e {m} ]; then echo '{{\"windows\":[{{\"used_percent\":7,\"resets_at\":\"2099-01-01T00:00:00Z\"}}]}}'; else echo '{{\"windows\":[]}}'; fi",
            m = marker.display()
        )
    }

    fn empty_windows_quota_script() -> String {
        "echo '{\"windows\":[]}'".to_string()
    }

    fn failure_then_success_quota_script(marker: &std::path::Path) -> String {
        format!(
            "if [ -e {m} ]; then echo '{{\"windows\":[{{\"used_percent\":3,\"resets_at\":\"2099-01-01T00:00:00Z\"}}]}}'; else echo 'auth error' >&2; exit 1; fi",
            m = marker.display()
        )
    }

    fn touch_marker_command(marker: &std::path::Path) -> String {
        format!("touch {}", marker.display())
    }

    fn providers_for_quota_fixture(quota_script: &str, refresh_cmd: &str) -> ProvidersConfig {
        cfg(TEST_PROVIDER, quota_script, Some(refresh_cmd))
    }

    fn retry_still_fails_fixture() -> RefreshFixture {
        let providers = cfg(TEST_PROVIDER, "exit 1", Some("true"));
        refresh_fixture(providers, None, None)
    }

    fn both_fail_fixture() -> RefreshFixture {
        let providers = cfg(
            TEST_PROVIDER,
            "exit 1",
            Some("echo 'token gone' >&2; exit 7"),
        );
        refresh_fixture(providers, None, None)
    }

    fn run_refresh_fixture(fixture: &RefreshFixture) -> RefreshOutcome {
        refresh_provider(
            TEST_PROVIDER,
            &fixture.providers,
            &fixture.in_flight,
            &fixture.state,
        )
    }

    fn assert_updated_window(outcome: RefreshOutcome, expected_used_percent: f64, context: &str) {
        match outcome {
            RefreshOutcome::Updated { windows } => {
                assert_eq!(windows.len(), 1);
                assert!((windows[0].used_percent - expected_used_percent).abs() < 1e-6);
            }
            other => panic!("expected Updated {context}, got {other:?}"),
        }
    }

    fn assert_empty_updated(outcome: RefreshOutcome) {
        assert!(
            matches!(outcome, RefreshOutcome::Updated { ref windows } if windows.is_empty()),
            "expected Updated with empty windows, got {outcome:?}"
        );
    }

    fn assert_failed_outcome(outcome: RefreshOutcome) {
        assert!(
            matches!(outcome, RefreshOutcome::Failed(_)),
            "expected Failed, got {outcome:?}"
        );
    }

    fn assert_combined_auth_refresh_error(outcome: RefreshOutcome) {
        match outcome {
            RefreshOutcome::Failed(msg) => {
                assert!(
                    msg.contains("auth_refresh_command also failed"),
                    "expected combined message, got: {msg}"
                );
                assert!(msg.contains("token gone"), "missing refresh stderr: {msg}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    fn assert_marker_exists(fixture: &RefreshFixture, message: &str) {
        assert!(
            fixture
                .marker
                .as_ref()
                .is_some_and(|marker| marker.exists()),
            "{message}"
        );
    }

    fn assert_marker_missing(fixture: &RefreshFixture, message: &str) {
        assert!(
            fixture
                .marker
                .as_ref()
                .is_some_and(|marker| !marker.exists()),
            "{message}"
        );
    }

    #[test]
    fn refresh_runs_auth_refresh_when_script_empty_and_prior_exists() {
        let fixture = empty_then_success_fixture();
        seed_prior_windows(&fixture.state, TEST_PROVIDER);

        let outcome = run_refresh_fixture(&fixture);

        assert_updated_window(outcome, 0.07, "after empty script auth refresh");
        assert_marker_exists(&fixture, "auth_refresh_command should have run");
    }

    #[test]
    fn refresh_skips_auth_refresh_when_no_prior_windows() {
        let fixture = first_empty_fixture();

        let outcome = run_refresh_fixture(&fixture);

        assert_empty_updated(outcome);
        assert_marker_missing(
            &fixture,
            "auth_refresh_command must not run on a first-time empty result",
        );
    }

    #[test]
    fn refresh_runs_auth_refresh_on_script_failure_then_retries() {
        let fixture = failure_then_success_fixture();

        let outcome = run_refresh_fixture(&fixture);

        assert_updated_window(outcome, 0.03, "after retry");
        assert_marker_exists(&fixture, "auth_refresh_command should have run");
    }

    #[test]
    fn refresh_returns_failed_when_retry_still_fails() {
        let fixture = retry_still_fails_fixture();

        let outcome = run_refresh_fixture(&fixture);

        assert_failed_outcome(outcome);
    }

    #[test]
    fn refresh_includes_auth_refresh_error_when_both_fail() {
        let fixture = both_fail_fixture();

        let outcome = run_refresh_fixture(&fixture);

        assert_combined_auth_refresh_error(outcome);
    }
}
