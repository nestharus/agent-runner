//! ## Declared roles
//!
//! `filter`, `formatter`, `mapper`, `orchestration`

mod candidate;
mod consumed;
mod state;

use oulipoly_state::mailbox::{
    MailboxDb, SessionLiveness, SessionRuntimeRow, WAKE_SWEEP_ABANDONED_ERROR, WakeSweepCandidate,
};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use super::auto_wake_env::is_auto_wake_invocation;
use super::constants::{
    LIVE_PTY_RETRY_INTERVAL_SECONDS, WAKE_RECLAIM_REAP_ROWS_PER_SESSION,
    WAKE_RECLAIM_REAP_SESSION_LIMIT, WAKE_RECLAIM_SWEEP_INTERVAL_SECONDS, WAKE_RECLAIM_SWEEP_LIMIT,
    WAKE_RECLAIM_SWEEP_SCAN_LIMIT,
};
use super::diagnostics::WakeDiagnostic;
use super::wake_start::{StartWakeInput, start_wake_chain};
use crate::mailbox_delivery::PtyMailboxDeliveryDiagnostic;

pub(crate) fn run_startup_wake_reclaim_sweep() {
    if is_auto_wake_invocation() {
        return;
    }
    run_wake_reclaim_sweep_or_warn("process_start");
}

pub(crate) fn start_wake_reclaim_maintenance_driver() {
    if is_auto_wake_invocation() {
        return;
    }
    static DRIVER: OnceLock<()> = OnceLock::new();
    DRIVER.get_or_init(spawn_wake_reclaim_maintenance_driver);
}

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
    spawn_live_pty_retry_driver()
}

fn spawn_live_pty_retry_driver() -> Option<LivePtyRetryDriverGuard> {
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

fn spawn_wake_reclaim_maintenance_driver() {
    let result = std::thread::Builder::new()
        .name("oulipoly-wake-reclaim-sweep".to_string())
        .spawn(wake_reclaim_maintenance_loop);
    if let Err(err) = result {
        warn_wake_reclaim_driver_start_failed(err);
    }
}

fn warn_wake_reclaim_driver_start_failed(err: std::io::Error) {
    tracing::warn!("Failed to start wake reclaim sweep driver: {err}");
}

fn wake_reclaim_maintenance_loop() {
    run_wake_reclaim_sweep_or_warn("maintenance_start");
    loop {
        std::thread::sleep(Duration::from_secs(WAKE_RECLAIM_SWEEP_INTERVAL_SECONDS));
        run_wake_reclaim_sweep_or_warn("maintenance_tick");
    }
}

fn run_wake_reclaim_sweep_or_warn(trigger: &str) {
    if let Err(err) = run_wake_reclaim_sweep(trigger) {
        warn_wake_reclaim_sweep_failed(trigger, err);
    }
}

fn warn_wake_reclaim_sweep_failed(trigger: &str, err: String) {
    tracing::warn!(trigger, "Wake reclaim sweep failed: {err}");
}

fn run_wake_reclaim_sweep(trigger: &str) -> Result<(), String> {
    let Some(mut db) = MailboxDb::open_default_if_exists()? else {
        return Ok(());
    };
    retry_pending_live_pty_deliveries(&mut db)?;
    let candidates = db.wake_sweep_candidates(
        super::constants::WAKE_CLAIM_STALE_AFTER_SECONDS,
        WAKE_RECLAIM_SWEEP_SCAN_LIMIT,
    )?;
    let plan = wake_sweep_plan(&mut db, candidates)?;
    reap_abandoned_sweep_candidates(&mut db, &plan.reap)?;
    drop(db);
    for candidate in plan.start {
        let diagnostic = start_wake_chain(wake_sweep_start_input(&candidate, trigger));
        trace_wake_sweep_candidate(&candidate.session_id, &diagnostic);
    }
    Ok(())
}

fn retry_pending_live_pty_deliveries_or_warn(trigger: &str) {
    if let Err(err) = retry_pending_live_pty_deliveries_from_default_db() {
        warn_wake_reclaim_sweep_failed(trigger, err);
    }
}

fn retry_pending_live_pty_deliveries_from_default_db() -> Result<(), String> {
    let Some(mut db) = MailboxDb::open_default_if_exists()? else {
        return Ok(());
    };
    retry_pending_live_pty_deliveries(&mut db)
}

fn retry_pending_live_pty_deliveries(db: &mut MailboxDb) -> Result<(), String> {
    let session_ids = db.pending_delivery_session_ids(WAKE_RECLAIM_SWEEP_SCAN_LIMIT)?;
    for session_id in session_ids {
        if live_pty_retry_is_applicable(db, &session_id)? {
            let diagnostic = crate::mailbox_delivery::attempt_pty_mailbox_delivery(db, &session_id);
            trace_live_pty_retry(&session_id, &diagnostic);
        }
    }
    Ok(())
}

fn live_pty_retry_is_applicable(db: &mut MailboxDb, session_id: &str) -> Result<bool, String> {
    let Some(runtime) = db.session_runtime(session_id)? else {
        return Ok(false);
    };
    if !runtime_is_running_pty_with_socket(&runtime) {
        return Ok(false);
    }
    Ok(db.session_liveness(session_id)? == SessionLiveness::Busy)
}

fn runtime_is_running_pty_with_socket(runtime: &SessionRuntimeRow) -> bool {
    runtime.mode == "pty_interactive"
        && runtime.run_state == "running"
        && runtime
            .pty_control_path
            .as_deref()
            .is_some_and(|path| !path.is_empty())
}

fn wake_sweep_start_input<'a>(
    candidate: &'a WakeSweepCandidate,
    trigger: &'a str,
) -> StartWakeInput<'a> {
    StartWakeInput {
        session_id: &candidate.session_id,
        reason: trigger,
        auto_wake_count: candidate.auto_wake_count,
        renew_token: None,
    }
}

