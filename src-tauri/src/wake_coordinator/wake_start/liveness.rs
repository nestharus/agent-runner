//! ## Declared roles
//!
//! `accessor`, `filter`, `orchestration`, `predicate`

use oulipoly_state::mailbox::{MailboxDb, SessionLiveness, SessionRuntimeRow};

#[cfg(unix)]
use oulipoly_runtime::executor::cli::pty_broker;

pub(super) fn pty_runtime_liveness(
    db: &mut MailboxDb,
    session_id: &str,
    runtime: Option<&SessionRuntimeRow>,
) -> Result<Option<SessionLiveness>, String> {
    if running_pty_runtime(runtime).is_none() {
        return Ok(None);
    };
    session_liveness_for_runtime(db, session_id).map(Some)
}

pub(super) fn cleanup_idle_runtime(
    runtime: Option<&SessionRuntimeRow>,
    liveness: Option<SessionLiveness>,
) {
    if let (Some(row), Some(SessionLiveness::Idle)) = (running_pty_runtime(runtime), liveness) {
        unlink_stale_pty_socket(row.pty_control_path.as_deref());
    }
}

pub(super) fn optional_pty_liveness_is_busy(liveness: Option<SessionLiveness>) -> bool {
    liveness.is_some_and(pty_liveness_is_busy)
}

fn running_pty_runtime(runtime: Option<&SessionRuntimeRow>) -> Option<&SessionRuntimeRow> {
    runtime.filter(|row| row.mode == "pty_interactive" && row.run_state == "running")
}

fn session_liveness_for_runtime(
    db: &mut MailboxDb,
    session_id: &str,
) -> Result<SessionLiveness, String> {
    db.session_liveness(session_id)
}

fn pty_liveness_is_busy(liveness: SessionLiveness) -> bool {
    liveness == SessionLiveness::Busy
}

#[cfg(unix)]
fn unlink_stale_pty_socket(path: Option<&str>) {
    if let Some(path) = path {
        let _ = pty_broker::unlink_control_socket_if_owned(path);
    }
}

#[cfg(not(unix))]
fn unlink_stale_pty_socket(_path: Option<&str>) {}
