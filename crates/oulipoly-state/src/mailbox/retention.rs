use super::{
    AGENT_BASH_COMPLETE_KIND, MailboxDb, RetiredPayload, TransactionBehavior,
    continuation_payload_retention_predicate,
};
use crate::live_history::TERMINAL_RETENTION_PRUNE;
use crate::retention::{
    PreservationReason, RetentionBatchCursor, RetentionBatchOutcome, RetentionBatchRequest,
    RetentionBatchStatus, RetentionFamily, format_timestamp_micros, parse_authoritative_timestamp,
};
use rusqlite::{ErrorCode, params};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug)]
struct MailboxCandidate {
    key: String,
    authoritative_at: String,
    payload: Option<RetiredPayload>,
}

impl MailboxDb {
    /// Execute one caller-bounded sidecar retention slice. This method owns no
    /// timer, detached worker, retry loop, singleton process, or supervisor
    /// hook; AGE-377 may resume it with the returned exact cursor.
    pub fn run_retention_batch(
        &mut self,
        request: &RetentionBatchRequest,
    ) -> Result<RetentionBatchOutcome, String> {
        self.run_retention_batch_after_snapshot(request, || {})
    }

    /// Detached maintenance reports through its exact maintenance job instead
    /// of opening the normal event-writer diagnostic sink.
    pub(crate) fn run_retention_batch_without_observation(
        &mut self,
        request: &RetentionBatchRequest,
    ) -> Result<RetentionBatchOutcome, String> {
        self.run_retention_batch_inner(request, || {}, false)
    }

    fn run_retention_batch_after_snapshot(
        &mut self,
        request: &RetentionBatchRequest,
        after_snapshot: impl FnOnce(),
    ) -> Result<RetentionBatchOutcome, String> {
        self.run_retention_batch_inner(request, after_snapshot, true)
    }

