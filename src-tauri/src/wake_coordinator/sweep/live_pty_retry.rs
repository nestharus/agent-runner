//! Live PTY mailbox-delivery retry driver and selection procedure.
//!
//! ## Declared roles
//!
//! `filter`, `orchestration`

use oulipoly_state::mailbox::{
    MailboxDb, RuntimeGenerationRow, RuntimeLifecycleState, SessionGenerationProjection,
    SessionLiveness,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::mailbox_delivery::PtyMailboxDeliveryDiagnostic;
use crate::wake_coordinator::auto_wake_env::is_auto_wake_invocation;
use crate::wake_coordinator::constants::{
    LIVE_PTY_RETRY_INTERVAL_SECONDS, WAKE_RECLAIM_SWEEP_SCAN_LIMIT,
};

pub(crate) struct LivePtyRetryDriverGuard {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for LivePtyRetryDriverGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            handle.thread().unpark();
            if let Err(err) = handle.join() {
                tracing::warn!(?err, "Live PTY retry driver failed to join cleanly");
            }
        }
    }
}

pub(crate) fn start_live_pty_retry_driver_for_owner() -> Option<LivePtyRetryDriverGuard> {
    if is_auto_wake_invocation() {
        return None;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    match std::thread::Builder::new()
        .name("oulipoly-live-pty-retry".to_string())
        .spawn(move || live_pty_retry_loop(thread_stop))
    {
        Ok(handle) => Some(LivePtyRetryDriverGuard {
            stop,
            handle: Some(handle),
        }),
        Err(err) => {
            tracing::warn!("Failed to start live PTY retry driver: {err}");
            None
        }
    }
}

fn live_pty_retry_loop(stop: Arc<AtomicBool>) {
    retry_pending_live_pty_deliveries_or_warn("live_pty_retry_start");
    loop {
        std::thread::park_timeout(Duration::from_secs(LIVE_PTY_RETRY_INTERVAL_SECONDS));
        if stop.load(Ordering::SeqCst) {
            break;
        }
        retry_pending_live_pty_deliveries_or_warn("live_pty_retry_tick");
    }
}

fn retry_pending_live_pty_deliveries_or_warn(trigger: &str) {
    if let Err(err) = retry_pending_live_pty_deliveries_from_default_db(trigger) {
        super::warn_wake_reclaim_sweep_failed(trigger, err);
    }
}

fn retry_pending_live_pty_deliveries_from_default_db(trigger: &str) -> Result<(), String> {
    let Some(mut db) = MailboxDb::open_default_if_exists()? else {
        return Ok(());
    };
    retry_pending_live_pty_deliveries(&mut db, trigger, &|| false)
}

pub(super) fn retry_pending_live_pty_deliveries(
    db: &mut MailboxDb,
    trigger: &str,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    let evidence_sessions =
        db.pending_delivery_evidence_obligation_session_ids(WAKE_RECLAIM_SWEEP_SCAN_LIMIT)?;
    for session_id in evidence_sessions {
        if is_cancelled() {
            return Ok(());
        }
        if let Err(error) =
            crate::mailbox_delivery::reconcile_pending_pty_delivery_evidence(db, &session_id)
        {
            tracing::warn!(
                trigger,
                session_id,
                "Failed to reconcile pending PTY delivery evidence: {error}"
            );
        }
    }
    let session_ids = db
        .wake_sessions()
        .pending_delivery_session_ids(WAKE_RECLAIM_SWEEP_SCAN_LIMIT)?;
    for session_id in session_ids {
        if is_cancelled() {
            return Ok(());
        }
        match live_pty_retry_applicability(db, &session_id)? {
            LivePtyRetryApplicability::Applicable => {
                let diagnostic = crate::mailbox_delivery::attempt_pty_mailbox_delivery_with_trigger(
                    db,
                    &session_id,
                    trigger,
                );
                trace_live_pty_retry(trigger, &session_id, &diagnostic);
            }
            LivePtyRetryApplicability::Skip { reason, liveness } => {
                trace_live_pty_retry_skip(trigger, &session_id, reason, liveness.as_deref());
            }
        }
    }
    Ok(())
}

enum LivePtyRetryApplicability {
    Applicable,
    Skip {
        reason: &'static str,
        liveness: Option<String>,
    },
}

fn live_pty_retry_applicability(
    db: &mut MailboxDb,
    session_id: &str,
) -> Result<LivePtyRetryApplicability, String> {
    let runtime = match db
        .runtime_lifecycle_reader()
        .session_generation_projection(session_id)
        .map_err(|error| error.to_string())?
    {
        SessionGenerationProjection::One(runtime) => runtime,
        SessionGenerationProjection::None => {
            return Ok(LivePtyRetryApplicability::Skip {
                reason: "no_runtime",
                liveness: None,
            });
        }
        SessionGenerationProjection::Multiple(_) => {
            return Ok(LivePtyRetryApplicability::Skip {
                reason: "ambiguous_runtime",
                liveness: None,
            });
        }
    };
    if !runtime_is_running_pty_with_socket(&runtime) {
        return Ok(LivePtyRetryApplicability::Skip {
            reason: runtime_skip_reason(&runtime),
            liveness: None,
        });
    }
    let liveness = db
        .runtime_lifecycle()
        .reconcile_session_liveness(session_id)?;
    if liveness == SessionLiveness::Busy {
        Ok(LivePtyRetryApplicability::Applicable)
    } else {
        Ok(LivePtyRetryApplicability::Skip {
            reason: "not_busy",
            liveness: Some(format!("{liveness:?}")),
        })
    }
}

fn runtime_skip_reason(runtime: &RuntimeGenerationRow) -> &'static str {
    if runtime.runtime_mode != "pty_interactive" {
        "not_pty"
    } else if runtime.lifecycle_state != RuntimeLifecycleState::Running {
        "not_running"
    } else {
        "no_socket"
    }
}

