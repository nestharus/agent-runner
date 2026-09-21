//! Conservative stale-running invocation reconciliation.
//!
//! Declared roles: orchestration, accessor, mapper, parser, predicate, formatter

use chrono::{DateTime, Utc};
use oulipoly_core::CancellationToken;
use oulipoly_state::StateDb;
use oulipoly_state::pid_identity::{
    PidIdentityDb, PidIdentityRow, ProcessIdentity, ProcessIdentityObservation,
    observe_live_process_identity,
};
use std::path::Path;

const STALE_ERROR_CATEGORY: &str = "stale_running";
const STALE_TERMINAL_REASON: &str = "stale_running_liveness";

struct RunningInvocation {
    row_id: i64,
    invocation_uuid: String,
    created_at: DateTime<Utc>,
}

enum LiveProcessIdentityState {
    Live(ProcessIdentity),
    Dead,
    Unknown,
}

pub(crate) fn reconcile_stale_running_invocations(state: &StateDb) -> Result<(), String> {
    let Some(sidecar) = open_pid_sidecar_read_only_optional()? else {
        return Ok(());
    };
    let now = Utc::now();
    for row in running_invocations(state)? {
        if !running_invocation_is_stale(&row, now) {
            continue;
        }
        if !invocation_has_dead_pid_evidence(&sidecar, &row.invocation_uuid)? {
            continue;
        }
        finalize_stale_invocation(state, row.row_id)?;
    }
    Ok(())
}

fn open_pid_sidecar_read_only_optional() -> Result<Option<PidIdentityDb>, String> {
    let path = PidIdentityDb::default_path()?;
    if !path_exists(&path) {
        return Ok(None);
    }
    PidIdentityDb::open_read_only(&path).map(Some)
}

fn path_exists(path: &Path) -> bool {
    path.exists()
}

fn running_invocations(state: &StateDb) -> Result<Vec<RunningInvocation>, String> {
    state
        .list_running_invocations_with_cancel(&CancellationToken::new())?
        .into_iter()
        .map(|row| {
            Ok(RunningInvocation {
                row_id: row.id,
                invocation_uuid: row.invocation_uuid,
                created_at: row.created_at,
            })
        })
        .collect()
}

fn running_invocation_is_stale(row: &RunningInvocation, now: DateTime<Utc>) -> bool {
    let age_seconds = now
        .signed_duration_since(row.created_at)
        .num_seconds()
        .max(0) as u64;
    age_seconds >= oulipoly_runtime::trace::STALE_RUNNING_THRESHOLD_SECONDS
}

fn invocation_has_dead_pid_evidence(
    sidecar: &PidIdentityDb,
    invocation_uuid: &str,
) -> Result<bool, String> {
    let rows = pid_identity_rows_for_invocation(sidecar, invocation_uuid)?;
    Ok(pid_identity_rows_have_dead_evidence(&rows))
}

fn pid_identity_rows_for_invocation(
    sidecar: &PidIdentityDb,
    invocation_uuid: &str,
) -> Result<Vec<PidIdentityRow>, String> {
    sidecar.lookup_by_invocation_uuid(invocation_uuid)
}

fn pid_identity_rows_have_dead_evidence(rows: &[PidIdentityRow]) -> bool {
    if rows.is_empty() {
        return false;
    }
    let mut has_dead_evidence = false;
    for row in rows {
        match pid_identity_row_liveness(row) {
            Some(true) => return false,
            Some(false) => has_dead_evidence = true,
            None => return false,
        }
    }
    has_dead_evidence
}

fn pid_identity_row_liveness(row: &PidIdentityRow) -> Option<bool> {
    match live_process_identity_state(row.os_pid) {
        LiveProcessIdentityState::Live(identity) => {
            Some(process_identity_matches_row(&identity, row))
        }
        LiveProcessIdentityState::Dead => Some(false),
        LiveProcessIdentityState::Unknown => None,
    }
}

fn live_process_identity_state(os_pid: i64) -> LiveProcessIdentityState {
    match observe_live_process_identity(os_pid) {
        ProcessIdentityObservation::ExactLive(identity) => LiveProcessIdentityState::Live(identity),
        ProcessIdentityObservation::Dead => LiveProcessIdentityState::Dead,
        ProcessIdentityObservation::Unsupported | ProcessIdentityObservation::ReadError(_) => {
            LiveProcessIdentityState::Unknown
        }
    }
}

fn process_identity_matches_row(identity: &ProcessIdentity, row: &PidIdentityRow) -> bool {
    identity == &row.identity()
}

fn finalize_stale_invocation(state: &StateDb, row_id: i64) -> Result<(), String> {
    match state.finalize_invocation(
        oulipoly_state::InvocationMutationAuthority::Standalone,
        row_id,
        false,
        -1,
        Some(STALE_ERROR_CATEGORY),
        Some(STALE_TERMINAL_REASON),
    ) {
        Ok(()) => Ok(()),
        Err(err) if invocation_already_finalized(&err) => Ok(()),
        Err(err) => Err(err),
    }
}

fn invocation_already_finalized(err: &str) -> bool {
    err.contains("already finalized")
}
