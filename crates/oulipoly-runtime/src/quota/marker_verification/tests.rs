//! Marker verification behavior tests.
//!
//! ## Declared roles
//! orchestration, accessor, mapper, formatter, validator
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/quota/marker_verification/tests.rs
//!     role: intrinsic-surface
//!     Domain: marker_verification_behavior_harness
//!     Owns:
//!       - tempfile fixture construction
//!       - in-memory and file-backed StateDb marker/quota seeding
//!       - quota script fixture formatting
//!       - marker verification runtime assertions
//!       - EnvGuard process-environment isolation
//!       - multi-thread refresh fan-in harness
//! ```

use super::test_support::EnvGuard;
use super::*;
use chrono::Duration;
use oulipoly_config::{ProviderEntry, ProvidersConfig, SessionsConfig};
use oulipoly_state::QuotaWindowInput;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use tempfile::tempdir;

struct FanInResult {
    attempts: usize,
    script_calls: usize,
    marker_cleared: bool,
}

fn open_db() -> StateDb {
    StateDb::open(Path::new(":memory:")).unwrap()
}

/// Returns a tempdir AND an env guard that points `OULIPOLY_DATA_HOME`
/// at that tempdir for the caller's stack frame. Caller must keep both
/// alive — dropping either restores the env / removes the tempdir.
fn isolated_lock_dir() -> (tempfile::TempDir, EnvGuard) {
    isolated_lock_dir_with_env(Vec::new())
}

