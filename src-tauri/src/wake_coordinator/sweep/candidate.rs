//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `predicate`

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
        trace_abandoned_transient_wake_skip(&candidate.session_id);
        return Ok(WakeSweepDisposition::Skip);
    }
    if wake_sweep_candidate_is_resumable(db, state, candidate)? {
        if wake_sweep_candidate_reached_cap(candidate) {
            emit_wake_sweep_candidate_cap_reached(&candidate.session_id, candidate);
        }
        return Ok(resumable_wake_sweep_disposition(candidate));
    }
    if wake_sweep_candidate_has_live_owner(db, &candidate.session_id)? {
        return Ok(WakeSweepDisposition::Skip);
    }
    Ok(WakeSweepDisposition::Abandoned)
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
        .map(|count| count > 0)
        .unwrap_or(false)
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
    let has_chain = state
        .chain_id_for_segment(provider_name, &runtime.session_id)
        .map_err(|err| err.to_string())?
        .is_some();
    let turn_count = state
        .count_session_turns(provider_name, &runtime.session_id)
        .map(|counts| counts.total)?;
    Ok(Some((has_chain, turn_count)))
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
    for row in rows {
        if mailbox_row_has_live_owner_identity(row)? {
            return Ok(true);
        }
    }
    Ok(false)
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
    db.wake_claim(session_id).map(|claim| claim.is_some())
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
    read_live_process_identity(recorded.os_pid).map(|live| live.as_ref() == Some(&recorded))
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
