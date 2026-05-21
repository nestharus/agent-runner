//! Verify-before-honor for `provider_quotas.next_available_at` markers.
//!
//! ## Declared roles
//! orchestration, predicate, accessor, validator
//!
//! Stale `next_available_at` (the AGE-163 working-set "provider is
//! unavailable until <ts>" marker) survives across a successful `--usage`
//! refresh because `write_quota_aggregate` only clears `exhausted_at`. The
//! routing layer then keeps treating the provider as unrouteable even when
//! every visible window is healthy.
//!
//! This module is the verification surface the balancer consults before
//! honoring such a marker. It:
//!
//! 1. Treats markers within `MARKER_RELEASE_SLACK_SECS` of their stated
//!    release as already expired (speculative retry near release time).
//! 2. Reuses the cached refresh when one happened within
//!    `MARKER_VERIFICATION_COOLDOWN_SECS`; verifies the cached windows
//!    against the marker and clears it if windows are healthy.
//! 3. Otherwise acquires a per-provider cross-process `flock(2)` over
//!    `<lock_dir>/<provider>.lock` (under `OULIPOLY_DATA_HOME`/
//!    `XDG_DATA_HOME`/platform data dir, suffixed
//!    `oulipoly-agent-runner/usage-refresh-locks`), runs `--usage` once,
//!    and clears the marker when the fresh windows look healthy. Many
//!    concurrent callers fan into one refresh — peers see the holder's
//!    write when they wake.
//!
//! The cooldown and slack are env-overridable
//! (`OULIPOLY_MARKER_VERIFICATION_COOLDOWN_SECS`,
//! `OULIPOLY_MARKER_RELEASE_SLACK_SECS`) for ops to tighten or loosen
//! without a rebuild.

use crate::quota::{InFlight, RefreshOutcome, refresh_provider_for_routing};
use chrono::{DateTime, Duration, Utc};
use oulipoly_config::{ProvidersConfig, SessionsConfig};
use oulipoly_state::{QuotaRecord, QuotaWindow, StateDb};
use std::fs::{self, File, OpenOptions};
use std::path::PathBuf;

/// Per-provider refresh cooldown. While a successful `--usage` is younger
/// than this we trust the cached windows and skip the script call. 60s
/// matches the `usage-refresh-locks` dir convention and is well under the
/// 5h smallest rolling window — see `MIN_TTL_SECS` upstream.
pub const DEFAULT_MARKER_VERIFICATION_COOLDOWN_SECS: i64 = 60;

/// Slack ahead of `next_available_at`. Once the marker is within this many
/// seconds of its stated release we treat it as already expired. Routing
/// then dispatches; if upstream is still down, the failure path re-applies
/// the marker with a fresh release timestamp.
pub const DEFAULT_MARKER_RELEASE_SLACK_SECS: i64 = 60;

const COOLDOWN_ENV_VAR: &str = "OULIPOLY_MARKER_VERIFICATION_COOLDOWN_SECS";
const SLACK_ENV_VAR: &str = "OULIPOLY_MARKER_RELEASE_SLACK_SECS";

/// Used-percent threshold (0..=1.0) above which a live window pins the
/// provider as exhausted. Mirrors balancer's `EXHAUSTED_USED_PERCENT` —
/// kept inline to avoid a balancer ↔ quota cycle on the constant.
const EXHAUSTED_USED_PERCENT: f64 = 1.0;

fn cooldown_secs() -> i64 {
    parse_env_secs(COOLDOWN_ENV_VAR).unwrap_or(DEFAULT_MARKER_VERIFICATION_COOLDOWN_SECS)
}

fn release_slack_secs() -> i64 {
    parse_env_secs(SLACK_ENV_VAR).unwrap_or(DEFAULT_MARKER_RELEASE_SLACK_SECS)
}

fn parse_env_secs(name: &str) -> Option<i64> {
    std::env::var(name).ok()?.trim().parse::<i64>().ok()
}