    fn run_retention_batch_inner(
        &mut self,
        request: &RetentionBatchRequest,
        after_snapshot: impl FnOnce(),
        emit_observation: bool,
    ) -> Result<RetentionBatchOutcome, String> {
        self.access_scope
            .authorize(TERMINAL_RETENTION_PRUNE, None)?;
        let cutoff = request.validate()?;
        let candidate_upper_bound = candidate_upper_bound(cutoff)?;
        let mut outcome = RetentionBatchOutcome::new(request, cutoff);
        if matches!(
            request.family,
            RetentionFamily::CompletionEventListener | RetentionFamily::RuntimeGeneration
        ) {
            // Listener/event continuity and runtime-generation rows remain
            // recovery authority. Their eligible timestamp can release owned
            // payloads, but the authority rows are intentionally retained.
            outcome.preserve(PreservationReason::RecoveryAuthoritative);
            if emit_observation {
                outcome.emit_independent_observation();
            }
            return Ok(outcome);
        }
        let mut candidates = match request.family {
            RetentionFamily::Mailbox => {
                mailbox_candidates(&self.conn, request, &candidate_upper_bound, cutoff)?
            }
            RetentionFamily::MailboxDeliveryAttempt => {
                delivery_attempt_candidates(&self.conn, request, &candidate_upper_bound, cutoff)?
            }
            RetentionFamily::CompletionEvent => {
                completion_payload_candidates(&self.conn, request, &candidate_upper_bound, cutoff)?
            }
            _ => {
                return Err(format!(
                    "retention family {} does not belong to the PID mailbox",
                    request.family.as_str()
                ));
            }
        };
        let more_work = candidates.len() > request.limit;
        candidates.truncate(request.limit);
        after_snapshot();

        if request.family == RetentionFamily::CompletionEvent {
            let previous_busy_timeout = sqlite_busy_timeout(&self.conn)?;
            self.conn.busy_timeout(Duration::ZERO).map_err(|error| {
                format!("failed to configure nonblocking payload retention: {error}")
            })?;
            for candidate in candidates {
                outcome.candidates_examined += 1;
                let result = candidate
                    .payload
                    .as_ref()
                    .ok_or_else(|| "completion payload candidate lost its payload".to_string())
                    .and_then(|payload| {
                        self.reclaim_payload_if_terminal_before(payload, Some(cutoff))
                    });
                match result {
                    Ok(_) => match payload_retirement_recorded(&self.conn, &candidate.key) {
                        Ok(true) => outcome.artifacts_retired += 1,
                        Ok(false) => outcome.preserve(PreservationReason::StaleCandidate),
                        Err(reason) => {
                            outcome.gap("payload_receipt", reason);
                        }
                    },
                    Err(reason) if sqlite_contention_reason(&reason) => {
                        outcome.preserve(PreservationReason::WriterBusy);
                        outcome.status = RetentionBatchStatus::Busy;
                        break;
                    }
                    Err(reason) => {
                        outcome.gap("completion_payload", reason);
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
            if emit_observation {
                outcome.emit_independent_observation();
            }
            return Ok(outcome);
        }

        let previous_busy_timeout = sqlite_busy_timeout(&self.conn)?;
        self.conn.busy_timeout(Duration::ZERO).map_err(|error| {
            format!("failed to configure nonblocking mailbox retention: {error}")
        })?;
        for candidate in candidates {
            outcome.candidates_examined += 1;
            let result = match request.family {
                RetentionFamily::Mailbox => {
                    delete_mailbox_candidate(&mut self.conn, &candidate, cutoff)
                }
                RetentionFamily::MailboxDeliveryAttempt => {
                    delete_attempt_candidate(&mut self.conn, &candidate, cutoff)
                }
                _ => Ok(false),
            };
            match result {
                Ok(true) => outcome.records_deleted += 1,
                Ok(false) if request.family != RetentionFamily::CompletionEvent => {
                    outcome.preserve(PreservationReason::StaleCandidate)
                }
                Ok(false) => {}
                Err(error) if sqlite_contention(&error) => {
                    outcome.preserve(PreservationReason::WriterBusy);
                    outcome.status = RetentionBatchStatus::Busy;
                    break;
                }
                Err(error) if sqlite_constraint(&error) => {
                    outcome.preserve(PreservationReason::RecoveryAuthoritative);
                }
                Err(error) => {
                    outcome.gap("mailbox_delete", typed_sqlite_reason(&error));
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
        if emit_observation {
            outcome.emit_independent_observation();
        }
        Ok(outcome)
    }
}

fn mailbox_candidates(
    conn: &rusqlite::Connection,
    request: &RetentionBatchRequest,
    candidate_upper_bound: &str,
    cutoff_micros: i64,
) -> Result<Vec<MailboxCandidate>, String> {
    let limit = sqlite_limit(request.limit)?;
    let (after_at, after_seq) = mailbox_cursor(request)?;
    let mut statement = conn
        .prepare(
            "SELECT candidate.seq,candidate.retention_eligible_at,
                    candidate.payload_file_path,candidate.payload_sha256
             FROM mailbox AS candidate
             WHERE candidate.delivered_at IS NOT NULL
               AND candidate.retention_status='eligible'
               AND candidate.retention_eligible_at IS NOT NULL
               AND candidate.retention_eligible_at<?1
               AND oulipoly_rfc3339_micros(candidate.retention_eligible_at)<=?2
               AND candidate.kind=?3
               AND (candidate.payload_file_path IS NULL OR EXISTS(
                    SELECT 1 FROM completion_event event
                    WHERE event.payload_sha256=candidate.payload_sha256))
               AND NOT EXISTS(
                    SELECT 1 FROM completion_event_listener listener
                    WHERE listener.mailbox_seq=candidate.seq
                      AND listener.acknowledged_at IS NULL)
               AND NOT EXISTS(
                    SELECT 1 FROM mailbox_delivery_attempt_items item
                    JOIN mailbox_delivery_attempts attempt
                      ON attempt.attempt_id=item.attempt_id
                    WHERE item.mailbox_seq=candidate.seq
                      AND (attempt.resolved_at IS NULL OR EXISTS(
                          SELECT 1 FROM mailbox_retained_delivery_finalizers finalizer
                          WHERE finalizer.attempt_id=attempt.attempt_id)))
               AND (?4 IS NULL OR candidate.retention_eligible_at>?4
                    OR (candidate.retention_eligible_at=?4 AND candidate.seq>?5))
             ORDER BY candidate.retention_eligible_at,candidate.seq
             LIMIT ?6",
        )
        .map_err(|error| format!("failed to prepare mailbox retention candidates: {error}"))?;
    let rows = statement
        .query_map(
            params![
                candidate_upper_bound,
                cutoff_micros,
                AGENT_BASH_COMPLETE_KIND,
                after_at,
                after_seq,
                limit
            ],
            |row| {
                let file_path = row.get::<_, Option<String>>(2)?;
                let sha256 = row.get::<_, Option<String>>(3)?;
                Ok(MailboxCandidate {
                    key: row.get::<_, i64>(0)?.to_string(),
                    authoritative_at: row.get(1)?,
                    payload: file_path
                        .zip(sha256)
                        .map(|(file_path, sha256)| RetiredPayload {
                            file_path: PathBuf::from(file_path),
                            sha256,
                        }),
                })
            },
        )
        .map_err(|error| format!("failed to query mailbox retention candidates: {error}"))?;
    collect_exact_candidates(rows, request)
}

fn delivery_attempt_candidates(
    conn: &rusqlite::Connection,
    request: &RetentionBatchRequest,
    candidate_upper_bound: &str,
    cutoff_micros: i64,
) -> Result<Vec<MailboxCandidate>, String> {
    let limit = sqlite_limit(request.limit)?;
    let (after_at, after_key) = text_cursor(request);
    let mut statement = conn
        .prepare(
            "SELECT candidate.attempt_id,candidate.retention_eligible_at
             FROM mailbox_delivery_attempts candidate
             WHERE candidate.resolved_at IS NOT NULL
               AND candidate.retention_status='eligible'
               AND candidate.retention_eligible_at IS NOT NULL
               AND candidate.retention_eligible_at<?1
               AND oulipoly_rfc3339_micros(candidate.retention_eligible_at)<=?2
               AND NOT EXISTS(
                    SELECT 1 FROM mailbox_retained_delivery_finalizers finalizer
                    WHERE finalizer.attempt_id=candidate.attempt_id)
               AND (candidate.evidence_disposition IS NULL
                    OR candidate.evidence_disposition NOT IN ('pending','legacy_pending')
                    OR candidate.evidence_reconciled_at IS NOT NULL)
               AND (?3 IS NULL OR candidate.retention_eligible_at>?3
                    OR (candidate.retention_eligible_at=?3 AND candidate.attempt_id>?4))
             ORDER BY candidate.retention_eligible_at,candidate.attempt_id
             LIMIT ?5",
        )
        .map_err(|error| format!("failed to prepare delivery retention candidates: {error}"))?;
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
                Ok(MailboxCandidate {
                    key: row.get(0)?,
                    authoritative_at: row.get(1)?,
                    payload: None,
                })
            },
        )
        .map_err(|error| format!("failed to query delivery retention candidates: {error}"))?;
    collect_exact_candidates(rows, request)
}

fn completion_payload_candidates(
    conn: &rusqlite::Connection,
    request: &RetentionBatchRequest,
    candidate_upper_bound: &str,
    cutoff_micros: i64,
) -> Result<Vec<MailboxCandidate>, String> {
    let limit = sqlite_limit(request.limit)?;
    let (after_at, after_key) = text_cursor(request);
    let continuation_predicate = continuation_payload_retention_predicate(conn)?;
    let mut statement = conn
        .prepare(&format!(
            "SELECT event.event_id,event.retention_eligible_at,
                    event.payload_file_path,event.payload_sha256
             FROM completion_event event
             WHERE event.state='triggered'
               AND event.retention_status='eligible'
               AND event.retention_eligible_at IS NOT NULL
               AND event.retention_eligible_at<?1
               AND oulipoly_rfc3339_micros(event.retention_eligible_at)<=?2
               AND event.payload_reclaimed_at IS NULL
               AND event.payload_file_path IS NOT NULL
               AND event.payload_sha256 IS NOT NULL
               {continuation_predicate}
               AND NOT EXISTS(SELECT 1 FROM mailbox
                              WHERE mailbox.payload_sha256=event.payload_sha256)
               AND NOT EXISTS(
                    SELECT 1 FROM completion_event shared_event
                    WHERE shared_event.payload_sha256=event.payload_sha256
                      AND (shared_event.retention_status!='eligible'
                           OR shared_event.retention_eligible_at IS NULL
                           OR oulipoly_rfc3339_micros(shared_event.retention_eligible_at) IS NULL
                           OR oulipoly_rfc3339_micros(shared_event.retention_eligible_at)>?2))
               AND NOT EXISTS(
                    SELECT 1 FROM completion_event representative
                    WHERE representative.payload_sha256=event.payload_sha256
                      AND representative.payload_reclaimed_at IS NULL
                      AND (representative.retention_eligible_at>event.retention_eligible_at
                           OR (representative.retention_eligible_at=event.retention_eligible_at
                               AND representative.event_id<event.event_id)))
               AND NOT EXISTS(
                    SELECT 1 FROM completion_event shared_event
                    JOIN completion_event_listener listener
                      ON listener.event_id=shared_event.event_id
                    WHERE shared_event.payload_sha256=event.payload_sha256
                      AND listener.acknowledged_at IS NULL)
               AND (?3 IS NULL OR event.retention_eligible_at>?3
                    OR (event.retention_eligible_at=?3 AND event.event_id>?4))
             ORDER BY event.retention_eligible_at,event.event_id
             LIMIT ?5"
        ))
        .map_err(|error| format!("failed to prepare completion retention candidates: {error}"))?;
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
                Ok(MailboxCandidate {
                    key: row.get(0)?,
                    authoritative_at: row.get(1)?,
                    payload: Some(RetiredPayload {
                        file_path: PathBuf::from(row.get::<_, String>(2)?),
                        sha256: row.get(3)?,
                    }),
                })
            },
        )
        .map_err(|error| format!("failed to query completion retention candidates: {error}"))?;
    collect_exact_candidates(rows, request)
}

fn collect_exact_candidates(
    rows: rusqlite::MappedRows<
        '_,
        impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<MailboxCandidate>,
    >,
    request: &RetentionBatchRequest,
) -> Result<Vec<MailboxCandidate>, String> {
    let cutoff = request.validate()?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to read mailbox retention candidate: {error}"))
        .map(|rows| {
            rows.into_iter()
                .filter(|candidate| {
                    parse_authoritative_timestamp(&candidate.authoritative_at)
                        .is_some_and(|timestamp| timestamp <= cutoff)
                })
                .collect()
        })
}

fn delete_mailbox_candidate(
    conn: &mut rusqlite::Connection,
    candidate: &MailboxCandidate,
    cutoff_micros: i64,
) -> rusqlite::Result<bool> {
    let seq = candidate.key.parse::<i64>().map_err(|_| {
        rusqlite::Error::InvalidParameterName("invalid mailbox retention cursor".to_string())
    })?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let still_eligible: bool = tx.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM mailbox candidate
             WHERE candidate.seq=?1 AND candidate.delivered_at IS NOT NULL
               AND candidate.kind=?2 AND candidate.retention_status='eligible'
               AND candidate.retention_eligible_at=?3
               AND oulipoly_rfc3339_micros(candidate.retention_eligible_at)<=?4
               AND (candidate.payload_file_path IS NULL OR EXISTS(
                    SELECT 1 FROM completion_event event
                    WHERE event.payload_sha256=candidate.payload_sha256))
               AND NOT EXISTS(SELECT 1 FROM completion_event_listener listener
                              WHERE listener.mailbox_seq=candidate.seq
                                AND listener.acknowledged_at IS NULL)
               AND NOT EXISTS(
                    SELECT 1 FROM mailbox_delivery_attempt_items item
                    JOIN mailbox_delivery_attempts attempt
                      ON attempt.attempt_id=item.attempt_id
                    WHERE item.mailbox_seq=candidate.seq
                      AND (attempt.resolved_at IS NULL OR EXISTS(
                          SELECT 1 FROM mailbox_retained_delivery_finalizers finalizer
                          WHERE finalizer.attempt_id=attempt.attempt_id))))",
        params![
            seq,
            AGENT_BASH_COMPLETE_KIND,
            candidate.authoritative_at,
            cutoff_micros
        ],
        |row| row.get(0),
    )?;
    if !still_eligible {
        tx.commit()?;
        return Ok(false);
    }
    tx.execute(
        "UPDATE completion_event_listener SET mailbox_seq=NULL
         WHERE mailbox_seq=?1 AND acknowledged_at IS NOT NULL",
        [seq],
    )?;
    tx.execute(
        "DELETE FROM mailbox_delivery_attempt_items WHERE mailbox_seq=?1",
        [seq],
    )?;
    let changed = tx.execute(
        "DELETE FROM mailbox WHERE seq=?1 AND delivered_at IS NOT NULL
         AND retention_status='eligible' AND retention_eligible_at=?2",
        params![seq, candidate.authoritative_at],
    )?;
    tx.commit()?;
    Ok(changed == 1)
}

