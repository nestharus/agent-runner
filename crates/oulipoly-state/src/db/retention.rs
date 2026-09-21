use super::{StateDb, sqlite};
use crate::live_history::TERMINAL_RETENTION_PRUNE;
use crate::retention::{
    PreservationReason, RetentionBatchCursor, RetentionBatchOutcome, RetentionBatchRequest,
    RetentionBatchStatus, RetentionFamily, format_timestamp_micros, parse_authoritative_timestamp,
};
use rusqlite::{ErrorCode, Transaction, TransactionBehavior, params};
use std::time::Duration;

#[derive(Debug, Clone)]
struct StateRetentionCandidate {
    key: String,
    authoritative_at: String,
}

impl StateDb {
    /// Execute one bounded State retention slice. Candidate discovery is an
    /// autocommit indexed read; every candidate gets its own short nonblocking
    /// write transaction and exact stale-snapshot predicate. The method never
    /// loops beyond the caller's row bound and never schedules more work.
    pub fn run_retention_batch(
        &mut self,
        request: &RetentionBatchRequest,
    ) -> Result<RetentionBatchOutcome, String> {
        self.run_retention_batch_after_snapshot(request, || {})
    }

    fn run_retention_batch_after_snapshot(
        &mut self,
        request: &RetentionBatchRequest,
        after_snapshot: impl FnOnce(),
    ) -> Result<RetentionBatchOutcome, String> {
        self.access_scope
            .authorize(TERMINAL_RETENTION_PRUNE, None)?;
        let cutoff = request.validate()?;
        let candidate_upper_bound = candidate_upper_bound(cutoff)?;
        let mut outcome = RetentionBatchOutcome::new(request, cutoff);
        let mut candidates = match request.family {
            RetentionFamily::CompletedTurn => {
                completed_turn_candidates(&self.conn, request, &candidate_upper_bound, cutoff)?
            }
            RetentionFamily::ProviderLogicalLaunch => {
                provider_launch_candidates(&self.conn, request, &candidate_upper_bound, cutoff)?
            }
            RetentionFamily::Invocation => {
                invocation_candidates(&self.conn, request, &candidate_upper_bound, cutoff)?
            }
            RetentionFamily::ProviderLaunchAttempt => {
                outcome.preserve(PreservationReason::Inherited);
                outcome.emit_independent_observation();
                return Ok(outcome);
            }
            _ => {
                return Err(format!(
                    "retention family {} does not belong to the State database",
                    request.family.as_str()
                ));
            }
        };
        let more_work = candidates.len() > request.limit;
        candidates.truncate(request.limit);
        after_snapshot();

        let previous_busy_timeout = sqlite_busy_timeout(&self.conn)?;
        self.conn
            .busy_timeout(Duration::ZERO)
            .map_err(|error| format!("failed to configure nonblocking State retention: {error}"))?;
        for candidate in candidates {
            outcome.candidates_examined += 1;
            match delete_state_candidate(&mut self.conn, request.family, &candidate, cutoff) {
                Ok(true) => outcome.records_deleted += 1,
                Ok(false) => outcome.preserve(PreservationReason::StaleCandidate),
                Err(error) if sqlite_contention(&error) => {
                    outcome.preserve(PreservationReason::WriterBusy);
                    outcome.status = RetentionBatchStatus::Busy;
                    break;
                }
                Err(error) if sqlite_constraint(&error) => {
                    // A concurrently materialized or previously unknown child
                    // authority is positive preservation evidence, not a
                    // deletion failure to work around.
                    outcome.preserve(PreservationReason::RecoveryAuthoritative);
                }
                Err(error) => {
                    outcome.gap("state_delete", typed_sqlite_reason(&error));
                    break;
                }
            }
            outcome.next_cursor = Some(RetentionBatchCursor {
                policy_version: request.policy.version.clone(),
                family: request.family,
                cutoff_unix_micros: cutoff,
                after_authoritative_at: candidate.authoritative_at,
                after_key: candidate.key,
            });
        }
        if outcome.status == RetentionBatchStatus::Complete && more_work {
            outcome.status = RetentionBatchStatus::MoreWork;
        }
        if let Err(error) = self
            .conn
            .busy_timeout(Duration::from_millis(previous_busy_timeout))
        {
            outcome.gap("restore_busy_timeout", typed_sqlite_reason(&error));
        }
        outcome.emit_independent_observation();
        Ok(outcome)
    }
}

