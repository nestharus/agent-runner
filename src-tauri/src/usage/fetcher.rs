//! Usage-CLI quota fetcher: per-account quota-script execution, auth-refresh
//! coalescing, and cache persistence.
//!
//! ## Declared roles
//! orchestration, accessor, formatter, mapper, predicate, validator
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/usage/fetcher.rs
//!     role: adapter
//!     Translates:
//!       - runtime quota refresh + shared lock-root contract (`oulipoly_runtime::quota`, `run_script`, `parse_output`, `run_auth_refresh_command_coalesced`, `AuthRefreshAttempt`, `QuotaScriptWindow`, `lock_data_home`, `sanitize_lock_key`)
//!       - usage-refresh-lock filesystem contract (`std::fs`, `File`, `OpenOptions`, `std::io`, `std::time::SystemTime`, `Duration`)
//!       - enumerated-account and state persistence contract (`EnumeratedAccount`, `StateDb`, `QuotaWindowInput`, `upsert_quota_refresh`, `get_windows`)
//! ```

use crate::usage::row::EnumeratedAccount;
use oulipoly_runtime::quota::{self, QuotaScriptWindow};
use oulipoly_state::{QuotaWindowInput, StateDb};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const USAGE_REFRESH_LOCK_STALE_AFTER: Duration = Duration::from_secs(30 * 60);

#[derive(Debug)]
pub(crate) enum QuotaScriptOutcome {
    Updated {
        script_windows: Vec<QuotaScriptWindow>,
    },
    NoScript,
    AlreadyInFlight,
    Failed(String),
}

pub(crate) fn fetch_all(
    state: &StateDb,
    accounts: &[EnumeratedAccount],
) -> Vec<(String, QuotaScriptOutcome)> {
    accounts
        .iter()
        .map(|account| (account.account_id.clone(), fetch_one(state, account)))
        .collect()
}

fn fetch_one(state: &StateDb, account: &EnumeratedAccount) -> QuotaScriptOutcome {
    let Some(script) = account.provider_entry.quota_script.as_deref() else {
        return QuotaScriptOutcome::NoScript;
    };

    let _guard = match RefreshFileLock::claim(&account.account_id) {
        Ok(Some(guard)) => guard,
        Ok(None) => return QuotaScriptOutcome::AlreadyInFlight,
        Err(err) => return QuotaScriptOutcome::Failed(format!("refresh lock failed: {err}")),
    };

    let first = run_and_parse(script);
    if should_attempt_auth_refresh(&account.account_id, &first, state)
        && let Some(refresh_cmd) = account.provider_entry.auth_refresh_command.as_deref()
    {
        let refresh_err = auth_refresh_error(&account.account_id, refresh_cmd);
        let retry =
            run_and_parse(script).map_err(|retry_err| combine_retry_error(retry_err, refresh_err));
        return finish_updated(&account.account_id, retry, state);
    }

    finish_updated(&account.account_id, first, state)
}

/// Run `auth_refresh_command` through the runtime's per-account single-flight
/// lock so the usage CLI can never race the balancer (or another usage run)
/// into a concurrent OAuth token rotation. Only a shell-out we actually ran can
/// contribute an error to the combined message; a coalesced / lock-unavailable
/// attempt is a clean skip.
fn auth_refresh_error(account_id: &str, refresh_cmd: &str) -> Option<String> {
    match quota::run_auth_refresh_command_coalesced(account_id, refresh_cmd) {
        quota::AuthRefreshAttempt::Ran(result) => result.err(),
        quota::AuthRefreshAttempt::Coalesced | quota::AuthRefreshAttempt::LockUnavailable => None,
    }
}

fn combine_retry_error(retry_err: String, refresh_err: Option<String>) -> String {
    match refresh_err {
        Some(err) => format!("{retry_err} (auth_refresh_command also failed: {err})"),
        None => retry_err,
    }
}

fn run_and_parse(script: &str) -> Result<Vec<QuotaScriptWindow>, String> {
    let raw = quota::run_script(script)?;
    quota::parse_output(&raw)
}

