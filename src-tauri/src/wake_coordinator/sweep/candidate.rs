//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `predicate`, `orchestration`

use oulipoly_state::StateDb;
use oulipoly_state::mailbox::{
    MailboxDb, MailboxRow, SessionGenerationProjection, SessionMetadataRow, WakeSweepCandidate,
};
use oulipoly_state::pid_identity::{ProcessIdentity, read_live_process_identity};

use super::consumed;
use super::plan::{WakeSweepAction, WakeSweepRetentionReason};

pub(super) fn wake_sweep_candidate_action(
    db: &mut MailboxDb,
    state: Option<&StateDb>,
    candidate: &WakeSweepCandidate,
) -> Result<WakeSweepAction, String> {
    if db.notifications_paused(&candidate.session_id)? {
        return Ok(WakeSweepAction::Retain(
            WakeSweepRetentionReason::NotStartable,
        ));
    }
    if consumed::pending_mailbox_consumed_marker_present(db, state, &candidate.session_id)? {
        return Ok(WakeSweepAction::Retain(
            WakeSweepRetentionReason::NotStartable,
        ));
    }
    if wake_sweep_candidate_is_unclaimed_abandoned_transient(db, &candidate.session_id)? {
        return abandoned_transient_action(db, state, candidate);
    }
    if wake_sweep_candidate_has_resumable_runtime(db, state, candidate)? {
        return Ok(resumable_wake_sweep_action(candidate));
    }
    if wake_sweep_candidate_has_live_owner(db, &candidate.session_id)? {
        return Ok(WakeSweepAction::Retain(
            WakeSweepRetentionReason::NotStartable,
        ));
    }
    Ok(WakeSweepAction::Retain(
        WakeSweepRetentionReason::DebrisWithoutReapAuthority,
    ))
}

/// Selects the retention reason for an unclaimed session with at least one
/// recorded owner identity and no currently live recorded owner. Such a session
/// is never auto-woken (anti-resurrection, #44/#55). When it also has no durable
/// resume evidence, its pending rows are undeliverable debris retained under the
/// fail-closed policy; a resumable session is also retained so a later deliberate
/// resume can consume it.
fn abandoned_transient_action(
    db: &MailboxDb,
    state: Option<&StateDb>,
    candidate: &WakeSweepCandidate,
) -> Result<WakeSweepAction, String> {
    // Classify as not startable only sessions with durable WORK: at least one
    // produced assistant turn. A bare resume target (a registered chain segment
    // with zero turns) is an empty registration, not work. Both classifications
    // remain pending because this scope assigns no terminal abandonment authority.
    if wake_sweep_candidate_has_produced_turns(db, state, candidate)? {
        trace_abandoned_transient_work_retained(&candidate.session_id);
        return Ok(WakeSweepAction::Retain(
            WakeSweepRetentionReason::NotStartable,
        ));
    }
    trace_abandoned_transient_wake_retained(&candidate.session_id);
    Ok(WakeSweepAction::Retain(
        WakeSweepRetentionReason::DebrisWithoutReapAuthority,
    ))
}

fn wake_sweep_candidate_has_produced_turns(
    db: &MailboxDb,
    state: Option<&StateDb>,
    candidate: &WakeSweepCandidate,
) -> Result<bool, String> {
    let Some(runtime) = db
        .wake_session_reader()
        .session_metadata(&candidate.session_id)?
    else {
        return Ok(false);
    };
    Ok(resume_evidence(state, &runtime)?.is_some_and(|evidence| evidence.turn_count > 0))
}

fn resumable_wake_sweep_action(candidate: &WakeSweepCandidate) -> WakeSweepAction {
    if !wake_sweep_candidate_has_deliverable_pending(&candidate.session_id) {
        return WakeSweepAction::Retain(WakeSweepRetentionReason::NotStartable);
    }
    WakeSweepAction::Start
}

fn wake_sweep_candidate_has_deliverable_pending(session_id: &str) -> bool {
    crate::mailbox_delivery::deliverable_pending_count(session_id)
        .map(|count| count > 0)
        .unwrap_or(false)
}

fn wake_sweep_candidate_has_resumable_runtime(
    db: &mut MailboxDb,
    state: Option<&StateDb>,
    candidate: &WakeSweepCandidate,
) -> Result<bool, String> {
    if db
        .runtime_lifecycle()
        .reconcile_session_liveness(&candidate.session_id)?
        == oulipoly_state::mailbox::SessionLiveness::Busy
    {
        return Ok(false);
    }
    if !matches!(
        db.runtime_lifecycle_reader()
            .session_generation_projection(&candidate.session_id)
            .map_err(|err| err.to_string())?,
        SessionGenerationProjection::None
    ) {
        return Ok(false);
    }
    let Some(runtime) = db
        .wake_session_reader()
        .session_metadata(&candidate.session_id)?
    else {
        return Ok(false);
    };
    if runtime.mode != "headless"
        || runtime
            .provider_name
            .as_deref()
            .is_none_or(|provider| provider.is_empty())
    {
        return Ok(false);
    }
    Ok(resume_evidence(state, &runtime)?
        .is_some_and(|evidence| evidence.has_chain || evidence.turn_count > 0))
}

#[derive(Clone, Copy)]
struct ResumeEvidence {
    has_chain: bool,
    turn_count: u64,
}

fn resume_evidence(
    state: Option<&StateDb>,
    runtime: &SessionMetadataRow,
) -> Result<Option<ResumeEvidence>, String> {
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
        .count_session_turns(provider_name, &runtime.session_id)?
        .total;
    Ok(Some(ResumeEvidence {
        has_chain,
        turn_count,
    }))
}

fn wake_sweep_candidate_has_live_owner(db: &MailboxDb, session_id: &str) -> Result<bool, String> {
    pending_rows_have_live_owner(&db.list_pending(session_id)?)
}

fn pending_rows_have_live_owner(rows: &[MailboxRow]) -> Result<bool, String> {
    let mut has_live_owner = false;
    for row in rows {
        let Some(recorded) = mailbox_row_owner_identity(row) else {
            continue;
        };
        if read_live_process_identity(recorded.os_pid)?.as_ref() == Some(&recorded) {
            has_live_owner = true;
        }
    }
    Ok(has_live_owner)
}

fn wake_sweep_candidate_is_unclaimed_abandoned_transient(
    db: &MailboxDb,
    session_id: &str,
) -> Result<bool, String> {
    if db.wake_session_reader().wake_claim(session_id)?.is_some() {
        return Ok(false);
    }
    let rows = db.list_pending(session_id)?;
    if !rows
        .iter()
        .any(|row| mailbox_row_owner_identity(row).is_some())
    {
        return Ok(false);
    }
    Ok(!pending_rows_have_live_owner(&rows)?)
}

fn mailbox_row_owner_identity(row: &MailboxRow) -> Option<ProcessIdentity> {
    Some(ProcessIdentity {
        os_pid: row.matched_os_pid?,
        os_boot_id: row.matched_os_boot_id.clone()?,
        os_pid_starttime_ticks: row.matched_os_pid_starttime_ticks?,
    })
}

fn trace_abandoned_transient_work_retained(session_id: &str) {
    tracing::warn!(
        session_id,
        "Retaining abandoned transient work with dead owner lineage because it is not startable"
    );
}

fn trace_abandoned_transient_wake_retained(session_id: &str) {
    tracing::warn!(
        session_id,
        "Retaining abandoned transient session with dead owner lineage and no resume evidence"
    );
}
