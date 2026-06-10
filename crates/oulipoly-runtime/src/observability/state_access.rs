//! ## Declared roles
//!
//! `accessor`, `formatter`, `mapper`, `orchestration`, `predicate`
//!
//! Read-only database opening and small storage helpers for observability.

use crate::observability::dto::{
    MonitorDiagnostic, MonitorDiagnosticSeverity, MonitorProcessIdentity,
};
use oulipoly_state::mailbox::MailboxDb;
use oulipoly_state::pid_identity::{PidIdentityDb, ProcessIdentity};
use oulipoly_state::{ReadOnlyOpenError, StateDb};
use std::path::{Path, PathBuf};

pub(crate) struct SnapshotStores {
    pub(crate) state: Option<StateDb>,
    pub(crate) pid: Option<PidIdentityDb>,
    pub(crate) mailbox: Option<MailboxDb>,
    pub(crate) diagnostics: Vec<MonitorDiagnostic>,
}

impl SnapshotStores {
    pub(crate) fn open_default_read_only() -> Self {
        let mut diagnostics = Vec::new();
        let state = open_state_read_only(&mut diagnostics);
        let pid_path = match read_pid_identity_default_path() {
            Ok(path) => path,
            Err(err) => {
                diagnostics.push(pid_identity_path_diagnostic(err));
                return snapshot_stores(state, None, None, diagnostics);
            }
        };
        let pid = open_pid_read_only(&pid_path, &mut diagnostics);
        let mailbox = open_mailbox_read_only(&pid_path, &mut diagnostics);
        snapshot_stores(state, pid, mailbox, diagnostics)
    }
}

fn read_pid_identity_default_path() -> Result<PathBuf, String> {
    PidIdentityDb::default_path()
}

fn pid_identity_path_diagnostic(message: String) -> MonitorDiagnostic {
    storage_diagnostic("pid-identity:path", message)
}

fn snapshot_stores(
    state: Option<StateDb>,
    pid: Option<PidIdentityDb>,
    mailbox: Option<MailboxDb>,
    diagnostics: Vec<MonitorDiagnostic>,
) -> SnapshotStores {
    SnapshotStores {
        state,
        pid,
        mailbox,
        diagnostics,
    }
}

pub(crate) fn process_identity_ref(identity: &ProcessIdentity) -> MonitorProcessIdentity {
    MonitorProcessIdentity {
        os_pid: identity.os_pid,
        os_boot_id: identity.os_boot_id.clone(),
        os_pid_starttime_ticks: identity.os_pid_starttime_ticks,
    }
}

pub(crate) fn storage_diagnostic(code: &str, message: String) -> MonitorDiagnostic {
    MonitorDiagnostic {
        code: code.to_string(),
        severity: MonitorDiagnosticSeverity::Error,
        message,
        node_id: None,
    }
}

fn open_state_read_only(diagnostics: &mut Vec<MonitorDiagnostic>) -> Option<StateDb> {
    let path = match state_default_path() {
        Ok(path) => path,
        Err(err) => {
            diagnostics.push(state_path_diagnostic(err));
            return None;
        }
    };
    if path_is_missing(&path) {
        return None;
    }
    match read_existing_state(&path) {
        Ok(db) => Some(db),
        Err(err) => {
            diagnostics.push(state_open_read_only_diagnostic(err));
            None
        }
    }
}

fn state_default_path() -> Result<PathBuf, String> {
    read_state_default_path()
}

fn read_state_default_path() -> Result<PathBuf, String> {
    StateDb::default_path()
}

fn state_path_diagnostic(message: String) -> MonitorDiagnostic {
    storage_diagnostic("state:path", message)
}

fn state_db_is_missing(path: &Path) -> bool {
    !path.exists()
}

fn path_is_missing(path: &Path) -> bool {
    state_db_is_missing(path)
}

fn open_existing_state_read_only(path: &Path) -> Result<StateDb, ReadOnlyOpenError> {
    StateDb::open_read_only(path)
}

fn read_existing_state(path: &Path) -> Result<StateDb, ReadOnlyOpenError> {
    open_existing_state_read_only(path)
}

fn state_open_read_only_diagnostic(err: ReadOnlyOpenError) -> MonitorDiagnostic {
    storage_diagnostic("state:open-read-only", format!("{err:?}"))
}

fn open_pid_read_only(
    path: &Path,
    diagnostics: &mut Vec<MonitorDiagnostic>,
) -> Option<PidIdentityDb> {
    if path_is_missing(path) {
        return None;
    }
    match read_existing_pid_identity(path) {
        Ok(db) => Some(db),
        Err(err) => {
            diagnostics.push(pid_open_read_only_diagnostic(err));
            None
        }
    }
}

fn read_existing_pid_identity(path: &Path) -> Result<PidIdentityDb, String> {
    PidIdentityDb::open_read_only(path)
}

fn pid_open_read_only_diagnostic(message: String) -> MonitorDiagnostic {
    storage_diagnostic("pid-identity:open-read-only", message)
}

fn open_mailbox_read_only(
    path: &Path,
    diagnostics: &mut Vec<MonitorDiagnostic>,
) -> Option<MailboxDb> {
    if path_is_missing(path) {
        return None;
    }
    match read_existing_mailbox(path) {
        Ok(db) => Some(db),
        Err(err) => {
            diagnostics.push(mailbox_open_read_only_diagnostic(err));
            None
        }
    }
}

fn read_existing_mailbox(path: &Path) -> Result<MailboxDb, String> {
    MailboxDb::open_read_only(path)
}

fn mailbox_open_read_only_diagnostic(message: String) -> MonitorDiagnostic {
    storage_diagnostic("mailbox:open-read-only", message)
}
