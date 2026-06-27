//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `predicate`, `orchestration`

use oulipoly_state::StateDb;
use oulipoly_state::mailbox::{MailboxDb, MailboxRow, SessionRuntimeRow, WakeSweepCandidate};
use oulipoly_state::pid_identity::{ProcessIdentity, read_live_process_identity};

use super::{WakeSweepDisposition, consumed};
use crate::wake_coordinator::auto_wake_env::{
    auto_wake_cap_reached, auto_wake_max, emit_auto_wake_cap_reached,
};

pub(super) fn wake_sweep_candidate_disposition(
    db: &MailboxDb,
    state: Option<&StateDb>,
    candidate: &WakeSweepCandidate,
) -> Result<WakeSweepDisposition, String> {
    // Recoverable means either an idle headless runtime with durable resume
    // evidence, or a live owner PID identity that must not be reaped. Missing
    // runtime/history with no live owner is abandoned debris, not resumable work.
    if consumed::pending_mailbox_consumed_marker_present(db, &candidate.session_id) {
        return Ok(WakeSweepDisposition::Skip);
    }
    if wake_sweep_candidate_is_unclaimed_abandoned_transient(db, &candidate.session_id)? {
        return abandoned_transient_disposition(db, state, candidate);
    }
    if wake_sweep_candidate_is_resumable(db, state, candidate)? {
        return Ok(resumable_disposition_with_cap_emit(candidate));
    }
    if wake_sweep_candidate_has_live_owner(db, &candidate.session_id)? {
        return Ok(WakeSweepDisposition::Skip);
    }
    Ok(WakeSweepDisposition::Abandoned)
}

/// Disposition for an unclaimed session whose pending rows all have a dead owner
/// PID lineage. Such a session is never auto-woken (anti-resurrection, #44/#55).
/// When it also has no durable resume evidence, its pending rows are
/// undeliverable debris and are reaped so they do not accumulate; a resumable
/// session is left pending so a later deliberate resume can still consume it.
fn abandoned_transient_disposition(
    db: &MailboxDb,
    state: Option<&StateDb>,
    candidate: &WakeSweepCandidate,
) -> Result<WakeSweepDisposition, String> {
    // Preserve only sessions with durable WORK — at least one produced assistant
    // turn. A bare resume target (a registered chain segment with zero turns) is
    // an empty registration, not work, so a dead-owner session with no produced
    // turns is reaped rather than left pending forever.
    if wake_sweep_candidate_has_produced_turns(db, state, candidate)? {
        trace_abandoned_transient_wake_skip(&candidate.session_id);
        return Ok(WakeSweepDisposition::Skip);
    }
    trace_abandoned_transient_wake_reap(&candidate.session_id);
    Ok(WakeSweepDisposition::Abandoned)
}

fn wake_sweep_candidate_has_produced_turns(
    db: &MailboxDb,
    state: Option<&StateDb>,
    candidate: &WakeSweepCandidate,
) -> Result<bool, String> {
    let Some(runtime) = wake_sweep_candidate_runtime(db, candidate)? else {
        return Ok(false);
    };
    let evidence = wake_sweep_runtime_resume_evidence_values(state, &runtime)?;
    Ok(runtime_has_produced_turns(evidence))
}

fn runtime_has_produced_turns(evidence: Option<(bool, u64)>) -> bool {
    evidence
        .map(|(_, turn_count)| turn_count > 0)
        .unwrap_or(false)
}

fn resumable_disposition_with_cap_emit(candidate: &WakeSweepCandidate) -> WakeSweepDisposition {
    emit_cap_reached_if_capped(candidate);
    resumable_wake_sweep_disposition(candidate)
}

fn emit_cap_reached_if_capped(candidate: &WakeSweepCandidate) {
    if wake_sweep_candidate_reached_cap(candidate) {
        emit_wake_sweep_candidate_cap_reached(&candidate.session_id, candidate);
    }
}

fn resumable_wake_sweep_disposition(candidate: &WakeSweepCandidate) -> WakeSweepDisposition {
    if wake_sweep_candidate_reached_cap(candidate) {
        return WakeSweepDisposition::Skip;
    }
    if !wake_sweep_candidate_has_deliverable_pending(&candidate.session_id) {
        return WakeSweepDisposition::Skip;
    }
    WakeSweepDisposition::Recoverable
}

fn wake_sweep_candidate_has_deliverable_pending(session_id: &str) -> bool {
    crate::mailbox_delivery::deliverable_pending_count(session_id)
        .map(count_is_positive)
        .unwrap_or(false)
}

fn count_is_positive(count: usize) -> bool {
    count > 0
}

fn option_is_present<T>(value: Option<T>) -> bool {
    value.is_some()
}

fn wake_sweep_candidate_is_resumable(
    db: &MailboxDb,
    state: Option<&StateDb>,
    candidate: &WakeSweepCandidate,
) -> Result<bool, String> {
    let Some(runtime) = wake_sweep_candidate_runtime(db, candidate)? else {
        return Ok(false);
    };
    wake_sweep_runtime_is_resumable(state, &runtime)
}

fn wake_sweep_candidate_runtime(
    db: &MailboxDb,
    candidate: &WakeSweepCandidate,
) -> Result<Option<SessionRuntimeRow>, String> {
    db.session_runtime(&candidate.session_id)
}

fn wake_sweep_runtime_is_resumable(
    state: Option<&StateDb>,
    runtime: &SessionRuntimeRow,
) -> Result<bool, String> {
    if !wake_sweep_runtime_can_resume(runtime) {
        return Ok(false);
    }
    wake_sweep_runtime_has_resume_evidence(state, runtime)
}

