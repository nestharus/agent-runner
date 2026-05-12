use crate::usage::row::EnumeratedAccount;
use oulipoly_runtime::quota::{self, QuotaScriptWindow};
use oulipoly_state::{QuotaWindowInput, StateDb};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::PathBuf;

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
        let refresh_err = quota::run_refresh_command(refresh_cmd).err();
        let retry = run_and_parse(script).map_err(|retry_err| match refresh_err {
            Some(err) => format!("{retry_err} (auth_refresh_command also failed: {err})"),
            None => retry_err,
        });
        return finish_updated(&account.account_id, retry, state);
    }

    finish_updated(&account.account_id, first, state)
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
        let path = dir.join(format!("{}.lock", sanitize_lock_name(account_id)));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => Ok(Some(Self { path, _file: file })),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Ok(None),
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