fn delete_attempt_candidate(
    conn: &mut rusqlite::Connection,
    candidate: &MailboxCandidate,
    cutoff_micros: i64,
) -> rusqlite::Result<bool> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let still_eligible: bool = tx.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM mailbox_delivery_attempts candidate
             WHERE candidate.attempt_id=?1 AND candidate.resolved_at IS NOT NULL
               AND candidate.retention_status='eligible'
               AND candidate.retention_eligible_at=?2
               AND oulipoly_rfc3339_micros(candidate.retention_eligible_at)<=?3
               AND NOT EXISTS(
                    SELECT 1 FROM mailbox_retained_delivery_finalizers finalizer
                    WHERE finalizer.attempt_id=candidate.attempt_id)
               AND (candidate.evidence_disposition IS NULL
                    OR candidate.evidence_disposition NOT IN ('pending','legacy_pending')
                    OR candidate.evidence_reconciled_at IS NOT NULL))",
        params![candidate.key, candidate.authoritative_at, cutoff_micros],
        |row| row.get(0),
    )?;
    if !still_eligible {
        tx.commit()?;
        return Ok(false);
    }
    tx.execute(
        "DELETE FROM mailbox_delivery_attempt_items WHERE attempt_id=?1",
        [&candidate.key],
    )?;
    let changed = tx.execute(
        "DELETE FROM mailbox_delivery_attempts
         WHERE attempt_id=?1 AND resolved_at IS NOT NULL
           AND retention_status='eligible' AND retention_eligible_at=?2",
        params![candidate.key, candidate.authoritative_at],
    )?;
    tx.commit()?;
    Ok(changed == 1)
}

