//! Per-provider quota refresh. Runs a user-supplied script (from
//! `providers.toml`) that hits the provider's usage API and prints JSON on
//! stdout. The parsed reading lands in `provider_quotas` + `provider_quota_windows`.

use crate::config::ProvidersConfig;
use crate::process::{
    CommandSpec, OutputSpec, ProcessErrorPhase, ProcessRunner, StdinSpec,
    process_error_phase_and_detail,
};
use crate::state::QuotaRepository;
use crate::state::QuotaWindowInput;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;

/// Minimum refresh TTL. Below 5 minutes we burn API calls without useful
/// signal change; the density projection already catches short-term spikes.
const MIN_TTL_SECS: i64 = 5 * 60;
/// Maximum refresh TTL. We never go longer than 24h even if every window is
/// long — covers the edge case where a script emits no resets_at.
const MAX_TTL_SECS: i64 = 24 * 3600;
/// Denominator for dynamic TTL: refresh N times per window lifetime.
const REFRESH_WINDOW_DIVISOR: i64 = 5;

/// Script timeout — scripts hitting the internet shouldn't hang the caller.
const SCRIPT_TIMEOUT_SECS: u64 = 30;
/// Auth-refresh command timeout. Should be quick (the CLI hits its own auth
/// endpoint and exits); kept tight to avoid hanging the quota path.
const REFRESH_TIMEOUT_SECS: u64 = 15;

/// Tracks in-flight refreshes so two callers for the same provider collapse
/// into one run. The set holds provider names currently being refreshed.
#[derive(Default)]
pub struct InFlight {
    inner: Mutex<HashSet<String>>,
}

impl InFlight {
    pub fn new() -> Self {
        Self::default()
    }

    /// Try to claim a slot for this provider. Returns a guard that releases
    /// on drop, or None if the provider is already being refreshed.
    pub fn try_claim(&self, provider: &str) -> Option<InFlightGuard<'_>> {
        let mut set = self.inner.lock().ok()?;
        if set.contains(provider) {
            return None;
        }
        set.insert(provider.to_string());
        Some(InFlightGuard {
            set: &self.inner,
            name: provider.to_string(),
        })
    }
}

pub struct InFlightGuard<'a> {
    set: &'a Mutex<HashSet<String>>,
    name: String,
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut set) = self.set.lock() {
            set.remove(&self.name);
        }
    }
}

