//! Conservative stale-running invocation reconciliation.

use chrono::{DateTime, Utc};
use oulipoly_state::StateDb;
use oulipoly_state::pid_identity::{
    PidIdentityDb, PidIdentityRow, ProcessIdentity, read_live_process_identity,
};
use std::path::Path;

const STALE_ERROR_CATEGORY: &str = "stale_running";
const STALE_TERMINAL_REASON: &str = "stale_running_liveness";

struct RunningInvocation {
    row_id: i64,
    invocation_uuid: String,
    created_at: DateTime<Utc>,
}

struct RunningInvocationRow {
    row_id: i64,
    invocation_uuid: String,
    created_at_raw: String,
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
    running_invocation_rows(state)?
        .into_iter()
        .map(running_invocation_from_row)
        .collect()
}

fn running_invocation_rows(state: &StateDb) -> Result<Vec<RunningInvocationRow>, String> {
    let mut stmt = state
        .connection()
        .prepare(
            "SELECT id, invocation_uuid, created_at
             FROM invocations
             WHERE status = 'running' AND finished_at IS NULL",
        )
        .map_err(|err| format!("Failed to prepare stale-running query: {err}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|err| format!("Failed to query stale-running rows: {err}"))?;
    rows.map(|row| {
        let (row_id, invocation_uuid, created_at_raw) =
            row.map_err(|err| format!("Failed to map stale-running row: {err}"))?;
        Ok(running_invocation_row(
            row_id,
            invocation_uuid,
            created_at_raw,
        ))
    })
    .collect()
}

fn running_invocation_row(
    row_id: i64,
    invocation_uuid: String,
    created_at_raw: String,
) -> RunningInvocationRow {
    RunningInvocationRow {
        row_id,
        invocation_uuid,
        created_at_raw,
    }
}

fn running_invocation_from_row(row: RunningInvocationRow) -> Result<RunningInvocation, String> {
    let created_at = parse_running_invocation_created_at(&row.created_at_raw)?;
    Ok(RunningInvocation {
        row_id: row.row_id,
        invocation_uuid: row.invocation_uuid,
        created_at,
    })
}

fn parse_running_invocation_created_at(created_at_raw: &str) -> Result<DateTime<Utc>, String> {
    let created_at = DateTime::parse_from_rfc3339(created_at_raw)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|err| format!("Failed to parse stale-running created_at: {err}"))?;
    Ok(created_at)
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
    let rows = sidecar.lookup_by_invocation_uuid(invocation_uuid)?;
    if rows.is_empty() {
        return Ok(false);
    }
    let mut has_dead_evidence = false;
    for row in rows {
        match pid_identity_row_liveness(&row) {
            Some(true) => return Ok(false),
            Some(false) => has_dead_evidence = true,
            None => return Ok(false),
        }
    }
    Ok(has_dead_evidence)
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
    match read_live_process_identity(os_pid) {
        Ok(Some(identity)) => LiveProcessIdentityState::Live(identity),
        Ok(None) => LiveProcessIdentityState::Dead,
        Err(_) => LiveProcessIdentityState::Unknown,
    }
}

fn process_identity_matches_row(identity: &ProcessIdentity, row: &PidIdentityRow) -> bool {
    identity == &row.identity()
}

fn finalize_stale_invocation(state: &StateDb, row_id: i64) -> Result<(), String> {
    match state.finalize_invocation(
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
