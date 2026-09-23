use super::*;
use crate::diagnostic_recorder::{
    OutcomeCertainty, SqliteAccessClass, SqliteMeasurementGap, SqlitePhaseEvidence,
    SqliteQueryPlanEvidence, SqliteTransactionPhase,
};
use crate::sqlite_observability::{
    SqliteOperationObserver, capture_query_plan, process_query_plan_node_limit,
    process_query_plans_enabled,
};

impl MailboxDb {
    pub fn reserve_continuation_attempt(
        &mut self,
        request: &ContinuationAttempt,
    ) -> Result<(), String> {
        if request.operation == "activation" {
            if let Some(session) = request.session_id.as_deref() {
                classify_one_pending_completion(
                    &mut self.conn,
                    session,
                    request.claim_token.as_deref(),
                )?;
            }
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        reserve_on(&tx, request)?;
        tx.commit().map_err(|e| e.to_string())
    }

    /// The same live driver can revoke an unaccepted reservation after request
    /// I/O failure. Accepted gates and predecessor obligations are never freed.
    pub fn revoke_unaccepted_continuation_attempt(
        &mut self,
        attempt: &ContinuationAttempt,
    ) -> Result<(), String> {
        if self.try_revoke_unaccepted_continuation_attempt(attempt)? {
            Ok(())
        } else {
            Err("reservation no longer unaccepted under this driver".into())
        }
    }

    /// Retry one exact proposal withdrawal without scanning the unresolved
    /// ledger. `false` means the root already accepted it (or this driver no
    /// longer owns the proposal), so the caller must stop retrying and leave
    /// reconciliation to the root authority. Storage errors remain retryable by
    /// the original live driver and never authorize a second launch.
    pub fn try_revoke_unaccepted_continuation_attempt(
        &mut self,
        attempt: &ContinuationAttempt,
    ) -> Result<bool, String> {
        require_exact_attempt(&self.conn, attempt)?;
        let live = crate::pid_identity::read_live_process_identity(i64::from(std::process::id()))?
            .ok_or("driver process disappeared")?;
        let identity = SourceProcessIdentity {
            pid: live.os_pid,
            boot_id: live.os_boot_id,
            starttime_ticks: live.os_pid_starttime_ticks,
        };
        let encoded = serde_json::to_string(&identity).map_err(|e| e.to_string())?;
        // Reject a known accepted/root-owned attempt without requesting a
        // SQLite writer. The exact UPDATE below rechecks every predicate after
        // acquisition, so this read is only a contention-avoiding hint.
        let eligible: bool = self
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1
                 FROM completion_continuation_attempt attempt
                 JOIN completion_continuation_owner owner
                   ON owner.generation=attempt.owner_generation
                 WHERE attempt.attempt_id=?1 AND attempt.owner_generation=?2
                   AND attempt.phase='reserved' AND attempt.revision=1
                   AND attempt.custodian_identity IS NULL
                   AND owner.phase='running' AND owner.driver_identity=?3)",
                params![attempt.attempt_id, attempt.owner_generation, encoded],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !eligible {
            return Ok(false);
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let changed = tx.execute(
            "UPDATE completion_continuation_attempt SET phase='never_started',revision=revision+1,integrated=1,drain_receipt=?3 WHERE attempt_id=?1 AND owner_generation=?2 AND phase='reserved' AND revision=1 AND custodian_identity IS NULL AND EXISTS(SELECT 1 FROM completion_continuation_owner WHERE generation=?2 AND phase='running' AND driver_identity=?4)",
            params![attempt.attempt_id, attempt.owner_generation,
                serde_json::json!({"attempt_id":attempt.attempt_id,"driver":identity,"gate":"unaccepted_reservation_revoked"}).to_string(), encoded],
        ).map_err(|e| e.to_string())?;
        if changed == 0 {
            tx.commit().map_err(|e| e.to_string())?;
            return Ok(false);
        }
        if attempt.operation == "activation" {
            tx.execute(
                "DELETE FROM session_wake_claim WHERE session_id=?1 AND claim_token=?2",
                params![attempt.session_id, attempt.claim_token],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(true)
    }

    pub fn advance_continuation_attempt(
        &mut self,
        attempt: &ContinuationAttempt,
        revision: i64,
        from: &str,
        to: &str,
        custodian: &SourceProcessIdentity,
    ) -> Result<i64, String> {
        require_exact_attempt(&self.conn, attempt)?;
        if !matches!((from, to), ("accepted", "starting")) {
            return Err("invalid continuation attempt transition".into());
        }
        let custodian = serde_json::to_string(custodian).map_err(|e| e.to_string())?;
        let observer = SqliteOperationObserver::process();
        let changed = match self.conn.execute("UPDATE completion_continuation_attempt SET phase=?4,revision=revision+1,custodian_identity=?5 WHERE attempt_id=?1 AND revision=?2 AND phase=?3 AND custodian_identity=?5 AND owner_generation=?6 AND EXISTS(SELECT 1 FROM completion_continuation_owner WHERE generation=?6 AND phase='running')", params![attempt.attempt_id,revision,from,to,custodian,attempt.owner_generation]) {
            Ok(changed) => {
                let _ = observer.record_success(
                    || {
                        continuation_write_span(
                            "root_supervisor_advance_attempt",
                            "completion_continuation.advance",
                            attempt,
                        )
                    },
                    DiagnosticPhase::Committed,
                    OutcomeCertainty::Committed,
                    |elapsed| autocommit_write_evidence(elapsed, Some(changed)),
                );
                changed
            }
            Err(error) => {
                let _ = observer.record_failure(
                    || {
                        continuation_write_span(
                            "root_supervisor_advance_attempt",
                            "completion_continuation.advance",
                            attempt,
                        )
                    },
                    &error,
                    OutcomeCertainty::StartedUnknown,
                    |elapsed| autocommit_write_evidence(elapsed, None),
                );
                return Err(error.to_string());
            }
        };
        if changed != 1 {
            return Err("stale continuation attempt owner/revision".into());
        }
        Ok(revision + 1)
    }

    /// Exact original custodian integration after its actual ECHILD observation.
    /// This operation intentionally has no listener ACK argument.
    pub fn discharge_continuation_attempt(
        &mut self,
        attempt: &ContinuationAttempt,
        custodian: &SourceProcessIdentity,
        receipt: &str,
    ) -> Result<(), String> {
        require_exact_attempt(&self.conn, attempt)?;
        if receipt.is_empty() {
            return Err("missing physical drain receipt".into());
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let changed = tx.execute("UPDATE completion_continuation_attempt SET phase='drained',revision=revision+1,integrated=1,drain_receipt=?3 WHERE attempt_id=?1 AND custodian_identity=?2 AND phase IN ('starting','running','unknown_custody')", params![attempt.attempt_id,serde_json::to_string(custodian).map_err(|e| e.to_string())?,receipt]).map_err(|e| e.to_string())?;
        if changed != 1 {
            let retained:Option<String>=tx.query_row("SELECT drain_receipt FROM completion_continuation_attempt WHERE attempt_id=?1 AND custodian_identity=?2 AND phase='drained'",params![attempt.attempt_id,serde_json::to_string(custodian).map_err(|e|e.to_string())?],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
            if retained.as_deref() != Some(receipt) {
                return Err("completion custodian mismatch or not started".into());
            }
        }
        if attempt.operation == "activation" {
            tx.execute(
                "DELETE FROM session_wake_claim WHERE session_id=?1 AND claim_token=?2",
                params![attempt.session_id, attempt.claim_token],
            )
            .map_err(|e| e.to_string())?;
        }
        #[cfg(feature = "age360-fault-fixtures")]
        if attempt.operation == "activation" && changed == 1 {
            crate::completion_continuation::age360_fault_barrier("original-drain-before-commit");
        }
        let result = tx.commit().map_err(|e| e.to_string());
        #[cfg(feature = "age360-fault-fixtures")]
        if attempt.operation == "activation" && changed == 1 {
            crate::completion_continuation::age360_fault_barrier(if result.is_ok() {
                "original-drain-commit-returned-ok"
            } else {
                "original-drain-commit-returned-error"
            });
        }
        result
    }

    pub fn pending_continuation_attempt_ids(
        &self,
        registration_id: &str,
    ) -> Result<Vec<String>, String> {
        let mut statement = self.conn.prepare("SELECT a.attempt_id FROM completion_continuation_attempt a WHERE a.phase NOT IN ('drained','never_started') AND (EXISTS (SELECT 1 FROM completion_continuation_attempt_source link WHERE link.attempt_id=a.attempt_id AND link.registration_id=?1) OR (a.operation!='activation' AND a.source_registration_id=?1) OR (a.association_completeness='unknown' AND a.source_registration_id=?1)) ORDER BY a.attempt_id").map_err(|e| e.to_string())?;
        statement
            .query_map([registration_id], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .map(|r| r.map_err(|e| e.to_string()))
            .collect()
    }

    /// Whether the per-source list can be read as exhaustive. A pre-v25
    /// activation in any session used by this source may have included it,
    /// even when its original claim was already deleted. This is a read-only
    /// historical check; schema/open never scans or hashes that history.
    pub fn continuation_attempt_association_completeness(
        &self,
        registration_id: &str,
    ) -> Result<&'static str, String> {
        let uncertain: bool = self
            .conn
            .query_row(
                "SELECT EXISTS(
                SELECT 1 FROM completion_continuation_source source
                JOIN completion_continuation_attempt attempt
                  ON attempt.domain_id=source.domain_id
                WHERE source.registration_id=?1 AND attempt.operation='activation'
                  AND attempt.association_completeness='unknown'
                  AND (attempt.source_registration_id=source.registration_id
                    OR EXISTS (SELECT 1 FROM completion_event_listener listener
                               WHERE listener.event_id=source.event_id
                                 AND listener.session_id=attempt.session_id)
                    OR NOT EXISTS (SELECT 1 FROM completion_event_listener listener
                                   WHERE listener.event_id=source.event_id)))",
                [registration_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        Ok(if uncertain { "unknown" } else { "known" })
    }

    /// Bounded unresolved custody for the current root and its explicitly
    /// inherited predecessors. Terminal history stays out of this working set.
    pub fn pending_continuation_attempts_for_supervisor(
        &self,
        supervisor_authority_id: &str,
        limit: usize,
    ) -> Result<Vec<ContinuationAttempt>, String> {
        let limit = i64::try_from(limit).map_err(|_| "continuation attempt limit overflow")?;
        self.access_scope
            .authorize(SUPERVISOR_PENDING_ATTEMPTS, usize::try_from(limit).ok())?;
        let mut statement = self
            .conn
            .prepare(
                "WITH RECURSIVE supervisor_scope(authority_id) AS (
                    SELECT ?1
                    UNION
                    SELECT inheritance.predecessor_authority_id
                    FROM completion_supervisor_inheritance inheritance
                    JOIN supervisor_scope scope
                      ON inheritance.authority_id=scope.authority_id)
                 SELECT attempt_id,owner_generation,operation,request_sha256,
                    source_registration_id,source_listener_revision,session_id,
                    claim_token,result_path
             FROM completion_continuation_attempt
             WHERE supervisor_authority_id IN (
                    SELECT authority_id FROM supervisor_scope)
               AND phase NOT IN ('drained','never_started')
             ORDER BY owner_generation,attempt_id LIMIT ?2",
            )
            .map_err(|error| error.to_string())?;
        statement
            .query_map(params![supervisor_authority_id, limit], |row| {
                Ok(ContinuationAttempt {
                    attempt_id: row.get(0)?,
                    owner_generation: row.get(1)?,
                    operation: row.get(2)?,
                    request_sha256: row.get(3)?,
                    source_registration_id: row.get(4)?,
                    source_listener_revision: row.get(5)?,
                    session_id: row.get(6)?,
                    claim_token: row.get(7)?,
                    result_path: row.get(8)?,
                })
            })
            .map_err(|error| error.to_string())?
            .map(|row| row.map_err(|error| error.to_string()))
            .collect()
    }

    /// Continue a bounded unresolved walk after an exact lexical cursor. The
    /// caller wraps to the beginning after a short page so retained unknowns at
    /// the front cannot permanently starve later receipts or reservations.
    pub fn pending_continuation_attempts_for_supervisor_after(
        &self,
        supervisor_authority_id: &str,
        after: Option<(&str, &str)>,
        limit: usize,
    ) -> Result<Vec<ContinuationAttempt>, String> {
        const QUERY: &str = "WITH RECURSIVE supervisor_scope(authority_id) AS (
                    SELECT ?1
                    UNION
                    SELECT inheritance.predecessor_authority_id
                    FROM completion_supervisor_inheritance inheritance
                    JOIN supervisor_scope scope
                      ON inheritance.authority_id=scope.authority_id)
                 SELECT attempt_id,owner_generation,operation,request_sha256,
                    source_registration_id,source_listener_revision,session_id,
                    claim_token,result_path
             FROM completion_continuation_attempt
             WHERE supervisor_authority_id IN (
                    SELECT authority_id FROM supervisor_scope)
               AND phase NOT IN ('drained','never_started')
               AND (?2 IS NULL OR owner_generation>?2
                    OR (owner_generation=?2 AND attempt_id>?3))
             ORDER BY owner_generation,attempt_id LIMIT ?4";
        let limit = i64::try_from(limit).map_err(|_| "continuation attempt limit overflow")?;
        self.access_scope
            .authorize(SUPERVISOR_PENDING_ATTEMPTS, usize::try_from(limit).ok())?;
        let (after_generation, after_attempt) = after
            .map(|(generation, attempt)| (Some(generation), Some(attempt)))
            .unwrap_or((None, None));
        let query_plans_enabled = process_query_plans_enabled();
        let query_plan_node_limit = process_query_plan_node_limit();
        let observer = SqliteOperationObserver::process();
        let execution_started = Instant::now();
        let result = (|| -> rusqlite::Result<Vec<ContinuationAttempt>> {
            let mut statement = self.conn.prepare(QUERY)?;
            statement
                .query_map(
                    params![
                        supervisor_authority_id,
                        after_generation,
                        after_attempt,
                        limit
                    ],
                    |row| {
                        Ok(ContinuationAttempt {
                            attempt_id: row.get(0)?,
                            owner_generation: row.get(1)?,
                            operation: row.get(2)?,
                            request_sha256: row.get(3)?,
                            source_registration_id: row.get(4)?,
                            source_listener_revision: row.get(5)?,
                            session_id: row.get(6)?,
                            claim_token: row.get(7)?,
                            result_path: row.get(8)?,
                        })
                    },
                )?
                .collect()
        })();
        let execution = execution_started.elapsed();
        match result {
            Ok(attempts) => {
                let rows_returned = u64::try_from(attempts.len()).unwrap_or(u64::MAX);
                let _ = observer.record_success(
                    || pending_supervisor_attempts_span(supervisor_authority_id),
                    DiagnosticPhase::Released,
                    OutcomeCertainty::Terminal,
                    |_| {
                        let plan = capture_query_plan(
                            &self.conn,
                            QUERY,
                            params![
                                supervisor_authority_id,
                                after_generation,
                                after_attempt,
                                limit
                            ],
                            query_plans_enabled,
                            query_plan_node_limit,
                        );
                        read_query_evidence(execution, plan).with_rows_returned(rows_returned)
                    },
                );
                Ok(attempts)
            }
            Err(error) => {
                let _ = observer.record_failure(
                    || pending_supervisor_attempts_span(supervisor_authority_id),
                    &error,
                    OutcomeCertainty::StartedUnknown,
                    |_| {
                        let plan = capture_query_plan(
                            &self.conn,
                            QUERY,
                            params![
                                supervisor_authority_id,
                                after_generation,
                                after_attempt,
                                limit
                            ],
                            query_plans_enabled,
                            query_plan_node_limit,
                        );
                        read_query_evidence(execution, plan)
                    },
                );
                Err(error.to_string())
            }
        }
    }
}

fn read_query_evidence(
    execution: StdDuration,
    plan: SqliteQueryPlanEvidence,
) -> SqlitePhaseEvidence {
    SqlitePhaseEvidence::for_phase(SqliteTransactionPhase::StatementExecution)
        .with_execution(execution)
        .with_gap(SqliteMeasurementGap::WriterAuthorityNotApplicable)
        .with_gap(SqliteMeasurementGap::CommitNotApplicable)
        .with_gap(SqliteMeasurementGap::PostCommitNotApplicable)
        .with_gap(SqliteMeasurementGap::RowsChangedNotApplicable)
        .with_gap(SqliteMeasurementGap::RowsExaminedNotExposed)
        .with_query_plan(plan)
}

fn autocommit_write_evidence(
    statement_total: StdDuration,
    rows_changed: Option<usize>,
) -> SqlitePhaseEvidence {
    let mut evidence = SqlitePhaseEvidence::for_phase(SqliteTransactionPhase::StatementExecution)
        .with_statement_total(statement_total)
        .with_gap(SqliteMeasurementGap::WriterWaitAndExecutionNotSeparable)
        .with_gap(SqliteMeasurementGap::CommitNotExposedByApi)
        .with_gap(SqliteMeasurementGap::RowsExaminedNotExposed)
        .with_gap(SqliteMeasurementGap::PostCommitOutsideBoundary);
    if let Some(rows_changed) = rows_changed {
        evidence = evidence.with_rows_changed(u64::try_from(rows_changed).unwrap_or(u64::MAX));
    } else {
        evidence = evidence.with_gap(SqliteMeasurementGap::RowsChangedNotReported);
    }
    evidence
}

fn pending_supervisor_attempts_span(supervisor_authority_id: &str) -> SpanStart {
    SpanStart::new("root_supervisor_pending_attempts", "pid_mailbox_sqlite")
        .with_lifecycle_phase("completion_recovery_authority")
        .with_sqlite_identity(
            SqliteEventIdentity::new(
                SqliteDatabaseRole::PidMailbox,
                SqlitePathClass::ManagedFile,
                "completion_continuation.pending_for_supervisor",
            )
            .with_access_class(SqliteAccessClass::BoundedCrossBoundary)
            .with_transaction_mode(SqliteTransactionMode::ReadOnly),
        )
        .with_hashed_correlation("supervisor_authority_id", supervisor_authority_id)
}

fn continuation_write_span(
    operation: &'static str,
    query_family: &'static str,
    attempt: &ContinuationAttempt,
) -> SpanStart {
    SpanStart::new(operation, "pid_mailbox_sqlite")
        .with_lifecycle_phase("completion_launch_authority")
        .with_sqlite_identity(
            SqliteEventIdentity::new(
                SqliteDatabaseRole::PidMailbox,
                SqlitePathClass::ManagedFile,
                query_family,
            )
            .with_transaction_mode(SqliteTransactionMode::Autocommit),
        )
        .with_hashed_correlation("attempt_id", &attempt.attempt_id)
        .with_hashed_correlation("owner_generation", &attempt.owner_generation)
}

fn reserve_on(tx: &Transaction<'_>, request: &ContinuationAttempt) -> Result<(), String> {
    let (domain, supervisor_authority_id): (String, String) = tx.query_row("SELECT domain_id,supervisor_authority_id FROM completion_continuation_owner WHERE generation=?1 AND phase='running'", [&request.owner_generation], |r| Ok((r.get(0)?,r.get(1)?))).map_err(|e| e.to_string())?;
    if !crate::completion_continuation::is_sha256(&request.request_sha256) {
        return Err("invalid continuation request digest".into());
    }
    if request.operation == "source_recovery" {
        let registration = request
            .source_registration_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or("source recovery requires registration")?;
        let blocked: bool = tx
            .query_row(
                "WITH RECURSIVE supervisor_scope(authority_id) AS (
                SELECT ?1 UNION SELECT inheritance.predecessor_authority_id
                FROM completion_supervisor_inheritance inheritance
                JOIN supervisor_scope scope ON inheritance.authority_id=scope.authority_id)
             SELECT (SELECT COUNT(*) FROM completion_continuation_attempt
                 WHERE supervisor_authority_id IN (SELECT authority_id FROM supervisor_scope)
                   AND operation='source_recovery'
                   AND phase NOT IN ('drained','never_started')) >= 4
             OR EXISTS(SELECT 1 FROM completion_continuation_attempt
                 WHERE supervisor_authority_id IN (SELECT authority_id FROM supervisor_scope)
                   AND operation='source_recovery' AND source_registration_id=?2
                   AND phase NOT IN ('drained','never_started'))",
                params![supervisor_authority_id, registration],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if blocked {
            return Err("source recovery retained custody bound".into());
        }
    }
    let mut association_sources = Vec::new();
    if request.operation == "activation" {
        let driver: String = tx
            .query_row(
                "SELECT driver_identity FROM completion_continuation_owner
                 WHERE generation=?1 AND phase='running'",
                [&request.owner_generation],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        let live = crate::pid_identity::read_live_process_identity(i64::from(std::process::id()))?
            .ok_or("driver identity unavailable")?;
        let actual = serde_json::to_string(&SourceProcessIdentity {
            pid: live.os_pid,
            boot_id: live.os_boot_id,
            starttime_ticks: live.os_pid_starttime_ticks,
        })
        .map_err(|error| error.to_string())?;
        if driver != actual {
            return Err("completion activation must be admitted by independent driver".into());
        }
        if super::super::session_admission_intent_on(
            tx,
            request
                .session_id
                .as_deref()
                .ok_or("activation session missing")?,
        )? {
            return Err("activation deferred behind session admission intent".into());
        }
        let claim: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM session_wake_claim WHERE session_id=?1 AND claim_token=?2)", params![request.session_id,request.claim_token], |r| r.get(0)).map_err(|e| e.to_string())?;
        if !claim {
            return Err("activation requires exact current wake claim".into());
        }
        association_sources = pending_notification_sources_on(
            tx,
            &domain,
            request
                .session_id
                .as_deref()
                .expect("validated activation session"),
            request
                .claim_token
                .as_deref()
                .expect("validated activation claim"),
        )?;
        let source = association_sources.first().cloned();
        let revision = source
            .as_ref()
            .map(|id| notification_source_listener_revision_on(tx, id))
            .transpose()?;
        if request.source_registration_id != source || request.source_listener_revision != revision
        {
            return Err("activation source differs from exact pending notification".into());
        }
    }
    tx.execute("INSERT INTO completion_continuation_attempt(attempt_id,domain_id,owner_generation,operation,request_sha256,source_registration_id,source_listener_revision,session_id,claim_token,phase,result_path,supervisor_authority_id,association_completeness) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'reserved',?10,?11,'known')", params![request.attempt_id,domain,request.owner_generation,request.operation,request.request_sha256,request.source_registration_id,request.source_listener_revision,request.session_id,request.claim_token,request.result_path,supervisor_authority_id]).map_err(|e| e.to_string())?;
    // The scalar remains the first accepted source for the v24 wire and
    // attempt identity. It is never an exhaustive association. The join is
    // committed with the reservation and survives ACK and claim deletion.
    for registration_id in association_sources {
        let same_domain: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM completion_continuation_source WHERE registration_id=?1 AND domain_id=?2)",
            params![registration_id,domain], |row| row.get(0),
        ).map_err(|error| error.to_string())?;
        if !same_domain {
            return Err("activation source domain changed before reservation".into());
        }
        let listener_revision = notification_source_listener_revision_on(tx, &registration_id)?;
        tx.execute("INSERT INTO completion_continuation_attempt_source(attempt_id,registration_id,listener_revision) VALUES(?1,?2,?3)", params![request.attempt_id,registration_id,listener_revision]).map_err(|e| e.to_string())?;
    }
    Ok(())
}

impl MailboxDb {
    /// Private catalog observation only: unlike activation(), distinguishes an
    /// absent row from a terminal row. One SELECT observes row and claim together
    /// on this reader's actual connection (including its detached snapshot).
    #[cfg(feature = "age360-fault-fixtures")]
    pub fn age360_observe_attempt(&self, attempt_id: &str) -> Result<Option<String>, String> {
        self.conn.query_row(
            "SELECT json_object('phase',phase,'revision',revision,'integrated',integrated,'receipt',drain_receipt,'claim',EXISTS(SELECT 1 FROM session_wake_claim c WHERE c.session_id=a.session_id AND c.claim_token=a.claim_token)) FROM completion_continuation_attempt a WHERE attempt_id=?1",
            [attempt_id], |r| r.get(0)).optional().map_err(|e|e.to_string())
    }

    pub fn cancel_unreleased_continuation_gate(
        &mut self,
        attempt: &ContinuationAttempt,
        custodian: &SourceProcessIdentity,
        receipt: &str,
    ) -> Result<(), String> {
        require_exact_attempt(&self.conn, attempt)?;
        if receipt.is_empty() {
            return Err("missing cancelled gate proof".into());
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let identity = serde_json::to_string(custodian).map_err(|e| e.to_string())?;
        let changed=tx.execute("UPDATE completion_continuation_attempt SET phase='never_started',revision=revision+1,custodian_identity=?2,integrated=1,drain_receipt=?3 WHERE attempt_id=?1 AND phase IN ('reserved','accepted','starting','unknown_custody') AND (custodian_identity IS NULL OR custodian_identity=?2)",params![attempt.attempt_id,identity,receipt]).map_err(|e|e.to_string())?;
        if changed != 1 {
            let retained:Option<String>=tx.query_row("SELECT drain_receipt FROM completion_continuation_attempt WHERE attempt_id=?1 AND custodian_identity=?2 AND phase='never_started'",params![attempt.attempt_id,identity],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
            if retained.as_deref() != Some(receipt) {
                return Err("cancelled gate does not match current attempt custody".into());
            }
        }
        if attempt.operation == "activation" {
            tx.execute(
                "DELETE FROM session_wake_claim WHERE session_id=?1 AND claim_token=?2",
                params![attempt.session_id, attempt.claim_token],
            )
            .map_err(|e| e.to_string())?;
        }
        #[cfg(feature = "age360-fault-fixtures")]
        crate::completion_continuation::age360_fault_barrier("unreleased-before-commit");
        let result = tx.commit().map_err(|e| e.to_string());
        #[cfg(feature = "age360-fault-fixtures")]
        crate::completion_continuation::age360_fault_barrier(if result.is_ok() {
            "unreleased-commit-returned-ok"
        } else {
            "unreleased-commit-returned-error"
        });
        result
    }
}

pub(in crate::mailbox) fn reserve_activation_on(
    tx: &Transaction<'_>,
    input: WakeClaimRequest<'_>,
    data_root: &std::path::Path,
) -> Result<(), String> {
    let Some(domain) = domain_on(tx)? else {
        return Ok(());
    };
    let (generation,driver):(String,String)=tx.query_row("SELECT generation,driver_identity FROM completion_continuation_owner WHERE domain_id=?1 AND phase='running'",[&domain],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())?
        .ok_or("completion_owner_unavailable: activation requires an independent owner; schema upgrade grants no actor custody")?;
    let driver: SourceProcessIdentity = serde_json::from_str(&driver).map_err(|e| e.to_string())?;
    let live = crate::pid_identity::read_live_process_identity(i64::from(std::process::id()))?
        .ok_or("driver identity unavailable")?;
    if driver.pid != live.os_pid
        || driver.boot_id != live.os_boot_id
        || driver.starttime_ticks != live.os_pid_starttime_ticks
    {
        return Err("completion activation must be admitted by independent driver".into());
    }
    let metadata = session_metadata_row(tx, input.session_id)?;
    if !native_wake_runtime_ready(metadata.as_ref()) {
        return Err("native activation runtime unavailable before reservation".into());
    }
    // Attribute a receiver activation only to an accepted notification that
    // was actually included in this claim. A registered source, a historical
    // listener, or another session's pending row is not activation authority.
    // Generic pending input may still use the same single receiver lane with
    // no completion source attached.
    let source = pending_notification_source_on(tx, &domain, input.session_id, input.claim_token)?;
    let request = ContinuationAttempt {
        attempt_id: uuid::Uuid::new_v4().to_string(),
        owner_generation: generation,
        operation: "activation".into(),
        request_sha256: super::activation_request_sha256(
            input.session_id,
            metadata.as_ref(),
            input.claim_token,
            input.auto_wake_count,
        ),
        source_registration_id: source.clone(),
        source_listener_revision: source
            .as_ref()
            .map(|id| notification_source_listener_revision_on(tx, id))
            .transpose()?,
        session_id: Some(input.session_id.into()),
        claim_token: Some(input.claim_token.into()),
        result_path: data_root
            .join("completion-continuation")
            .join(&domain)
            .join("attempts")
            .join(input.claim_token)
            .join("result.json")
            .to_string_lossy()
            .into_owned(),
    };
    reserve_on(tx, &request)
}

fn notification_source_listener_revision_on(
    tx: &Transaction<'_>,
    registration_id: &str,
) -> Result<i64, String> {
    tx.query_row(
        "SELECT COUNT(*) FROM completion_event_listener AS listener
         JOIN completion_continuation_source AS source
           ON source.event_id=listener.event_id
         WHERE source.registration_id=?1",
        [registration_id],
        |row| row.get(0),
    )
    .map_err(|error| error.to_string())
}

fn pending_notification_source_on(
    tx: &Transaction<'_>,
    domain: &str,
    session: &str,
    token: &str,
) -> Result<Option<String>, String> {
    Ok(pending_notification_sources_on(tx, domain, session, token)?
        .into_iter()
        .next())
}

fn pending_notification_sources_on(
    tx: &Transaction<'_>,
    domain: &str,
    session: &str,
    token: &str,
) -> Result<Vec<String>, String> {
    // Start at the claimed mailbox rows. An inner join here can erase a v2
    // completion whose source or listener projection is missing, turning it
    // into apparent authority for a source-free activation.
    let mut statement = tx
        .prepare(
            "SELECT source.registration_id,source.domain_id,source.phase,notification.policy,
                listener.event_id,listener.listener_id,listener.session_id,
                listener.owner_invocation_uuid,listener.active,listener.retirement_pending,
                COALESCE(source.payload_sha256=message.payload_sha256,0),
                COALESCE(listener.acknowledged_at IS NULL,0),
                notification.policy_origin,
                message.owner_invocation_uuid,
                COALESCE(source.event_id=listener.event_id,0),
                message.completion_provenance
         FROM session_wake_claim AS claim
         JOIN mailbox AS message
           ON message.session_id=claim.session_id
          AND message.seq BETWEEN claim.min_pending_seq_at_claim
                              AND claim.max_pending_seq_at_claim
         LEFT JOIN completion_event_listener AS listener
           ON listener.mailbox_seq=message.seq
         LEFT JOIN completion_continuation_source AS source
           ON source.event_id=listener.event_id
           OR message.handle=source.event_id
           OR (message.owner_invocation_uuid IS NOT NULL
               AND source.event_id=substr(message.handle,1,
                   length(message.handle)-length(message.owner_invocation_uuid)-1)
               AND message.handle=source.event_id || ':' || message.owner_invocation_uuid)
         LEFT JOIN completion_continuation_notification AS notification
           ON notification.event_id=COALESCE(listener.event_id,source.event_id)
          AND notification.listener_id=message.owner_invocation_uuid
         WHERE claim.session_id=?1 AND claim.claim_token=?2
           AND message.kind='agent_bash_complete'
           AND message.delivered_at IS NULL
         ORDER BY message.seq",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![session, token], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<bool>>(8)?,
                row.get::<_, Option<bool>>(9)?,
                row.get::<_, bool>(10)?,
                row.get::<_, bool>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, Option<String>>(13)?,
                row.get::<_, bool>(14)?,
                row.get::<_, String>(15)?,
            ))
        })
        .map_err(|error| error.to_string())?;
    let mut selected = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for row in rows {
        let (
            source,
            source_domain,
            phase,
            policy,
            event,
            listener_id,
            listener_session,
            listener_owner,
            active,
            retirement_pending,
            exact_payload,
            unacknowledged,
            policy_origin,
            message_owner,
            exact_event,
            provenance,
        ) = row.map_err(|error| error.to_string())?;
        if provenance == "unclassified" {
            return Err(format!(
                "pending completion provenance unclassified for session {session}, claim {token}; recover exact source or immutable payload before activation"
            ));
        }
        // The mailbox provenance survives total loss of the relational v2
        // projections. Only a positively classified legacy row can use the
        // generic wake lane.
        if provenance == "legacy" && source.is_none() && policy_origin.is_none() {
            continue;
        }
        if provenance != "v2" {
            return Err("pending completion provenance conflicts with v2 source evidence".into());
        }
        let exact_policy_origin = source
            .as_ref()
            .map(|registration_id| format!("exact_admitted_binding:{registration_id}"));
        if source_domain.as_deref() != Some(domain)
            || phase.as_deref() != Some("accepted")
            || policy.as_deref() != Some("notify")
            || policy_origin != exact_policy_origin
            || event.is_none()
            || listener_id != message_owner
            || listener_session.as_deref() != Some(session)
            || listener_owner != message_owner
            || active != Some(true)
            || retirement_pending != Some(true)
            || !unacknowledged
            || !exact_payload
            || !exact_event
        {
            return Err("pending v2 notification lacks exact accepted source authority".into());
        }
        if let Some(source) = source {
            if seen.insert(source.clone()) {
                selected.push(source);
            }
        }
    }
    Ok(selected)
}

/// Perform at most one artifact verification per attempt, before taking the
/// sidecar writer. A short conditional update records the verified result.
/// A changed row or claim is left unknown for a later retry.
pub(in crate::mailbox) fn classify_one_pending_completion(
    conn: &mut rusqlite::Connection,
    session: &str,
    token: Option<&str>,
) -> Result<bool, String> {
    classify_one_pending_completion_with_hook(conn, session, token, || {})
}

fn classify_one_pending_completion_with_hook(
    conn: &mut rusqlite::Connection,
    session: &str,
    token: Option<&str>,
    before_writer: impl FnOnce(),
) -> Result<bool, String> {
    let candidate = conn.query_row(
        "SELECT message.seq,message.payload_json,message.payload_file_path,
                message.payload_sha256,message.payload_byte_len,
                message.payload_retention_policy,message.payload_compacted_at,
                EXISTS(SELECT 1 FROM completion_event_listener AS listener
                       JOIN completion_continuation_source AS source
                         ON source.event_id=listener.event_id
                       WHERE listener.mailbox_seq=message.seq)
                OR EXISTS(SELECT 1 FROM completion_continuation_source AS source
                          WHERE message.handle=source.event_id
                             OR (message.owner_invocation_uuid IS NOT NULL
                                 AND message.handle=source.event_id || ':' || message.owner_invocation_uuid))
                OR EXISTS(SELECT 1 FROM completion_event_listener AS listener
                          JOIN completion_continuation_notification AS notification
                            ON notification.event_id=listener.event_id
                           AND notification.listener_id=listener.listener_id
                          WHERE listener.mailbox_seq=message.seq
                            AND notification.policy_origin LIKE 'exact_admitted_binding:%')
         FROM mailbox AS message
         WHERE message.session_id=?1
           AND (?2 IS NULL OR EXISTS(
               SELECT 1 FROM session_wake_claim AS claim
               WHERE claim.session_id=?1 AND claim.claim_token=?2
                 AND message.seq BETWEEN claim.min_pending_seq_at_claim
                                     AND claim.max_pending_seq_at_claim))
           AND message.kind='agent_bash_complete' AND message.delivered_at IS NULL
           AND message.completion_provenance='unclassified'
         ORDER BY message.seq LIMIT 1",
        params![session, token], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, bool>(7)?,
            ))
        }).optional().map_err(|error| error.to_string())?;
    let Some((seq, envelope, path, digest, len, policy, compacted_at, source_evidence)) = candidate
    else {
        return Ok(false);
    };
    let provenance = if source_evidence {
        Some("v2")
    } else {
        super::super::schema::classify_existing_completion_payload(
            conn,
            &envelope,
            path.as_deref(),
            digest.as_deref(),
            len,
            policy.as_deref(),
            compacted_at.as_deref(),
        )
    };
    let Some(provenance) = provenance else {
        return Ok(false);
    };
    before_writer();
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    // A newly materialized exact source must never be relabelled legacy.
    let current_source: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM completion_event_listener AS listener
                       JOIN completion_continuation_source AS source
                         ON source.event_id=listener.event_id
                       WHERE listener.mailbox_seq=?1)
             OR EXISTS(SELECT 1 FROM completion_continuation_source AS source
                       JOIN mailbox AS message ON message.seq=?1
                       WHERE message.handle=source.event_id
                          OR (message.owner_invocation_uuid IS NOT NULL
                              AND message.handle=source.event_id || ':' || message.owner_invocation_uuid))
             OR EXISTS(SELECT 1 FROM completion_event_listener AS listener
                       JOIN completion_continuation_notification AS notification
                         ON notification.event_id=listener.event_id
                        AND notification.listener_id=listener.listener_id
                       WHERE listener.mailbox_seq=?1
                         AND notification.policy_origin LIKE 'exact_admitted_binding:%')",
        [seq], |row| row.get(0),
    ).map_err(|error| error.to_string())?;
    if provenance == "legacy" && current_source {
        return Ok(false);
    }
    let changed = tx
        .execute(
            "UPDATE mailbox SET completion_provenance=?3
         WHERE seq=?1 AND session_id=?2 AND kind='agent_bash_complete'
           AND delivered_at IS NULL AND completion_provenance='unclassified'
           AND payload_json IS ?4 AND payload_file_path IS ?5
           AND payload_sha256 IS ?6 AND payload_byte_len IS ?7
           AND payload_retention_policy IS ?8 AND payload_compacted_at IS ?9
           AND (?10 IS NULL OR EXISTS(
               SELECT 1 FROM session_wake_claim AS claim
               WHERE claim.session_id=?2 AND claim.claim_token=?10
                 AND mailbox.seq BETWEEN claim.min_pending_seq_at_claim
                                     AND claim.max_pending_seq_at_claim))",
            params![
                seq,
                session,
                provenance,
                envelope,
                path,
                digest,
                len,
                policy,
                compacted_at,
                token
            ],
        )
        .map_err(|error| error.to_string())?;
    tx.commit().map_err(|error| error.to_string())?;
    Ok(changed == 1)
}