fn candidate_upper_bound(cutoff_micros: i64) -> Result<String, String> {
    let next_second = cutoff_micros
        .div_euclid(1_000_000)
        .checked_add(1)
        .and_then(|seconds| seconds.checked_mul(1_000_000))
        .ok_or_else(|| "retention cutoff cannot be rounded for indexed selection".to_string())?;
    format_timestamp_micros(next_second)
}

fn payload_retirement_recorded(
    conn: &rusqlite::Connection,
    event_id: &str,
) -> Result<bool, String> {
    conn.query_row(
        "SELECT payload_reclaimed_at IS NOT NULL FROM completion_event WHERE event_id=?1",
        [event_id],
        |row| row.get(0),
    )
    .map_err(|error| format!("failed to confirm payload retirement receipt: {error}"))
}

fn sqlite_contention_reason(reason: &str) -> bool {
    reason.contains("database is locked") || reason.contains("database is busy")
}

fn sqlite_limit(limit: usize) -> Result<i64, String> {
    i64::try_from(limit.saturating_add(1))
        .map_err(|_| "retention batch limit does not fit SQLite INTEGER".to_string())
}

fn text_cursor(request: &RetentionBatchRequest) -> (Option<&str>, &str) {
    request
        .cursor
        .as_ref()
        .map(|cursor| {
            (
                Some(cursor.after_authoritative_at.as_str()),
                cursor.after_key.as_str(),
            )
        })
        .unwrap_or((None, ""))
}