fn completed_turn_candidates(
    conn: &sqlite::Connection,
    request: &RetentionBatchRequest,
    candidate_upper_bound: &str,
    cutoff_micros: i64,
) -> Result<Vec<StateRetentionCandidate>, String> {
    query_candidates(
        conn,
        "SELECT invocation_uuid, retention_eligible_at
         FROM completed_turns
         WHERE recovery_pending=0
           AND committed_at IS NOT NULL
           AND retention_status='eligible'
           AND retention_eligible_at IS NOT NULL
           AND retention_eligible_at < ?1
           AND oulipoly_rfc3339_micros(retention_eligible_at) <= ?2
           AND (?3 IS NULL
                OR retention_eligible_at > ?3
                OR (retention_eligible_at = ?3 AND invocation_uuid > ?4))
         ORDER BY retention_eligible_at, invocation_uuid
         LIMIT ?5",
        request,
        candidate_upper_bound,
        cutoff_micros,
    )
}

fn provider_launch_candidates(
    conn: &sqlite::Connection,
    request: &RetentionBatchRequest,
    candidate_upper_bound: &str,
    cutoff_micros: i64,
) -> Result<Vec<StateRetentionCandidate>, String> {
    query_candidates(
        conn,
        "SELECT logical_launch_id, retention_eligible_at
         FROM provider_logical_launches AS launch
         WHERE launch.status IN ('succeeded','failed','cancelled')
           AND launch.retention_status='eligible'
           AND launch.retention_eligible_at IS NOT NULL
           AND launch.retention_eligible_at < ?1
           AND oulipoly_rfc3339_micros(launch.retention_eligible_at) <= ?2
           AND NOT EXISTS (
               SELECT 1 FROM provider_launch_attempts AS attempt
               WHERE attempt.logical_launch_id=launch.logical_launch_id
                 AND (attempt.status NOT IN ('superseded','succeeded','failed','cancelled')
                      OR attempt.retention_status!='eligible'
                      OR attempt.retention_eligible_at IS NULL
                      OR oulipoly_rfc3339_micros(attempt.retention_eligible_at) IS NULL
                      OR oulipoly_rfc3339_micros(attempt.retention_eligible_at) > ?2
                      OR attempt.actor_custody_state IN ('active','uncertain')
                      OR attempt.return_channel_state IN ('quarantined','cleanup_failed'))
           )
           AND NOT EXISTS (
               SELECT 1 FROM provider_launch_transition_replays AS replay
               WHERE replay.logical_launch_id=launch.logical_launch_id
                 AND replay.retention_status!='inherits_parent')
           AND NOT EXISTS (
               SELECT 1 FROM provider_launch_native_channel_duties AS duty
               WHERE duty.logical_launch_id=launch.logical_launch_id)
           AND (?3 IS NULL
                OR launch.retention_eligible_at > ?3
                OR (launch.retention_eligible_at = ?3
                    AND launch.logical_launch_id > ?4))
         ORDER BY launch.retention_eligible_at, launch.logical_launch_id
         LIMIT ?5",
        request,
        candidate_upper_bound,
        cutoff_micros,
    )
}

fn invocation_candidates(
    conn: &sqlite::Connection,
    request: &RetentionBatchRequest,
    candidate_upper_bound: &str,
    cutoff_micros: i64,
) -> Result<Vec<StateRetentionCandidate>, String> {
    // The root is considered only when the known recovery and user-result
    // surfaces are absent. Any additional foreign-key authority appearing
    // before deletion is caught by the exact short transaction.
    query_candidates(
        conn,
        "SELECT invocation_uuid, retention_eligible_at
         FROM invocations AS invocation
         WHERE invocation.status IN ('succeeded','failed')
           AND invocation.retention_status='eligible'
           AND invocation.retention_eligible_at IS NOT NULL
           AND invocation.retention_eligible_at < ?1
           AND oulipoly_rfc3339_micros(invocation.retention_eligible_at) <= ?2
           AND NOT EXISTS (SELECT 1 FROM invocations child
                           WHERE child.parent_invocation_id=invocation.id)
           AND NOT EXISTS (SELECT 1 FROM provider_launch_attempts attempt
                           WHERE attempt.invocation_id=invocation.id)
           AND NOT EXISTS (SELECT 1 FROM completed_turns turn
                           WHERE turn.invocation_id=invocation.id)
           AND NOT EXISTS (SELECT 1 FROM invocation_returned_artifacts artifact
                           WHERE artifact.invocation_id=invocation.id)
           AND NOT EXISTS (SELECT 1 FROM invocation_output_deliveries delivery
                           WHERE delivery.invocation_id=invocation.id)
           AND NOT EXISTS (SELECT 1 FROM invocation_provider_session_authority authority
                           WHERE authority.invocation_id=invocation.id)
           AND (?3 IS NULL
                OR invocation.retention_eligible_at > ?3
                OR (invocation.retention_eligible_at = ?3
                    AND invocation.invocation_uuid > ?4))
         ORDER BY invocation.retention_eligible_at, invocation.invocation_uuid
         LIMIT ?5",
        request,
        candidate_upper_bound,
        cutoff_micros,
    )
}

