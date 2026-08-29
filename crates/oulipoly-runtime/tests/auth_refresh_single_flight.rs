//! Invariant: every `auth_refresh_command` shell-out is funneled through one
//! cross-process, per-account single-flight lock.
//!
//! - Concurrent attempts for the SAME account collapse to AT MOST ONE real
//!   shell-out; the rest coalesce/skip (this is what prevents two processes
//!   from concurrently rotating the same single-use OAuth refresh token and
//!   tripping the provider's reuse-detection revocation, which forces re-login).
//! - Attempts for DIFFERENT accounts each run independently (they are keyed by
//!   distinct lock files and never coalesce against one another), so they may
//!   proceed in parallel.
//!
//! `flock(2)` excludes across separate open file descriptions regardless of
//! whether they belong to the same process, so a multi-thread harness exercises
//! the same exclusion path a multi-process fleet would.
//!
//! ## Declared roles
//! orchestration, accessor, formatter, mapper, validator
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/tests/auth_refresh_single_flight.rs
//!     role: intrinsic-surface
//!     Domain: auth_refresh_single_flight_behavior_harness
//!     Owns:
//!       - OULIPOLY_DATA_HOME and OULIPOLY_DATA_DIR env-scope isolation under a process mutex
//!       - shell counting-command formatting and shell-out counting
//!       - concurrent multi-thread/barrier attempt orchestration
//!       - disposition tally mapping and single-flight invariant assertions
//! ```

use oulipoly_runtime::quota::{AuthRefreshAttempt, run_auth_refresh_command_coalesced};
use std::ffi::OsString;
use std::path::Path;
use std::sync::{Arc, Barrier, Mutex, MutexGuard, OnceLock};
use std::thread;

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Isolates `OULIPOLY_DATA_HOME` (where the auth-refresh lock dir lives) and
/// `OULIPOLY_DATA_DIR` to a fresh tempdir, optionally sets extra env vars, and
/// restores all of them on drop. Holds the process env lock for its lifetime.
struct EnvScope {
    _home: tempfile::TempDir,
    _lock: MutexGuard<'static, ()>,
    restores: Vec<(&'static str, Option<OsString>)>,
}

impl EnvScope {
    fn new(extra: &[(&'static str, &str)]) -> Self {
        let lock = env_lock();
        let home = tempfile::tempdir().expect("data home tempdir");
        let mut restores = vec![
            ("OULIPOLY_DATA_HOME", std::env::var_os("OULIPOLY_DATA_HOME")),
            ("OULIPOLY_DATA_DIR", std::env::var_os("OULIPOLY_DATA_DIR")),
        ];
        set_env(
            "OULIPOLY_DATA_DIR",
            home.path().join("oulipoly-agent-runner").as_os_str(),
        );
        set_env("OULIPOLY_DATA_HOME", home.path().as_os_str());
        for (key, value) in extra {
            restores.push((*key, std::env::var_os(key)));
            set_env(key, value.as_ref());
        }
        Self {
            _home: home,
            _lock: lock,
            restores,
        }
    }
}

impl Drop for EnvScope {
    fn drop(&mut self) {
        for (key, previous) in self.restores.drain(..).rev() {
            match previous {
                Some(value) => set_env(key, value.as_os_str()),
                None => remove_env(key),
            }
        }
    }
}

fn set_env(key: &str, value: &std::ffi::OsStr) {
    // SAFETY: EnvScope holds ENV_LOCK for its whole lifetime, serializing this
    // process-global mutation against every other env-mutating test here.
    unsafe {
        std::env::set_var(key, value);
    }
}

fn remove_env(key: &str) {
    // SAFETY: see set_env -- the owning EnvScope still holds ENV_LOCK.
    unsafe {
        std::env::remove_var(key);
    }
}

/// A command that appends one byte to a per-account counter file every time it
/// actually runs, so the file length counts real shell-outs.
fn counting_command(counter: &Path) -> String {
    format!("printf '.' >> {}", shell_quote(counter))
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

fn shell_out_count(counter: &Path) -> usize {
    std::fs::read(counter).map(|bytes| bytes.len()).unwrap_or(0)
}

#[derive(Default)]
struct Tally {
    ran: usize,
    coalesced: usize,
    lock_unavailable: usize,
}

fn tally(dispositions: &[AuthRefreshAttempt]) -> Tally {
    let mut tally = Tally::default();
    for disposition in dispositions {
        match disposition {
            AuthRefreshAttempt::Ran(_) => tally.ran += 1,
            AuthRefreshAttempt::Coalesced => tally.coalesced += 1,
            AuthRefreshAttempt::LockUnavailable => tally.lock_unavailable += 1,
        }
    }
    tally
}

#[test]
fn concurrent_attempts_for_one_account_collapse_to_a_single_shell_out() {
    let _env = EnvScope::new(&[]);
    let dir = tempfile::tempdir().unwrap();
    let counter = dir.path().join("acct-1.count");
    let command = counting_command(&counter);

    let threads = 50;
    let barrier = Arc::new(Barrier::new(threads));
    let mut handles = Vec::with_capacity(threads);
    for _ in 0..threads {
        let command = command.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            run_auth_refresh_command_coalesced("acct-1", &command)
        }));
    }
    let dispositions: Vec<AuthRefreshAttempt> =
        handles.into_iter().map(|h| h.join().unwrap()).collect();