fn mailbox_cursor(request: &RetentionBatchRequest) -> Result<(Option<&str>, i64), String> {
    request
        .cursor
        .as_ref()
        .map(|cursor| {
            cursor
                .after_key
                .parse::<i64>()
                .map(|seq| (Some(cursor.after_authoritative_at.as_str()), seq))
                .map_err(|_| "mailbox retention cursor key is not an integer".to_string())
        })
        .unwrap_or(Ok((None, 0)))
}

fn sqlite_busy_timeout(conn: &rusqlite::Connection) -> Result<u64, String> {
    let value: i64 = conn
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .map_err(|error| format!("failed to read mailbox retention busy timeout: {error}"))?;
    u64::try_from(value).map_err(|_| "mailbox retention busy timeout is negative".to_string())
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
    use crate::retention::{DEFAULT_TERMINAL_RETENTION_MICROS, RetentionPolicy};
    use rusqlite::Connection;
    use tempfile::TempDir;

    const NOW: i64 = 4_000_000_000_000_000;

    fn fixture() -> (TempDir, MailboxDb) {
        let directory = tempfile::tempdir().unwrap();
        let db = MailboxDb::open_historical(&directory.path().join("pid-identity.db")).unwrap();
        (directory, db)
    }

    fn request(family: RetentionFamily, limit: usize) -> RetentionBatchRequest {
        RetentionBatchRequest {
            policy: RetentionPolicy::default(),
            family,
            as_of_unix_micros: NOW,
            limit,
            cursor: None,
        }
    }

    fn insert_mailbox(db: &MailboxDb, handle: &str, offset: i64, delivered: bool) -> i64 {
        let closed =
            format_timestamp_micros(NOW - DEFAULT_TERMINAL_RETENTION_MICROS + offset).unwrap();
        let enqueued =
            format_timestamp_micros(NOW - DEFAULT_TERMINAL_RETENTION_MICROS - 1_000_000).unwrap();
        db.conn
            .execute(
                "INSERT INTO mailbox(session_id,kind,handle,payload_json,enqueued_at,
                 delivered_at,state_dir,meta_path,log_path,rc_path,rc)
                 VALUES('session',?1,?2,'{}',?3,?4,'/state','/meta','/log','/rc',0)",
                params![
                    AGENT_BASH_COMPLETE_KIND,
                    handle,
                    enqueued,
                    delivered.then_some(closed)
                ],
            )
            .unwrap();
        db.conn.last_insert_rowid()
    }

    #[test]
    fn mailbox_boundary_batches_resume_and_preserve_pending() {
        let (_directory, mut db) = fixture();
        insert_mailbox(&db, "older", -1, true);
        insert_mailbox(&db, "equal", 0, true);
        insert_mailbox(&db, "younger", 1, true);
        insert_mailbox(&db, "pending", -1, false);
        let first = db
            .run_retention_batch(&request(RetentionFamily::Mailbox, 1))
            .unwrap();
        assert_eq!(first.records_deleted, 1);
        assert_eq!(first.status, RetentionBatchStatus::MoreWork);
        let mut resume = request(RetentionFamily::Mailbox, 1);
        resume.cursor = first.next_cursor;
        let second = db.run_retention_batch(&resume).unwrap();
        assert_eq!(second.records_deleted, 1);
        let handles = db
            .conn
            .prepare("SELECT handle FROM mailbox ORDER BY handle")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(handles, vec!["pending", "younger"]);
        let replay = db
            .run_retention_batch(&request(RetentionFamily::Mailbox, 8))
            .unwrap();
        assert_eq!(replay.records_deleted, 0);
    }

    #[test]
    fn delivery_attempt_requires_resolution_and_reconciled_evidence() {
        let (_directory, mut db) = fixture();
        let old = format_timestamp_micros(NOW - DEFAULT_TERMINAL_RETENTION_MICROS - 1).unwrap();
        db.conn
            .execute_batch(&format!(
                "INSERT INTO mailbox_delivery_attempts(attempt_id,session_id,
             delivery_invocation_uuid,created_at,prepared_remaining_count)
             VALUES('eligible','session','invocation','{old}',0),
                   ('unresolved','session','invocation','{old}',0),
                   ('uncertain','session','invocation','{old}',0);
             UPDATE mailbox_delivery_attempts SET resolved_at=created_at
             WHERE attempt_id IN ('eligible','uncertain');
             UPDATE mailbox_delivery_attempts SET evidence_disposition='pending'
             WHERE attempt_id='uncertain';"
            ))
            .unwrap();
        let outcome = db
            .run_retention_batch(&request(RetentionFamily::MailboxDeliveryAttempt, 8))
            .unwrap();
        assert_eq!(outcome.records_deleted, 1);
        let remaining = db
            .conn
            .prepare("SELECT attempt_id FROM mailbox_delivery_attempts ORDER BY attempt_id")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(remaining, vec!["uncertain", "unresolved"]);
    }

    #[test]
    fn stale_snapshot_does_not_capture_a_concurrent_new_mailbox_row() {
        let (_directory, mut db) = fixture();
        insert_mailbox(&db, "old", -1, true);
        let path = db.path().to_path_buf();
        let outcome = db
            .run_retention_batch_after_snapshot(&request(RetentionFamily::Mailbox, 8), || {
                let concurrent = Connection::open(&path).unwrap();
                let now = format_timestamp_micros(NOW).unwrap();
                concurrent
                    .execute(
                        "INSERT INTO mailbox(session_id,kind,handle,payload_json,enqueued_at,
                     delivered_at,state_dir,meta_path,log_path,rc_path,rc)
                     VALUES('session',?1,'concurrent','{}',?2,?2,
                     '/state','/meta','/log','/rc',0)",
                        params![AGENT_BASH_COMPLETE_KIND, now],
                    )
                    .unwrap();
            })
            .unwrap();
        assert_eq!(outcome.records_deleted, 1);
        assert_eq!(
            db.conn
                .query_row("SELECT handle FROM mailbox", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "concurrent"
        );
    }

    #[test]
    fn mailbox_writer_contention_is_explicit_and_nonblocking() {
        let (_directory, mut db) = fixture();
        insert_mailbox(&db, "old", -1, true);
        let blocker = Connection::open(db.path()).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        let outcome = db
            .run_retention_batch(&request(RetentionFamily::Mailbox, 1))
            .unwrap();
        assert_eq!(outcome.status, RetentionBatchStatus::Busy);
        assert_eq!(outcome.records_deleted, 0);
        assert_eq!(
            outcome
                .preservation_reasons
                .get(&PreservationReason::WriterBusy),
            Some(&1)
        );
        blocker.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn authority_only_sidecar_families_are_explicitly_preserved() {
        let (_directory, mut db) = fixture();
        for family in [
            RetentionFamily::CompletionEventListener,
            RetentionFamily::RuntimeGeneration,
        ] {
            let outcome = db.run_retention_batch(&request(family, 1)).unwrap();
            assert_eq!(outcome.records_deleted, 0);
            assert_eq!(
                outcome
                    .preservation_reasons
                    .get(&PreservationReason::RecoveryAuthoritative),
                Some(&1)
            );
        }
    }

    #[test]
    fn completion_payload_retirement_is_age_gated_and_idempotent() {
        let (_directory, mut db) = fixture();
        let published = db
            .payloads()
            .publish_immutable_payload(b"{\"value\":1}")
            .unwrap();
        let old = format_timestamp_micros(NOW - DEFAULT_TERMINAL_RETENTION_MICROS - 1).unwrap();
        db.conn
            .execute(
                "INSERT INTO completion_event(event_id,kind,state,delivery_mode,state_dir,
             meta_path,log_path,rc_path,rc,payload_json,payload_file_path,payload_sha256,
             payload_byte_len,payload_retention_policy,created_at,triggered_at,
             updated_at,closed_at,retention_eligible_at,retention_status)
             VALUES('old-event',?1,'triggered','async','/state','/meta','/log','/rc',0,
             '{}',?2,?3,?4,?5,?6,?6,?6,?6,?6,'eligible')",
                params![
                    AGENT_BASH_COMPLETE_KIND,
                    published.file_path.to_string_lossy(),
                    published.sha256,
                    published.byte_len as i64,
                    published.retention_policy,
                    old
                ],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO completion_event(event_id,kind,state,delivery_mode,state_dir,
             meta_path,log_path,rc_path,rc,payload_json,payload_file_path,payload_sha256,
             payload_byte_len,payload_retention_policy,created_at,triggered_at,
             updated_at,closed_at,retention_eligible_at,retention_status)
             VALUES('old-event-shared',?1,'triggered','async','/state','/meta','/log','/rc',0,
             '{}',?2,?3,?4,?5,?6,?6,?6,?6,?6,'eligible')",
                params![
                    AGENT_BASH_COMPLETE_KIND,
                    published.file_path.to_string_lossy(),
                    published.sha256,
                    published.byte_len as i64,
                    published.retention_policy,
                    old
                ],
            )
            .unwrap();
        let outcome = db
            .run_retention_batch(&request(RetentionFamily::CompletionEvent, 1))
            .unwrap();
        assert_eq!(outcome.artifacts_retired, 1);
        assert_eq!(outcome.candidates_examined, 1);
        assert!(!published.file_path.exists());
        let marked: i64 = db
            .conn
            .query_row(
                "SELECT count(*) FROM completion_event WHERE payload_reclaimed_at IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(marked, 2);
        let replay = db
            .run_retention_batch(&request(RetentionFamily::CompletionEvent, 1))
            .unwrap();
        assert_eq!(replay.artifacts_retired, 0);
    }

    #[test]
    fn stale_payload_snapshot_preserves_a_concurrent_new_reference() {
        let (_directory, mut db) = fixture();
        let published = db
            .payloads()
            .publish_immutable_payload(b"{\"value\":2}")
            .unwrap();
        let old = format_timestamp_micros(NOW - DEFAULT_TERMINAL_RETENTION_MICROS - 1).unwrap();
        db.conn
            .execute(
                "INSERT INTO completion_event(event_id,kind,state,delivery_mode,state_dir,
             meta_path,log_path,rc_path,rc,payload_json,payload_file_path,payload_sha256,
             payload_byte_len,payload_retention_policy,created_at,triggered_at,
             updated_at,closed_at,retention_eligible_at,retention_status)
             VALUES('old-reference',?1,'triggered','async','/state','/meta','/log','/rc',0,
             '{}',?2,?3,?4,?5,?6,?6,?6,?6,?6,'eligible')",
                params![
                    AGENT_BASH_COMPLETE_KIND,
                    published.file_path.to_string_lossy(),
                    published.sha256,
                    published.byte_len as i64,
                    published.retention_policy,
                    old
                ],
            )
            .unwrap();
        let path = db.path().to_path_buf();
        let payload_path = published.file_path.clone();
        let payload_sha256 = published.sha256.clone();
        let payload_policy = published.retention_policy.clone();
        let payload_bytes = published.byte_len as i64;
        let young = format_timestamp_micros(NOW).unwrap();
        let outcome = db
            .run_retention_batch_after_snapshot(
                &request(RetentionFamily::CompletionEvent, 1),
                || {
                    let concurrent = Connection::open(path).unwrap();
                    concurrent.execute(
                        "INSERT INTO completion_event(event_id,kind,state,delivery_mode,state_dir,
                         meta_path,log_path,rc_path,rc,payload_json,payload_file_path,payload_sha256,
                         payload_byte_len,payload_retention_policy,created_at,triggered_at,
                         updated_at,closed_at,retention_eligible_at,retention_status)
                         VALUES('new-reference',?1,'triggered','async','/state','/meta','/log','/rc',0,
                         '{}',?2,?3,?4,?5,?6,?6,?6,?6,?6,'eligible')",
                        params![
                            AGENT_BASH_COMPLETE_KIND,
                            payload_path.to_string_lossy(),
                            payload_sha256,
                            payload_bytes,
                            payload_policy,
                            young
                        ],
                    ).unwrap();
                },
            )
            .unwrap();
        assert_eq!(outcome.artifacts_retired, 0);
        assert_eq!(
            outcome
                .preservation_reasons
                .get(&PreservationReason::StaleCandidate),
            Some(&1)
        );
        assert!(published.file_path.exists());
        let reclaimed: Option<String> = db
            .conn
            .query_row(
                "SELECT payload_reclaimed_at FROM completion_event WHERE event_id='old-reference'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(reclaimed.is_none());
    }
}