/// Script output shape — prefer the new `windows` array, fall back to the
/// old flat `{used_percent, resets_at}` shape so existing scripts keep working.
#[derive(Debug, Deserialize)]
struct QuotaScriptOutput {
    /// New multi-window shape. One entry per rolling window the CLI exposes.
    #[serde(default)]
    windows: Option<Vec<QuotaScriptWindow>>,
    /// Legacy single-window shape.
    #[serde(default)]
    used_percent: Option<f64>,
    #[serde(default)]
    resets_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct QuotaScriptWindow {
    /// Percent on a 0..100 scale. Values outside that range are rejected.
    /// Scripts wrapping APIs that report a 0..1 fraction must multiply by 100
    /// before emitting.
    used_percent: f64,
    resets_at: String,
}

#[derive(Debug)]
pub enum RefreshOutcome {
    Updated {
        windows: Vec<QuotaWindowInput>,
    },
    /// Provider is missing a `quota_script` entry in providers.toml — caller
    /// should fall back to invocation-count.
    NoScript,
    /// Another caller is already refreshing — use the existing cached value.
    AlreadyInFlight,
    /// Script ran but the output didn't parse / exit was non-zero.
    Failed(String),
}

/// Refresh one provider's quota. Caller is responsible for checking staleness.
///
/// If the script fails or returns an empty windows list on a provider that
/// previously had non-empty windows (the typical signature of an expired
/// OAuth token where the bundled script silently degrades to empty output),
/// and an `auth_refresh_command` is configured, the runner shells out to it
/// — letting each CLI's own OAuth code handle token refresh — and retries
/// the quota script once. Refresh-command failure is non-fatal on its own;
/// only the retry's outcome is recorded.
pub fn refresh_provider(
    provider_name: &str,
    providers_cfg: &ProvidersConfig,
    in_flight: &InFlight,
    quota_repo: &dyn QuotaRepository,
    runner: &dyn ProcessRunner,
) -> RefreshOutcome {
    let Some(entry) = providers_cfg.get(provider_name) else {
        return RefreshOutcome::NoScript;
    };
    let Some(script) = entry.quota_script.as_deref() else {
        return RefreshOutcome::NoScript;
    };

    let Some(_guard) = in_flight.try_claim(provider_name) else {
        return RefreshOutcome::AlreadyInFlight;
    };

    let first = run_script(runner, script);
    if should_attempt_auth_refresh(provider_name, &first, quota_repo)
        && let Some(refresh_cmd) = entry.auth_refresh_command.as_deref()
    {
        let refresh_err = run_refresh_command(runner, refresh_cmd).err();
        match run_script(runner, script) {
            Ok(windows) => {
                if let Err(e) = quota_repo.upsert_quota_refresh(provider_name, &windows) {
                    return RefreshOutcome::Failed(e);
                }
                return RefreshOutcome::Updated { windows };
            }
            Err(retry_err) => {
                let msg = match refresh_err {
                    Some(r) => format!("{retry_err} (auth_refresh_command also failed: {r})"),
                    None => retry_err,
                };
                return RefreshOutcome::Failed(msg);
            }
        }
    }

    match first {
        Ok(windows) => {
            if let Err(e) = quota_repo.upsert_quota_refresh(provider_name, &windows) {
                return RefreshOutcome::Failed(e);
            }
            RefreshOutcome::Updated { windows }
        }
        Err(e) => RefreshOutcome::Failed(e),
    }
}

/// Decide whether to invoke `auth_refresh_command` after a quota script run.
/// Triggers on hard failure or on an empty-windows result for a provider that
/// previously had non-empty windows (transient empty on first contact is not
/// a refresh signal — there's no stale token to repair).
fn should_attempt_auth_refresh(
    provider_name: &str,
    result: &Result<Vec<QuotaWindowInput>, String>,
    quota_repo: &dyn QuotaRepository,
) -> bool {
    match result {
        Err(_) => true,
        Ok(windows) if windows.is_empty() => quota_repo
            .get_windows(provider_name)
            .map(|prior| !prior.is_empty())
            .unwrap_or(false),
        Ok(_) => false,
    }
}

/// True if the provider has no cached quota OR its oldest refresh is past
/// the dynamic TTL computed from its window lengths. TTL is
/// `min(hours_until_reset) / DIVISOR`, clamped to `[MIN_TTL, MAX_TTL]`.
/// A provider row with zero windows is inconsistent state; force stale.
pub fn is_stale(quota_repo: &dyn QuotaRepository, provider_name: &str) -> bool {
    let Ok(Some(q)) = quota_repo.get_quota(provider_name) else {
        return true;
    };
    let Some(refreshed_at) = q.refreshed_at else {
        return true;
    };
    let windows = quota_repo.get_windows(provider_name).unwrap_or_default();
    if windows.is_empty() {
        return true;
    }
    let ttl_secs = dynamic_ttl_secs(&windows);
    let age_secs = (Utc::now() - refreshed_at).num_seconds();
    age_secs >= ttl_secs
}

/// Compute the refresh TTL for a provider based on its reported windows.
/// If no windows are present (first-time fetch for the provider), fall back
/// to MAX_TTL — we want some signal before burning API calls.
pub fn dynamic_ttl_secs(windows: &[crate::state::QuotaWindow]) -> i64 {
    if windows.is_empty() {
        return MAX_TTL_SECS;
    }
    let now = Utc::now();
    let min_hours = windows
        .iter()
        .map(|w| (w.resets_at - now).num_seconds().max(0))
        .min()
        .unwrap_or(MAX_TTL_SECS);
    (min_hours / REFRESH_WINDOW_DIVISOR).clamp(MIN_TTL_SECS, MAX_TTL_SECS)
}

fn run_refresh_command(runner: &dyn ProcessRunner, cmd_str: &str) -> Result<(), String> {
    let output = runner
        .run(CommandSpec {
            program: "sh".to_string(),
            args: vec!["-c".to_string(), cmd_str.to_string()],
            cwd: None,
            env: Default::default(),
            stdin: StdinSpec::Null,
            stdout: OutputSpec::Null,
            stderr: OutputSpec::Capture,
            timeout: Some(Duration::from_secs(REFRESH_TIMEOUT_SECS)),
            description: "auth_refresh_command".to_string(),
        })
        .map_err(|e| {
            if e.contains("timed out") {
                format!("auth_refresh_command timed out after {REFRESH_TIMEOUT_SECS}s")
            } else {
                quota_process_error("auth_refresh_command", &e)
            }
        })?;

    if output.exit_code != 0 {
        let stderr_text = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "auth_refresh_command exited {}: {}",
            output.exit_code,
            stderr_text.trim()
        ));
    }
    Ok(())
}