/// Verify a possibly-stale `next_available_at` marker against the live
/// quota, clearing it when the marker is past its slack window or the
/// refreshed cache shows a healthy provider. No-op when the provider has
/// no marker. Refresh I/O is dedup'd across callers via the per-provider
/// file lock (cross-process) and the existing `InFlight` set (intra-process).
pub fn verify_or_clear_marker(
    state: &StateDb,
    providers_cfg: &ProvidersConfig,
    sessions_cfg: &SessionsConfig,
    in_flight: &InFlight,
    provider_name: &str,
    now: DateTime<Utc>,
) {
    let Some(quota) = read_quota(state, provider_name) else {
        return;
    };
    let Some(next_at) = quota.next_available_at else {
        return;
    };
    if marker_within_release_slack(next_at, now) {
        clear_marker(state, provider_name);
        return;
    }
    if cached_refresh_is_fresh(quota.refreshed_at, now) {
        verify_against_cached_windows(state, provider_name, now);
        return;
    }
    refresh_and_verify_under_lock(
        state,
        providers_cfg,
        sessions_cfg,
        in_flight,
        provider_name,
        now,
    );
}

fn read_quota(state: &StateDb, provider_name: &str) -> Option<QuotaRecord> {
    state.get_quota(provider_name).ok().flatten()
}

fn marker_within_release_slack(next_at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    next_at <= now + Duration::seconds(release_slack_secs())
}

fn cached_refresh_is_fresh(refreshed_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    let Some(refreshed_at) = refreshed_at else {
        return false;
    };
    now - refreshed_at < Duration::seconds(cooldown_secs())
}

fn verify_against_cached_windows(state: &StateDb, provider_name: &str, now: DateTime<Utc>) {
    if provider_is_actually_healthy(state, provider_name, now) {
        clear_marker(state, provider_name);
    }
}

fn refresh_and_verify_under_lock(
    state: &StateDb,
    providers_cfg: &ProvidersConfig,
    sessions_cfg: &SessionsConfig,
    in_flight: &InFlight,
    provider_name: &str,
    now: DateTime<Utc>,
) {
    let Ok(_guard) = RefreshFileLock::acquire_blocking(provider_name) else {
        // Lock acquisition failed (e.g. data dir not writable). Conservative
        // behaviour: leave the marker as-is rather than guess at health.
        return;
    };
    if !still_needs_refresh_under_lock(state, provider_name, now) {
        verify_against_cached_windows(state, provider_name, now);
        return;
    }
    let outcome =
        refresh_provider_for_routing(provider_name, providers_cfg, sessions_cfg, in_flight, state);
    if matches!(outcome, RefreshOutcome::Updated { .. })
        && provider_is_actually_healthy(state, provider_name, now)
    {
        clear_marker(state, provider_name);
    }
}

fn still_needs_refresh_under_lock(
    state: &StateDb,
    provider_name: &str,
    now: DateTime<Utc>,
) -> bool {
    let Some(quota) = read_quota(state, provider_name) else {
        return true;
    };
    !cached_refresh_is_fresh(quota.refreshed_at, now)
}

fn provider_is_actually_healthy(state: &StateDb, provider_name: &str, now: DateTime<Utc>) -> bool {
    let windows = state.get_windows(provider_name).unwrap_or_default();
    if windows.is_empty() {
        return false;
    }
    !windows
        .iter()
        .any(|window| window_pins_exhausted(window, now))
}

fn window_pins_exhausted(window: &QuotaWindow, now: DateTime<Utc>) -> bool {
    window.resets_at > now && window.used_percent >= EXHAUSTED_USED_PERCENT
}

fn clear_marker(state: &StateDb, provider_name: &str) {
    if let Err(err) = state.clear_provider_unavailable(provider_name) {
        tracing::warn!(
            provider_name = provider_name,
            error = err.as_str(),
            "failed to clear stale next_available_at marker"
        );
    }
}

/// Per-provider blocking file lock — `flock(2)` over a single sentinel
/// per provider under the usage-refresh-locks directory. Lifetime owns the
/// open fd; drop releases the lock.
pub(crate) struct RefreshFileLock {
    _file: File,
}

impl RefreshFileLock {
    pub(crate) fn acquire_blocking(provider_name: &str) -> Result<Self, String> {
        let dir = usage_lock_dir();
        fs::create_dir_all(&dir).map_err(|err| {
            format!(
                "failed to create usage-refresh-locks dir {}: {err}",
                dir.display()
            )
        })?;
        let path = dir.join(format!("{}.lock", sanitize_lock_name(provider_name)));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|err| format!("failed to open lock file {}: {err}", path.display()))?;
        <File as fs4::FileExt>::lock(&file)
            .map_err(|err| format!("failed to flock {}: {err}", path.display()))?;
        Ok(Self { _file: file })
    }
}

