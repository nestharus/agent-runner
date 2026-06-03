//! Cross-process, per-account single-flight lock for `auth_refresh_command`.
//!
//! ## Declared roles
//! orchestration, accessor, predicate, mapper, formatter, parser
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/quota/auth_refresh_lock.rs
//!     role: adapter
//!     Translates:
//!       - shared lock-root and filesystem lock contract (`super::lock_paths::data_home`, `super::lock_paths::sanitize_lock_name`, `std::fs`, `File`, `OpenOptions`, `fs4::FileExt::lock`, `fs4::FileExt::unlock`)
//!       - freshness-stamp contract (`std::io::Seek`, `std::io::Read`, `std::io::Write`, `SystemTime`, `Duration`)
//!       - quota auth-refresh shell-out contract (`super::process::run_refresh_command`)
//!       - process environment coalesce-window contract (`OULIPOLY_AUTH_REFRESH_COALESCE_SECS`)
//!       - diagnostic logging contract (`tracing::warn`)
//! ```
//!
//! `auth_refresh_command` shells out to a provider CLI's login/status
//! subcommand whose OAuth code rotates a *single-use* refresh token. If two
//! runner processes run it for the same account concurrently, both read the
//! same token, both rotate, and the provider's reuse-detection revokes the
//! whole token family -- the forced-re-login outage. agent-runner
//! never reads or writes `auth.json` itself; the only way it can cause that
//! outage is by invoking the (token-mutating) `auth_refresh_command` without
//! synchronizing across processes.
//!
//! Every auth-refresh shell-out -- the runtime quota path
//! (`refresh::refresh_after_auth_command`, reached by the balancer topology /
//! routing probes and marker verification) and the usage CLI
//! (`usage::fetcher::fetch_one`) -- funnels through
//! [`run_auth_refresh_command_coalesced`]. It:
//!
//!   1. takes one cross-process `flock`, keyed by the account, held across the
//!      entire shell-out, so two holders can never rotate concurrently
//!      (correctness); and
//!   2. coalesces a thundering herd: once a holder has refreshed within the
//!      freshness window, queued waiters skip the shell-out entirely, so a
//!      burst of concurrent dispatches collapses to a single refresh per
//!      account (and back-to-back queued waiters never re-rotate a just-rotated
//!      token).
//!
//! The lock file's contents double as the freshness stamp (unix millis of the
//! last attempt), shared across processes under the same exclusive lock.
//! Account identity is the provider/account name -- the same string the usage
//! path calls `account_id` and the runtime path calls `provider_name` -- so all
//! call sites share one key space. Lock or stamp I/O failure is fail-closed:
//! the shell-out is skipped rather than run unsynchronized, and auth-refresh
//! failure is non-fatal to the surrounding quota refresh (the caller just
//! retries the quota script with the existing token).

use super::lock_paths::{data_home, sanitize_lock_name};
use super::process::run_refresh_command;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Default coalesce window. Within this many seconds of a prior attempt for the
/// same account, a queued lock holder skips its own shell-out. 60s comfortably
/// collapses a dispatch burst (each quota script can take up to its own 30s
/// timeout before triggering a refresh) while still permitting a genuine
/// re-refresh a minute later. Overridable via `OULIPOLY_AUTH_REFRESH_COALESCE_SECS`.
const DEFAULT_AUTH_REFRESH_COALESCE_SECS: i64 = 60;
const COALESCE_ENV_VAR: &str = "OULIPOLY_AUTH_REFRESH_COALESCE_SECS";

/// Disposition of a coalesced auth-refresh attempt.
#[derive(Debug)]
pub enum AuthRefreshAttempt {
    /// Held the lock and invoked the command; inner is the command result
    /// (`Ok(())` on success, `Err(message)` on non-zero exit / spawn failure).
    Ran(Result<(), String>),
    /// Another holder refreshed within the freshness window; shell-out skipped.
    Coalesced,
    /// The lock could not be realized; fail-closed and skip the shell-out
    /// rather than risk an unsynchronized token rotation.
    LockUnavailable,
}

/// Run `auth_refresh_command` for `account_key` under the unified per-account
/// single-flight lock, collapsing concurrent and closely-following attempts
/// into a single shell-out. See the module docs for the invariant this holds.
pub fn run_auth_refresh_command_coalesced(
    account_key: &str,
    auth_refresh_command: &str,
) -> AuthRefreshAttempt {
    let mut lock = match AuthRefreshLock::acquire_blocking(account_key) {
        Ok(lock) => lock,
        Err(err) => return lock_unavailable(account_key, &err),
    };
    if lock.refreshed_within_window() {
        return AuthRefreshAttempt::Coalesced;
    }
    lock.stamp_attempt();
    AuthRefreshAttempt::Ran(run_refresh_command(auth_refresh_command))
}