struct WakeSweepPlan {
    start: Vec<WakeSweepCandidate>,
    reap: Vec<WakeSweepCandidate>,
}

fn wake_sweep_plan(
    db: &mut MailboxDb,
    candidates: Vec<WakeSweepCandidate>,
) -> Result<WakeSweepPlan, String> {
    let (recoverable, reap) = partition_wake_sweep_candidates(db, candidates)?;
    let start = select_recoverable_sweep_candidates(recoverable);
    Ok(wake_sweep_plan_from_selected(start, reap))
}

fn partition_wake_sweep_candidates(
    db: &mut MailboxDb,
    candidates: Vec<WakeSweepCandidate>,
) -> Result<(Vec<WakeSweepCandidate>, Vec<WakeSweepCandidate>), String> {
    let state = state::open_default_state_read_only();
    let mut recoverable = Vec::new();
    let mut reap = Vec::new();
    for candidate in candidates {
        match candidate::wake_sweep_candidate_disposition(db, state.as_ref(), &candidate)? {
            WakeSweepDisposition::Recoverable => {
                recoverable.push(candidate);
            }
            WakeSweepDisposition::Abandoned => {
                if reap.len() < WAKE_RECLAIM_REAP_SESSION_LIMIT {
                    reap.push(candidate);
                }
            }
            WakeSweepDisposition::Skip => {}
        }
    }
    Ok((recoverable, reap))
}

fn wake_sweep_plan_from_selected(
    start: Vec<WakeSweepCandidate>,
    reap: Vec<WakeSweepCandidate>,
) -> WakeSweepPlan {
    WakeSweepPlan { start, reap }
}

fn select_recoverable_sweep_candidates(
    mut candidates: Vec<WakeSweepCandidate>,
) -> Vec<WakeSweepCandidate> {
    if candidates.len() <= WAKE_RECLAIM_SWEEP_LIMIT {
        return candidates;
    }
    candidates.sort_by_key(|candidate| candidate.min_pending_seq);
    let oldest_slots = WAKE_RECLAIM_SWEEP_LIMIT.div_ceil(2);
    let newest_slots = WAKE_RECLAIM_SWEEP_LIMIT.saturating_sub(oldest_slots);
    let mut selected = candidates
        .iter()
        .take(oldest_slots)
        .cloned()
        .collect::<Vec<_>>();
    for candidate in candidates.iter().rev().take(newest_slots) {
        if !selected
            .iter()
            .any(|existing| existing.session_id == candidate.session_id)
        {
            selected.push(candidate.clone());
        }
    }
    selected
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WakeSweepDisposition {
    Recoverable,
    Abandoned,
    Skip,
}

fn reap_abandoned_sweep_candidates(
    db: &mut MailboxDb,
    candidates: &[WakeSweepCandidate],
) -> Result<(), String> {
    for candidate in candidates {
        db.mark_pending_abandoned(
            &candidate.session_id,
            WAKE_SWEEP_ABANDONED_ERROR,
            WAKE_RECLAIM_REAP_ROWS_PER_SESSION,
        )?;
    }
    Ok(())
}

fn trace_wake_sweep_candidate(session_id: &str, diagnostic: &WakeDiagnostic) {
    tracing::debug!(
        session_id,
        status = diagnostic.status.as_str(),
        "Wake reclaim sweep candidate evaluated"
    );
}

fn trace_live_pty_retry(session_id: &str, diagnostic: &PtyMailboxDeliveryDiagnostic) {
    tracing::debug!(
        session_id,
        status = diagnostic.status.as_str(),
        submitted = diagnostic.submitted,
        delivered = diagnostic.delivered_seqs.len(),
        "Wake reclaim sweep retried live PTY mailbox delivery"
    );
}