fn should_attempt_auth_refresh(
    account_id: &str,
    result: &Result<Vec<QuotaScriptWindow>, String>,
    state: &StateDb,
) -> bool {
    match result {
        Err(_) => true,
        Ok(windows) if windows.is_empty() => state
            .get_windows(account_id)
            .map(|prior| !prior.is_empty())
            .unwrap_or(false),
        Ok(_) => false,
    }
}

fn finish_updated(
    account_id: &str,
    result: Result<Vec<QuotaScriptWindow>, String>,
    state: &StateDb,
) -> QuotaScriptOutcome {
    let script_windows = match result {
        Ok(windows) => windows,
        Err(err) => return QuotaScriptOutcome::Failed(err),
    };
    let routing_windows: Vec<QuotaWindowInput> = script_windows
        .iter()
        .map(QuotaScriptWindow::to_quota_window_input)
        .collect();
    if let Err(err) = state.upsert_quota_refresh(account_id, &routing_windows) {
        return QuotaScriptOutcome::Failed(format!("cache write failed: {err}"));
    }
    QuotaScriptOutcome::Updated { script_windows }
}

struct RefreshFileLock {
    path: PathBuf,
    _file: File,
}

impl RefreshFileLock {
    fn claim(account_id: &str) -> Result<Option<Self>, String> {
        let dir = usage_lock_dir();
        fs::create_dir_all(&dir)
            .map_err(|err| format!("failed to create {}: {err}", dir.display()))?;
        let path = dir.join(format!("{}.lock", quota::sanitize_lock_key(account_id)));

        match create_lock_file(&path) {
            Ok(Some(file)) => Ok(Some(Self { path, _file: file })),
            Ok(None) if existing_lock_is_stale(&path)? => {
                remove_stale_lock(&path)?;
                match create_lock_file(&path) {
                    Ok(Some(file)) => Ok(Some(Self { path, _file: file })),
                    Ok(None) => Ok(None),
                    Err(err) => Err(format!("failed to create {}: {err}", path.display())),
                }
            }
            Ok(None) => Ok(None),
            Err(err) => Err(format!("failed to create {}: {err}", path.display())),
        }
    }
}

impl Drop for RefreshFileLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn usage_lock_dir() -> PathBuf {
    quota::lock_data_home()
        .join("oulipoly-agent-runner")
        .join("usage-refresh-locks")
}

fn create_lock_file(path: &Path) -> Result<Option<File>, io::Error> {
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => Ok(Some(file)),
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Ok(None),
        Err(err) => Err(err),
    }
}

fn existing_lock_is_stale(path: &Path) -> Result<bool, String> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(err) => return Err(format!("failed to inspect {}: {err}", path.display())),
    };
    let modified = metadata
        .modified()
        .map_err(|err| format!("failed to inspect {} mtime: {err}", path.display()))?;
    Ok(lock_modified_time_is_stale(modified, SystemTime::now()))
}

fn lock_modified_time_is_stale(modified: SystemTime, now: SystemTime) -> bool {
    now.duration_since(modified)
        .map(|age| age > USAGE_REFRESH_LOCK_STALE_AFTER)
        .unwrap_or(false)
}

fn remove_stale_lock(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("failed to remove stale {}: {err}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_modified_time_older_than_threshold_is_stale() {
        let now = SystemTime::UNIX_EPOCH + USAGE_REFRESH_LOCK_STALE_AFTER + Duration::from_secs(1);

        assert!(lock_modified_time_is_stale(SystemTime::UNIX_EPOCH, now));
    }

    #[test]
    fn lock_modified_time_within_threshold_is_not_stale() {
        let now = SystemTime::UNIX_EPOCH + USAGE_REFRESH_LOCK_STALE_AFTER;
        let modified = now - (USAGE_REFRESH_LOCK_STALE_AFTER - Duration::from_secs(1));

        assert!(!lock_modified_time_is_stale(modified, now));
    }

    #[test]
    fn future_lock_modified_time_is_not_stale() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
        let modified = now + Duration::from_secs(1);

        assert!(!lock_modified_time_is_stale(modified, now));
    }
}