fn run_script(runner: &dyn ProcessRunner, script: &str) -> Result<Vec<QuotaWindowInput>, String> {
    let output = runner
        .run(CommandSpec {
            program: "sh".to_string(),
            args: vec!["-c".to_string(), script.to_string()],
            cwd: None,
            env: Default::default(),
            stdin: StdinSpec::Null,
            stdout: OutputSpec::Capture,
            stderr: OutputSpec::Capture,
            timeout: Some(Duration::from_secs(SCRIPT_TIMEOUT_SECS)),
            description: "Quota script".to_string(),
        })
        .map_err(|e| {
            if e.contains("timed out") {
                format!("Quota script timed out after {SCRIPT_TIMEOUT_SECS}s")
            } else {
                quota_process_error("quota script", &e)
            }
        })?;

    let stdout_text = String::from_utf8_lossy(&output.stdout);
    let stderr_text = String::from_utf8_lossy(&output.stderr);

    if output.exit_code != 0 {
        return Err(format!(
            "Quota script exited {}: {}",
            output.exit_code,
            stderr_text.trim()
        ));
    }

    parse_output(&stdout_text)
}

fn quota_process_error(kind: &str, error: &str) -> String {
    let (phase, detail) = process_error_phase_and_detail(error);
    match (kind, phase) {
        ("auth_refresh_command", Some(ProcessErrorPhase::Wait)) => {
            format!("auth_refresh_command wait failed: {detail}")
        }
        ("quota script", Some(ProcessErrorPhase::Wait)) => {
            format!("Quota script wait failed: {detail}")
        }
        ("auth_refresh_command", _) => format!("Failed to spawn auth_refresh_command: {detail}"),
        ("quota script", _) => format!("Failed to spawn quota script: {detail}"),
        _ => detail.to_string(),
    }
}

fn parse_output(stdout: &str) -> Result<Vec<QuotaWindowInput>, String> {
    let trimmed = stdout.trim();
    let parsed: QuotaScriptOutput = serde_json::from_str(trimmed)
        .map_err(|e| format!("Invalid JSON from quota script: {e} (got: {stdout})"))?;

    let raw_windows: Vec<QuotaScriptWindow> = match parsed.windows {
        Some(ws) => ws,
        None => {
            // Legacy single-window shape.
            let Some(pct) = parsed.used_percent else {
                return Err(format!(
                    "quota script emitted neither `windows` nor `used_percent` (got: {stdout})"
                ));
            };
            let Some(resets_at) = parsed.resets_at else {
                return Err(format!(
                    "legacy quota script emitted `used_percent` without `resets_at` (got: {stdout})"
                ));
            };
            vec![QuotaScriptWindow {
                used_percent: pct,
                resets_at,
            }]
        }
    };

    let mut out = Vec::with_capacity(raw_windows.len());
    for w in raw_windows {
        let resets_at = DateTime::parse_from_rfc3339(&w.resets_at)
            .map_err(|e| format!("Bad resets_at {}: {e}", w.resets_at))?
            .with_timezone(&Utc);
        if !(0.0..=100.0).contains(&w.used_percent) || w.used_percent.is_nan() {
            return Err(format!(
                "quota script emitted used_percent={} outside 0..100 (got: {stdout})",
                w.used_percent
            ));
        }
        let used = w.used_percent / 100.0;
        out.push(QuotaWindowInput {
            used_percent: used,
            resets_at,
        });
    }
    Ok(out)
}