impl Drop for RefreshFileLock {
    fn drop(&mut self) {
        let _ = <File as fs4::FileExt>::unlock(&self._file);
    }
}

fn usage_lock_dir() -> PathBuf {
    if let Some(root) = std::env::var_os("OULIPOLY_DATA_HOME") {
        return PathBuf::from(root)
            .join("oulipoly-agent-runner")
            .join("usage-refresh-locks");
    }
    if let Some(root) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(root)
            .join("oulipoly-agent-runner")
            .join("usage-refresh-locks");
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("oulipoly-agent-runner")
        .join("usage-refresh-locks")
}

fn sanitize_lock_name(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_config::{ProviderEntry, ProvidersConfig, SessionsConfig};
    use oulipoly_state::QuotaWindowInput;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;
    use tempfile::tempdir;

    fn open_db() -> StateDb {
        StateDb::open(Path::new(":memory:")).unwrap()
    }

    fn isolated_lock_dir() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        // Safety: tests run in serial-by-default for env mutation; we never
        // unset DATA_HOME so subsequent tests in the same process still see
        // an isolated path under the TempDir owned by the caller.
        unsafe {
            std::env::set_var("OULIPOLY_DATA_HOME", dir.path());
        }
        dir
    }

    fn seed_marker(db: &StateDb, provider: &str, next_at: DateTime<Utc>) {
        db.record_provider_unavailable(provider, Some(next_at), "UpstreamApiDown")
            .unwrap();
    }

    fn seed_window(db: &StateDb, provider: &str, used: f64, resets_in_hours: i64) {
        db.upsert_quota_refresh(
            provider,
            &[QuotaWindowInput {
                used_percent: used,
                resets_at: Utc::now() + Duration::hours(resets_in_hours),
            }],
        )
        .unwrap();
    }

    fn providers_with_script(provider: &str, script: &str) -> ProvidersConfig {
        let mut cfg = ProvidersConfig::default();
        cfg.entries.insert(
            provider.to_string(),
            ProviderEntry {
                quota_script: Some(script.to_string()),
                ..ProviderEntry::default()
            },
        );
        cfg
    }

    #[test]
    fn marker_within_release_slack_is_cleared_without_refresh() {
        let _lock_dir = isolated_lock_dir();
        let db = open_db();
        let now = Utc::now();
        seed_marker(&db, "p", now + Duration::seconds(5));

        // Script that would fail if called — proves we did NOT refresh.
        let providers = providers_with_script("p", "exit 1");
        let sessions = SessionsConfig::default();
        let in_flight = InFlight::new();
        verify_or_clear_marker(&db, &providers, &sessions, &in_flight, "p", now);

        let quota = db.get_quota("p").unwrap().expect("row exists");
        assert!(
            quota.next_available_at.is_none(),
            "marker should be cleared when within slack of release"
        );
    }

    #[test]
    fn marker_with_fresh_cache_and_healthy_windows_is_cleared() {
        let _lock_dir = isolated_lock_dir();
        let db = open_db();
        let now = Utc::now();
        // Healthy windows from a fresh refresh.
        seed_window(&db, "p", 0.10, 5);
        seed_marker(&db, "p", now + Duration::hours(1));

        let providers = providers_with_script("p", "exit 1");
        let sessions = SessionsConfig::default();
        let in_flight = InFlight::new();
        verify_or_clear_marker(&db, &providers, &sessions, &in_flight, "p", now);

        let quota = db.get_quota("p").unwrap().expect("row exists");
        assert!(
            quota.next_available_at.is_none(),
            "marker should be cleared when cache is fresh and healthy"
        );
    }

    #[test]
    fn marker_with_fresh_cache_and_exhausted_window_is_kept() {
        let _lock_dir = isolated_lock_dir();
        let db = open_db();
        let now = Utc::now();
        // Exhausted live window — verification must NOT clear the marker.
        seed_window(&db, "p", 1.0, 2);
        let marker_at = now + Duration::hours(1);
        seed_marker(&db, "p", marker_at);

        let providers = providers_with_script("p", "exit 1");
        let sessions = SessionsConfig::default();
        let in_flight = InFlight::new();
        verify_or_clear_marker(&db, &providers, &sessions, &in_flight, "p", now);

        let quota = db.get_quota("p").unwrap().expect("row exists");
        assert_eq!(quota.next_available_at, Some(marker_at));
    }

    #[test]
    fn marker_with_stale_cache_runs_refresh_and_clears_when_healthy() {
        let _lock_dir = isolated_lock_dir();
        let db = open_db();
        let now = Utc::now();
        // Marker set; no prior refresh ⇒ refreshed_at is None ⇒ stale.
        seed_marker(&db, "p", now + Duration::hours(1));

        // Healthy refresh script.
        let providers = providers_with_script(
            "p",
            r#"echo '{"windows":[{"used_percent":3,"resets_at":"2099-01-01T00:00:00Z"}]}'"#,
        );
        let sessions = SessionsConfig::default();
        let in_flight = InFlight::new();
        verify_or_clear_marker(&db, &providers, &sessions, &in_flight, "p", now);

        let quota = db.get_quota("p").unwrap().expect("row exists");
        assert!(
            quota.next_available_at.is_none(),
            "marker should be cleared after healthy refresh"
        );
        assert!(quota.refreshed_at.is_some());
    }

    #[test]
    fn marker_with_stale_cache_keeps_marker_when_refresh_shows_exhausted() {
        let _lock_dir = isolated_lock_dir();
        let db = open_db();
        let now = Utc::now();
        let marker_at = now + Duration::hours(1);
        seed_marker(&db, "p", marker_at);

        // Refresh returns a saturated 5h window.
        let providers = providers_with_script(
            "p",
            r#"echo '{"windows":[{"used_percent":100,"resets_at":"2099-01-01T00:00:00Z"}]}'"#,
        );
        let sessions = SessionsConfig::default();
        let in_flight = InFlight::new();
        verify_or_clear_marker(&db, &providers, &sessions, &in_flight, "p", now);

        let quota = db.get_quota("p").unwrap().expect("row exists");
        assert_eq!(
            quota.next_available_at,
            Some(marker_at),
            "marker should survive refresh that reports exhaustion"
        );
    }

    #[test]
    fn no_marker_no_action() {
        let _lock_dir = isolated_lock_dir();
        let db = open_db();
        let now = Utc::now();
        // Provider has a refresh row but no marker.
        seed_window(&db, "p", 0.10, 5);

        let providers = providers_with_script("p", "exit 1");
        let sessions = SessionsConfig::default();
        let in_flight = InFlight::new();
        // Must not panic, must not call the (failing) script.
        verify_or_clear_marker(&db, &providers, &sessions, &in_flight, "p", now);
    }

    #[test]
    fn many_threads_collapse_into_one_refresh() {
        let _lock_dir = isolated_lock_dir();
        let dir = tempdir().unwrap();
        let counter_path = dir.path().join("calls");
        std::fs::write(&counter_path, "").unwrap();

        // Script appends a byte per invocation; later we count.
        let script = format!(
            r#"printf '.' >> {p}; echo '{{"windows":[{{"used_percent":4,"resets_at":"2099-01-01T00:00:00Z"}}]}}'"#,
            p = counter_path.to_string_lossy()
        );

        let db_path = dir.path().join("state.db");
        let providers = providers_with_script("p", &script);
        // Seed marker via direct StateDb (this DB will be reused by threads).
        let db = StateDb::open(&db_path).unwrap();
        seed_marker(&db, "p", Utc::now() + Duration::hours(1));
        drop(db);

        let in_flight = Arc::new(InFlight::new());
        let providers = Arc::new(providers);
        let sessions = Arc::new(SessionsConfig::default());
        let db_path = Arc::new(db_path);
        let attempts = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(50));

        let handles: Vec<_> = (0..50)
            .map(|_| {
                let in_flight = in_flight.clone();
                let providers = providers.clone();
                let sessions = sessions.clone();
                let db_path = db_path.clone();
                let attempts = attempts.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    let db = StateDb::open(&db_path).unwrap();
                    let now = Utc::now();
                    barrier.wait();
                    verify_or_clear_marker(&db, &providers, &sessions, &in_flight, "p", now);
                    attempts.fetch_add(1, Ordering::SeqCst);
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(attempts.load(Ordering::SeqCst), 50);
        let script_calls = std::fs::read_to_string(&counter_path).unwrap().len();
        assert_eq!(
            script_calls, 1,
            "exactly one --usage script call should fan into 50 waiters"
        );

        let final_db = StateDb::open(&db_path).unwrap();
        let quota = final_db.get_quota("p").unwrap().expect("row exists");
        assert!(quota.next_available_at.is_none());
    }
}