fn wake_sweep_runtime_can_resume(runtime: &SessionRuntimeRow) -> bool {
    runtime.mode == "headless"
        && runtime.run_state != "running"
        && runtime
            .provider_name
            .as_deref()
            .is_some_and(|provider| !provider.is_empty())
}

fn wake_sweep_runtime_has_resume_evidence(
    state: Option<&StateDb>,
    runtime: &SessionRuntimeRow,
) -> Result<bool, String> {
    let evidence = wake_sweep_runtime_resume_evidence_values(state, runtime)?;
    Ok(resume_evidence_values_present(evidence))
}

fn wake_sweep_runtime_resume_evidence_values(
    state: Option<&StateDb>,
    runtime: &SessionRuntimeRow,
) -> Result<Option<(bool, u64)>, String> {
    let Some(state) = state else {
        return Ok(None);
    };
    let Some(provider_name) = runtime.provider_name.as_deref() else {
        return Ok(None);
    };
    let has_chain = segment_has_chain(state, provider_name, &runtime.session_id)?;
    let turn_count = session_total_turn_count(state, provider_name, &runtime.session_id)?;
    Ok(Some((has_chain, turn_count)))
}

fn segment_has_chain(
    state: &StateDb,
    provider_name: &str,
    session_id: &str,
) -> Result<bool, String> {
    state
        .chain_id_for_segment(provider_name, session_id)
        .map(option_is_present)
        .map_err(|err| err.to_string())
}

fn session_total_turn_count(
    state: &StateDb,
    provider_name: &str,
    session_id: &str,
) -> Result<u64, String> {
    Ok(state.count_session_turns(provider_name, session_id)?.total)
}

fn resume_evidence_values_present(evidence: Option<(bool, u64)>) -> bool {
    evidence
        .map(|(has_chain, turn_count)| has_chain || turn_count > 0)
        .unwrap_or(false)
}

fn wake_sweep_candidate_has_live_owner(db: &MailboxDb, session_id: &str) -> Result<bool, String> {
    let rows = pending_mailbox_rows(db, session_id)?;
    pending_rows_have_live_owner(&rows)
}

fn pending_mailbox_rows(db: &MailboxDb, session_id: &str) -> Result<Vec<MailboxRow>, String> {
    db.list_pending(session_id)
}

fn pending_rows_have_live_owner(rows: &[MailboxRow]) -> Result<bool, String> {
    Ok(pending_rows_liveness(rows)?.contains(&true))
}

fn pending_rows_liveness(rows: &[MailboxRow]) -> Result<Vec<bool>, String> {
    rows.iter()
        .map(mailbox_row_has_live_owner_identity)
        .collect()
}

fn wake_sweep_candidate_is_abandoned_transient(
    db: &MailboxDb,
    session_id: &str,
) -> Result<bool, String> {
    let rows = pending_mailbox_rows(db, session_id)?;
    pending_rows_are_abandoned_transient(&rows)
}

fn wake_sweep_candidate_is_unclaimed_abandoned_transient(
    db: &MailboxDb,
    session_id: &str,
) -> Result<bool, String> {
    if wake_sweep_candidate_has_wake_claim(db, session_id)? {
        return Ok(false);
    }
    wake_sweep_candidate_is_abandoned_transient(db, session_id)
}

fn wake_sweep_candidate_has_wake_claim(db: &MailboxDb, session_id: &str) -> Result<bool, String> {
    db.wake_claim(session_id).map(option_is_present)
}

fn pending_rows_are_abandoned_transient(rows: &[MailboxRow]) -> Result<bool, String> {
    if !pending_rows_have_owner_identity(rows) {
        return Ok(false);
    }
    Ok(!pending_rows_have_live_owner(rows)?)
}

fn pending_rows_have_owner_identity(rows: &[MailboxRow]) -> bool {
    rows.iter().any(mailbox_row_owner_identity_present)
}

fn mailbox_row_owner_identity_present(row: &MailboxRow) -> bool {
    mailbox_row_owner_identity(row).is_some()
}

fn mailbox_row_has_live_owner_identity(row: &MailboxRow) -> Result<bool, String> {
    let Some(recorded) = mailbox_row_owner_identity(row) else {
        return Ok(false);
    };
    recorded_identity_is_live(&recorded)
}

fn recorded_identity_is_live(recorded: &ProcessIdentity) -> Result<bool, String> {
    read_live_process_identity(recorded.os_pid)
        .map(|live| live_identity_matches(live.as_ref(), recorded))
}

fn live_identity_matches(live: Option<&ProcessIdentity>, recorded: &ProcessIdentity) -> bool {
    live == Some(recorded)
}

fn mailbox_row_owner_identity(row: &MailboxRow) -> Option<ProcessIdentity> {
    Some(ProcessIdentity {
        os_pid: row.matched_os_pid?,
        os_boot_id: row.matched_os_boot_id.clone()?,
        os_pid_starttime_ticks: row.matched_os_pid_starttime_ticks?,
    })
}

fn wake_sweep_candidate_reached_cap(candidate: &WakeSweepCandidate) -> bool {
    auto_wake_cap_reached(candidate.auto_wake_count.saturating_sub(1), auto_wake_max())
}

fn emit_wake_sweep_candidate_cap_reached(session_id: &str, candidate: &WakeSweepCandidate) {
    emit_auto_wake_cap_reached(
        session_id,
        candidate.auto_wake_count.saturating_sub(1),
        auto_wake_max(),
    );
}

fn trace_abandoned_transient_wake_skip(session_id: &str) {
    tracing::warn!(
        session_id,
        "Skipping auto wake for abandoned transient session with dead owner lineage"
    );
}

fn trace_abandoned_transient_wake_reap(session_id: &str) {
    tracing::warn!(
        session_id,
        "Reaping abandoned transient session with dead owner lineage and no resume evidence"
    );
}