impl MailboxDb {
    pub fn accept_continuation_attempt(
        &mut self,
        attempt: &ContinuationAttempt,
    ) -> Result<(), String> {
        require_exact_attempt(&self.conn, attempt)?;
        let observer = SqliteOperationObserver::process();
        let changed = match self.conn.execute("UPDATE completion_continuation_attempt SET phase='accepted',revision=revision+1 WHERE attempt_id=?1 AND owner_generation=?2 AND phase='reserved' AND revision=1 AND EXISTS(SELECT 1 FROM completion_continuation_owner WHERE generation=?2 AND phase='running')",params![attempt.attempt_id,attempt.owner_generation]) {
            Ok(changed) => {
                let _ = observer.record_success(
                    || {
                        continuation_write_span(
                            "root_supervisor_accept_attempt",
                            "completion_continuation.accept",
                            attempt,
                        )
                    },
                    DiagnosticPhase::Committed,
                    OutcomeCertainty::Committed,
                    |elapsed| autocommit_write_evidence(elapsed, Some(changed)),
                );
                changed
            }
            Err(error) => {
                let _ = observer.record_failure(
                    || {
                        continuation_write_span(
                            "root_supervisor_accept_attempt",
                            "completion_continuation.accept",
                            attempt,
                        )
                    },
                    &error,
                    OutcomeCertainty::StartedUnknown,
                    |elapsed| autocommit_write_evidence(elapsed, None),
                );
                return Err(error.to_string());
            }
        };
        if changed != 1 {
            return Err("continuation acceptance lost current reservation".into());
        }
        Ok(())
    }
    pub fn attach_continuation_custodian(
        &mut self,
        attempt: &ContinuationAttempt,
        custodian: &SourceProcessIdentity,
    ) -> Result<(), String> {
        self.attach_continuation_custodian_with_adopter(attempt, custodian, None)
    }
    pub fn attach_continuation_custodian_with_adopter(
        &mut self,
        attempt: &ContinuationAttempt,
        custodian: &SourceProcessIdentity,
        adopter: Option<&SourceProcessIdentity>,
    ) -> Result<(), String> {
        require_exact_attempt(&self.conn, attempt)?;
        let custodian = serde_json::to_string(custodian).map_err(|e| e.to_string())?;
        let adopter = adopter
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| e.to_string())?;
        let observer = SqliteOperationObserver::process();
        let changed = match self.conn.execute("UPDATE completion_continuation_attempt SET revision=revision+1,custodian_identity=?3,adopter_identity=?4 WHERE attempt_id=?1 AND owner_generation=?2 AND phase='accepted' AND revision=2 AND custodian_identity IS NULL AND EXISTS(SELECT 1 FROM completion_continuation_owner WHERE generation=?2 AND phase='running')",params![attempt.attempt_id,attempt.owner_generation,custodian,adopter]) {
            Ok(changed) => {
                let _ = observer.record_success(
                    || {
                        continuation_write_span(
                            "root_supervisor_attach_custodian",
                            "completion_continuation.attach_custodian",
                            attempt,
                        )
                    },
                    DiagnosticPhase::Committed,
                    OutcomeCertainty::Committed,
                    |elapsed| autocommit_write_evidence(elapsed, Some(changed)),
                );
                changed
            }
            Err(error) => {
                let _ = observer.record_failure(
                    || {
                        continuation_write_span(
                            "root_supervisor_attach_custodian",
                            "completion_continuation.attach_custodian",
                            attempt,
                        )
                    },
                    &error,
                    OutcomeCertainty::StartedUnknown,
                    |elapsed| autocommit_write_evidence(elapsed, None),
                );
                return Err(error.to_string());
            }
        };
        if changed != 1 {
            return Err("continuation custodian publication lost current reservation".into());
        }
        Ok(())
    }
    /// Retained birth testimony from the SAME original driver, not a successor's
    /// inferred identity. This records custody only and moves an unfinished
    /// starting gate to unknown custody, never restoring execution authority.
    /// The immutable actor attachment and revision schema remain enforced.
    pub fn attach_original_continuation_custody(
        &mut self,
        attempt: &ContinuationAttempt,
        driver: &SourceProcessIdentity,
        custodian: &SourceProcessIdentity,
        adopter: &SourceProcessIdentity,
    ) -> Result<(), String> {
        require_exact_attempt(&self.conn, attempt)?;
        let live = crate::pid_identity::read_live_process_identity(i64::from(std::process::id()))?
            .ok_or("original driver absent")?;
        if driver.pid != live.os_pid
            || driver.boot_id != live.os_boot_id
            || driver.starttime_ticks != live.os_pid_starttime_ticks
        {
            return Err("original birth driver identity conflict".into());
        }
        let driver = serde_json::to_string(driver).map_err(|e| e.to_string())?;
        let custodian = serde_json::to_string(custodian).map_err(|e| e.to_string())?;
        let adopter = serde_json::to_string(adopter).map_err(|e| e.to_string())?;
        let recorded_driver: String = self
            .conn
            .query_row(
                "SELECT driver_identity FROM completion_continuation_owner WHERE generation=?1",
                [&attempt.owner_generation],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if recorded_driver != driver {
            return Err("original birth driver generation conflict".into());
        }
        let (old_custodian, old_adopter, phase): (Option<String>, Option<String>, String) = self.conn.query_row(
            "SELECT custodian_identity,adopter_identity,phase FROM completion_continuation_attempt WHERE attempt_id=?1", [&attempt.attempt_id], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|e|e.to_string())?;
        if old_custodian.as_ref().is_some_and(|v| v != &custodian)
            || old_adopter.as_ref().is_some_and(|v| v != &adopter)
            || !matches!(
                phase.as_str(),
                "accepted" | "starting" | "unknown_custody" | "drained" | "never_started"
            )
        {
            return Err("original birth attachment fence conflict".into());
        }
        if old_custodian.is_some()
            && old_adopter.is_some()
            && !matches!(phase.as_str(), "accepted" | "starting")
        {
            return Ok(());
        }
        let changed = self.conn.execute("UPDATE completion_continuation_attempt SET revision=revision+1,custodian_identity=?3,adopter_identity=?4,phase=CASE WHEN phase IN ('accepted','starting') THEN 'unknown_custody' ELSE phase END WHERE attempt_id=?1 AND owner_generation=?2 AND phase IN ('accepted','starting','unknown_custody','drained','never_started') AND (custodian_identity IS NULL OR custodian_identity=?3) AND (adopter_identity IS NULL OR adopter_identity=?4)", params![attempt.attempt_id,attempt.owner_generation,custodian,adopter]).map_err(|e| e.to_string())?;
        if changed != 1 {
            return Err("original birth attachment fence conflict".into());
        }
        Ok(())
    }
    pub fn continuation_activation(
        &self,
        session_id: &str,
        token: &str,
    ) -> Result<Option<ContinuationAttempt>, String> {
        if domain_on(&self.conn)?.is_none() {
            return Ok(None);
        }
        self.conn.query_row("SELECT attempt_id,owner_generation,operation,request_sha256,source_registration_id,source_listener_revision,session_id,claim_token,result_path FROM completion_continuation_attempt WHERE session_id=?1 AND claim_token=?2 AND operation='activation' AND phase NOT IN ('drained','never_started')",params![session_id,token],|r|Ok(ContinuationAttempt{
            attempt_id:r.get(0)?,owner_generation:r.get(1)?,operation:r.get(2)?,request_sha256:r.get(3)?,source_registration_id:r.get(4)?,source_listener_revision:r.get(5)?,session_id:r.get(6)?,claim_token:r.get(7)?,result_path:r.get(8)?,
        })).optional().map_err(|e|e.to_string())
    }
}

