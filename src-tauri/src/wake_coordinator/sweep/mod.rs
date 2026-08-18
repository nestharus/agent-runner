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

pub(crate) fn start_startup_wake_reclaim_sweep() {
    if is_auto_wake_invocation() {
        return;
    }
    start_wake_reclaim_driver("process_start");
}

pub(crate) fn start_wake_reclaim_maintenance_driver() {
    if is_auto_wake_invocation() {
        return;
    }
    start_wake_reclaim_driver("maintenance_start");
}

fn start_wake_reclaim_driver(initial_trigger: &'static str) {
    static DRIVER: OnceLock<()> = OnceLock::new();
    DRIVER.get_or_init(|| spawn_wake_reclaim_maintenance_driver(initial_trigger));
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

fn spawn_wake_reclaim_maintenance_driver(initial_trigger: &'static str) {
    let result = spawn_wake_reclaim_thread(move || wake_reclaim_maintenance_loop(initial_trigger));
    if let Err(err) = result {
        warn_wake_reclaim_driver_start_failed(err);
    }
}

fn spawn_wake_reclaim_thread(
    worker: impl FnOnce() + Send + 'static,
) -> Result<JoinHandle<()>, std::io::Error> {
    std::thread::Builder::new()
        .name("oulipoly-wake-reclaim-sweep".to_string())
        .spawn(worker)
}

fn warn_wake_reclaim_driver_start_failed(err: std::io::Error) {
    tracing::warn!("Failed to start wake reclaim sweep driver: {err}");
}

fn wake_reclaim_maintenance_loop(initial_trigger: &'static str) {
    run_wake_reclaim_sweep_or_warn(initial_trigger);
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
    retry_pending_live_pty_deliveries(&mut db, trigger)?;
    let candidates = db.wake_sweep_candidates(
        super::constants::WAKE_CLAIM_STALE_AFTER_SECONDS,
        WAKE_RECLAIM_SWEEP_SCAN_LIMIT,
    )?;
    if candidates.is_empty() {
        return Ok(());
    }
    let start =
        plan_and_reap_wake_sweep(&mut db, candidates, state::open_default_state_read_only())?;
    drop(db);
    for candidate in start {
        let diagnostic = start_wake_chain(wake_sweep_start_input(&candidate, trigger));
        trace_wake_sweep_candidate(&candidate.session_id, &diagnostic);
    }
    Ok(())
}

fn retry_pending_live_pty_deliveries_or_warn(trigger: &str) {
    if let Err(err) = retry_pending_live_pty_deliveries_from_default_db(trigger) {
        warn_wake_reclaim_sweep_failed(trigger, err);
    }
}

fn retry_pending_live_pty_deliveries_from_default_db(trigger: &str) -> Result<(), String> {
    let Some(mut db) = MailboxDb::open_default_if_exists()? else {
        return Ok(());
    };
    retry_pending_live_pty_deliveries(&mut db, trigger)
}

fn retry_pending_live_pty_deliveries(db: &mut MailboxDb, trigger: &str) -> Result<(), String> {
    let session_ids = db.pending_delivery_session_ids(WAKE_RECLAIM_SWEEP_SCAN_LIMIT)?;
    for session_id in session_ids {
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
    let Some(runtime) = db.session_runtime(session_id)? else {
        return Ok(LivePtyRetryApplicability::Skip {
            reason: "no_runtime",
            liveness: None,
        });
    };
    if !runtime_is_running_pty_with_socket(&runtime) {
        return Ok(LivePtyRetryApplicability::Skip {
            reason: runtime_skip_reason(&runtime),
            liveness: None,
        });
    }
    let liveness = db.session_liveness(session_id)?;
    if liveness == SessionLiveness::Busy {
        Ok(LivePtyRetryApplicability::Applicable)
    } else {
        Ok(LivePtyRetryApplicability::Skip {
            reason: "not_busy",
            liveness: Some(format!("{liveness:?}")),
        })
    }
}

fn runtime_skip_reason(runtime: &SessionRuntimeRow) -> &'static str {
    if runtime.mode != "pty_interactive" {
        "not_pty"
    } else if runtime.run_state != "running" {
        "not_running"
    } else {
        "no_socket"
    }
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
    state: Option<&oulipoly_state::StateDb>,
) -> Result<WakeSweepPlan, String> {
    let (recoverable, reap) = partition_wake_sweep_candidates(db, candidates, state)?;
    let start = select_recoverable_sweep_candidates(recoverable);
    Ok(wake_sweep_plan_from_selected(start, reap))
}

fn partition_wake_sweep_candidates(
    db: &mut MailboxDb,
    candidates: Vec<WakeSweepCandidate>,
    state: Option<&oulipoly_state::StateDb>,
) -> Result<(Vec<WakeSweepCandidate>, Vec<WakeSweepCandidate>), String> {
    let mut recoverable = Vec::new();
    let mut reap = Vec::new();
    for candidate in candidates {
        match candidate::wake_sweep_candidate_disposition(db, state, &candidate)? {
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

fn plan_and_reap_wake_sweep(
    db: &mut MailboxDb,
    candidates: Vec<WakeSweepCandidate>,
    state: Result<Option<oulipoly_state::StateDb>, String>,
) -> Result<Vec<WakeSweepCandidate>, String> {
    let state = state?;
    let plan = wake_sweep_plan(db, candidates, state.as_ref())?;
    reap_abandoned_sweep_candidates(db, &plan.reap)?;
    Ok(plan.start)
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

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_state::mailbox::{
        AgentBashCompleteEnqueue, EnqueueResult, WakeClaimAcquireResult, WakeClaimRequest,
    };

    #[test]
    fn startup_wake_reclaim_thread_returns_while_the_sweep_is_blocked() {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let worker_release = Arc::clone(&release);

        let handle = spawn_wake_reclaim_thread(move || {
            started_tx.send(()).unwrap();
            let (lock, condition) = &*worker_release;
            let released = lock.lock().unwrap();
            drop(
                condition
                    .wait_while(released, |released| !*released)
                    .unwrap(),
            );
        })
        .unwrap();

        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(!handle.is_finished());
        let (lock, condition) = &*release;
        *lock.lock().unwrap() = true;
        condition.notify_one();
        handle.join().unwrap();
    }

    #[test]
    fn unavailable_state_prevents_terminal_reap_mutation() {
        let directory = tempfile::tempdir().unwrap();
        let state_dir = directory.path().join("agent-bash").join("blocked-state");
        std::fs::create_dir_all(&state_dir).unwrap();
        let meta = state_dir.join("meta.json");
        let log = state_dir.join("log");
        let rc = state_dir.join("rc");
        std::fs::write(&meta, r#"{"caller_chain":[]}"#).unwrap();
        std::fs::write(&log, "pending\n").unwrap();
        std::fs::write(&rc, "0\n").unwrap();
        let state_dir = state_dir.to_string_lossy().into_owned();
        let meta = meta.to_string_lossy().into_owned();
        let log = log.to_string_lossy().into_owned();
        let rc = rc.to_string_lossy().into_owned();
        let mut db = MailboxDb::open(&directory.path().join("pid-identity.db")).unwrap();
        let row = match db
            .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
                session_id: "state-unavailable-session",
                handle: "state-unavailable-handle",
                payload_json: r#"{"schema_version":1,"kind":"agent_bash_complete"}"#,
                owner_invocation_uuid: None,
                matched_os_pid: None,
                matched_os_boot_id: None,
                matched_os_pid_starttime_ticks: None,
                matched_chain_index: None,
                state_dir: &state_dir,
                meta_path: &meta,
                log_path: &log,
                rc_path: &rc,
                rc: 0,
            })
            .unwrap()
        {
            EnqueueResult::Inserted(row) => row,
            other => panic!("unexpected enqueue result: {other:?}"),
        };
        let claim_token = "state-unavailable-claim";
        let acquired = db
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: &row.session_id,
                claim_token,
                reason: "maintenance_tick",
                auto_wake_count: 1,
                wake_invocation_uuid: None,
                stale_after_seconds: 600,
            })
            .unwrap();
        assert!(matches!(acquired, WakeClaimAcquireResult::Acquired(_)));
        let claim_before = db.wake_claim(&row.session_id).unwrap().unwrap();
        let candidate = WakeSweepCandidate {
            session_id: row.session_id.clone(),
            auto_wake_count: 0,
            min_pending_seq: row.seq,
            max_pending_seq: row.seq,
        };

        let invalid_state = directory.path().join("invalid-state.db");
        std::fs::write(&invalid_state, "not sqlite").unwrap();
        let result = plan_and_reap_wake_sweep(
            &mut db,
            vec![candidate],
            state::open_state_read_only_at(&invalid_state),
        );

        assert!(
            result
                .unwrap_err()
                .contains("Failed to open State read-only for wake sweep")
        );
        let rows = db.list_mailbox("state-unavailable-session", true).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].delivered_at.is_none());
        assert!(rows[0].delivery_error.is_none());
        assert_eq!(rows[0].delivery_attempts, 0);
        let claim_after = db.wake_claim(&row.session_id).unwrap().unwrap();
        assert_eq!(claim_after.claim_token, claim_before.claim_token);
        assert_eq!(claim_after.auto_wake_count, claim_before.auto_wake_count);
    }
}