fn query_candidates(
    conn: &sqlite::Connection,
    sql: &str,
    request: &RetentionBatchRequest,
    candidate_upper_bound: &str,
    cutoff_micros: i64,
) -> Result<Vec<StateRetentionCandidate>, String> {
    let limit = i64::try_from(request.limit.saturating_add(1))
        .map_err(|_| "retention batch limit does not fit SQLite INTEGER".to_string())?;
    let (after_at, after_key) = request
        .cursor
        .as_ref()
        .map(|cursor| {
            (
                Some(cursor.after_authoritative_at.as_str()),
                cursor.after_key.as_str(),
            )
        })
        .unwrap_or((None, ""));
    let mut statement = conn
        .prepare(sql)
        .map_err(|error| format!("failed to prepare State retention candidates: {error}"))?;
    let rows = statement
        .query_map(
            params![
                candidate_upper_bound,
                cutoff_micros,
                after_at,
                after_key,
                limit
            ],
            |row| {
                Ok(StateRetentionCandidate {
                    key: row.get(0)?,
                    authoritative_at: row.get(1)?,
                })
            },
        )
        .map_err(|error| format!("failed to query State retention candidates: {error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to read State retention candidate: {error}"))
        .map(|rows| {
            rows.into_iter()
                .filter(|candidate| {
                    parse_authoritative_timestamp(&candidate.authoritative_at)
                        .is_some_and(|timestamp| timestamp <= cutoff_micros)
                })
                .collect()
        })
}

fn delete_state_candidate(
    conn: &mut sqlite::Connection,
    family: RetentionFamily,
    candidate: &StateRetentionCandidate,
    cutoff_micros: i64,
) -> Result<bool, rusqlite::Error> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let changed = match family {
        RetentionFamily::CompletedTurn => delete_completed_turn(&tx, candidate, cutoff_micros)?,
        RetentionFamily::ProviderLogicalLaunch => {
            delete_provider_launch(&tx, candidate, cutoff_micros)?
        }
        RetentionFamily::Invocation => delete_invocation(&tx, candidate, cutoff_micros)?,
        _ => 0,
    };
    tx.commit()?;
    Ok(changed == 1)
}

fn delete_completed_turn(
    tx: &Transaction<'_>,
    candidate: &StateRetentionCandidate,
    cutoff_micros: i64,
) -> rusqlite::Result<usize> {
    tx.execute(
        "DELETE FROM completed_turn_selections
         WHERE invocation_id=(
             SELECT invocation_id FROM completed_turns
             WHERE invocation_uuid=?1 AND retention_eligible_at=?2
               AND retention_status='eligible' AND recovery_pending=0
               AND committed_at IS NOT NULL
               AND oulipoly_rfc3339_micros(retention_eligible_at)<=?3)",
        params![candidate.key, candidate.authoritative_at, cutoff_micros],
    )?;
    tx.execute(
        "DELETE FROM completed_turns
         WHERE invocation_uuid=?1 AND retention_eligible_at=?2
           AND retention_status='eligible' AND recovery_pending=0
           AND committed_at IS NOT NULL
           AND oulipoly_rfc3339_micros(retention_eligible_at)<=?3",
        params![candidate.key, candidate.authoritative_at, cutoff_micros],
    )
}

