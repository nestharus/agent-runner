//! Per-provider quota refresh. Runs a user-supplied script (from
//! `providers.toml`) that hits the provider's usage API and prints JSON on
//! stdout. The parsed reading lands in `provider_quotas`.

use crate::config::ProvidersConfig;
use crate::state::StateDb;
use chrono::Utc;
use serde::Deserialize;
use std::collections::HashSet;
use std::process::{Command, Stdio};
use std::sync::Mutex;

/// Staleness threshold: don't re-run the script for a provider that was
/// refreshed within this window.
pub const REFRESH_TTL_HOURS: i64 = 12;

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

#[derive(Debug, Deserialize)]
struct QuotaScriptOutput {
    /// Either a 0..1 fraction or 0..100 percent. Normalized during parsing.
    used_percent: f64,
    #[serde(default)]
    resets_at: Option<String>,
}

#[derive(Debug)]
pub enum RefreshOutcome {
    Updated {
        used_percent: f64,
        resets_at: Option<String>,
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
        Ok((used, resets_at)) => {
            if let Err(e) = state.upsert_quota_refresh(provider_name, used, resets_at.as_deref()) {
                return RefreshOutcome::Failed(e);
            }
            RefreshOutcome::Updated {
                used_percent: used,
                resets_at,
            }
        }
        Err(e) => RefreshOutcome::Failed(e),
    }
}

/// True if the provider has no cached quota or was last refreshed more than
/// `REFRESH_TTL_HOURS` ago.
pub fn is_stale(state: &StateDb, provider_name: &str) -> bool {
    match state.get_quota(provider_name) {
        Ok(Some(q)) => match q.refreshed_at {
            Some(when) => {
                let age = Utc::now() - when;
                age.num_hours() >= REFRESH_TTL_HOURS
            }
            None => true,
        },
        _ => true,
    }
}

fn run_script(script: &str) -> Result<(f64, Option<String>), String> {
    // Shell out so users can wire `anthropic-usage ~/.claude/.credentials.json`
    // directly without worrying about argv splitting quirks.
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(script);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn quota script: {e}"))?;

    let timeout = std::time::Duration::from_secs(SCRIPT_TIMEOUT_SECS);
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    return Err(format!("Quota script timed out after {SCRIPT_TIMEOUT_SECS}s"));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => return Err(format!("Quota script wait failed: {e}")),
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed to read quota script output: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Quota script exited {}: {}",
            output.status.code().unwrap_or(-1),
            stderr.trim()
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: QuotaScriptOutput = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("Invalid JSON from quota script: {e} (got: {stdout})"))?;

    // Contract: scripts emit `used_percent` on a 0..100 scale (matches what
    // the underlying provider APIs return). Normalize to 0..1 here.
    let used = (parsed.used_percent / 100.0).clamp(0.0, 1.0);
    Ok((used, parsed.resets_at))
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
}
