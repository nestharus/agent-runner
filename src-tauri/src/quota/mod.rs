//! Per-provider quota refresh. Runs a user-supplied script (from
//! `providers.toml`) that hits the provider's usage API and prints JSON on
//! stdout. The parsed reading lands in `provider_quotas` + `provider_quota_windows`.

use crate::config::ProvidersConfig;
use crate::state::{QuotaWindowInput, StateDb};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashSet;
use std::process::{Command, Stdio};
use std::sync::Mutex;

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
    /// Either a 0..1 fraction or 0..100 percent. Normalized during parsing.
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

    let Some(_guard) = in_flight.try_claim(provider_name) else {
        return RefreshOutcome::AlreadyInFlight;
    };

    match run_script(script) {
        Ok(windows) => {
            if let Err(e) = state.upsert_quota_refresh(provider_name, &windows) {
                return RefreshOutcome::Failed(e);
            }
            RefreshOutcome::Updated { windows }
        }
        Err(e) => RefreshOutcome::Failed(e),
    }
}

/// True if the provider has no cached quota OR its oldest refresh is past
/// the dynamic TTL computed from its window lengths. TTL is
/// `min(hours_until_reset) / DIVISOR`, clamped to `[MIN_TTL, MAX_TTL]`.
/// A provider row with zero windows is inconsistent state; force stale.
pub fn is_stale(state: &StateDb, provider_name: &str) -> bool {
    let Ok(Some(q)) = state.get_quota(provider_name) else {
        return true;
    };
    let Some(refreshed_at) = q.refreshed_at else {
        return true;
    };
    let windows = state.get_windows(provider_name).unwrap_or_default();
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

fn run_script(script: &str) -> Result<Vec<QuotaWindowInput>, String> {
    use std::io::Read;

    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(script);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn quota script: {e}"))?;

    // Drain stdout/stderr concurrently to avoid pipe-full deadlocks for
    // scripts that write a lot (unlikely for quota scripts but consistent
    // with the sessions-module pattern).
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let stdout_handle = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = { stdout }.read_to_string(&mut buf);
        buf
    });
    let stderr_handle = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = { stderr }.read_to_string(&mut buf);
        buf
    });

    let timeout = std::time::Duration::from_secs(SCRIPT_TIMEOUT_SECS);
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    return Err(format!(
                        "Quota script timed out after {SCRIPT_TIMEOUT_SECS}s"
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => return Err(format!("Quota script wait failed: {e}")),
        }
    };

    let stdout_text = stdout_handle.join().unwrap_or_default();
    let stderr_text = stderr_handle.join().unwrap_or_default();

    if !status.success() {
        return Err(format!(
            "Quota script exited {}: {}",
            status.code().unwrap_or(-1),
            stderr_text.trim()
        ));
    }

    parse_output(&stdout_text)
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
        // Normalize used_percent: scripts can emit 0..1 OR 0..100.
        // Heuristic: values >1.0 are treated as 0..100 and divided.
        let used = if w.used_percent > 1.0 {
            (w.used_percent / 100.0).clamp(0.0, 1.0)
        } else {
            w.used_percent.clamp(0.0, 1.0)
        };
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
            {"used_percent":23, "resets_at":"2026-04-23T19:00:00Z"},
            {"used_percent":0.45, "resets_at":"2026-04-17T15:00:00Z"}
        ]}"#;
        let windows = parse_output(json).unwrap();
        assert_eq!(windows.len(), 2);
        // First: 23 → 0.23; second: already 0.45
        assert!((windows[0].used_percent - 0.23).abs() < 1e-6);
        assert!((windows[1].used_percent - 0.45).abs() < 1e-6);
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
        };
        let seven_day = QuotaWindow {
            provider_name: "x".into(),
            window_id: 1,
            used_percent: 0.0,
            resets_at: hours_from_now(24 * 7),
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
        };
        let ttl = dynamic_ttl_secs(&[near_reset]);
        assert_eq!(ttl, MIN_TTL_SECS);
    }

    #[test]
    fn ttl_empty_windows_falls_back_to_max() {
        assert_eq!(dynamic_ttl_secs(&[]), MAX_TTL_SECS);
    }
}