fn delete_provider_launch(
    tx: &Transaction<'_>,
    candidate: &StateRetentionCandidate,
    cutoff_micros: i64,
) -> rusqlite::Result<usize> {
    let still_eligible: bool = tx.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM provider_logical_launches AS launch
            WHERE launch.logical_launch_id=?1
              AND launch.retention_eligible_at=?2
              AND launch.retention_status='eligible'
              AND launch.status IN ('succeeded','failed','cancelled')
              AND oulipoly_rfc3339_micros(launch.retention_eligible_at)<=?3
              AND NOT EXISTS(
                  SELECT 1 FROM provider_launch_attempts AS attempt
                  WHERE attempt.logical_launch_id=launch.logical_launch_id
                    AND (attempt.status NOT IN ('superseded','succeeded','failed','cancelled')
                         OR attempt.retention_status!='eligible'
                         OR attempt.retention_eligible_at IS NULL
                         OR oulipoly_rfc3339_micros(attempt.retention_eligible_at) IS NULL
                         OR oulipoly_rfc3339_micros(attempt.retention_eligible_at)>?3
                         OR attempt.actor_custody_state IN ('active','uncertain')
                         OR attempt.return_channel_state IN ('quarantined','cleanup_failed')))
              AND NOT EXISTS(
                  SELECT 1 FROM provider_launch_transition_replays AS replay
                  WHERE replay.logical_launch_id=launch.logical_launch_id
                    AND replay.retention_status!='inherits_parent')
              AND NOT EXISTS(
                  SELECT 1 FROM provider_launch_native_channel_duties AS duty
                  WHERE duty.logical_launch_id=launch.logical_launch_id))",
        params![candidate.key, candidate.authoritative_at, cutoff_micros],
        |row| row.get(0),
    )?;
    if !still_eligible {
        return Ok(0);
    }
    tx.execute(
        "DELETE FROM provider_launch_transition_replays WHERE logical_launch_id=?1",
        [&candidate.key],
    )?;
    tx.execute(
        "DELETE FROM provider_launch_attempts WHERE logical_launch_id=?1",
        [&candidate.key],
    )?;
    tx.execute(
        "DELETE FROM provider_logical_launches
         WHERE logical_launch_id=?1 AND retention_eligible_at=?2
           AND retention_status='eligible'",
        params![candidate.key, candidate.authoritative_at],
    )
}

fn delete_invocation(
    tx: &Transaction<'_>,
    candidate: &StateRetentionCandidate,
    cutoff_micros: i64,
) -> rusqlite::Result<usize> {
    tx.execute(
        "DELETE FROM invocations
         WHERE invocation_uuid=?1 AND retention_eligible_at=?2
           AND retention_status='eligible' AND status IN ('succeeded','failed')
           AND oulipoly_rfc3339_micros(retention_eligible_at)<=?3
           AND NOT EXISTS (SELECT 1 FROM invocations child
                           WHERE child.parent_invocation_id=invocations.id)
           AND NOT EXISTS (SELECT 1 FROM provider_launch_attempts attempt
                           WHERE attempt.invocation_id=invocations.id)
           AND NOT EXISTS (SELECT 1 FROM completed_turns turn
                           WHERE turn.invocation_id=invocations.id)
           AND NOT EXISTS (SELECT 1 FROM invocation_returned_artifacts artifact
                           WHERE artifact.invocation_id=invocations.id)
           AND NOT EXISTS (SELECT 1 FROM invocation_output_deliveries delivery
                           WHERE delivery.invocation_id=invocations.id)
           AND NOT EXISTS (SELECT 1 FROM invocation_provider_session_authority authority
                           WHERE authority.invocation_id=invocations.id)",
        params![candidate.key, candidate.authoritative_at, cutoff_micros],
    )
}

fn candidate_upper_bound(cutoff_micros: i64) -> Result<String, String> {
    let next_second = cutoff_micros
        .div_euclid(1_000_000)
        .checked_add(1)
        .and_then(|seconds| seconds.checked_mul(1_000_000))
        .ok_or_else(|| "retention cutoff cannot be rounded for indexed selection".to_string())?;
    format_timestamp_micros(next_second)
}

fn sqlite_busy_timeout(conn: &sqlite::Connection) -> Result<u64, String> {
    let value: i64 = conn
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .map_err(|error| format!("failed to read State retention busy timeout: {error}"))?;
    u64::try_from(value).map_err(|_| "State retention busy timeout is negative".to_string())
}

fn sqlite_contention(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

fn sqlite_constraint(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(ErrorCode::ConstraintViolation)
    )
}