    let tally = tally(&dispositions);
    assert_eq!(
        shell_out_count(&counter),
        1,
        "exactly one auth_refresh_command shell-out must occur for {threads} concurrent same-account attempts"
    );
    assert_eq!(tally.ran, 1, "exactly one attempt may report Ran");
    assert_eq!(
        tally.coalesced,
        threads - 1,
        "every other same-account attempt must coalesce"
    );
    assert_eq!(
        tally.lock_unavailable, 0,
        "the lock must be acquirable for all attempts"
    );
}

#[test]
fn concurrent_attempts_for_different_accounts_each_run() {
    let _env = EnvScope::new(&[]);
    let dir = tempfile::tempdir().unwrap();
    let accounts = ["acct-1", "acct-2", "acct-3", "acct-4", "acct-5"];

    let barrier = Arc::new(Barrier::new(accounts.len()));
    let mut handles = Vec::with_capacity(accounts.len());
    for account in accounts {
        let counter = dir.path().join(format!("{account}.count"));
        let command = counting_command(&counter);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            let disposition = run_auth_refresh_command_coalesced(account, &command);
            (account, counter, disposition)
        }));
    }

    for handle in handles {
        let (account, counter, disposition) = handle.join().unwrap();
        assert!(
            matches!(disposition, AuthRefreshAttempt::Ran(Ok(()))),
            "account {account} must run its own auth_refresh_command (got {disposition:?})"
        );
        assert_eq!(
            shell_out_count(&counter),
            1,
            "account {account} must shell out exactly once -- distinct accounts never coalesce together"
        );
    }
}

#[test]
fn back_to_back_same_account_attempts_coalesce_within_the_window() {
    let _env = EnvScope::new(&[]);
    let dir = tempfile::tempdir().unwrap();
    let counter = dir.path().join("acct-1.count");
    let command = counting_command(&counter);

    let first = run_auth_refresh_command_coalesced("acct-1", &command);
    let second = run_auth_refresh_command_coalesced("acct-1", &command);

    assert!(matches!(first, AuthRefreshAttempt::Ran(Ok(()))));
    assert!(
        matches!(second, AuthRefreshAttempt::Coalesced),
        "a second attempt inside the freshness window must coalesce (got {second:?})"
    );
    assert_eq!(shell_out_count(&counter), 1);
}

#[test]
fn zero_window_override_disables_coalescing() {
    let _env = EnvScope::new(&[("OULIPOLY_AUTH_REFRESH_COALESCE_SECS", "0")]);
    let dir = tempfile::tempdir().unwrap();
    let counter = dir.path().join("acct-1.count");
    let command = counting_command(&counter);

    let first = run_auth_refresh_command_coalesced("acct-1", &command);
    let second = run_auth_refresh_command_coalesced("acct-1", &command);

    assert!(matches!(first, AuthRefreshAttempt::Ran(Ok(()))));
    assert!(
        matches!(second, AuthRefreshAttempt::Ran(Ok(()))),
        "with a zero-second window every attempt runs (got {second:?})"
    );
    assert_eq!(shell_out_count(&counter), 2);
}
