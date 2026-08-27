//! Startable wake-sweep candidate planning and selection.
//!
//! ## Declared roles
//!
//! `filter`, `mapper`, `orchestration`

use oulipoly_state::StateDb;
use oulipoly_state::mailbox::{MailboxDb, WakeSweepCandidate};

use super::candidate::wake_sweep_candidate_action;
use crate::wake_coordinator::diagnostics::WakeDiagnostic;
use crate::wake_coordinator::wake_start::StartWakeInput;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WakeSweepAction {
    Start,
    Retain(WakeSweepRetentionReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WakeSweepRetentionReason {
    NotStartable,
    DebrisWithoutReapAuthority,
}

pub(super) fn plan_wake_sweep(
    db: &mut MailboxDb,
    candidates: Vec<WakeSweepCandidate>,
    state: Result<Option<StateDb>, String>,
) -> Result<Option<WakeSweepCandidate>, String> {
    let state = state?;
    let mut startable = Vec::new();
    for candidate in candidates {
        match wake_sweep_candidate_action(db, state.as_ref(), &candidate)? {
            WakeSweepAction::Start => startable.push(candidate),
            WakeSweepAction::Retain(WakeSweepRetentionReason::DebrisWithoutReapAuthority) => {
                trace_retained_debris_candidate(&candidate.session_id);
            }
            WakeSweepAction::Retain(WakeSweepRetentionReason::NotStartable) => {}
        }
    }
    Ok(select_startable_sweep_candidate(startable))
}

pub(super) fn select_startable_sweep_candidate(
    candidates: Vec<WakeSweepCandidate>,
) -> Option<WakeSweepCandidate> {
    candidates
        .into_iter()
        .min_by_key(|candidate| candidate.min_pending_seq)
}

pub(super) fn wake_sweep_start_input<'a>(
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

fn trace_retained_debris_candidate(session_id: &str) {
    tracing::warn!(
        session_id,
        "Wake reclaim sweep retained debris candidate because terminal reap lacks cross-store authority"
    );
}

pub(super) fn trace_wake_sweep_candidate(session_id: &str, diagnostic: &WakeDiagnostic) {
    tracing::debug!(
        session_id,
        status = diagnostic.status.as_str(),
        "Wake reclaim sweep candidate evaluated"
    );
}