fn runtime_is_running_pty_with_socket(runtime: &RuntimeGenerationRow) -> bool {
    runtime.runtime_mode == "pty_interactive"
        && runtime.lifecycle_state == RuntimeLifecycleState::Running
        && runtime
            .pty_control_path
            .as_deref()
            .is_some_and(|path| !path.is_empty())
}

fn trace_live_pty_retry(
    trigger: &str,
    session_id: &str,
    diagnostic: &PtyMailboxDeliveryDiagnostic,
) {
    tracing::debug!(
        session_id,
        status = diagnostic.status.as_str(),
        submitted = diagnostic.submitted,
        delivered = diagnostic.delivered_seqs.len(),
        "Wake reclaim sweep retried live PTY mailbox delivery"
    );
    if crate::mailbox_delivery::trace_notify_enabled() {
        eprintln!(
            concat!(
                "oulipoly_notify_trace trigger={} session_id={} liveness=Busy ",
                "attempted={} decision={} inject_status={} submitted={} ",
                "remaining_pending={:?} message={:?}"
            ),
            trigger,
            session_id,
            diagnostic.attempted,
            if diagnostic.submitted {
                "inject"
            } else {
                "skip"
            },
            diagnostic.status,
            diagnostic.submitted,
            diagnostic.remaining_pending,
            diagnostic.message,
        );
    }
}

fn trace_live_pty_retry_skip(
    trigger: &str,
    session_id: &str,
    reason: &str,
    liveness: Option<&str>,
) {
    if !crate::mailbox_delivery::trace_notify_enabled() {
        return;
    }
    eprintln!(
        concat!(
            "oulipoly_notify_trace trigger={} session_id={} liveness={} ",
            "attempted=false decision=skip inject_status={} submitted=false"
        ),
        trigger,
        session_id,
        liveness.unwrap_or("unknown"),
        reason,
    );
}