// Keep for tests that want to model "short" vs "long" windows by constructing
// synthetic resets_at values.
#[cfg(test)]
fn hours_from_now(h: i64) -> DateTime<Utc> {
    Utc::now() + chrono::Duration::hours(h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::OsProcessRunner;
    use crate::state::StateDb;

    #[test]
    fn in_flight_single_claim() {
        let f = InFlight::new();
        let g = f.try_claim("claude").unwrap();
        assert!(f.try_claim("claude").is_none());
        drop(g);
        assert!(f.try_claim("claude").is_some());
    }

    #[test]
    fn in_flight_different_providers() {
        let f = InFlight::new();
        let _a = f.try_claim("claude").unwrap();
        let _b = f.try_claim("codex").unwrap();
    }

    #[test]
    fn parse_multi_window_output() {
        let json = r#"{"windows":[
            {"used_percent":1, "resets_at":"2026-04-23T19:00:00Z"},
            {"used_percent":86, "resets_at":"2026-04-17T15:00:00Z"}
        ]}"#;
        let windows = parse_output(json).unwrap();
        assert_eq!(windows.len(), 2);
        assert!((windows[0].used_percent - 0.01).abs() < 1e-6);
        assert!((windows[1].used_percent - 0.86).abs() < 1e-6);
    }

    #[test]
    fn parse_legacy_single_window_output() {
        let json = r#"{"used_percent":12, "resets_at":"2026-04-23T19:00:00Z"}"#;
        let windows = parse_output(json).unwrap();
        assert_eq!(windows.len(), 1);
        assert!((windows[0].used_percent - 0.12).abs() < 1e-6);
    }

    #[test]
    fn parse_rejects_legacy_without_resets_at() {
        let json = r#"{"used_percent":12}"#;
        assert!(parse_output(json).is_err());
    }

    #[test]
    fn parse_rejects_used_percent_above_100() {
        let json = r#"{"windows":[{"used_percent":150, "resets_at":"2026-04-23T19:00:00Z"}]}"#;
        let err = parse_output(json).unwrap_err();
        assert!(err.contains("outside 0..100"), "unexpected error: {err}");
    }

    #[test]
    fn parse_rejects_negative_used_percent() {
        let json = r#"{"windows":[{"used_percent":-1, "resets_at":"2026-04-23T19:00:00Z"}]}"#;
        let err = parse_output(json).unwrap_err();
        assert!(err.contains("outside 0..100"), "unexpected error: {err}");
    }

    #[test]
    fn is_stale_forces_refresh_when_windows_empty() {
        let state = StateDb::open(std::path::Path::new(":memory:")).unwrap();
        state
            .insert_quota_row_without_windows_for_test("p", &Utc::now())
            .unwrap();

        assert!(is_stale(&state, "p"));
    }

    #[test]
    fn is_stale_honors_ttl_when_windows_present() {
        let state = StateDb::open(std::path::Path::new(":memory:")).unwrap();
        let window = QuotaWindowInput {
            used_percent: 0.10,
            resets_at: hours_from_now(24),
        };
        state.upsert_quota_refresh("p", &[window]).unwrap();

        assert!(!is_stale(&state, "p"));
    }

    #[test]
    fn is_stale_treats_missing_quota_row_as_stale() {
        let state = StateDb::open(std::path::Path::new(":memory:")).unwrap();

        assert!(is_stale(&state, "p"));
    }

    #[test]
    fn ttl_shrinks_for_short_windows() {
        use crate::state::QuotaWindow;
        let five_hour = QuotaWindow {
            provider_name: "x".into(),
            window_id: 0,
            used_percent: 0.0,
            resets_at: hours_from_now(5),
            last_delta_percent: None,
            last_delta_calls: None,
        };
        let seven_day = QuotaWindow {
            provider_name: "x".into(),
            window_id: 1,
            used_percent: 0.0,
            resets_at: hours_from_now(24 * 7),
            last_delta_percent: None,
            last_delta_calls: None,
        };
        // min(5h, 168h) / 5 ≈ 1h, clamped within [5min, 24h] → 1h
        let ttl = dynamic_ttl_secs(&[five_hour, seven_day]);
        assert!((3500..=3700).contains(&ttl), "expected ~1h, got {ttl}s");
    }

    #[test]
    fn ttl_clamps_to_min_for_nearly_expired_windows() {
        use crate::state::QuotaWindow;
        let near_reset = QuotaWindow {
            provider_name: "x".into(),
            window_id: 0,
            used_percent: 0.0,
            resets_at: Utc::now() + chrono::Duration::seconds(10),
            last_delta_percent: None,
            last_delta_calls: None,
        };
        let ttl = dynamic_ttl_secs(&[near_reset]);
        assert_eq!(ttl, MIN_TTL_SECS);
    }

    #[test]
    fn ttl_empty_windows_falls_back_to_max() {
        assert_eq!(dynamic_ttl_secs(&[]), MAX_TTL_SECS);
    }

    /// Build a ProvidersConfig with one provider, optionally configured
    /// with an auth_refresh_command.
    fn cfg(name: &str, quota_script: &str, refresh: Option<&str>) -> ProvidersConfig {
        use crate::config::ProviderEntry;
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

        let runner = OsProcessRunner;
        let outcome = refresh_provider("p", &providers, &in_flight, &state, &runner);
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

        let runner = OsProcessRunner;
        let outcome = refresh_provider("p", &providers, &in_flight, &state, &runner);
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

        let runner = OsProcessRunner;
        let outcome = refresh_provider("p", &providers, &in_flight, &state, &runner);
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

        let runner = OsProcessRunner;
        let outcome = refresh_provider("p", &providers, &in_flight, &state, &runner);
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

        let runner = OsProcessRunner;
        let outcome = refresh_provider("p", &providers, &in_flight, &state, &runner);
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
