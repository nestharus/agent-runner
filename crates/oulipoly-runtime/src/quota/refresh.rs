use super::{
    InFlight, QuotaScriptWindow, RefreshOutcome, parse_output, run_refresh_command, run_script,
};
use crate::services::{QuotaServiceOutput, QuotaServicePort, QuotaServiceRequest, ServiceError};
use oulipoly_config::{ProvidersConfig, SessionsConfig};
use oulipoly_state::{QuotaWindowInput, StateDb};

pub struct RuntimeQuotaService;

impl QuotaServicePort for RuntimeQuotaService {
    fn refresh_quota(
        &self,
        request: QuotaServiceRequest<'_>,
    ) -> Result<QuotaServiceOutput, ServiceError> {
        let outcome = refresh_provider(
            &request.provider_name,
            request.providers_cfg,
            request.in_flight,
            request.state,
        );

        Ok(QuotaServiceOutput { outcome })
    }
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

/// Refresh a provider for routing. Prefer the explicit providers.toml
/// `quota_script`; when legacy migrated configs only have provider/session
/// storage adapters, derive the standard quota adapter from those roots.
pub fn refresh_provider_for_routing(
    provider_name: &str,
    providers_cfg: &ProvidersConfig,
    sessions_cfg: &SessionsConfig,
    in_flight: &InFlight,
    state: &StateDb,
) -> RefreshOutcome {
    let Some(source) = super::source::refresh_source(provider_name, providers_cfg, sessions_cfg)
    else {
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
    let refresh_err = run_refresh_command(refresh_cmd).err();
    match run_quota_script(script) {
        Ok(windows) => persist_windows(provider_name, windows, state),
        Err(retry_err) => RefreshOutcome::Failed(combine_retry_error(retry_err, refresh_err)),
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
    let routing_windows: Vec<QuotaWindowInput> = windows
        .iter()
        .map(QuotaScriptWindow::to_quota_window_input)
        .collect();
    if let Err(e) = state.upsert_quota_refresh(provider_name, &routing_windows) {
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
    use chrono::Utc;
    use oulipoly_config::ProviderEntry;

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

    #[test]
    fn refresh_runs_auth_refresh_when_script_empty_and_prior_exists() {
        // First quota_script call emits empty windows; auth_refresh_command
        // creates a marker; second quota_script call sees the marker and
        // emits populated windows.
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("refreshed");
        let quota_script = format!(
            "if [ -e {m} ]; then echo '{{\"windows\":[{{\"used_percent\":7,\"resets_at\":\"2099-01-01T00:00:00Z\"}}]}}'; else echo '{{\"windows\":[]}}'; fi",
            m = marker.display()
        );
        let refresh_cmd = format!("touch {}", marker.display());
        let providers = cfg("p", &quota_script, Some(&refresh_cmd));

        let state = StateDb::open(std::path::Path::new(":memory:")).unwrap();
        seed_prior_windows(&state, "p");
        let in_flight = InFlight::new();

        let outcome = refresh_provider("p", &providers, &in_flight, &state);
        match outcome {
            RefreshOutcome::Updated { windows } => {
                assert_eq!(windows.len(), 1);
                assert!((windows[0].used_percent - 0.07).abs() < 1e-6);
            }
            other => panic!("expected Updated, got {other:?}"),
        }
        assert!(marker.exists(), "auth_refresh_command should have run");
    }

    #[test]
    fn refresh_skips_auth_refresh_when_no_prior_windows() {
        // Empty windows on a first-time fetch is normal (not an expired
        // token). auth_refresh_command must NOT run.
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ran");
        let quota_script = "echo '{\"windows\":[]}'".to_string();
        let refresh_cmd = format!("touch {}", marker.display());
        let providers = cfg("p", &quota_script, Some(&refresh_cmd));

        let state = StateDb::open(std::path::Path::new(":memory:")).unwrap();
        let in_flight = InFlight::new();

        let outcome = refresh_provider("p", &providers, &in_flight, &state);
        assert!(
            matches!(outcome, RefreshOutcome::Updated { ref windows } if windows.is_empty()),
            "expected Updated with empty windows, got {outcome:?}"
        );
        assert!(
            !marker.exists(),
            "auth_refresh_command must not run on a first-time empty result"
        );
    }

    #[test]
    fn refresh_runs_auth_refresh_on_script_failure_then_retries() {
        // First call exits 1; refresh creates marker; second call succeeds.
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("refreshed");
        let quota_script = format!(
            "if [ -e {m} ]; then echo '{{\"windows\":[{{\"used_percent\":3,\"resets_at\":\"2099-01-01T00:00:00Z\"}}]}}'; else echo 'auth error' >&2; exit 1; fi",
            m = marker.display()
        );
        let refresh_cmd = format!("touch {}", marker.display());
        let providers = cfg("p", &quota_script, Some(&refresh_cmd));

        let state = StateDb::open(std::path::Path::new(":memory:")).unwrap();
        let in_flight = InFlight::new();

        let outcome = refresh_provider("p", &providers, &in_flight, &state);
        match outcome {
            RefreshOutcome::Updated { windows } => {
                assert_eq!(windows.len(), 1);
                assert!((windows[0].used_percent - 0.03).abs() < 1e-6);
            }
            other => panic!("expected Updated after retry, got {other:?}"),
        }
        assert!(marker.exists());
    }

    #[test]
    fn refresh_returns_failed_when_retry_still_fails() {
        let quota_script = "exit 1".to_string();
        let refresh_cmd = "true".to_string();
        let providers = cfg("p", &quota_script, Some(&refresh_cmd));

        let state = StateDb::open(std::path::Path::new(":memory:")).unwrap();
        let in_flight = InFlight::new();

        let outcome = refresh_provider("p", &providers, &in_flight, &state);
        assert!(
            matches!(outcome, RefreshOutcome::Failed(_)),
            "expected Failed, got {outcome:?}"
        );
    }

    #[test]
    fn refresh_includes_auth_refresh_error_when_both_fail() {
        let quota_script = "exit 1".to_string();
        let refresh_cmd = "echo 'token gone' >&2; exit 7".to_string();
        let providers = cfg("p", &quota_script, Some(&refresh_cmd));

        let state = StateDb::open(std::path::Path::new(":memory:")).unwrap();
        let in_flight = InFlight::new();

        let outcome = refresh_provider("p", &providers, &in_flight, &state);
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
}