pub(in crate::mailbox) fn cancel_unaccepted_activation_on(
    tx: &Transaction<'_>,
    session: &str,
    token: &str,
) -> Result<(), String> {
    if domain_on(tx)?.is_none() {
        return Ok(());
    }
    let live = crate::pid_identity::read_live_process_identity(i64::from(std::process::id()))?
        .ok_or("driver process disappeared")?;
    let driver = serde_json::to_string(&SourceProcessIdentity {
        pid: live.os_pid,
        boot_id: live.os_boot_id,
        starttime_ticks: live.os_pid_starttime_ticks,
    })
    .map_err(|error| error.to_string())?;
    // Accepted is persisted before any custodian fork. Thus a current reserved
    // row is conclusive unspent launch authority, unlike a dead accepted owner.
    // Only the exact current driver may withdraw its proposal. A nested caller
    // cannot erase the root's pending launch authority through claim cleanup.
    tx.execute("UPDATE completion_continuation_attempt AS attempt SET phase='never_started',revision=revision+1,integrated=1,drain_receipt='cancelled_unaccepted_reservation' WHERE session_id=?1 AND claim_token=?2 AND operation='activation' AND phase='reserved' AND revision=1 AND custodian_identity IS NULL AND EXISTS(SELECT 1 FROM completion_continuation_owner owner WHERE owner.generation=attempt.owner_generation AND owner.phase='running' AND owner.driver_identity=?3)",params![session,token,driver]).map_err(|e|e.to_string())?;
    Ok(())
}