fn typed_sqlite_reason(error: &rusqlite::Error) -> String {
    error
        .sqlite_error_code()
        .map(|code| format!("sqlite_{code:?}").to_ascii_lowercase())
        .unwrap_or_else(|| "sqlite_adapter_error".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retention::{
        DEFAULT_TERMINAL_RETENTION_MICROS, RetentionObservationDelivery, RetentionPolicy,
    };
    use rusqlite::Connection;
    use tempfile::TempDir;

    const NOW: i64 = 4_000_000_000_000_000;

    fn request(family: RetentionFamily, limit: usize) -> RetentionBatchRequest {
        RetentionBatchRequest {
            policy: RetentionPolicy::default(),
            family,
            as_of_unix_micros: NOW,
            limit,
            cursor: None,
        }
    }

    fn fixture() -> (TempDir, StateDb) {
        let directory = tempfile::tempdir().unwrap();
        let state = StateDb::open_historical(&directory.path().join("state.db")).unwrap();
        (directory, state)
    }

    fn insert_terminal_invocation(state: &StateDb, uuid: &str, offset: i64) -> i64 {
        let closed =
            format_timestamp_micros(NOW - DEFAULT_TERMINAL_RETENTION_MICROS + offset).unwrap();
        let created =
            format_timestamp_micros(NOW - DEFAULT_TERMINAL_RETENTION_MICROS - 1_000_000).unwrap();
        state
            .conn
            .execute(
                "INSERT INTO invocations(
                    invocation_uuid,model_name,provider_name,provider_index,status,
                    success,exit_code,created_at,finished_at)
                 VALUES(?1,'model','provider',0,'succeeded',1,0,?2,?3)",
                params![uuid, created, closed],
            )
            .unwrap();
        state.conn.last_insert_rowid()
    }

    fn insert_closed_turn(state: &StateDb, row: i64, uuid: &str) {
        let closed: String = state
            .conn
            .query_row(
                "SELECT finished_at FROM invocations WHERE id=?1",
                [row],
                |query_row| query_row.get(0),
            )
            .unwrap();
        state
            .conn
            .execute(
                "INSERT INTO completed_turns(
                    invocation_id,invocation_uuid,settlement_id,effects_json,
                    context_json,content_sha256,committed_at,tails_json,
                    recovery_pending,created_at,updated_at,closed_at,
                    retention_eligible_at,retention_status)
                 VALUES(?1,?2,?3,'{}','{}',?4,?5,'{}',0,?5,?5,?5,?5,'eligible')",
                params![
                    row,
                    uuid,
                    format!("settlement-{uuid}"),
                    "0".repeat(64),
                    closed
                ],
            )
            .unwrap();
    }

    fn insert_provider_launch_aggregate(
        state: &StateDb,
        launch_id: &str,
        attempt_id: &str,
        invocation_uuid: &str,
        close_attempt: bool,
    ) {
        let created =
            format_timestamp_micros(NOW - DEFAULT_TERMINAL_RETENTION_MICROS - 1_000_000).unwrap();
        let closed = format_timestamp_micros(NOW - DEFAULT_TERMINAL_RETENTION_MICROS - 1).unwrap();
        state
            .conn
            .execute(
                "INSERT INTO invocations(invocation_uuid,model_name,provider_name,provider_index,
             status,created_at) VALUES(?1,'model','provider',0,'running',?2)",
                params![invocation_uuid, created],
            )
            .unwrap();
        let invocation_id = state.conn.last_insert_rowid();
        let tx = state.conn.unchecked_transaction().unwrap();
        tx.execute(
            "INSERT INTO provider_logical_launches(logical_launch_id,request_identity_sha256,
             model_name,start_mode,candidate_plan_json,candidate_plan_sha256,status,
             current_attempt_id,owner_epoch,created_at,updated_at)
             VALUES(?1,?2,'model','create','[]',?2,'active',?3,1,?4,?4)",
            params![launch_id, "1".repeat(64), attempt_id, created],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO provider_launch_attempts(attempt_id,logical_launch_id,attempt_ordinal,
             owner_epoch,invocation_id,invocation_uuid,provider_index,account_name,
             runtime_generation_uuid,return_channel_id,status,actor_custody_state,
             return_channel_state,created_at)
             VALUES(?1,?2,0,1,?3,?4,0,'provider',?5,?6,'leased','not_started',
             'not_created',?7)",
            params![
                attempt_id,
                launch_id,
                invocation_id,
                invocation_uuid,
                format!("generation-{attempt_id}"),
                format!("return-{attempt_id}"),
                created
            ],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO provider_launch_transition_replays(logical_launch_id,operation_key,
             request_sha256,result_json,recorded_at,retention_status)
             VALUES(?1,'begin',?2,'{}',?3,'inherits_parent')",
            params![launch_id, "2".repeat(64), created],
        )
        .unwrap();
        tx.commit().unwrap();
        state
            .conn
            .execute(
                "UPDATE invocations SET status='succeeded',success=1,exit_code=0,finished_at=?2
             WHERE invocation_uuid=?1",
                params![invocation_uuid, closed],
            )
            .unwrap();
        if close_attempt {
            state
                .conn
                .execute(
                    "UPDATE provider_launch_attempts SET status='succeeded',finished_at=?2
                 WHERE attempt_id=?1",
                    params![attempt_id, closed],
                )
                .unwrap();
        }
        state.conn.execute(
            "UPDATE provider_logical_launches SET status='succeeded',finished_at=?2,updated_at=?2
             WHERE logical_launch_id=?1",
            params![launch_id, closed],
        ).unwrap();
    }

    #[test]
    fn batches_are_bounded_resumable_and_idempotent() {
        let (_directory, mut state) = fixture();
        for index in 0..3 {
            let uuid = format!("00000000-0000-4000-8000-{index:012}");
            let row = insert_terminal_invocation(&state, &uuid, -1);
            insert_closed_turn(&state, row, &uuid);
        }
        let first = state
            .run_retention_batch(&request(RetentionFamily::CompletedTurn, 2))
            .unwrap();
        assert_eq!(first.records_deleted, 2);
        assert_eq!(first.status, RetentionBatchStatus::MoreWork);
        let mut resumed = request(RetentionFamily::CompletedTurn, 2);
        resumed.cursor = first.next_cursor;
        let second = state.run_retention_batch(&resumed).unwrap();
        assert_eq!(second.records_deleted, 1);
        assert_eq!(second.status, RetentionBatchStatus::Complete);
        let replay = state
            .run_retention_batch(&request(RetentionFamily::CompletedTurn, 2))
            .unwrap();
        assert_eq!(replay.records_deleted, 0);
    }

    #[test]
    fn standalone_invocation_and_provider_aggregate_use_their_exact_roots() {
        let (_directory, mut state) = fixture();
        let standalone = "40000000-0000-4000-8000-000000000001";
        insert_terminal_invocation(&state, standalone, -1);
        let invocation_outcome = state
            .run_retention_batch(&request(RetentionFamily::Invocation, 1))
            .unwrap();
        assert_eq!(invocation_outcome.records_deleted, 1);

        insert_provider_launch_aggregate(
            &state,
            "launch-eligible",
            "attempt-eligible",
            "40000000-0000-4000-8000-000000000002",
            true,
        );
        insert_provider_launch_aggregate(
            &state,
            "launch-unresolved",
            "attempt-unresolved",
            "40000000-0000-4000-8000-000000000003",
            false,
        );
        let launch_outcome = state
            .run_retention_batch(&request(RetentionFamily::ProviderLogicalLaunch, 8))
            .unwrap();
        assert_eq!(launch_outcome.records_deleted, 1);
        let launches = state.conn.prepare(
            "SELECT logical_launch_id FROM provider_logical_launches ORDER BY logical_launch_id"
        ).unwrap().query_map([], |row| row.get::<_, String>(0)).unwrap()
            .collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(launches, vec!["launch-unresolved"]);
        assert_eq!(
            state
                .conn
                .query_row(
                    "SELECT count(*) FROM provider_launch_transition_replays
                 WHERE logical_launch_id='launch-eligible'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        let inherited = state
            .run_retention_batch(&request(RetentionFamily::ProviderLaunchAttempt, 8))
            .unwrap();
        assert_eq!(inherited.records_deleted, 0);
        assert_eq!(
            inherited
                .preservation_reasons
                .get(&PreservationReason::Inherited),
            Some(&1)
        );
    }

    #[test]
    fn concurrent_new_write_is_not_captured_by_stale_snapshot() {
        let (_directory, mut state) = fixture();
        let old_uuid = "10000000-0000-4000-8000-000000000001";
        let old_row = insert_terminal_invocation(&state, old_uuid, -1);
        insert_closed_turn(&state, old_row, old_uuid);
        let state_path = state.path().to_path_buf();
        let outcome = state
            .run_retention_batch_after_snapshot(&request(RetentionFamily::CompletedTurn, 8), || {
                let concurrent = Connection::open(&state_path).unwrap();
                concurrent
                    .pragma_update(None, "foreign_keys", true)
                    .unwrap();
                let young = format_timestamp_micros(NOW).unwrap();
                concurrent
                    .execute(
                        "INSERT INTO invocations(invocation_uuid,model_name,provider_name,
                         provider_index,status,success,exit_code,created_at,finished_at)
                         VALUES('10000000-0000-4000-8000-000000000002','model','provider',0,
                         'succeeded',1,0,?1,?1)",
                        [&young],
                    )
                    .unwrap();
            })
            .unwrap();
        assert_eq!(outcome.records_deleted, 1);
        assert_eq!(
            state
                .conn
                .query_row("SELECT count(*) FROM invocations", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }

    #[test]
    fn writer_contention_is_an_explicit_nonblocking_outcome() {
        let (_directory, mut state) = fixture();
        let uuid = "20000000-0000-4000-8000-000000000001";
        let row = insert_terminal_invocation(&state, uuid, -1);
        insert_closed_turn(&state, row, uuid);
        let blocker = Connection::open(state.path()).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        let outcome = state
            .run_retention_batch(&request(RetentionFamily::CompletedTurn, 1))
            .unwrap();
        assert_eq!(outcome.status, RetentionBatchStatus::Busy);
        assert_eq!(outcome.records_deleted, 0);
        assert_eq!(
            outcome
                .preservation_reasons
                .get(&PreservationReason::WriterBusy),
            Some(&1)
        );
        assert!(matches!(
            outcome.observation_delivery,
            Some(RetentionObservationDelivery::Gap)
                | Some(RetentionObservationDelivery::Queued)
                | Some(RetentionObservationDelivery::Appended)
                | Some(RetentionObservationDelivery::Fallback)
        ));
        blocker.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn exact_boundary_and_unknown_or_recovery_turns_are_preserved() {
        let (_directory, mut state) = fixture();
        let exact_uuid = "30000000-0000-4000-8000-000000000001";
        let exact_row = insert_terminal_invocation(&state, exact_uuid, 0);
        insert_closed_turn(&state, exact_row, exact_uuid);
        let young_uuid = "30000000-0000-4000-8000-000000000002";
        let young_row = insert_terminal_invocation(&state, young_uuid, 1);
        insert_closed_turn(&state, young_row, young_uuid);
        let recovery_uuid = "30000000-0000-4000-8000-000000000003";
        let recovery_row = insert_terminal_invocation(&state, recovery_uuid, -1);
        insert_closed_turn(&state, recovery_row, recovery_uuid);
        state
            .conn
            .execute(
                "UPDATE completed_turns SET recovery_pending=1,closed_at=NULL,
                 retention_eligible_at=NULL,retention_status='pending'
                 WHERE invocation_uuid=?1",
                [recovery_uuid],
            )
            .unwrap_err();
        // The reopen guard itself proves an established closed record cannot
        // be made recovery-pending. Insert a legacy recovery row directly as
        // pending instead.
        state
            .conn
            .execute(
                "DELETE FROM completed_turns WHERE invocation_uuid=?1",
                [recovery_uuid],
            )
            .unwrap();
        state.conn.execute(
            "INSERT INTO completed_turns(invocation_id,invocation_uuid,settlement_id,
             effects_json,context_json,content_sha256,committed_at,tails_json,
             recovery_pending,created_at,updated_at,retention_status)
             VALUES(?1,?2,'pending-settlement','{}','{}',?3,NULL,'{}',1,NULL,NULL,'legacy_unknown')",
            params![recovery_row,recovery_uuid,"0".repeat(64)],
        ).unwrap();
        let outcome = state
            .run_retention_batch(&request(RetentionFamily::CompletedTurn, 8))
            .unwrap();
        assert_eq!(outcome.records_deleted, 1);
        let remaining: i64 = state
            .conn
            .query_row("SELECT count(*) FROM completed_turns", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, 2);
    }
}