fn isolated_lock_dir_with_env(
    extra_env: Vec<(&'static str, Option<std::ffi::OsString>)>,
) -> (tempfile::TempDir, EnvGuard) {
    let dir = tempdir().unwrap();
    let vars = lock_dir_env_vars(dir.path(), extra_env);
    let guard = EnvGuard::set_many(vars);
    (dir, guard)
}

fn lock_dir_env_vars(
    path: &Path,
    extra_env: Vec<(&'static str, Option<std::ffi::OsString>)>,
) -> Vec<(&'static str, Option<std::ffi::OsString>)> {
    let mut vars = vec![("OULIPOLY_DATA_HOME", Some(path.as_os_str().to_os_string()))];
    vars.extend(extra_env);
    vars
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

fn providers_without_script(provider: &str) -> ProvidersConfig {
    let mut cfg = ProvidersConfig::default();
    cfg.entries
        .insert(provider.to_string(), ProviderEntry::default());
    cfg
}

fn exhausted_counting_refresh_script(counter_path: &Path) -> String {
    format!(
        r#"printf '.' >> {p}; echo '{{"windows":[{{"used_percent":100,"resets_at":"2099-01-01T00:00:00Z"}}]}}'"#,
        p = counter_path.to_string_lossy()
    )
}

fn healthy_counting_refresh_script(counter_path: &Path) -> String {
    format!(
        r#"printf '.' >> {p}; echo '{{"windows":[{{"used_percent":4,"resets_at":"2099-01-01T00:00:00Z"}}]}}'"#,
        p = counter_path.to_string_lossy()
    )
}

fn healthy_refresh_script() -> &'static str {
    r#"echo '{"windows":[{"used_percent":3,"resets_at":"2099-01-01T00:00:00Z"}]}'"#
}

fn exhausted_refresh_script() -> &'static str {
    r#"echo '{"windows":[{"used_percent":100,"resets_at":"2099-01-01T00:00:00Z"}]}'"#
}

fn assert_marker_retained(db: &StateDb, provider: &str, marker_at: DateTime<Utc>) {
    let quota = db.get_quota(provider).unwrap().expect("row exists");
    assert_eq!(quota.next_available_at, Some(marker_at));
    assert_eq!(quota.failure_class.as_deref(), Some("UpstreamApiDown"));
}

fn assert_marker_cleared(db: &StateDb, provider: &str) {
    let quota = db.get_quota(provider).unwrap().expect("row exists");
    assert!(quota.next_available_at.is_none());
}

#[test]
fn release_slack_secs_uses_default_trimmed_override_and_invalid_fallback() {
    {
        let _env_guard = EnvGuard::remove("OULIPOLY_MARKER_RELEASE_SLACK_SECS");
        assert_eq!(
            release_slack_secs(),
            config::DEFAULT_MARKER_RELEASE_SLACK_SECS
        );
    }

    {
        let _env_guard = EnvGuard::set("OULIPOLY_MARKER_RELEASE_SLACK_SECS", "  17  ");
        assert_eq!(release_slack_secs(), 17);
    }

    {
        let _env_guard = EnvGuard::set("OULIPOLY_MARKER_RELEASE_SLACK_SECS", "soon");
        assert_eq!(
            release_slack_secs(),
            config::DEFAULT_MARKER_RELEASE_SLACK_SECS
        );
    }
}

#[test]
fn cooldown_secs_uses_default_trimmed_override_and_invalid_fallback() {
    {
        let _env_guard = EnvGuard::remove("OULIPOLY_MARKER_VERIFICATION_COOLDOWN_SECS");
        assert_eq!(
            config::cooldown_secs(),
            config::DEFAULT_MARKER_VERIFICATION_COOLDOWN_SECS
        );
    }

    {
        let _env_guard = EnvGuard::set("OULIPOLY_MARKER_VERIFICATION_COOLDOWN_SECS", "  5  ");
        assert_eq!(config::cooldown_secs(), 5);
    }

    {
        let _env_guard = EnvGuard::set(
            "OULIPOLY_MARKER_VERIFICATION_COOLDOWN_SECS",
            "not-a-duration",
        );
        assert_eq!(
            config::cooldown_secs(),
            config::DEFAULT_MARKER_VERIFICATION_COOLDOWN_SECS
        );
    }
}

#[test]
fn shortened_cooldown_forces_stale_cache_refresh_instead_of_fresh_cache_bypass() {
    let (_lock_dir, _env_guard) = isolated_lock_dir_with_env(vec![(
        "OULIPOLY_MARKER_VERIFICATION_COOLDOWN_SECS",
        Some("0".into()),
    )]);
    let dir = tempdir().unwrap();
    let counter_path = dir.path().join("calls");
    let script = exhausted_counting_refresh_script(&counter_path);

    let db = open_db();
    seed_window(&db, "p", 0.10, 5);
    let now = Utc::now();
    let marker_at = now + Duration::hours(1);
    seed_marker(&db, "p", marker_at);

    let providers = providers_with_script("p", &script);
    let sessions = SessionsConfig::default();
    let in_flight = InFlight::new();
    verify_or_clear_marker(&db, &providers, &sessions, &in_flight, "p", now);

    assert_eq!(std::fs::read_to_string(&counter_path).unwrap().len(), 1);
    assert_marker_retained(&db, "p", marker_at);
}

#[test]
fn marker_with_fresh_cache_and_empty_windows_is_kept() {
    let (_lock_dir, _env_guard) = isolated_lock_dir();
    let db = open_db();
    let now = Utc::now();
    db.upsert_quota_refresh("p", &[]).unwrap();
    let marker_at = now + Duration::hours(1);
    seed_marker(&db, "p", marker_at);

    let providers = providers_with_script("p", "exit 1");
    let sessions = SessionsConfig::default();
    let in_flight = InFlight::new();
    verify_or_clear_marker(&db, &providers, &sessions, &in_flight, "p", now);

    assert_marker_retained(&db, "p", marker_at);
}

#[test]
fn past_reset_exhausted_window_does_not_pin_but_future_exhausted_window_does() {
    let (_lock_dir, _env_guard) = isolated_lock_dir();
    let db = open_db();
    let now = Utc::now();
    let past_marker_at = now + Duration::hours(1);
    let future_marker_at = now + Duration::hours(2);

    seed_window(&db, "past", 1.0, -1);
    seed_marker(&db, "past", past_marker_at);
    seed_window(&db, "future", 1.0, 1);
    seed_marker(&db, "future", future_marker_at);

    let providers = ProvidersConfig::default();
    let sessions = SessionsConfig::default();
    let in_flight = InFlight::new();
    verify_or_clear_marker(&db, &providers, &sessions, &in_flight, "past", now);
    verify_or_clear_marker(&db, &providers, &sessions, &in_flight, "future", now);

    assert_marker_cleared(&db, "past");
    assert_marker_retained(&db, "future", future_marker_at);
}

#[test]
fn stale_marker_with_configured_provider_but_no_quota_script_keeps_marker_and_failure_class() {
    let (_lock_dir, _env_guard) = isolated_lock_dir();
    let db = open_db();
    let now = Utc::now();
    let marker_at = now + Duration::hours(1);
    seed_marker(&db, "p", marker_at);

    let providers = providers_without_script("p");
    let sessions = SessionsConfig::default();
    let in_flight = InFlight::new();
    verify_or_clear_marker(&db, &providers, &sessions, &in_flight, "p", now);

    assert_marker_retained(&db, "p", marker_at);
    let quota = db.get_quota("p").unwrap().expect("row exists");
    assert!(quota.refreshed_at.is_none());
}

#[test]
fn stale_marker_with_failed_quota_script_keeps_marker_and_failure_class() {
    let (_lock_dir, _env_guard) = isolated_lock_dir();
    let db = open_db();
    let now = Utc::now();
    let marker_at = now + Duration::hours(1);
    seed_marker(&db, "p", marker_at);

    let providers = providers_with_script("p", "exit 2");
    let sessions = SessionsConfig::default();
    let in_flight = InFlight::new();
    verify_or_clear_marker(&db, &providers, &sessions, &in_flight, "p", now);

    assert_marker_retained(&db, "p", marker_at);
    let quota = db.get_quota("p").unwrap().expect("row exists");
    assert!(quota.refreshed_at.is_none());
}

#[test]
fn lock_name_sanitization_and_lock_dir_env_precedence_are_stable() {
    assert_eq!(lock::sanitize_lock_name("az-AZ_09"), "az-AZ_09");
    assert_eq!(lock::sanitize_lock_name("a/b c.☃"), "a_b_c__");

    let oulipoly_home = tempdir().unwrap();
    let xdg_home = tempdir().unwrap();
    {
        let _env_guard = EnvGuard::set_many(vec![
            (
                "OULIPOLY_DATA_HOME",
                Some(oulipoly_home.path().as_os_str().to_os_string()),
            ),
            (
                "XDG_DATA_HOME",
                Some(xdg_home.path().as_os_str().to_os_string()),
            ),
        ]);
        assert_eq!(
            lock::usage_lock_dir(),
            oulipoly_home
                .path()
                .join("oulipoly-agent-runner")
                .join("usage-refresh-locks")
        );
    }

    {
        let _env_guard = EnvGuard::set_many(vec![
            ("OULIPOLY_DATA_HOME", None),
            (
                "XDG_DATA_HOME",
                Some(xdg_home.path().as_os_str().to_os_string()),
            ),
        ]);
        assert_eq!(
            lock::usage_lock_dir(),
            xdg_home
                .path()
                .join("oulipoly-agent-runner")
                .join("usage-refresh-locks")
        );
    }
}

#[test]
fn env_guard_restores_previous_values_and_removes_absent_variables() {
    const RESTORE_ENV: &str = "OULIPOLY_TEST_ENV_GUARD_RESTORE";
    const ABSENT_ENV: &str = "OULIPOLY_TEST_ENV_GUARD_ABSENT";

    unsafe {
        std::env::set_var(RESTORE_ENV, "original");
        std::env::remove_var(ABSENT_ENV);
    }
    {
        let _env_guard = EnvGuard::set(RESTORE_ENV, "temporary");
        assert_eq!(std::env::var(RESTORE_ENV).as_deref(), Ok("temporary"));
    }
    assert_eq!(std::env::var(RESTORE_ENV).as_deref(), Ok("original"));

    {
        let _env_guard = EnvGuard::set(ABSENT_ENV, "temporary");
        assert_eq!(std::env::var(ABSENT_ENV).as_deref(), Ok("temporary"));
    }
    assert!(std::env::var_os(ABSENT_ENV).is_none());

    unsafe {
        std::env::remove_var(RESTORE_ENV);
    }
}

#[test]
fn marker_within_release_slack_is_cleared_without_refresh() {
    let (_lock_dir, _env_guard) = isolated_lock_dir();
    let db = open_db();
    let now = Utc::now();
    seed_marker(&db, "p", now + Duration::seconds(5));

    let providers = providers_with_script("p", "exit 1");
    let sessions = SessionsConfig::default();
    let in_flight = InFlight::new();
    verify_or_clear_marker(&db, &providers, &sessions, &in_flight, "p", now);

    assert_marker_cleared(&db, "p");
}

#[test]
fn marker_with_fresh_cache_and_healthy_windows_is_cleared() {
    let (_lock_dir, _env_guard) = isolated_lock_dir();
    let db = open_db();
    let now = Utc::now();
    seed_window(&db, "p", 0.10, 5);
    seed_marker(&db, "p", now + Duration::hours(1));

    let providers = providers_with_script("p", "exit 1");
    let sessions = SessionsConfig::default();
    let in_flight = InFlight::new();
    verify_or_clear_marker(&db, &providers, &sessions, &in_flight, "p", now);

    assert_marker_cleared(&db, "p");
}

#[test]
fn clear_marker_failure_does_not_panic_and_leaves_marker_intact() {
    let (_lock_dir, _env_guard) = isolated_lock_dir();
    let db = open_db();
    let now = Utc::now();
    let marker_at = now + Duration::hours(1);
    seed_window(&db, "p", 0.10, 5);
    seed_marker(&db, "p", marker_at);
    db.connection()
        .execute("PRAGMA query_only = ON", [])
        .unwrap();

    let providers = providers_with_script("p", "exit 1");
    let sessions = SessionsConfig::default();
    let in_flight = InFlight::new();
    verify_or_clear_marker(&db, &providers, &sessions, &in_flight, "p", now);

    assert_marker_retained(&db, "p", marker_at);
}

#[test]
fn marker_with_fresh_cache_and_exhausted_window_is_kept() {
    let (_lock_dir, _env_guard) = isolated_lock_dir();
    let db = open_db();
    let now = Utc::now();
    seed_window(&db, "p", 1.0, 2);
    let marker_at = now + Duration::hours(1);
    seed_marker(&db, "p", marker_at);

    let providers = providers_with_script("p", "exit 1");
    let sessions = SessionsConfig::default();
    let in_flight = InFlight::new();
    verify_or_clear_marker(&db, &providers, &sessions, &in_flight, "p", now);

    assert_marker_retained(&db, "p", marker_at);
}

#[test]
fn marker_with_stale_cache_runs_refresh_and_clears_when_healthy() {
    let (_lock_dir, _env_guard) = isolated_lock_dir();
    let db = open_db();
    let now = Utc::now();
    seed_marker(&db, "p", now + Duration::hours(1));

    let providers = providers_with_script("p", healthy_refresh_script());
    let sessions = SessionsConfig::default();
    let in_flight = InFlight::new();
    verify_or_clear_marker(&db, &providers, &sessions, &in_flight, "p", now);

    assert_marker_cleared(&db, "p");
    let quota = db.get_quota("p").unwrap().expect("row exists");
    assert!(quota.refreshed_at.is_some());
}

#[test]
fn marker_with_stale_cache_keeps_marker_when_refresh_shows_exhausted() {
    let (_lock_dir, _env_guard) = isolated_lock_dir();
    let db = open_db();
    let now = Utc::now();
    let marker_at = now + Duration::hours(1);
    seed_marker(&db, "p", marker_at);

    let providers = providers_with_script("p", exhausted_refresh_script());
    let sessions = SessionsConfig::default();
    let in_flight = InFlight::new();
    verify_or_clear_marker(&db, &providers, &sessions, &in_flight, "p", now);

    assert_marker_retained(&db, "p", marker_at);
}

#[test]
fn no_marker_no_action() {
    let (_lock_dir, _env_guard) = isolated_lock_dir();
    let db = open_db();
    let now = Utc::now();
    seed_window(&db, "p", 0.10, 5);

    let providers = providers_with_script("p", "exit 1");
    let sessions = SessionsConfig::default();
    let in_flight = InFlight::new();
    verify_or_clear_marker(&db, &providers, &sessions, &in_flight, "p", now);
}

#[test]
fn lock_dir_create_failure_retains_marker() {
    let dir = tempdir().unwrap();
    let file_root = dir.path().join("not-a-directory");
    std::fs::write(&file_root, "").unwrap();
    let _env_guard = EnvGuard::set("OULIPOLY_DATA_HOME", file_root.as_os_str());
    let db = open_db();
    let now = Utc::now();
    let marker_at = now + Duration::hours(1);
    seed_marker(&db, "p", marker_at);

    let providers = providers_with_script("p", healthy_refresh_script());
    let sessions = SessionsConfig::default();
    let in_flight = InFlight::new();
    verify_or_clear_marker(&db, &providers, &sessions, &in_flight, "p", now);

    assert_marker_retained(&db, "p", marker_at);
}

#[test]
fn lock_file_open_failure_retains_marker() {
    let (lock_home, _env_guard) = isolated_lock_dir();
    let lock_dir = lock_home
        .path()
        .join("oulipoly-agent-runner")
        .join("usage-refresh-locks");
    std::fs::create_dir_all(&lock_dir).unwrap();
    std::fs::create_dir(lock_dir.join("p.lock")).unwrap();
    let db = open_db();
    let now = Utc::now();
    let marker_at = now + Duration::hours(1);
    seed_marker(&db, "p", marker_at);

    let providers = providers_with_script("p", healthy_refresh_script());
    let sessions = SessionsConfig::default();
    let in_flight = InFlight::new();
    verify_or_clear_marker(&db, &providers, &sessions, &in_flight, "p", now);

    assert_marker_retained(&db, "p", marker_at);
}

#[test]
fn many_threads_collapse_into_one_refresh() {
    let result = run_many_thread_refresh();
    assert_eq!(result.attempts, 50);
    assert_eq!(
        result.script_calls, 1,
        "exactly one --usage script call should fan into 50 waiters"
    );
    assert!(result.marker_cleared);
}

fn run_many_thread_refresh() -> FanInResult {
    let (_lock_dir, _env_guard) = isolated_lock_dir();
    let dir = tempdir().unwrap();
    let counter_path = dir.path().join("calls");
    std::fs::write(&counter_path, "").unwrap();
    let script = healthy_counting_refresh_script(&counter_path);
    let db_path = seeded_fan_in_db_path(dir.path());
    let providers = providers_with_script("p", &script);
    let attempts = Arc::new(AtomicUsize::new(0));
    let handles = spawn_refresh_workers(db_path.clone(), providers, attempts.clone());
    join_refresh_workers(handles);
    fan_in_result(&db_path, &counter_path, attempts)
}

fn seeded_fan_in_db_path(dir: &Path) -> PathBuf {
    let db_path = dir.join("state.db");
    let db = StateDb::open(&db_path).unwrap();
    seed_marker(&db, "p", Utc::now() + Duration::hours(1));
    db_path
}

fn spawn_refresh_workers(
    db_path: PathBuf,
    providers: ProvidersConfig,
    attempts: Arc<AtomicUsize>,
) -> Vec<thread::JoinHandle<()>> {
    let in_flight = Arc::new(InFlight::new());
    let providers = Arc::new(providers);
    let sessions = Arc::new(SessionsConfig::default());
    let db_path = Arc::new(db_path);
    let barrier = Arc::new(Barrier::new(50));
    let mut handles = Vec::with_capacity(50);
    for _ in 0..50 {
        handles.push(spawn_refresh_worker(
            db_path.clone(),
            providers.clone(),
            sessions.clone(),
            in_flight.clone(),
            attempts.clone(),
            barrier.clone(),
        ));
    }
    handles
}

fn spawn_refresh_worker(
    db_path: Arc<PathBuf>,
    providers: Arc<ProvidersConfig>,
    sessions: Arc<SessionsConfig>,
    in_flight: Arc<InFlight>,
    attempts: Arc<AtomicUsize>,
    barrier: Arc<Barrier>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let db = StateDb::open(&db_path).unwrap();
        let now = Utc::now();
        barrier.wait();
        verify_or_clear_marker(&db, &providers, &sessions, &in_flight, "p", now);
        attempts.fetch_add(1, Ordering::SeqCst);
    })
}

fn join_refresh_workers(handles: Vec<thread::JoinHandle<()>>) {
    for handle in handles {
        handle.join().unwrap();
    }
}

fn fan_in_result(db_path: &Path, counter_path: &Path, attempts: Arc<AtomicUsize>) -> FanInResult {
    let final_db = StateDb::open(db_path).unwrap();
    let quota = final_db.get_quota("p").unwrap().expect("row exists");
    FanInResult {
        attempts: attempts.load(Ordering::SeqCst),
        script_calls: std::fs::read_to_string(counter_path).unwrap().len(),
        marker_cleared: quota.next_available_at.is_none(),
    }
}