pub(in crate::mailbox) fn admit_launcher_on(
    tx: &Transaction<'_>,
    session: &str,
    token: &str,
    child: &ProcessIdentity,
) -> Result<(), String> {
    if domain_on(tx)?.is_none() {
        return Ok(());
    }
    let (attempt,custodian,existing):(String,String,Option<String>)=tx.query_row("SELECT attempt_id,custodian_identity,launcher_identity FROM completion_continuation_attempt WHERE session_id=?1 AND claim_token=?2 AND operation='activation' AND phase IN ('starting','running','unknown_custody')",params![session,token],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|e|e.to_string())?
        .ok_or("unsupported_legacy_activation_recovery: retained wake claim has no admitted v2 attempt; migration cannot invent launcher custody")?;
    let custodian: SourceProcessIdentity =
        serde_json::from_str(&custodian).map_err(|e| e.to_string())?;
    if child.os_pid != i64::from(std::process::id()) {
        return Err("activation launcher must present its own exact process identity".into());
    }
    #[cfg(target_os = "linux")]
    {
        let parent = unsafe { libc::getppid() };
        let expected = crate::pid_identity::read_live_process_identity(i64::from(parent))?
            .ok_or("activation custodian disappeared")?;
        if custodian.pid != expected.os_pid
            || custodian.boot_id != expected.os_boot_id
            || custodian.starttime_ticks != expected.os_pid_starttime_ticks
        {
            return Err("activation launcher is not beneath its exact custodian".into());
        }
    }
    let identity = serde_json::to_string(&SourceProcessIdentity {
        pid: child.os_pid,
        boot_id: child.os_boot_id.clone(),
        starttime_ticks: child.os_pid_starttime_ticks,
    })
    .map_err(|e| e.to_string())?;
    if let Some(existing) = existing {
        return if existing == identity {
            Ok(())
        } else {
            Err("activation launcher identity conflict".into())
        };
    }
    tx.execute("UPDATE completion_continuation_attempt SET launcher_identity=?2,revision=revision+1 WHERE attempt_id=?1 AND launcher_identity IS NULL",params![attempt,identity]).map_err(|e|e.to_string())?;
    Ok(())
}

pub(in crate::mailbox) fn bind_generation_on(
    tx: &Transaction<'_>,
    request: CreateRuntimeGeneration<'_>,
    creator: &ProcessIdentity,
) -> Result<(), super::super::GenerationStorageError> {
    use super::super::{GenerationStorageDiagnostic, GenerationStorageError};
    let storage = |message: String| {
        GenerationStorageError::new(message)
            .with_diagnostic(GenerationStorageDiagnostic::NativeActivationBinding)
    };
    if domain_on(tx).map_err(storage)?.is_none() {
        return Ok(());
    }
    let Some(session) = request.session_id else {
        return Ok(());
    };
    let active:Option<(String,Option<String>,Option<String>)>=tx.query_row("SELECT attempt_id,launcher_identity,runtime_generation_uuid FROM completion_continuation_attempt WHERE session_id=?1 AND operation='activation' AND phase NOT IN ('drained','never_started')",[session],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|e|storage(e.to_string()))?;
    let Some((attempt, launcher, generation)) = active else {
        return Ok(());
    };
    let identity = serde_json::to_string(&SourceProcessIdentity {
        pid: creator.os_pid,
        boot_id: creator.os_boot_id.clone(),
        starttime_ticks: creator.os_pid_starttime_ticks,
    })
    .map_err(|e| storage(e.to_string()))?;
    if launcher.as_deref() != Some(&identity)
        || generation
            .as_ref()
            .is_some_and(|id| *id != request.generation_id.to_string())
    {
        return Err(GenerationStorageError::new(
            "runtime creation conflicts with current session activation reservation".into(),
        )
        .with_diagnostic(GenerationStorageDiagnostic::NativeActivationConflict));
    }
    tx.execute("UPDATE completion_continuation_attempt SET spawn_invocation_uuid=?2,runtime_generation_uuid=?3,revision=revision+1 WHERE attempt_id=?1",params![attempt,request.spawn_invocation_uuid,request.generation_id.to_string()]).map_err(|e|storage(e.to_string()))?;
    Ok(())
}

fn require_exact_attempt(conn: &Connection, request: &ContinuationAttempt) -> Result<(), String> {
    let retained=conn.query_row("SELECT attempt_id,owner_generation,operation,request_sha256,source_registration_id,source_listener_revision,session_id,claim_token,result_path FROM completion_continuation_attempt WHERE attempt_id=?1",[&request.attempt_id],|r|Ok(ContinuationAttempt{attempt_id:r.get(0)?,owner_generation:r.get(1)?,operation:r.get(2)?,request_sha256:r.get(3)?,source_registration_id:r.get(4)?,source_listener_revision:r.get(5)?,session_id:r.get(6)?,claim_token:r.get(7)?,result_path:r.get(8)?})).map_err(|e|e.to_string())?;
    if &retained != request {
        return Err("immutable continuation request conflict".into());
    }
    Ok(())
}

impl MailboxDb {
    /// The root forked a worker but never sent its execution grant because the
    /// worker's exact process identity could not be established. The guardian
    /// retained the grant endpoint and waited the worker, so provider effects
    /// are conclusively absent even though "never forked" would be false.
    pub fn record_supervisor_unreleased_worker(
        &mut self,
        attempt: &ContinuationAttempt,
        guardian: &SourceProcessIdentity,
        worker_pid: i64,
        reason: &str,
    ) -> Result<(), String> {
        require_exact_attempt(&self.conn, attempt)?;
        if worker_pid <= 0 || reason.is_empty() {
            return Err("invalid unreleased root worker receipt".into());
        }
        require_exact_live_guardian(guardian)?;
        let encoded = serde_json::to_string(guardian).map_err(|error| error.to_string())?;
        let receipt = serde_json::json!({
            "attempt_id": attempt.attempt_id,
            "supervisor": guardian,
            "worker_pid": worker_pid,
            "gate": "root_worker_forked_execution_grant_not_sent",
            "reason": reason,
        })
        .to_string();
        settle_supervisor_unreleased_attempt(self, attempt, &encoded, &receipt)
    }

    /// The live root supervisor accepted the immutable request, but the worker
    /// fork/exec syscall failed conclusively before a worker identity existed
    /// or an execution grant could be sent.  Unlike the legacy driver helper,
    /// this authority is the generation's recorded guardian.
    pub fn record_supervisor_never_forked(
        &mut self,
        attempt: &ContinuationAttempt,
        guardian: &SourceProcessIdentity,
        reason: &str,
    ) -> Result<(), String> {
        require_exact_attempt(&self.conn, attempt)?;
        if reason.is_empty() {
            return Err("missing root supervisor fork failure reason".into());
        }
        require_exact_live_guardian(guardian)?;
        let encoded = serde_json::to_string(guardian).map_err(|error| error.to_string())?;
        let receipt = serde_json::json!({
            "attempt_id": attempt.attempt_id,
            "supervisor": guardian,
            "gate": "root_worker_not_forked",
            "reason": reason,
        })
        .to_string();
        settle_supervisor_unreleased_attempt(self, attempt, &encoded, &receipt)
    }

