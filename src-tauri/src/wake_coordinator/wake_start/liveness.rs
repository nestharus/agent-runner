//! ## Declared roles
//!
//! `accessor`, `filter`, `orchestration`, `predicate`

use oulipoly_state::mailbox::{
    MailboxDb, RuntimeLifecycleState, SessionGenerationProjection, SessionLiveness,
};

#[cfg(unix)]
use oulipoly_runtime::executor::cli::pty_broker;

pub(super) struct RuntimeLivenessCheck {
    pub(super) liveness: SessionLiveness,
    pty_control_path: Option<String>,
}

pub(super) fn runtime_liveness(
    db: &mut MailboxDb,
    session_id: &str,
) -> Result<RuntimeLivenessCheck, String> {
    let pty_control_path = running_pty_control_path(db, session_id)?;
    let liveness = db
        .runtime_lifecycle()
        .reconcile_session_liveness(session_id)?;
    Ok(RuntimeLivenessCheck {
        liveness,
        pty_control_path,
    })
}

pub(super) fn cleanup_idle_runtime(check: &RuntimeLivenessCheck) {
    if check.liveness == SessionLiveness::Idle {
        unlink_stale_pty_socket(check.pty_control_path.as_deref());
    }
}

fn running_pty_control_path(db: &MailboxDb, session_id: &str) -> Result<Option<String>, String> {
    let projection = db
        .runtime_lifecycle_reader()
        .session_generation_projection(session_id)
        .map_err(|error| error.to_string())?;
    let SessionGenerationProjection::One(generation) = projection else {
        return Ok(None);
    };
    Ok((generation.runtime_mode == "pty_interactive"
        && generation.lifecycle_state == RuntimeLifecycleState::Running)
        .then_some(generation.pty_control_path)
        .flatten())
}

#[cfg(unix)]
fn unlink_stale_pty_socket(path: Option<&str>) {
    if let Some(path) = path {
        let _ = pty_broker::unlink_control_socket_if_owned(path);
    }
}

#[cfg(not(unix))]
fn unlink_stale_pty_socket(_path: Option<&str>) {}