fn lock_unavailable(account_key: &str, err: &str) -> AuthRefreshAttempt {
    tracing::warn!(
        account = account_key,
        error = err,
        "auth-refresh single-flight lock unavailable; skipping auth_refresh_command"
    );
    AuthRefreshAttempt::LockUnavailable
}

fn coalesce_window_secs() -> i64 {
    coalesce_window_override().unwrap_or(DEFAULT_AUTH_REFRESH_COALESCE_SECS)
}

fn coalesce_window_override() -> Option<i64> {
    coalesce_window_env().and_then(|value| parse_trimmed_i64(&value))
}

fn coalesce_window_env() -> Option<String> {
    std::env::var(COALESCE_ENV_VAR).ok()
}

fn parse_trimmed_i64(value: &str) -> Option<i64> {
    value.trim().parse::<i64>().ok()
}

/// Per-account blocking `flock(2)` over a single stamp file. The open fd is
/// owned for the guard lifetime; drop releases the lock. The file's contents
/// are the freshness stamp (unix millis of the last attempt).
struct AuthRefreshLock {
    file: File,
}

impl AuthRefreshLock {
    fn acquire_blocking(account_key: &str) -> Result<Self, String> {
        let path = lock_path(account_key);
        ensure_parent_dir(&path)?;
        let file = open_lock_file(&path)?;
        lock_file(&file, &path)?;
        Ok(Self { file })
    }

    /// True when the stamp shows an attempt newer than the coalesce window.
    /// Unreadable / unparseable / future stamps read as "not fresh" so we fall
    /// through and run rather than wrongly suppress a needed refresh.
    fn refreshed_within_window(&mut self) -> bool {
        let (Some(last), Some(now)) = (self.read_stamp_millis(), now_millis()) else {
            return false;
        };
        let window_millis = coalesce_window_secs().saturating_mul(1000);
        now >= last && now - last < window_millis
    }

    fn read_stamp_millis(&mut self) -> Option<i64> {
        parse_trimmed_i64(&self.read_stamp_text()?)
    }

    fn read_stamp_text(&mut self) -> Option<String> {
        let mut buf = String::new();
        self.file.seek(SeekFrom::Start(0)).ok()?;
        self.file.read_to_string(&mut buf).ok()?;
        Some(buf)
    }

    /// Record "an auth refresh for this account was initiated now". Written
    /// before the shell-out so even a slow/killed command still coalesces the
    /// queued waiters. Best-effort: a write failure leaves the prior stamp.
    fn stamp_attempt(&mut self) {
        let Some(now) = now_millis() else {
            return;
        };
        let _ = self.write_stamp_millis(now);
    }

    fn write_stamp_millis(&mut self, millis: i64) -> std::io::Result<()> {
        self.file.seek(SeekFrom::Start(0))?;
        self.file.set_len(0)?;
        self.file.write_all(millis.to_string().as_bytes())?;
        self.file.flush()
    }
}

impl Drop for AuthRefreshLock {
    fn drop(&mut self) {
        let _ = <File as fs4::FileExt>::unlock(&self.file);
    }
}

fn lock_path(account_key: &str) -> PathBuf {
    auth_refresh_lock_dir().join(format!("{}.lock", sanitize_lock_name(account_key)))
}

fn auth_refresh_lock_dir() -> PathBuf {
    data_home()
        .join("oulipoly-agent-runner")
        .join("auth-refresh-locks")
}

fn ensure_parent_dir(path: &Path) -> Result<(), String> {
    let dir = lock_file_parent(path)?;
    fs::create_dir_all(dir).map_err(|err| create_dir_error(dir, &err))
}

fn lock_file_parent(path: &Path) -> Result<&Path, String> {
    path.parent().ok_or_else(|| missing_parent_error(path))
}

fn open_lock_file(path: &Path) -> Result<File, String> {
    lock_file_open_options()
        .open(path)
        .map_err(|err| open_error(path, &err))
}

fn lock_file_open_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    options
}

fn lock_file(file: &File, path: &Path) -> Result<(), String> {
    <File as fs4::FileExt>::lock(file).map_err(|err| flock_error(path, &err))
}

fn missing_parent_error(path: &Path) -> String {
    format!("no parent dir for {}", path.display())
}

fn create_dir_error(dir: &Path, err: &std::io::Error) -> String {
    format!("create {}: {err}", dir.display())
}

fn open_error(path: &Path, err: &std::io::Error) -> String {
    format!("open {}: {err}", path.display())
}

fn flock_error(path: &Path, err: &std::io::Error) -> String {
    format!("flock {}: {err}", path.display())
}

fn now_millis() -> Option<i64> {
    system_now_since_epoch().and_then(duration_to_millis)
}

fn system_now_since_epoch() -> Option<Duration> {
    SystemTime::now().duration_since(UNIX_EPOCH).ok()
}

fn duration_to_millis(dur: Duration) -> Option<i64> {
    i64::try_from(dur.as_millis()).ok()
}