    /// Called only by the actual recorded driver after a local gate/fork syscall
    /// conclusively failed before any custodian existed. Announcement EOF uses
    /// a separate no-effect classification, not a claim that no AC was forked.
    /// Owner absence or an empty successor is never sufficient for this transition.
    pub fn record_continuation_never_forked(
        &mut self,
        attempt: &ContinuationAttempt,
        reason: &str,
    ) -> Result<(), String> {
        self.record_driver_unreleased(attempt, reason, "no_custodian_created")
    }

    /// Original driver observed announcement EOF while still holding the unsent
    /// execution grant, then waited its exact adopter. This says no effects were
    /// released, not that no AC was created. No replacement emptiness qualifies.
    pub fn record_continuation_unreleased_before_announcement(
        &mut self,
        attempt: &ContinuationAttempt,
        reason: &str,
    ) -> Result<(), String> {
        self.record_driver_unreleased(attempt, reason, "unreleased_announcement_eof")
    }

    fn record_driver_unreleased(
        &mut self,
        attempt: &ContinuationAttempt,
        reason: &str,
        gate: &str,
    ) -> Result<(), String> {
        require_exact_attempt(&self.conn, attempt)?;
        let live = crate::pid_identity::read_live_process_identity(i64::from(std::process::id()))?
            .ok_or("driver process disappeared")?;
        let identity = SourceProcessIdentity {
            pid: live.os_pid,
            boot_id: live.os_boot_id,
            starttime_ticks: live.os_pid_starttime_ticks,
        };
        let encoded = serde_json::to_string(&identity).map_err(|e| e.to_string())?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let driver: String = tx
            .query_row(
                "SELECT driver_identity FROM completion_continuation_owner WHERE generation=?1",
                [&attempt.owner_generation],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if driver != encoded || reason.is_empty() {
            return Err("unreleased receipt is not from exact driver".into());
        }
        let receipt=serde_json::json!({"attempt_id":attempt.attempt_id,"driver":identity,"gate":gate,"reason":reason}).to_string();
        let changed=tx.execute("UPDATE completion_continuation_attempt SET phase='never_started',revision=revision+1,integrated=1,drain_receipt=?2 WHERE attempt_id=?1 AND phase IN ('accepted','unknown_custody') AND custodian_identity IS NULL",params![attempt.attempt_id,receipt]).map_err(|e|e.to_string())?;
        if changed != 1 {
            let retained:Option<String>=tx.query_row("SELECT drain_receipt FROM completion_continuation_attempt WHERE attempt_id=?1 AND phase='never_started' AND custodian_identity IS NULL",[&attempt.attempt_id],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
            if retained.as_deref() != Some(&receipt) {
                return Err("receipt no longer matches original unreleased gate".into());
            }
        }
        if attempt.operation == "activation" {
            tx.execute(
                "DELETE FROM session_wake_claim WHERE session_id=?1 AND claim_token=?2",
                params![attempt.session_id, attempt.claim_token],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }
}

fn require_exact_live_guardian(guardian: &SourceProcessIdentity) -> Result<(), String> {
    let live = crate::pid_identity::read_live_process_identity(i64::from(std::process::id()))?
        .ok_or("root supervisor process disappeared")?;
    let caller = SourceProcessIdentity {
        pid: live.os_pid,
        boot_id: live.os_boot_id,
        starttime_ticks: live.os_pid_starttime_ticks,
    };
    if &caller != guardian {
        return Err("root supervisor fork receipt caller mismatch".into());
    }
    Ok(())
}

fn settle_supervisor_unreleased_attempt(
    db: &mut MailboxDb,
    attempt: &ContinuationAttempt,
    encoded_guardian: &str,
    receipt: &str,
) -> Result<(), String> {
    let tx = db
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let changed = tx
        .execute(
            "UPDATE completion_continuation_attempt
             SET phase='never_started',revision=revision+1,integrated=1,
                 drain_receipt=?3
             WHERE attempt_id=?1 AND owner_generation=?2
               AND phase='accepted' AND custodian_identity IS NULL
               AND EXISTS(
                 SELECT 1 FROM completion_continuation_owner owner
                 WHERE owner.phase='running'
                   AND owner.supervisor_authority_id=(
                     SELECT supervisor_authority_id
                     FROM completion_continuation_attempt WHERE attempt_id=?1)
                   AND owner.guardian_identity=?4)",
            params![
                attempt.attempt_id,
                attempt.owner_generation,
                receipt,
                encoded_guardian
            ],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        let retained: Option<String> = tx
            .query_row(
                "SELECT drain_receipt FROM completion_continuation_attempt
                 WHERE attempt_id=?1 AND owner_generation=?2
                   AND phase='never_started' AND custodian_identity IS NULL",
                params![attempt.attempt_id, attempt.owner_generation],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if retained.as_deref() != Some(receipt) {
            return Err("root supervisor fork receipt no longer matches attempt".into());
        }
    }
    if attempt.operation == "activation" {
        tx.execute(
            "DELETE FROM session_wake_claim WHERE session_id=?1 AND claim_token=?2",
            params![attempt.session_id, attempt.claim_token],
        )
        .map_err(|error| error.to_string())?;
    }
    tx.commit().map_err(|error| error.to_string())
}

impl MailboxDb {
    /// Re-executed custodian validates the exact admitted gate before any effect.
    pub fn require_continuation_launch_gate(
        &self,
        attempt: &ContinuationAttempt,
        custodian: &SourceProcessIdentity,
    ) -> Result<(), String> {
        require_exact_attempt(&self.conn, attempt)?;
        let live = crate::pid_identity::read_live_process_identity(i64::from(std::process::id()))?
            .ok_or("custodian disappeared")?;
        if custodian.pid != live.os_pid
            || custodian.boot_id != live.os_boot_id
            || custodian.starttime_ticks != live.os_pid_starttime_ticks
        {
            return Err("custodian gate caller mismatch".into());
        }
        let ready:bool=self.conn.query_row("SELECT EXISTS(SELECT 1 FROM completion_continuation_attempt a JOIN completion_continuation_owner o ON o.generation=a.owner_generation WHERE a.attempt_id=?1 AND a.phase='starting' AND a.custodian_identity=?2 AND o.phase='running')",params![attempt.attempt_id,serde_json::to_string(custodian).map_err(|e|e.to_string())?],|r|r.get(0)).map_err(|e|e.to_string())?;
        if !ready {
            return Err("custodian launch gate is not current and released".into());
        }
        Ok(())
    }
}

impl MailboxDb {
    pub fn continuation_domain_drained_runtime(
        &self,
        domain: &str,
        generation: &str,
        invocation: &str,
    ) -> Result<bool, String> {
        self.conn.query_row("SELECT EXISTS(SELECT 1 FROM completion_continuation_attempt
                INDEXED BY completion_continuation_attempt_native_runtime
            WHERE domain_id=?1 AND operation='activation' AND phase='drained' AND integrated=1 AND runtime_generation_uuid=?2 AND spawn_invocation_uuid=?3)",
            params![domain, generation, invocation], |r| r.get(0)).map_err(|e| e.to_string())
    }

    pub fn continuation_runtime_identity(
        &self,
        attempt: &ContinuationAttempt,
    ) -> Result<Option<(String, String)>, String> {
        require_exact_attempt(&self.conn, attempt)?;
        self.conn.query_row("SELECT runtime_generation_uuid,spawn_invocation_uuid FROM completion_continuation_attempt WHERE attempt_id=?1 AND runtime_generation_uuid IS NOT NULL AND spawn_invocation_uuid IS NOT NULL",[&attempt.attempt_id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())
    }
}

impl MailboxDb {
    /// Continuing obligations, not a process-local driver population.
    pub fn pending_continuation_attempts(&self) -> Result<Vec<ContinuationAttempt>, String> {
        let mut statement = self.conn.prepare("SELECT attempt_id,owner_generation,operation,request_sha256,source_registration_id,source_listener_revision,session_id,claim_token,result_path FROM completion_continuation_attempt WHERE phase NOT IN ('drained','never_started')").map_err(|e| e.to_string())?;
        statement
            .query_map([], |r| {
                Ok(ContinuationAttempt {
                    attempt_id: r.get(0)?,
                    owner_generation: r.get(1)?,
                    operation: r.get(2)?,
                    request_sha256: r.get(3)?,
                    source_registration_id: r.get(4)?,
                    source_listener_revision: r.get(5)?,
                    session_id: r.get(6)?,
                    claim_token: r.get(7)?,
                    result_path: r.get(8)?,
                })
            })
            .map_err(|e| e.to_string())?
            .map(|r| r.map_err(|e| e.to_string()))
            .collect()
    }

    /// Only the retained *original adopting boundary* may certify its drain.
    /// The caller may replay that immutable receipt after producer loss. A new
    /// owner, missing PID, ACK, or an empty unrelated tree grants no discharge.
    pub fn discharge_adopted_continuation_attempt(
        &mut self,
        attempt: &ContinuationAttempt,
        receipt: &str,
    ) -> Result<(), String> {
        require_exact_attempt(&self.conn, attempt)?;
        let value: serde_json::Value = serde_json::from_str(receipt).map_err(|e| e.to_string())?;
        let (custodian, adopter): (String, String) = self.conn.query_row("SELECT custodian_identity,adopter_identity FROM completion_continuation_attempt WHERE attempt_id=?1", [&attempt.attempt_id], |r| Ok((r.get(0)?,r.get(1)?))).map_err(|e| e.to_string())?;
        if value["attempt_id"] != attempt.attempt_id
            || value["owned_children"] != "ECHILD"
            || value["custodian"]
                != serde_json::from_str::<serde_json::Value>(&custodian)
                    .map_err(|e| e.to_string())?
            || value["adopter"]
                != serde_json::from_str::<serde_json::Value>(&adopter).map_err(|e| e.to_string())?
            || value["custodian_wait_status"].as_i64().is_none()
        {
            return Err("adopting drain receipt identity/evidence conflict".into());
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let changed = tx.execute("UPDATE completion_continuation_attempt SET phase='drained',revision=revision+1,integrated=1,drain_receipt=?2 WHERE attempt_id=?1 AND phase IN ('accepted','starting','running','unknown_custody')", params![attempt.attempt_id,receipt]).map_err(|e| e.to_string())?;
        if changed != 1 {
            let retained: Option<String> = tx.query_row("SELECT drain_receipt FROM completion_continuation_attempt WHERE attempt_id=?1 AND phase='drained'", [&attempt.attempt_id], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
            if retained.as_deref() != Some(receipt) {
                return Err("adopting result conflicts with terminal evidence".into());
            }
        }
        if attempt.operation == "activation" {
            tx.execute(
                "DELETE FROM session_wake_claim WHERE session_id=?1 AND claim_token=?2",
                params![attempt.session_id, attempt.claim_token],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }
}

impl MailboxDb {
    pub fn continuation_launcher_identity(
        &self,
        attempt: &ContinuationAttempt,
    ) -> Result<Option<SourceProcessIdentity>, String> {
        require_exact_attempt(&self.conn, attempt)?;
        let identity: Option<String> = self
            .conn
            .query_row(
                "SELECT launcher_identity FROM completion_continuation_attempt WHERE attempt_id=?1",
                [&attempt.attempt_id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        identity
            .map(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
            .transpose()
    }
}

impl MailboxDb {
    /// The immutable native association remains an obligation during both the
    /// running and drained phases; retirement must not race the drain write.
    pub fn native_runtime_in_domain(
        &self,
        domain: &str,
        generation: &str,
        invocation: &str,
    ) -> Result<bool, String> {
        if self.completion_continuation_domain()?.is_none() {
            return Ok(false);
        }
        self.conn.query_row("SELECT EXISTS(SELECT 1 FROM completion_continuation_attempt INDEXED BY completion_continuation_attempt_native_runtime WHERE operation='activation' AND domain_id=?1 AND runtime_generation_uuid=?2 AND spawn_invocation_uuid=?3)",
            params![domain,generation,invocation], |r|r.get(0)).map_err(|e|e.to_string())
    }
    /// Exact original enclosing boundary already integrated its drain. This is
    /// attribution/physical evidence, not actor receipts or channel settlement.
    pub fn native_original_drain(
        &self,
        generation: &str,
        invocation: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        native_original_drain_on(&self.conn, generation, invocation)
    }
}

impl MailboxDb {
    /// Fresh exact producer Q, read on this caller's live connection. Empty
    /// current claims or an enclosing AC drain are not substitute Q evidence.
    /// This does not attest publication of evidence obtained from other snapshots.
    pub fn require_native_runtime_quiescent(
        &self,
        generation: &str,
        invocation: &str,
    ) -> Result<(), String> {
        let id = RuntimeGenerationId::parse(generation).map_err(|e| e.to_string())?;
        let row = runtime_generation_by_id_on(&self.conn, &id)
            .map_err(|e| e.to_string())?
            .ok_or("native_runtime_absent")?;
        if row.spawn_invocation_uuid != invocation {
            return Err("native_original_process_identity_conflict".into());
        }
        if custody_proof_observation(&self.conn, &row) != Some(true) {
            return Err("native_original_current_quiescence_absent".into());
        }
        Ok(())
    }
    /// Original launched-runtime exit projection, independent of late cancellation.
    /// The runtime producer supplies its retained original attempt; State joins the
    /// exact recorded process and integrated original activation drain. No grant,
    /// transfer claim, or existing terminal history is replaced.
    pub fn exit_native_launched_after_original_drain(
        &mut self,
        generation: &str,
        invocation: &str,
        process: &SourceProcessIdentity,
        reason: RuntimeTerminalReason,
        exit_code: Option<i32>,
        drain_request: Option<&str>,
    ) -> Result<(), String> {
        self.native_original_drain(generation, invocation)?
            .ok_or("native_original_drain_absent")?;
        if !matches!(
            reason,
            RuntimeTerminalReason::OrderlyCompletion | RuntimeTerminalReason::AbnormalTermination
        ) {
            return Err("native_original_process_outcome_absent".into());
        }
        let id = RuntimeGenerationId::parse(generation).map_err(|e| e.to_string())?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let before = runtime_generation_by_id_on(&tx, &id)
            .map_err(|e| e.to_string())?
            .ok_or("native_runtime_absent")?;
        let exact = matches!(&before.exact_process_evidence, ExactProcessEvidence::Recorded(p)
            if p.os_pid == process.pid && p.os_boot_id == process.boot_id && p.os_pid_starttime_ticks == process.starttime_ticks);
        if before.spawn_invocation_uuid != invocation
            || before.spawned_os_pid != Some(process.pid)
            || !exact
        {
            return Err("native_original_process_identity_conflict".into());
        }
        if before.active_delivery_claim_id.is_some()
            || !before.active_delivery_seqs.is_empty()
            || before.active_delivery_claimed_at.is_some()
        {
            return Err("native_delivery_claim_unsettled".into());
        }
        if before.lifecycle_state == RuntimeLifecycleState::Exited {
            return Ok(());
        }
        // A pending intent or explicitly retained evidence refusal can support
        // this NEW legal operation only with actual fresh producer Q. Neither
        // missing result nor current empty claims supplies historical Q/ACK.
        if custody_proof_observation(&tx, &before) != Some(true) {
            return Err("native_original_current_quiescence_absent".into());
        }
        // Physical drain is not new lifecycle authority. Keep the same
        // predecessor distinction as finish-drain and non-orderly exit.
        match reason {
            RuntimeTerminalReason::OrderlyCompletion
                if before.lifecycle_state == RuntimeLifecycleState::Draining
                    && drain_request.is_some()
                    && before
                        .drain_request_id
                        .as_ref()
                        .map(ToString::to_string)
                        .as_deref()
                        == drain_request => {}
            RuntimeTerminalReason::AbnormalTermination
                if matches!(
                    before.lifecycle_state,
                    RuntimeLifecycleState::Starting | RuntimeLifecycleState::Running
                ) => {}
            _ => return Err("native_original_runtime_illegal_predecessor".into()),
        }
        let reason = match reason {
            RuntimeTerminalReason::OrderlyCompletion => "orderly_completion",
            _ => "abnormal_termination",
        };
        let now = now_rfc3339();
        tx.execute("UPDATE runtime_generation SET lifecycle_state='exited',exited_at=?3,terminal_reason=?4,exit_code=?5 WHERE generation_uuid=?1 AND spawn_invocation_uuid=?2", params![generation,invocation,now,reason,exit_code]).map_err(|e| e.to_string())?;
        settle_runtime_generation_admission_on(&tx, &id).map_err(|e| e.to_string())?;
        let row = runtime_generation_by_id_on(&tx, &id)
            .map_err(|e| e.to_string())?
            .ok_or("native_runtime_absent_after_exit")?;
        project_exited_generation_on(&tx, &row, &now, row.exit_code).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
    /// Cancellation-only runtime projection from its original activation drain.
    /// The Starting monitor's lost Q is not rewritten. Generic recovery and
    /// successor-transfer custody fences continue to require their original proof.
    pub fn exit_native_cancelled_after_original_drain(
        &mut self,
        generation: &str,
        invocation: &str,
        launch_never_invoked: bool,
    ) -> Result<(), String> {
        let drain = self
            .native_original_drain(generation, invocation)?
            .ok_or("native_original_drain_absent")?;
        // An explicitly never-invoked Launch projects startup failure from its
        // original physical drain, not from a claim of earlier cancellation.
        // Launched work still requires original cancellation evidence.
        if !launch_never_invoked && !drain["receipt"]["accepted_cancellation"].is_string() {
            return Err("original_native_cancellation_absent".into());
        }
        let id = RuntimeGenerationId::parse(generation).map_err(|e| e.to_string())?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let before = runtime_generation_by_id_on(&tx, &id)
            .map_err(|e| e.to_string())?
            .ok_or("native_runtime_absent")?;
        if before.spawn_invocation_uuid != invocation {
            return Err("native_runtime_fence_conflict".into());
        }
        if before.lifecycle_state == RuntimeLifecycleState::Exited {
            return Ok(());
        }
        // No delivery claim is silently discarded by physical cancellation.
        if before.active_delivery_claim_id.is_some()
            || !before.active_delivery_seqs.is_empty()
            || before.active_delivery_claimed_at.is_some()
        {
            return Err("native_delivery_claim_unsettled".into());
        }
        let reason = if launch_never_invoked {
            if before.spawned_os_pid.is_some()
                || !matches!(
                    before.exact_process_evidence,
                    ExactProcessEvidence::NotRecorded
                )
            {
                return Err("native_never_invoked_launch_has_runtime_process".into());
            }
            "startup_failed"
        } else {
            "cancelled"
        };
        let now = now_rfc3339();
        tx.execute("UPDATE runtime_generation SET lifecycle_state='exited',exited_at=?3,terminal_reason=?4,exit_code=NULL WHERE generation_uuid=?1 AND spawn_invocation_uuid=?2",
            params![generation,invocation,now,reason]).map_err(|e|e.to_string())?;
        settle_runtime_generation_admission_on(&tx, &id).map_err(|e| e.to_string())?;
        let row = runtime_generation_by_id_on(&tx, &id)
            .map_err(|e| e.to_string())?
            .ok_or("native_runtime_absent_after_exit")?;
        project_exited_generation_on(&tx, &row, &now, None).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
}

pub(in crate::mailbox) fn native_original_drain_on(
    conn: &Connection,
    generation: &str,
    invocation: &str,
) -> Result<Option<serde_json::Value>, String> {
    if domain_on(conn)?.is_none() {
        return Ok(None);
    }
    let row: Option<(String, String, String, String, String, String)> = conn.query_row(
            "SELECT attempt_id,result_path,custodian_identity,adopter_identity,drain_receipt,domain_id FROM completion_continuation_attempt INDEXED BY completion_continuation_attempt_native_runtime WHERE operation='activation' AND phase='drained' AND integrated=1 AND runtime_generation_uuid=?1 AND spawn_invocation_uuid=?2",
            params![generation, invocation], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional().map_err(|e|e.to_string())?;
    row.map(|(attempt, path, ac, adopter, receipt, domain)| -> Result<_, String> {
            Ok(serde_json::json!({"attempt_id":attempt, "result_path":path,"domain_id":domain,
                "custodian":serde_json::from_str::<serde_json::Value>(&ac).map_err(|e|e.to_string())?,
                "adopter":serde_json::from_str::<serde_json::Value>(&adopter).map_err(|e|e.to_string())?,
                "receipt":serde_json::from_str::<serde_json::Value>(&receipt).map_err(|e|e.to_string())?}))
        }).transpose()
}

#[cfg(test)]
mod notification_activation_tests {
    use super::*;
    use crate::mailbox::{
        AgentBashCompleteEnqueue, CompletionEventRegistrationInput, CompletionEventTriggerInput,
    };

    fn pending_old_row(db: &MailboxDb, handle: &str, template: &str) {
        db.conn
            .execute(
                "INSERT INTO mailbox(session_id,kind,handle,payload_json,enqueued_at,
                state_dir,meta_path,log_path,rc_path,rc,payload_file_path,
                payload_sha256,payload_byte_len,payload_retention_policy,payload_compacted_at)
             SELECT 'receiver',kind,?1,payload_json,enqueued_at,state_dir,meta_path,
                log_path,rc_path,rc,payload_file_path,payload_sha256,payload_byte_len,
                payload_retention_policy,payload_compacted_at
             FROM mailbox WHERE handle=?2",
                params![handle, template],
            )
            .unwrap();
    }

    #[test]
    fn old_payload_verification_releases_writer_and_cas_rejects_changed_row() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pid-identity.db");
        let mut db = MailboxDb::open(&path).unwrap();
        db.enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
            session_id: "receiver",
            handle: "template",
            payload_json: r#"{"schema_version":2,"kind":"agent_bash_complete","meta":{}}"#,
            owner_invocation_uuid: None,
            matched_os_pid: None,
            matched_os_boot_id: None,
            matched_os_pid_starttime_ticks: None,
            matched_chain_index: None,
            state_dir: "/private/source",
            meta_path: "/private/source/meta.json",
            log_path: "/private/source/log",
            rc_path: "/private/source/rc",
            rc: 0,
        })
        .unwrap();
        pending_old_row(&db, "pending", "template");
        let original_digest: String = db
            .conn
            .query_row(
                "SELECT payload_sha256 FROM mailbox WHERE handle='pending'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let mut conn = rusqlite::Connection::open(path).unwrap();
            classify_one_pending_completion_with_hook(&mut conn, "receiver", None, || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            })
            .unwrap()
        });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        // The classifier is paused after reading the artifact. This unrelated
        // writer must complete, and its changed identity must defeat the CAS.
        db.conn
            .execute(
                "UPDATE mailbox SET payload_sha256=?1 WHERE handle='pending'",
                ["0".repeat(64)],
            )
            .unwrap();
        release_tx.send(()).unwrap();
        assert!(!worker.join().unwrap());
        let marker: String = db
            .conn
            .query_row(
                "SELECT completion_provenance FROM mailbox WHERE handle='pending'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(marker, "unclassified");
        db.conn
            .execute(
                "UPDATE mailbox SET payload_sha256=?1 WHERE handle='pending'",
                [original_digest],
            )
            .unwrap();
        assert!(classify_one_pending_completion(&mut db.conn, "receiver", None).unwrap());
        assert!(!classify_one_pending_completion(&mut db.conn, "receiver", None).unwrap());
        pending_old_row(&db, "missing", "template");
        let artifact: String = db
            .conn
            .query_row(
                "SELECT payload_file_path FROM mailbox WHERE handle='template'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        std::fs::remove_file(artifact).unwrap();
        assert!(!classify_one_pending_completion(&mut db.conn, "receiver", None).unwrap());
        let missing_marker: String = db
            .conn
            .query_row(
                "SELECT completion_provenance FROM mailbox WHERE handle='missing'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(missing_marker, "unclassified");
    }

    #[test]
    fn legacy_proof_loses_to_source_materialized_before_cas() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pid-identity.db");
        let mut db = MailboxDb::open(&path).unwrap();
        let domain = db.completion_continuation_domain().unwrap().unwrap();
        db.register_completion_event(CompletionEventRegistrationInput {
            event_id: "pending",
            delivery_mode: "async",
            owner_session_id: Some("receiver"),
            owner_invocation_uuid: Some("listener"),
            state_dir: "/private/source",
            meta_path: "/private/source/meta.json",
            log_path: "/private/source/log",
            rc_path: "/private/source/rc",
        })
        .unwrap();
        db.enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
            session_id: "receiver",
            handle: "template",
            payload_json: r#"{"schema_version":2,"kind":"agent_bash_complete","meta":{}}"#,
            owner_invocation_uuid: None,
            matched_os_pid: None,
            matched_os_boot_id: None,
            matched_os_pid_starttime_ticks: None,
            matched_chain_index: None,
            state_dir: "/private/source",
            meta_path: "/private/source/meta.json",
            log_path: "/private/source/log",
            rc_path: "/private/source/rc",
            rc: 0,
        })
        .unwrap();
        pending_old_row(&db, "pending", "template");
        assert!(
            !classify_one_pending_completion_with_hook(&mut db.conn, "receiver", None, || {
                let concurrent = rusqlite::Connection::open(&path).unwrap();
                concurrent
                    .execute(
                        "INSERT INTO completion_continuation_source
                     (registration_id,domain_id,source_id,event_id,registration_digest,binding)
                     VALUES('pending',?1,'pending','pending','digest',x'00')",
                        [&domain],
                    )
                    .unwrap();
            },)
            .unwrap()
        );
        let unknown: String = db
            .conn
            .query_row(
                "SELECT completion_provenance FROM mailbox WHERE handle='pending'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unknown, "unclassified");
        assert!(classify_one_pending_completion(&mut db.conn, "receiver", None).unwrap());
        db.conn
            .execute(
                "DELETE FROM completion_continuation_source WHERE event_id='pending'",
                [],
            )
            .unwrap();
        let v2: String = db
            .conn
            .query_row(
                "SELECT completion_provenance FROM mailbox WHERE handle='pending'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(v2, "v2");
        assert!(!classify_one_pending_completion(&mut db.conn, "receiver", None).unwrap());
    }

    #[test]
    fn listener_replay_preserves_lazily_classified_provenance() {
        let directory = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&directory.path().join("pid-identity.db")).unwrap();
        for (event, payload, expected) in [
            (
                "old-legacy",
                r#"{"schema_version":2,"kind":"agent_bash_complete","meta":{}}"#,
                "legacy",
            ),
            (
                "old-v2",
                r#"{"completion_protocol":"completion-continuation-v2"}"#,
                "v2",
            ),
        ] {
            db.register_completion_event(CompletionEventRegistrationInput {
                event_id: event,
                delivery_mode: "async",
                owner_session_id: Some("receiver"),
                owner_invocation_uuid: Some("listener"),
                state_dir: "/private/source",
                meta_path: "/private/source/meta.json",
                log_path: "/private/source/log",
                rc_path: "/private/source/rc",
            })
            .unwrap();
            let input = CompletionEventTriggerInput {
                event_id: event,
                payload_json: payload,
                state_dir: "/private/source",
                meta_path: "/private/source/meta.json",
                log_path: "/private/source/log",
                rc_path: "/private/source/rc",
                rc: 0,
            };
            db.trigger_completion_event(input).unwrap();
            assert!(classify_one_pending_completion(&mut db.conn, "receiver", None).unwrap());
            db.trigger_completion_event(input).unwrap();
            let marker: String = db
                .conn
                .query_row(
                    "SELECT completion_provenance FROM mailbox WHERE handle=?1",
                    [event],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(marker, expected);
        }
    }

    #[test]
    fn unclassified_old_completion_recovers_only_from_verified_payload() {
        let directory = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&directory.path().join("pid-identity.db")).unwrap();
        let domain = db.completion_continuation_domain().unwrap().unwrap();
        for (handle, payload) in [
            (
                "template-legacy",
                r#"{"schema_version":2,"kind":"agent_bash_complete","meta":{}}"#,
            ),
            (
                "template-v2",
                r#"{"completion_protocol":"completion-continuation-v2"}"#,
            ),
        ] {
            db.enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
                session_id: "receiver",
                handle,
                payload_json: payload,
                owner_invocation_uuid: None,
                matched_os_pid: None,
                matched_os_boot_id: None,
                matched_os_pid_starttime_ticks: None,
                matched_chain_index: None,
                state_dir: "/private/source",
                meta_path: "/private/source/meta.json",
                log_path: "/private/source/log",
                rc_path: "/private/source/rc",
                rc: 0,
            })
            .unwrap();
        }
        for handle in ["pending-legacy", "pending-v2"] {
            db.conn
                .execute(
                    "INSERT INTO mailbox(session_id,kind,handle,payload_json,enqueued_at,
                    state_dir,meta_path,log_path,rc_path,rc)
                 VALUES('receiver','agent_bash_complete',?1,'{\"schema_version\":1}',
                    '2026-09-23T00:00:00Z','/private/source','/private/source/meta.json',
                    '/private/source/log','/private/source/rc',0)",
                    [handle],
                )
                .unwrap();
        }
        let legacy_seq: i64 = db
            .conn
            .query_row(
                "SELECT seq FROM mailbox WHERE handle='pending-legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let v2_seq: i64 = db
            .conn
            .query_row(
                "SELECT seq FROM mailbox WHERE handle='pending-v2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO session_wake_claim(session_id,claim_token,claimed_at,reason,
                auto_wake_count,min_pending_seq_at_claim,max_pending_seq_at_claim)
             VALUES('receiver','claim','2026-09-23T00:00:00Z','notify_idle',1,?1,?1)",
                [legacy_seq],
            )
            .unwrap();
        let tx = db.conn.transaction().unwrap();
        assert!(
            pending_notification_source_on(&tx, &domain, "receiver", "claim")
                .unwrap_err()
                .contains("provenance unclassified")
        );
        tx.execute(
            "UPDATE mailbox SET
                payload_file_path=(SELECT payload_file_path FROM mailbox WHERE handle='template-legacy'),
                payload_sha256=(SELECT payload_sha256 FROM mailbox WHERE handle='template-legacy'),
                payload_byte_len=(SELECT payload_byte_len FROM mailbox WHERE handle='template-legacy'),
                payload_retention_policy=(SELECT payload_retention_policy FROM mailbox WHERE handle='template-legacy')
             WHERE handle='pending-legacy'", [],
        ).unwrap();
        tx.commit().unwrap();
        assert!(classify_one_pending_completion(&mut db.conn, "receiver", Some("claim")).unwrap());
        let tx = db.conn.transaction().unwrap();
        assert_eq!(
            pending_notification_source_on(&tx, &domain, "receiver", "claim").unwrap(),
            None
        );
        let legacy_provenance: String = tx
            .query_row(
                "SELECT completion_provenance FROM mailbox WHERE handle='pending-legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(legacy_provenance, "legacy");
        tx.execute(
            "UPDATE session_wake_claim SET min_pending_seq_at_claim=?1,
                max_pending_seq_at_claim=?1 WHERE session_id='receiver'",
            [v2_seq],
        )
        .unwrap();
        assert!(
            pending_notification_source_on(&tx, &domain, "receiver", "claim")
                .unwrap_err()
                .contains("provenance unclassified")
        );
        tx.execute(
            "UPDATE mailbox SET
                payload_file_path=(SELECT payload_file_path FROM mailbox WHERE handle='template-v2'),
                payload_sha256=(SELECT payload_sha256 FROM mailbox WHERE handle='template-v2'),
                payload_byte_len=(SELECT payload_byte_len FROM mailbox WHERE handle='template-v2'),
                payload_retention_policy=(SELECT payload_retention_policy FROM mailbox WHERE handle='template-v2')
             WHERE handle='pending-v2'", [],
        ).unwrap();
        tx.commit().unwrap();
        assert!(classify_one_pending_completion(&mut db.conn, "receiver", Some("claim")).unwrap());
        let tx = db.conn.transaction().unwrap();
        assert!(
            pending_notification_source_on(&tx, &domain, "receiver", "claim")
                .unwrap_err()
                .contains("lacks exact accepted source authority")
        );
        let v2_provenance: String = tx
            .query_row(
                "SELECT completion_provenance FROM mailbox WHERE handle='pending-v2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(v2_provenance, "v2");
        assert!(wake_claim_tx(&tx, "receiver").unwrap().is_some());
        tx.commit().unwrap();
    }

    #[test]
    fn one_claim_retains_every_accepted_source_through_ack_and_drain() {
        let directory = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&directory.path().join("pid-identity.db")).unwrap();
        let domain = db.completion_continuation_domain().unwrap().unwrap();
        let listener = "11111111-1111-4111-8111-111111111111";
        let mut seqs = Vec::new();
        for event in ["first", "legacy", "second"] {
            db.register_completion_event(CompletionEventRegistrationInput {
                event_id: event,
                delivery_mode: "async",
                owner_session_id: Some("receiver"),
                owner_invocation_uuid: Some(listener),
                state_dir: "/private/source",
                meta_path: "/private/source/meta.json",
                log_path: "/private/source/log",
                rc_path: "/private/source/rc",
            })
            .unwrap();
            let triggered = db.trigger_completion_event(CompletionEventTriggerInput {
                event_id: event,
                payload_json: if event == "legacy" {
                    r#"{"schema_version":2,"kind":"agent_bash_complete","meta":{}}"#
                } else {
                    r#"{"kind":"agent_bash_complete","completion_protocol":"completion-continuation-v2"}"#
                },
                state_dir: "/private/source",
                meta_path: "/private/source/meta.json",
                log_path: "/private/source/log",
                rc_path: "/private/source/rc",
                rc: 0,
            }).unwrap();
            seqs.push((
                event,
                triggered.mailbox_rows[0].seq,
                triggered.event.payload_sha256.unwrap(),
            ));
        }
        for (event, _, digest) in [&seqs[0], &seqs[2]] {
            db.conn
                .execute(
                    "INSERT INTO completion_continuation_source
                 (registration_id,domain_id,source_id,event_id,registration_digest,binding,phase,
                  snapshot_sha256,outcome_sha256,payload_sha256,payload_byte_len)
                 VALUES(?1,?2,?1,?1,'digest',x'00','accepted','snapshot','outcome',?3,1)",
                    params![event, domain, digest],
                )
                .unwrap();
            db.conn
                .execute(
                    "INSERT INTO completion_continuation_notification
                 (event_id,listener_id,policy,policy_origin,policy_recorded_at)
                 VALUES(?1,?2,'notify',?3,'2026-01-01T00:00:00Z')",
                    params![event, listener, format!("exact_admitted_binding:{event}")],
                )
                .unwrap();
        }
        for _ in 0..3 {
            assert!(classify_one_pending_completion(&mut db.conn, "receiver", None).unwrap());
        }
        db.conn
            .execute(
                "INSERT INTO session_wake_claim
             (session_id,claim_token,claimed_at,reason,auto_wake_count,
              min_pending_seq_at_claim,max_pending_seq_at_claim)
             VALUES('receiver','shared-token','2026-01-01T00:00:00Z','notify_idle',1,?1,?2)",
                params![seqs[0].1, seqs[2].1],
            )
            .unwrap();
        let live = crate::pid_identity::read_live_process_identity(i64::from(std::process::id()))
            .unwrap()
            .unwrap();
        let identity = SourceProcessIdentity {
            pid: live.os_pid,
            boot_id: live.os_boot_id,
            starttime_ticks: live.os_pid_starttime_ticks,
        };
        let owner = CompletionDomainOwner {
            protocol: PROTOCOL.into(),
            domain_id: domain.clone(),
            supervisor_authority_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: uuid::Uuid::new_v4().to_string(),
            guardian_identity: identity.clone(),
            driver_identity: identity.clone(),
            endpoint: "/private/owner".into(),
        };
        db.publish_completion_continuation_owner(&owner).unwrap();
        let attempt = ContinuationAttempt {
            attempt_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: owner.owner_generation.clone(),
            operation: "activation".into(),
            request_sha256: "0".repeat(64),
            source_registration_id: Some("first".into()),
            source_listener_revision: Some(1),
            session_id: Some("receiver".into()),
            claim_token: Some("shared-token".into()),
            result_path: "/private/result.json".into(),
        };
        let changes_before = db.conn.total_changes();
        db.reserve_continuation_attempt(&attempt).unwrap();
        assert_eq!(
            db.conn.total_changes() - changes_before,
            3,
            "one attempt row and one retained association per accepted v2 source"
        );
        for source in ["first", "second"] {
            assert_eq!(
                db.pending_continuation_attempt_ids(source).unwrap(),
                vec![attempt.attempt_id.clone()]
            );
            assert_eq!(
                db.continuation_attempt_association_completeness(source)
                    .unwrap(),
                "known"
            );
            let recovery = db.completion_recovery_attempts(source).unwrap();
            assert_eq!(recovery.len(), 1);
            assert_eq!(recovery[0]["phase"], "reserved");
            assert_eq!(recovery[0]["association_completeness"], "known");
        }
        assert_eq!(
            db.pending_continuation_attempts_for_supervisor(&owner.supervisor_authority_id, 8)
                .unwrap()
                .len(),
            1
        );
        db.mark_delivered(
            "receiver",
            None,
            &[seqs[0].1, seqs[1].1, seqs[2].1],
            "receiver-invocation",
        )
        .unwrap();
        for source in ["first", "second"] {
            assert_eq!(
                db.pending_continuation_attempt_ids(source).unwrap(),
                vec![attempt.attempt_id.clone()]
            );
        }
        assert!(wake_claim(&db.conn, "receiver").unwrap().is_some());
        let other_retirement_debt: (i64, i64, i64) = db
            .conn
            .query_row(
                "SELECT
                (SELECT COUNT(*) FROM completion_continuation_source WHERE phase='registered'),
                (SELECT COUNT(*) FROM completion_event_listener WHERE retirement_pending=1),
                (SELECT COUNT(*) FROM mailbox WHERE delivered_at IS NULL)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(other_retirement_debt, (0, 0, 0));
        assert!(
            !db.close_idle_continuation_generation(&owner, None).unwrap(),
            "ACK of the whole mixed claim cannot retire unresolved physical custody"
        );
        db.accept_continuation_attempt(&attempt).unwrap();
        db.attach_continuation_custodian(&attempt, &identity)
            .unwrap();
        db.advance_continuation_attempt(&attempt, 3, "accepted", "starting", &identity)
            .unwrap();
        let receipt = "private fixture original custodian drain";
        db.discharge_continuation_attempt(&attempt, &identity, receipt)
            .unwrap();
        assert!(wake_claim(&db.conn, "receiver").unwrap().is_none());
        assert!(db.close_idle_continuation_generation(&owner, None).unwrap());
        for source in ["first", "second"] {
            assert!(
                db.pending_continuation_attempt_ids(source)
                    .unwrap()
                    .is_empty()
            );
            let recovery = db.completion_recovery_attempts(source).unwrap();
            assert_eq!(recovery.len(), 1);
            assert_eq!(recovery[0]["phase"], "drained");
            assert_eq!(recovery[0]["drain_receipt"], receipt);
            assert_eq!(recovery[0]["association_completeness"], "known");
        }

        // A pre-v25 activation can have lost its claim. The scalar identifies
        // one source, but cannot prove the second was absent from that claim.
        db.conn
            .execute(
                "INSERT INTO completion_continuation_attempt
             (attempt_id,domain_id,owner_generation,operation,request_sha256,
              source_registration_id,session_id,claim_token,phase,result_path,
              supervisor_authority_id,integrated,drain_receipt)
             VALUES('historical',?1,?2,'activation',?3,'first','receiver',
                    'deleted-claim','drained','/private/old-result',?4,1,'old-receipt')",
                params![
                    domain,
                    owner.owner_generation,
                    "0".repeat(64),
                    owner.supervisor_authority_id
                ],
            )
            .unwrap();
        for source in ["first", "second"] {
            assert_eq!(
                db.continuation_attempt_association_completeness(source)
                    .unwrap(),
                "unknown"
            );
        }
        assert_eq!(db.completion_recovery_attempts("first").unwrap().len(), 2);
        assert_eq!(db.completion_recovery_attempts("second").unwrap().len(), 1);
    }

    #[test]
    fn activation_source_is_the_accepted_pending_row_in_the_exact_claim() {
        let directory = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&directory.path().join("pid-identity.db")).unwrap();
        let domain = db.completion_continuation_domain().unwrap().unwrap();
        let listener = "11111111-1111-4111-8111-111111111111";
        for event in [
            "a_registered",
            "b_accepted",
            "c_later",
            "d_legacy",
            "e_accepted",
        ] {
            db.register_completion_event(CompletionEventRegistrationInput {
                event_id: event,
                delivery_mode: "async",
                owner_session_id: Some("receiver"),
                owner_invocation_uuid: Some(listener),
                state_dir: "/private/source",
                meta_path: "/private/source/meta.json",
                log_path: "/private/source/log",
                rc_path: "/private/source/rc",
            })
            .unwrap();
        }
        let mut accepted = Vec::new();
        for event in [
            "a_registered",
            "b_accepted",
            "c_later",
            "d_legacy",
            "e_accepted",
        ] {
            let triggered = db
                .trigger_completion_event(CompletionEventTriggerInput {
                    event_id: event,
                    payload_json: if event == "d_legacy" {
                        r#"{"schema_version":2,"kind":"agent_bash_complete","meta":{}}"#
                    } else {
                        r#"{"kind":"agent_bash_complete","completion_protocol":"completion-continuation-v2"}"#
                    },
                    state_dir: "/private/source",
                    meta_path: "/private/source/meta.json",
                    log_path: "/private/source/log",
                    rc_path: "/private/source/rc",
                    rc: 0,
                })
                .unwrap();
            accepted.push((
                event,
                triggered.mailbox_rows[0].seq,
                triggered.event.payload_sha256.unwrap(),
            ));
        }
        for (event, phase, digest) in [
            ("a_registered", "registered", None),
            ("b_accepted", "accepted", Some(accepted[1].2.as_str())),
            ("c_later", "accepted", Some(accepted[2].2.as_str())),
            ("e_accepted", "accepted", Some(accepted[4].2.as_str())),
        ] {
            db.conn
                .execute(
                    "INSERT INTO completion_continuation_source
                 (registration_id,domain_id,source_id,event_id,registration_digest,binding,phase,
                  snapshot_sha256,outcome_sha256,payload_sha256,payload_byte_len)
                 VALUES(?1,?2,?1,?3,'digest',x'00',?4,?5,?5,?6,?7)",
                    params![
                        event,
                        domain,
                        event,
                        phase,
                        digest.map(|_| "snapshot"),
                        digest,
                        digest.map(|_| 1)
                    ],
                )
                .unwrap();
            db.conn
                .execute(
                    "INSERT INTO completion_continuation_notification
                 (event_id,listener_id,policy,policy_origin,policy_recorded_at)
                 VALUES(?1,?2,'notify',?3,'2026-01-01T00:00:00Z')",
                    params![event, listener, format!("exact_admitted_binding:{event}")],
                )
                .unwrap();
        }
        for _ in 0..5 {
            assert!(classify_one_pending_completion(&mut db.conn, "receiver", None).unwrap());
        }
        let claimed_seq = accepted[1].1;
        db.conn
            .execute(
                "INSERT INTO session_wake_claim
             (session_id,claim_token,claimed_at,reason,auto_wake_count,
              min_pending_seq_at_claim,max_pending_seq_at_claim)
             VALUES('receiver','exact-token','2026-01-01T00:00:00Z','notify_idle',1,?1,?1)",
                [claimed_seq],
            )
            .unwrap();
        let tx = db.conn.transaction().unwrap();
        assert_eq!(
            pending_notification_source_on(&tx, &domain, "receiver", "exact-token")
                .unwrap()
                .as_deref(),
            Some("b_accepted")
        );
        assert_eq!(
            pending_notification_source_on(&tx, &domain, "receiver", "wrong-token").unwrap(),
            None
        );
        tx.commit().unwrap();

        let live = crate::pid_identity::read_live_process_identity(i64::from(std::process::id()))
            .unwrap()
            .unwrap();
        let identity = SourceProcessIdentity {
            pid: live.os_pid,
            boot_id: live.os_boot_id,
            starttime_ticks: live.os_pid_starttime_ticks,
        };
        let owner = CompletionDomainOwner {
            protocol: PROTOCOL.into(),
            domain_id: domain.clone(),
            supervisor_authority_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: uuid::Uuid::new_v4().to_string(),
            guardian_identity: identity.clone(),
            driver_identity: identity,
            endpoint: "/private/owner".into(),
        };
        db.publish_completion_continuation_owner(&owner).unwrap();
        let mut attempt = ContinuationAttempt {
            attempt_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: owner.owner_generation,
            operation: "activation".into(),
            request_sha256: "0".repeat(64),
            source_registration_id: None,
            source_listener_revision: None,
            session_id: Some("receiver".into()),
            claim_token: Some("exact-token".into()),
            result_path: "/private/result.json".into(),
        };
        // With no earlier activation attempt to trip the one-live fence, the
        // pre-provenance query returned None and admitted this generic proposal.
        let total_loss = db.conn.transaction().unwrap();
        total_loss
            .execute(
                "UPDATE session_wake_claim SET min_pending_seq_at_claim=?1,
                max_pending_seq_at_claim=?1
             WHERE session_id='receiver' AND claim_token='exact-token'",
                [accepted[4].1],
            )
            .unwrap();
        total_loss
            .execute(
                "DELETE FROM completion_continuation_notification WHERE event_id='e_accepted'",
                [],
            )
            .unwrap();
        total_loss
            .execute(
                "DELETE FROM completion_continuation_source WHERE event_id='e_accepted'",
                [],
            )
            .unwrap();
        total_loss
            .execute(
                "DELETE FROM completion_event_listener WHERE event_id='e_accepted'",
                [],
            )
            .unwrap();
        assert!(
            pending_notification_source_on(&total_loss, &domain, "receiver", "exact-token")
                .unwrap_err()
                .contains("lacks exact accepted source authority")
        );
        assert!(
            reserve_on(&total_loss, &attempt)
                .unwrap_err()
                .contains("lacks exact accepted source authority")
        );
        assert!(wake_claim_tx(&total_loss, "receiver").unwrap().is_some());
        total_loss.rollback().unwrap();
        db.conn.execute(
            "UPDATE session_wake_claim SET min_pending_seq_at_claim=?1,max_pending_seq_at_claim=?1
             WHERE session_id='receiver' AND claim_token='exact-token'",
            [accepted[3].1],
        ).unwrap();
        let legacy_tx = db.conn.transaction().unwrap();
        reserve_on(&legacy_tx, &attempt).unwrap();
        legacy_tx.rollback().unwrap();
        db.conn.execute(
            "UPDATE session_wake_claim SET min_pending_seq_at_claim=?1,max_pending_seq_at_claim=?1
             WHERE session_id='receiver' AND claim_token='exact-token'",
            [accepted[1].1],
        ).unwrap();
        assert!(
            db.reserve_continuation_attempt(&attempt)
                .unwrap_err()
                .contains("source differs from exact pending notification")
        );
        attempt.source_registration_id = Some("b_accepted".into());
        attempt.source_listener_revision = Some(1);
        db.reserve_continuation_attempt(&attempt).unwrap();

        db.mark_delivered("receiver", None, &[claimed_seq], "receiver-invocation")
            .unwrap();
        let tx = db.conn.transaction().unwrap();
        assert_eq!(
            pending_notification_source_on(&tx, &domain, "receiver", "exact-token").unwrap(),
            None,
            "recipient acknowledgement cannot authorize another activation"
        );
        let phase: String = tx
            .query_row(
                "SELECT phase FROM completion_continuation_attempt WHERE attempt_id=?1",
                [&attempt.attempt_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(phase, "reserved", "ACK cannot settle physical custody");
        assert!(wake_claim_tx(&tx, "receiver").unwrap().is_some());
        tx.commit().unwrap();
        db.conn.execute(
            "UPDATE session_wake_claim SET min_pending_seq_at_claim=?1,max_pending_seq_at_claim=?1
             WHERE session_id='receiver' AND claim_token='exact-token'",
            [accepted[0].1],
        ).unwrap();
        let tx = db.conn.transaction().unwrap();
        assert!(
            pending_notification_source_on(&tx, &domain, "receiver", "exact-token")
                .unwrap_err()
                .contains("lacks exact accepted source authority"),
            "registered-only source must not be laundered through a generic wake"
        );
        tx.execute(
            "UPDATE session_wake_claim SET min_pending_seq_at_claim=?1,max_pending_seq_at_claim=?1
             WHERE session_id='receiver' AND claim_token='exact-token'",
            [accepted[3].1],
        )
        .unwrap();
        assert_eq!(
            pending_notification_source_on(&tx, &domain, "receiver", "exact-token").unwrap(),
            None,
            "a legacy completion with no v2 source or classified policy keeps generic wake"
        );
        tx.execute(
            "UPDATE session_wake_claim SET min_pending_seq_at_claim=?1,max_pending_seq_at_claim=?1
             WHERE session_id='receiver' AND claim_token='exact-token'",
            [accepted[2].1],
        )
        .unwrap();
        tx.execute(
            "DELETE FROM completion_continuation_source WHERE event_id='c_later'",
            [],
        )
        .unwrap();
        assert!(
            pending_notification_source_on(&tx, &domain, "receiver", "exact-token")
                .unwrap_err()
                .contains("lacks exact accepted source authority"),
            "a classified v2 notification without its source must retain wake debt"
        );
        let mut forged_generic = attempt.clone();
        forged_generic.attempt_id = uuid::Uuid::new_v4().to_string();
        forged_generic.source_registration_id = None;
        forged_generic.source_listener_revision = None;
        assert!(
            reserve_on(&tx, &forged_generic)
                .unwrap_err()
                .contains("lacks exact accepted source authority"),
            "missing source must reject a source-free reservation"
        );
        assert!(wake_claim_tx(&tx, "receiver").unwrap().is_some());
        tx.execute(
            "UPDATE session_wake_claim SET min_pending_seq_at_claim=?1,max_pending_seq_at_claim=?1
             WHERE session_id='receiver' AND claim_token='exact-token'",
            [accepted[4].1],
        )
        .unwrap();
        tx.execute(
            "UPDATE completion_event_listener SET mailbox_seq=NULL WHERE event_id='e_accepted'",
            [],
        )
        .unwrap();
        assert!(
            pending_notification_source_on(&tx, &domain, "receiver", "exact-token")
                .unwrap_err()
                .contains("lacks exact accepted source authority"),
            "a v2 source without its listener mailbox projection must retain wake debt"
        );
        assert!(
            reserve_on(&tx, &forged_generic)
                .unwrap_err()
                .contains("lacks exact accepted source authority"),
            "missing listener projection must reject a source-free reservation"
        );
        assert!(wake_claim_tx(&tx, "receiver").unwrap().is_some());
        tx.execute(
            "UPDATE completion_event_listener SET mailbox_seq=?1 WHERE event_id='e_accepted'",
            [accepted[4].1],
        )
        .unwrap();
        tx.execute(
            "DELETE FROM completion_continuation_notification WHERE event_id='e_accepted'",
            [],
        )
        .unwrap();
        assert!(
            reserve_on(&tx, &forged_generic)
                .unwrap_err()
                .contains("lacks exact accepted source authority"),
            "missing notification policy must reject a source-free reservation"
        );
        assert!(wake_claim_tx(&tx, "receiver").unwrap().is_some());
        tx.execute(
            "INSERT INTO completion_continuation_notification
             (event_id,listener_id,policy,policy_origin,policy_recorded_at)
             VALUES('e_accepted',?1,'notify','exact_admitted_binding:wrong',
                    '2026-01-01T00:00:00Z')",
            [listener],
        )
        .unwrap();
        assert!(
            reserve_on(&tx, &forged_generic)
                .unwrap_err()
                .contains("lacks exact accepted source authority"),
            "a policy classified for another registration cannot authorize activation"
        );
        assert!(wake_claim_tx(&tx, "receiver").unwrap().is_some());
        tx.execute(
            "DELETE FROM completion_continuation_notification WHERE event_id='e_accepted'",
            [],
        )
        .unwrap();
        tx.execute(
            "DELETE FROM completion_continuation_source WHERE event_id='e_accepted'",
            [],
        )
        .unwrap();
        tx.execute(
            "DELETE FROM completion_event_listener WHERE event_id='e_accepted'",
            [],
        )
        .unwrap();
        assert!(
            pending_notification_source_on(&tx, &domain, "receiver", "exact-token")
                .unwrap_err()
                .contains("lacks exact accepted source authority"),
            "total projection loss must not turn a pending v2 row into a generic wake"
        );
        assert!(
            reserve_on(&tx, &forged_generic)
                .unwrap_err()
                .contains("lacks exact accepted source authority"),
            "total projection loss must retain the claim and reject generic activation"
        );
        assert!(wake_claim_tx(&tx, "receiver").unwrap().is_some());
        tx.rollback().unwrap();
        db.accept_continuation_attempt(&attempt).unwrap();
        db.record_continuation_never_forked(&attempt, "fixture-no-worker")
            .unwrap();
        assert!(
            db.wake_session_reader()
                .wake_claim("receiver")
                .unwrap()
                .is_none()
        );
        let provenance_after_drain: String = db
            .conn
            .query_row(
                "SELECT completion_provenance FROM mailbox WHERE seq=?1",
                [accepted[1].1],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(provenance_after_drain, "v2");
    }
}
