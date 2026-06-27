//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `parser`,
//! `predicate`, `validator`
//!
//! Resume-backed notification mailbox storage in the PID identity sidecar DB.
//! This module deliberately owns only additive sidecar tables and never touches
//! the versioned `state.db` schema.

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::pid_identity::{self, ProcessIdentity};

pub const AGENT_BASH_COMPLETE_KIND: &str = "agent_bash_complete";
pub const WAKE_SWEEP_ABANDONED_ERROR: &str = "wake_sweep_abandoned";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MailboxRow {
    pub seq: i64,
    pub session_id: String,
    pub kind: String,
    pub handle: String,
    pub payload_json: String,
    pub enqueued_at: String,
    pub delivered_at: Option<String>,
    pub delivered_by_invocation_uuid: Option<String>,
    pub delivery_attempts: i64,
    pub delivery_error: Option<String>,
    pub owner_invocation_uuid: Option<String>,
    pub matched_os_pid: Option<i64>,
    pub matched_os_boot_id: Option<String>,
    pub matched_os_pid_starttime_ticks: Option<i64>,
    pub matched_chain_index: Option<i64>,
    pub state_dir: String,
    pub meta_path: String,
    pub log_path: String,
    pub rc_path: String,
    pub rc: i32,
}

#[derive(Debug, Clone, Copy)]
pub struct AgentBashCompleteEnqueue<'a> {
    pub session_id: &'a str,
    pub handle: &'a str,
    pub payload_json: &'a str,
    pub owner_invocation_uuid: Option<&'a str>,
    pub matched_os_pid: Option<i64>,
    pub matched_os_boot_id: Option<&'a str>,
    pub matched_os_pid_starttime_ticks: Option<i64>,
    pub matched_chain_index: Option<i64>,
    pub state_dir: &'a str,
    pub meta_path: &'a str,
    pub log_path: &'a str,
    pub rc_path: &'a str,
    pub rc: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnqueueResult {
    Inserted(MailboxRow),
    AlreadyEnqueued(MailboxRow),
    Conflict { existing: MailboxRow },
}

#[derive(Debug, Clone, Copy)]
pub struct SessionRuntimeUpsert<'a> {
    pub session_id: &'a str,
    pub mode: &'a str,
    pub invocation_uuid: Option<&'a str>,
    pub provider_name: Option<&'a str>,
    pub model_name: Option<&'a str>,
    pub pty_control_path: Option<&'a str>,
    pub models_dir: Option<&'a str>,
    pub effective_cwd: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
pub struct SessionRuntimeRunningUpdate<'a> {
    pub session_id: &'a str,
    pub mode: &'a str,
    pub invocation_uuid: &'a str,
    pub provider_name: Option<&'a str>,
    pub model_name: Option<&'a str>,
    pub identity: &'a ProcessIdentity,
    pub pty_control_path: Option<&'a str>,
    pub turn_start_max_mailbox_seq: Option<i64>,
    pub models_dir: Option<&'a str>,
    pub effective_cwd: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
pub struct SessionRuntimeIdleUpdate<'a> {
    pub session_id: &'a str,
    pub invocation_uuid: &'a str,
    pub last_exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRuntimeRow {
    pub session_id: String,
    pub mode: String,
    pub invocation_uuid: Option<String>,
    pub provider_name: Option<String>,
    pub model_name: Option<String>,
    pub pty_control_path: Option<String>,
    pub updated_at: String,
    pub run_state: String,
    pub running_invocation_uuid: Option<String>,
    pub running_os_pid: Option<i64>,
    pub running_os_boot_id: Option<String>,
    pub running_os_pid_starttime_ticks: Option<i64>,
    pub turn_started_at: Option<String>,
    pub turn_ended_at: Option<String>,
    pub turn_start_max_mailbox_seq: Option<i64>,
    pub last_exit_code: Option<i32>,
    pub models_dir: Option<String>,
    pub effective_cwd: Option<String>,
    pub auto_wake_count: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionLiveness {
    Busy,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRuntimeReadOnlyLiveness {
    Busy,
    Idle,
    StaleMissingInvocation,
    StaleMissingIdentity,
    StaleDead,
    StalePidReused,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SessionRuntimeLivenessDecision {
    Busy,
    Idle,
    Stale {
        running_invocation_uuid: Option<String>,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct WakeClaimRequest<'a> {
    pub session_id: &'a str,
    pub claim_token: &'a str,
    pub reason: &'a str,
    pub auto_wake_count: i64,
    pub wake_invocation_uuid: Option<&'a str>,
    pub stale_after_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeClaimAcquireResult {
    Acquired(WakeClaimRow),
    NoPending,
    Busy,
    AlreadyInFlight(WakeClaimRow),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeClaimRow {
    pub session_id: String,
    pub claim_token: String,
    pub claimed_at: String,
    pub wake_pid: Option<i64>,
    pub wake_invocation_uuid: Option<String>,
    pub reason: String,
    pub auto_wake_count: i64,
    pub min_pending_seq_at_claim: Option<i64>,
    pub max_pending_seq_at_claim: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeSweepCandidate {
    pub session_id: String,
    pub auto_wake_count: i64,
    pub min_pending_seq: i64,
    pub max_pending_seq: i64,
}

struct WakeSweepSessionState {
    session_id: String,
    min_pending_seq: i64,
    max_pending_seq: i64,
    claim: Option<WakeClaimRow>,
}

pub struct MailboxDb {
    conn: Connection,
    path: PathBuf,
}

enum BoundedMailboxRowsError {
    Prepare(rusqlite::Error),
    Query(rusqlite::Error),
    Row(rusqlite::Error),
}

impl MailboxDb {
    pub fn default_path() -> Result<PathBuf, String> {
        pid_identity::default_path()
    }

    pub fn open_default() -> Result<Self, String> {
        let path = Self::default_path()?;
        Self::open(&path)
    }

    pub fn open_default_if_exists() -> Result<Option<Self>, String> {
        let path = Self::default_path()?;
        if !path.exists() {
            return Ok(None);
        }
        Self::open(&path).map(Some)
    }

    pub fn open(path: &Path) -> Result<Self, String> {
        ensure_parent_dir(path)?;
        let conn = Connection::open(path)
            .map_err(|err| format!("Failed to open PID mailbox sidecar: {err}"))?;
        set_wal_mode(&conn)?;
        pid_identity::ensure_identity_schema(&conn)?;
        ensure_mailbox_schema(&conn)?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
    }

    pub fn open_read_only(path: &Path) -> Result<Self, String> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|err| format!("Failed to open PID mailbox sidecar read-only: {err}"))?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    pub fn enqueue_agent_bash_complete(
        &mut self,
        input: &AgentBashCompleteEnqueue<'_>,
    ) -> Result<EnqueueResult, String> {
        let now = now_rfc3339();
        let tx = self
            .conn
            .transaction()
            .map_err(|err| format!("Failed to start mailbox enqueue transaction: {err}"))?;
        let result = enqueue_agent_bash_complete_in_tx(&tx, input, &now)?;
        tx.commit()
            .map_err(|err| format!("Failed to commit mailbox enqueue transaction: {err}"))?;
        Ok(result)
    }

    pub fn list_pending(&self, session_id: &str) -> Result<Vec<MailboxRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT seq, session_id, kind, handle, payload_json, enqueued_at,
                        delivered_at, delivered_by_invocation_uuid, delivery_attempts,
                        delivery_error, owner_invocation_uuid, matched_os_pid,
                        matched_os_boot_id, matched_os_pid_starttime_ticks,
                        matched_chain_index, state_dir, meta_path, log_path, rc_path, rc
                 FROM mailbox
                 WHERE session_id = ?1 AND delivered_at IS NULL
                 ORDER BY seq ASC",
            )
            .map_err(|err| format!("Failed to prepare pending mailbox query: {err}"))?;
        let rows = stmt
            .query_map(params![session_id], map_mailbox_row)
            .map_err(|err| format!("Failed to query pending mailbox rows: {err}"))?;
        collect_rows(rows)
    }

    pub fn list_mailbox(&self, session_id: &str, all: bool) -> Result<Vec<MailboxRow>, String> {
        if all {
            self.list_mailbox_all(session_id)
        } else {
            self.list_pending(session_id)
        }
    }

    pub fn list_mailbox_bounded(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<MailboxRow>, String> {
        if bounded_mailbox_limit_is_zero(limit) {
            return Ok(Vec::new());
        }
        self.bounded_mailbox_rows(session_id, bounded_mailbox_sql_limit(limit))
            .map_err(format_bounded_mailbox_rows_error)
    }

    fn bounded_mailbox_rows(
        &self,
        session_id: &str,
        limit: i64,
    ) -> Result<Vec<MailboxRow>, BoundedMailboxRowsError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT seq, session_id, kind, handle, payload_json, enqueued_at,
                        delivered_at, delivered_by_invocation_uuid, delivery_attempts,
                        delivery_error, owner_invocation_uuid, matched_os_pid,
                        matched_os_boot_id, matched_os_pid_starttime_ticks,
                        matched_chain_index, state_dir, meta_path, log_path, rc_path, rc
                 FROM mailbox
                 WHERE session_id = ?1
                 ORDER BY CASE WHEN delivered_at IS NULL THEN 0 ELSE 1 END, seq DESC
                  LIMIT ?2",
            )
            .map_err(BoundedMailboxRowsError::Prepare)?;
        let rows = stmt
            .query_map(params![session_id, limit], map_mailbox_row)
            .map_err(BoundedMailboxRowsError::Query)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(BoundedMailboxRowsError::Row)
    }

    pub fn mark_delivered(
        &mut self,
        session_id: &str,
        seqs: &[i64],
        delivered_by_invocation_uuid: &str,
    ) -> Result<(), String> {
        if seqs.is_empty() {
            return Ok(());
        }
        let now = now_rfc3339();
        let tx = self
            .conn
            .transaction()
            .map_err(|err| format!("Failed to start mailbox delivery transaction: {err}"))?;
        for seq in seqs {
            tx.execute(
                "UPDATE mailbox
                 SET delivered_at = ?3,
                     delivered_by_invocation_uuid = ?4,
                     delivery_attempts = delivery_attempts + 1,
                     delivery_error = NULL
                 WHERE session_id = ?1
                   AND seq = ?2
                   AND delivered_at IS NULL",
                params![session_id, seq, &now, delivered_by_invocation_uuid],
            )
            .map_err(|err| format!("Failed to mark mailbox row delivered: {err}"))?;
        }
        tx.commit()
            .map_err(|err| format!("Failed to commit mailbox delivery transaction: {err}"))
    }

    pub fn mark_delivery_failed(
        &mut self,
        session_id: &str,
        seqs: &[i64],
        delivery_error: &str,
    ) -> Result<(), String> {
        if seqs.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction().map_err(|err| {
            format!("Failed to start mailbox delivery failure transaction: {err}")
        })?;
        for seq in seqs {
            tx.execute(
                "UPDATE mailbox
                 SET delivery_attempts = delivery_attempts + 1,
                     delivery_error = ?3
                 WHERE session_id = ?1
                   AND seq = ?2
                   AND delivered_at IS NULL",
                params![session_id, seq, delivery_error],
            )
            .map_err(|err| format!("Failed to mark mailbox row delivery failed: {err}"))?;
        }
        tx.commit()
            .map_err(|err| format!("Failed to commit mailbox delivery failure transaction: {err}"))
    }

    pub fn mark_pending_abandoned(
        &mut self,
        session_id: &str,
        delivery_error: &str,
        limit: usize,
    ) -> Result<usize, String> {
        if limit == 0 {
            return Ok(0);
        }
        let tx = self
            .conn
            .transaction()
            .map_err(|err| format!("Failed to start mailbox abandonment transaction: {err}"))?;
        let changed = tx
            .execute(
                "UPDATE mailbox
                 SET delivery_error = ?2
                 WHERE seq IN (
                    SELECT seq
                    FROM mailbox
                    WHERE session_id = ?1
                      AND delivered_at IS NULL
                      AND (delivery_error IS NULL OR delivery_error != ?2)
                    ORDER BY seq ASC
                    LIMIT ?3
                 )",
                params![session_id, delivery_error, limit as i64],
            )
            .map_err(|err| format!("Failed to mark mailbox rows abandoned: {err}"))?;
        if changed > 0 {
            tx.execute(
                "DELETE FROM session_wake_claim WHERE session_id = ?1",
                params![session_id],
            )
            .map_err(|err| format!("Failed to release abandoned wake claim: {err}"))?;
        }
        tx.commit()
            .map_err(|err| format!("Failed to commit mailbox abandonment transaction: {err}"))?;
        Ok(changed)
    }

    pub fn upsert_session_runtime(
        &mut self,
        input: SessionRuntimeUpsert<'_>,
    ) -> Result<(), String> {
        let now = now_rfc3339();
        self.conn
            .execute(
                "INSERT INTO session_runtime (
                    session_id,
                    mode,
                    invocation_uuid,
                    provider_name,
                    model_name,
                    pty_control_path,
                    updated_at,
                    models_dir,
                    effective_cwd
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(session_id)
                 DO UPDATE SET
                    mode = excluded.mode,
                    invocation_uuid = excluded.invocation_uuid,
                    provider_name = excluded.provider_name,
                    model_name = excluded.model_name,
                    pty_control_path = excluded.pty_control_path,
                    updated_at = excluded.updated_at,
                    models_dir = COALESCE(excluded.models_dir, session_runtime.models_dir),
                    effective_cwd = COALESCE(excluded.effective_cwd, session_runtime.effective_cwd)",
                params![
                    input.session_id,
                    input.mode,
                    input.invocation_uuid,
                    input.provider_name,
                    input.model_name,
                    input.pty_control_path,
                    &now,
                    input.models_dir,
                    input.effective_cwd,
                ],
            )
            .map_err(|err| format!("Failed to upsert session runtime row: {err}"))?;
        Ok(())
    }

    pub fn mark_session_running(
        &mut self,
        input: SessionRuntimeRunningUpdate<'_>,
    ) -> Result<(), String> {
        validate_running_run_state()?;
        let now = now_rfc3339();
        let turn_start_max_mailbox_seq = self.running_turn_start_max_mailbox_seq(&input)?;
        mark_session_running_row(&self.conn, input, &now, turn_start_max_mailbox_seq)
    }

    pub fn mark_session_idle(
        &mut self,
        input: SessionRuntimeIdleUpdate<'_>,
    ) -> Result<bool, String> {
        validate_idle_run_state()?;
        let now = now_rfc3339();
        mark_session_idle_row(&self.conn, input, &now)
    }

    pub fn session_runtime(&self, session_id: &str) -> Result<Option<SessionRuntimeRow>, String> {
        let row = session_runtime_row(&self.conn, session_id)?;
        validate_session_runtime_row(row.as_ref())?;
        Ok(row)
    }

    pub fn session_liveness(&mut self, session_id: &str) -> Result<SessionLiveness, String> {
        let row = self.session_runtime(session_id)?;
        let decision = session_runtime_liveness_decision(row.as_ref())?;
        self.clear_stale_running_row_for_liveness(session_id, &decision)?;
        Ok(session_liveness_from_decision(&decision))
    }

    pub fn classify_session_runtime_read_only(
        &self,
        session_id: &str,
    ) -> Result<SessionRuntimeReadOnlyLiveness, String> {
        let row = self.session_runtime(session_id)?;
        classify_session_runtime_row_read_only(row.as_ref())
    }

    pub fn try_acquire_wake_claim(
        &mut self,
        input: WakeClaimRequest<'_>,
    ) -> Result<WakeClaimAcquireResult, String> {
        self.try_acquire_or_renew_wake_claim(input, None)
    }

    pub fn try_acquire_or_renew_wake_claim(
        &mut self,
        input: WakeClaimRequest<'_>,
        renew_token: Option<&str>,
    ) -> Result<WakeClaimAcquireResult, String> {
        if let Some(result) = self.wake_claim_start_blocker(input.session_id)? {
            return Ok(result);
        }
        let now = now_rfc3339();
        let tx = begin_wake_claim_transaction(&mut self.conn)?;
        let pending_bounds = pending_seq_bounds_for_claim_tx(&tx, input.session_id)?;
        let Some((min_seq, max_seq)) = pending_bounds else {
            commit_empty_wake_claim_transaction(tx)?;
            return Ok(WakeClaimAcquireResult::NoPending);
        };
        if let Some(existing) = fresh_in_flight_wake_claim_for_input(&tx, input, renew_token)? {
            commit_existing_wake_claim_transaction(tx)?;
            return Ok(WakeClaimAcquireResult::AlreadyInFlight(existing));
        }
        let claim = acquire_wake_claim_tx(&tx, input, &now, min_seq, max_seq)?;
        commit_wake_claim_transaction(tx)?;
        Ok(WakeClaimAcquireResult::Acquired(claim))
    }

    fn wake_claim_start_blocker(
        &mut self,
        session_id: &str,
    ) -> Result<Option<WakeClaimAcquireResult>, String> {
        if !self.session_has_pending_mailbox(session_id)? {
            return Ok(Some(WakeClaimAcquireResult::NoPending));
        }
        if self.session_is_busy(session_id)? {
            return Ok(Some(WakeClaimAcquireResult::Busy));
        }
        Ok(None)
    }

    pub fn wake_claim(&self, session_id: &str) -> Result<Option<WakeClaimRow>, String> {
        wake_claim(&self.conn, session_id)
    }

    pub fn release_wake_claim(
        &mut self,
        session_id: &str,
        claim_token: Option<&str>,
    ) -> Result<bool, String> {
        let changed = match claim_token {
            Some(token) => self.conn.execute(
                "DELETE FROM session_wake_claim WHERE session_id = ?1 AND claim_token = ?2",
                params![session_id, token],
            ),
            None => self.conn.execute(
                "DELETE FROM session_wake_claim WHERE session_id = ?1",
                params![session_id],
            ),
        }
        .map_err(|err| format!("Failed to release wake claim: {err}"))?;
        Ok(changed > 0)
    }

    pub fn record_wake_claim_pid(
        &mut self,
        session_id: &str,
        claim_token: &str,
        wake_pid: i64,
    ) -> Result<bool, String> {
        let changed = self
            .conn
            .execute(
                "UPDATE session_wake_claim
                 SET wake_pid = ?3
                 WHERE session_id = ?1
                   AND claim_token = ?2",
                params![session_id, claim_token, wake_pid],
            )
            .map_err(|err| format!("Failed to record wake claim PID: {err}"))?;
        Ok(changed > 0)
    }

    pub fn record_wake_claim_pid_identity(
        &mut self,
        session_id: &str,
        claim_token: &str,
        wake_pid: i64,
        provider_name: Option<&str>,
        model_name: Option<&str>,
    ) -> Result<bool, String> {
        if let Some(identity) = pid_identity::read_live_process_identity(wake_pid)? {
            let recorded_at = now_rfc3339();
            let record = wake_claim_pid_identity_record(
                &identity,
                claim_token,
                session_id,
                provider_name,
                model_name,
                &recorded_at,
            );
            pid_identity::PidIdentityDb::open(self.path())?.record_identity(record)?;
        }
        self.record_wake_claim_pid(session_id, claim_token, wake_pid)
    }

    pub fn wake_sweep_candidates(
        &mut self,
        stale_after_seconds: i64,
        limit: usize,
    ) -> Result<Vec<WakeSweepCandidate>, String> {
        let session_ids = self.pending_wake_session_ids(limit)?;
        self.wake_sweep_candidates_for_sessions(stale_after_seconds, limit, session_ids)
    }

    pub fn pending_delivery_session_ids(&self, limit: usize) -> Result<Vec<String>, String> {
        self.pending_wake_session_ids(limit)
    }

    fn wake_sweep_candidates_for_sessions(
        &mut self,
        stale_after_seconds: i64,
        limit: usize,
        session_ids: Vec<String>,
    ) -> Result<Vec<WakeSweepCandidate>, String> {
        let mut candidates = Vec::new();
        for session_id in session_ids {
            if wake_sweep_candidates_at_limit(&candidates, limit) {
                break;
            }
            self.push_wake_sweep_candidate_for_session(
                &mut candidates,
                session_id,
                stale_after_seconds,
            )?;
        }
        Ok(candidates)
    }

    fn push_wake_sweep_candidate_for_session(
        &mut self,
        candidates: &mut Vec<WakeSweepCandidate>,
        session_id: String,
        stale_after_seconds: i64,
    ) -> Result<(), String> {
        if let Some(candidate) =
            self.wake_sweep_candidate_for_session(session_id, stale_after_seconds)?
        {
            candidates.push(candidate);
        }
        Ok(())
    }

    fn wake_sweep_candidate_for_session(
        &mut self,
        session_id: String,
        stale_after_seconds: i64,
    ) -> Result<Option<WakeSweepCandidate>, String> {
        let Some(state) = self.wake_sweep_session_state(session_id)? else {
            return Ok(None);
        };
        self.wake_sweep_candidate_from_state(state, stale_after_seconds)
    }

    fn wake_sweep_session_state(
        &mut self,
        session_id: String,
    ) -> Result<Option<WakeSweepSessionState>, String> {
        if self.session_is_busy(&session_id)? {
            return Ok(None);
        }
        let Some((min_pending_seq, max_pending_seq)) = self.pending_seq_bounds(&session_id)? else {
            return Ok(None);
        };
        let claim = self.wake_claim(&session_id)?;
        Ok(Some(WakeSweepSessionState {
            session_id,
            min_pending_seq,
            max_pending_seq,
            claim,
        }))
    }

    fn wake_sweep_candidate_from_state(
        &self,
        state: WakeSweepSessionState,
        stale_after_seconds: i64,
    ) -> Result<Option<WakeSweepCandidate>, String> {
        if !self.wake_sweep_state_is_candidate(&state, stale_after_seconds)? {
            return Ok(None);
        }
        Ok(Some(self.wake_sweep_candidate_from_eligible_state(state)?))
    }

    fn wake_sweep_state_is_candidate(
        &self,
        state: &WakeSweepSessionState,
        stale_after_seconds: i64,
    ) -> Result<bool, String> {
        match state.claim.as_ref() {
            Some(claim) => wake_claim_is_reclaimable(&self.conn, claim, stale_after_seconds),
            None => Ok(true),
        }
    }

    fn wake_sweep_candidate_from_eligible_state(
        &self,
        state: WakeSweepSessionState,
    ) -> Result<WakeSweepCandidate, String> {
        Ok(wake_sweep_candidate(
            state.session_id.clone(),
            self.next_auto_wake_count_for_session(&state.session_id, state.claim.as_ref())?,
            state.min_pending_seq,
            state.max_pending_seq,
        ))
    }

    fn next_auto_wake_count_for_session(
        &self,
        session_id: &str,
        claim: Option<&WakeClaimRow>,
    ) -> Result<i64, String> {
        let persisted = self.persisted_auto_wake_count(session_id)?;
        Ok(next_auto_wake_count(
            persisted,
            claim_auto_wake_count(claim),
        ))
    }

    fn persisted_auto_wake_count(&self, session_id: &str) -> Result<i64, String> {
        Ok(self
            .session_runtime(session_id)?
            .map(|runtime| runtime.auto_wake_count)
            .unwrap_or(0))
    }

    pub fn validate_wake_claim_for_child(
        &mut self,
        session_id: &str,
        claim_token: &str,
    ) -> Result<bool, String> {
        let claim = self.wake_claim(session_id)?;
        let valid = wake_claim_is_valid_for_child(claim.as_ref(), claim_token);
        self.release_after_child_claim_validation(session_id, claim_token, valid)
    }

    #[cfg(test)]
    fn force_wake_claim_age_for_test(
        &mut self,
        session_id: &str,
        seconds_old: i64,
    ) -> Result<(), String> {
        let claimed_at = aged_wake_claim_timestamp(seconds_old);
        self.conn
            .execute(
                "UPDATE session_wake_claim SET claimed_at = ?2 WHERE session_id = ?1",
                params![session_id, &claimed_at],
            )
            .map_err(|err| format!("Failed to age wake claim for test: {err}"))?;
        Ok(())
    }

    fn list_mailbox_all(&self, session_id: &str) -> Result<Vec<MailboxRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT seq, session_id, kind, handle, payload_json, enqueued_at,
                        delivered_at, delivered_by_invocation_uuid, delivery_attempts,
                        delivery_error, owner_invocation_uuid, matched_os_pid,
                        matched_os_boot_id, matched_os_pid_starttime_ticks,
                        matched_chain_index, state_dir, meta_path, log_path, rc_path, rc
                 FROM mailbox
                 WHERE session_id = ?1
                 ORDER BY seq ASC",
            )
            .map_err(|err| format!("Failed to prepare mailbox query: {err}"))?;
        let rows = stmt
            .query_map(params![session_id], map_mailbox_row)
            .map_err(|err| format!("Failed to query mailbox rows: {err}"))?;
        collect_rows(rows)
    }

    fn pending_wake_session_ids(&self, limit: usize) -> Result<Vec<String>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let oldest_limit = limit.saturating_sub(limit / 2);
        let newest_limit = limit.saturating_sub(oldest_limit);
        let oldest = self.oldest_pending_wake_session_ids(oldest_limit)?;
        let newest = self.newest_pending_wake_session_ids(newest_limit)?;
        Ok(merge_pending_wake_session_ids(limit, oldest, newest))
    }

    fn oldest_pending_wake_session_ids(&self, limit: usize) -> Result<Vec<String>, String> {
        self.pending_wake_session_ids_by_oldest_seq(limit, "ASC")
    }

    fn newest_pending_wake_session_ids(&self, limit: usize) -> Result<Vec<String>, String> {
        self.pending_wake_session_ids_by_oldest_seq(limit, "DESC")
    }

    fn pending_wake_session_ids_by_oldest_seq(
        &self,
        limit: usize,
        direction: &str,
    ) -> Result<Vec<String>, String> {
        let query = pending_wake_session_ids_by_oldest_seq_query(direction);
        let mut stmt = self
            .conn
            .prepare(&query)
            .map_err(|err| format!("Failed to prepare pending wake session query: {err}"))?;
        let rows = stmt
            .query_map(params![limit as i64, WAKE_SWEEP_ABANDONED_ERROR], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|err| format!("Failed to query pending wake sessions: {err}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("Failed to read pending wake session row: {err}"))
    }

    fn release_after_child_claim_validation(
        &mut self,
        session_id: &str,
        claim_token: &str,
        valid: bool,
    ) -> Result<bool, String> {
        if !valid {
            return Ok(false);
        }
        self.release_busy_child_wake_claim(session_id, claim_token)
    }

    fn pending_seq_bounds(&self, session_id: &str) -> Result<Option<(i64, i64)>, String> {
        pending_seq_bounds_on(&self.conn, session_id)
    }

    #[cfg(test)]
    fn enqueue_agent_bash_complete_then_rollback(
        &mut self,
        input: &AgentBashCompleteEnqueue<'_>,
    ) -> Result<(), String> {
        let now = now_rfc3339();
        let tx = self
            .conn
            .transaction()
            .map_err(|err| format!("Failed to start mailbox rollback test transaction: {err}"))?;
        let _ = enqueue_agent_bash_complete_in_tx(&tx, input, &now)?;
        Err("forced rollback before commit".to_string())
    }

    fn max_mailbox_seq(&self, session_id: &str) -> Result<Option<i64>, String> {
        self.conn
            .query_row(
                "SELECT MAX(seq) FROM mailbox WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .map_err(|err| format!("Failed to read mailbox max seq: {err}"))
    }

    fn running_turn_start_max_mailbox_seq(
        &self,
        input: &SessionRuntimeRunningUpdate<'_>,
    ) -> Result<Option<i64>, String> {
        let Some(seq) = input.turn_start_max_mailbox_seq else {
            return self.max_mailbox_seq(input.session_id);
        };
        Ok(Some(seq))
    }

    fn session_has_pending_mailbox(&self, session_id: &str) -> Result<bool, String> {
        self.list_pending(session_id)
            .map(|pending| !pending.is_empty())
    }

    fn session_is_busy(&mut self, session_id: &str) -> Result<bool, String> {
        self.session_liveness(session_id)
            .map(|liveness| liveness == SessionLiveness::Busy)
    }

    fn release_busy_child_wake_claim(
        &mut self,
        session_id: &str,
        claim_token: &str,
    ) -> Result<bool, String> {
        if self.busy_child_wake_claim_should_release(session_id)? {
            self.release_child_wake_claim(session_id, claim_token)?;
            return Ok(false);
        }
        Ok(true)
    }

    fn busy_child_wake_claim_should_release(&mut self, session_id: &str) -> Result<bool, String> {
        self.session_is_busy(session_id)
    }

    fn release_child_wake_claim(
        &mut self,
        session_id: &str,
        claim_token: &str,
    ) -> Result<(), String> {
        self.release_wake_claim(session_id, Some(claim_token))?;
        Ok(())
    }

    fn clear_stale_running_row_for_liveness(
        &mut self,
        session_id: &str,
        decision: &SessionRuntimeLivenessDecision,
    ) -> Result<(), String> {
        if let SessionRuntimeLivenessDecision::Stale {
            running_invocation_uuid,
        } = decision
        {
            self.clear_stale_running_row(session_id, running_invocation_uuid.as_deref())?;
        }
        Ok(())
    }

    fn clear_stale_running_row(
        &mut self,
        session_id: &str,
        running_invocation_uuid: Option<&str>,
    ) -> Result<(), String> {
        let now = now_rfc3339();
        self.conn
            .execute(
                "UPDATE session_runtime
                 SET run_state = 'idle',
                     updated_at = ?3,
                     pty_control_path = NULL,
                     running_invocation_uuid = NULL,
                     running_os_pid = NULL,
                     running_os_boot_id = NULL,
                     running_os_pid_starttime_ticks = NULL,
                     turn_ended_at = COALESCE(turn_ended_at, ?3)
                  WHERE session_id = ?1
                    AND run_state = 'running'
                    AND ((?2 IS NULL AND running_invocation_uuid IS NULL)
                         OR running_invocation_uuid = ?2)",
                params![session_id, running_invocation_uuid, &now],
            )
            .map_err(|err| format!("Failed to clear stale session runtime row: {err}"))?;
        Ok(())
    }
}

fn wake_claim_pid_identity_record<'a>(
    identity: &'a ProcessIdentity,
    claim_token: &'a str,
    session_id: &'a str,
    provider_name: Option<&'a str>,
    model_name: Option<&'a str>,
    recorded_at: &'a str,
) -> pid_identity::PidIdentityRecord<'a> {
    pid_identity::PidIdentityRecord {
        identity,
        os_pgid: None,
        invocation_uuid: claim_token,
        session_id: Some(session_id),
        provider_name,
        model_name,
        recorded_at,
    }
}

#[cfg(test)]
fn aged_wake_claim_timestamp(seconds_old: i64) -> String {
    (Utc::now() - Duration::seconds(seconds_old)).to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn merge_pending_wake_session_ids(
    limit: usize,
    mut oldest: Vec<String>,
    newest: Vec<String>,
) -> Vec<String> {
    for session_id in newest {
        if oldest.len() >= limit {
            break;
        }
        if !oldest.iter().any(|existing| existing == &session_id) {
            oldest.push(session_id);
        }
    }
    oldest
}

fn wake_sweep_candidates_at_limit(candidates: &[WakeSweepCandidate], limit: usize) -> bool {
    candidates.len() >= limit
}

fn pending_wake_session_ids_by_oldest_seq_query(direction: &str) -> String {
    format!(
        "SELECT session_id
                  FROM mailbox
                  WHERE delivered_at IS NULL
                    AND (delivery_error IS NULL OR delivery_error != ?2)
                  GROUP BY session_id
                  ORDER BY MIN(seq) {direction}
                  LIMIT ?1",
    )
}

fn wake_sweep_candidate(
    session_id: String,
    auto_wake_count: i64,
    min_pending_seq: i64,
    max_pending_seq: i64,
) -> WakeSweepCandidate {
    WakeSweepCandidate {
        session_id,
        auto_wake_count,
        min_pending_seq,
        max_pending_seq,
    }
}

fn begin_wake_claim_transaction(
    conn: &mut Connection,
) -> Result<rusqlite::Transaction<'_>, String> {
    conn.transaction().map_err(format_start_wake_claim_tx_error)
}

fn format_start_wake_claim_tx_error(err: rusqlite::Error) -> String {
    format!("Failed to start wake claim transaction: {err}")
}

fn pending_seq_bounds_for_claim_tx(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
) -> Result<Option<(i64, i64)>, String> {
    pending_seq_bounds_tx(tx, session_id)
}

fn fresh_in_flight_wake_claim_for_input(
    tx: &rusqlite::Transaction<'_>,
    input: WakeClaimRequest<'_>,
    renew_token: Option<&str>,
) -> Result<Option<WakeClaimRow>, String> {
    fresh_in_flight_wake_claim(
        tx,
        wake_claim_tx(tx, input.session_id)?,
        input.stale_after_seconds,
        renew_token,
    )
}

fn commit_empty_wake_claim_transaction(tx: rusqlite::Transaction<'_>) -> Result<(), String> {
    tx.commit().map_err(format_empty_wake_claim_commit_error)
}

fn format_empty_wake_claim_commit_error(err: rusqlite::Error) -> String {
    format!("Failed to commit empty wake claim transaction: {err}")
}

fn commit_existing_wake_claim_transaction(tx: rusqlite::Transaction<'_>) -> Result<(), String> {
    tx.commit().map_err(format_existing_wake_claim_commit_error)
}

fn format_existing_wake_claim_commit_error(err: rusqlite::Error) -> String {
    format!("Failed to commit existing wake claim transaction: {err}")
}

fn commit_wake_claim_transaction(tx: rusqlite::Transaction<'_>) -> Result<(), String> {
    tx.commit().map_err(format_wake_claim_commit_error)
}

fn format_wake_claim_commit_error(err: rusqlite::Error) -> String {
    format!("Failed to commit wake claim transaction: {err}")
}

fn claim_auto_wake_count(claim: Option<&WakeClaimRow>) -> Option<i64> {
    claim.map(|claim| claim.auto_wake_count)
}

fn next_auto_wake_count(persisted: i64, claim: Option<i64>) -> i64 {
    claim
        .unwrap_or(persisted)
        .max(persisted)
        .saturating_add(1)
        .max(1)
}

fn bounded_mailbox_limit_is_zero(limit: usize) -> bool {
    limit == 0
}

fn bounded_mailbox_sql_limit(limit: usize) -> i64 {
    limit as i64
}

fn format_bounded_mailbox_rows_error(err: BoundedMailboxRowsError) -> String {
    match err {
        BoundedMailboxRowsError::Prepare(err) => {
            format!("Failed to prepare bounded mailbox query: {err}")
        }
        BoundedMailboxRowsError::Query(err) => {
            format!("Failed to query bounded mailbox rows: {err}")
        }
        BoundedMailboxRowsError::Row(err) => format!("Failed to read mailbox row: {err}"),
    }
}

fn enqueue_agent_bash_complete_in_tx(
    tx: &rusqlite::Transaction<'_>,
    input: &AgentBashCompleteEnqueue<'_>,
    now: &str,
) -> Result<EnqueueResult, String> {
    let changed = insert_agent_bash_complete_tx(tx, input, now)?;
    let row = query_mailbox_by_kind_handle_tx(tx, AGENT_BASH_COMPLETE_KIND, input.handle)?
        .ok_or_else(|| "Mailbox row missing after enqueue conflict check".to_string())?;
    Ok(agent_bash_enqueue_result(changed, row, input))
}

fn insert_agent_bash_complete_tx(
    tx: &rusqlite::Transaction<'_>,
    input: &AgentBashCompleteEnqueue<'_>,
    now: &str,
) -> Result<usize, String> {
    tx.execute(
        "INSERT OR IGNORE INTO mailbox (
            session_id,
            kind,
            handle,
            payload_json,
            enqueued_at,
            owner_invocation_uuid,
            matched_os_pid,
            matched_os_boot_id,
            matched_os_pid_starttime_ticks,
            matched_chain_index,
            state_dir,
            meta_path,
            log_path,
            rc_path,
            rc
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            input.session_id,
            AGENT_BASH_COMPLETE_KIND,
            input.handle,
            input.payload_json,
            now,
            input.owner_invocation_uuid,
            input.matched_os_pid,
            input.matched_os_boot_id,
            input.matched_os_pid_starttime_ticks,
            input.matched_chain_index,
            input.state_dir,
            input.meta_path,
            input.log_path,
            input.rc_path,
            input.rc,
        ],
    )
    .map_err(|err| format!("Failed to insert mailbox row: {err}"))
}

fn agent_bash_enqueue_result(
    changed: usize,
    row: MailboxRow,
    input: &AgentBashCompleteEnqueue<'_>,
) -> EnqueueResult {
    if changed > 0 {
        return EnqueueResult::Inserted(row);
    }
    if row.session_id == input.session_id {
        EnqueueResult::AlreadyEnqueued(row)
    } else {
        EnqueueResult::Conflict { existing: row }
    }
}

fn mark_session_running_row(
    conn: &Connection,
    input: SessionRuntimeRunningUpdate<'_>,
    now: &str,
    turn_start_max_mailbox_seq: Option<i64>,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO session_runtime (
            session_id,
            mode,
            invocation_uuid,
            provider_name,
            model_name,
            pty_control_path,
            updated_at,
            run_state,
            running_invocation_uuid,
            running_os_pid,
            running_os_boot_id,
            running_os_pid_starttime_ticks,
            turn_started_at,
            turn_ended_at,
            turn_start_max_mailbox_seq,
            last_exit_code,
            models_dir,
            effective_cwd
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'running', ?3, ?8, ?9, ?10, ?7, NULL, ?11, NULL, ?12, ?13)
         ON CONFLICT(session_id)
         DO UPDATE SET
            mode = excluded.mode,
            invocation_uuid = excluded.invocation_uuid,
            provider_name = excluded.provider_name,
            model_name = excluded.model_name,
            pty_control_path = excluded.pty_control_path,
            updated_at = excluded.updated_at,
            run_state = 'running',
            running_invocation_uuid = excluded.running_invocation_uuid,
            running_os_pid = excluded.running_os_pid,
            running_os_boot_id = excluded.running_os_boot_id,
            running_os_pid_starttime_ticks = excluded.running_os_pid_starttime_ticks,
            turn_started_at = excluded.turn_started_at,
            turn_ended_at = NULL,
            turn_start_max_mailbox_seq = excluded.turn_start_max_mailbox_seq,
            last_exit_code = NULL,
            models_dir = COALESCE(excluded.models_dir, session_runtime.models_dir),
            effective_cwd = COALESCE(excluded.effective_cwd, session_runtime.effective_cwd)",
        params![
            input.session_id,
            input.mode,
            input.invocation_uuid,
            input.provider_name,
            input.model_name,
            input.pty_control_path,
            now,
            input.identity.os_pid,
            &input.identity.os_boot_id,
            input.identity.os_pid_starttime_ticks,
            turn_start_max_mailbox_seq,
            input.models_dir,
            input.effective_cwd,
        ],
    )
    .map_err(|err| format!("Failed to mark session runtime running: {err}"))?;
    Ok(())
}

fn mark_session_idle_row(
    conn: &Connection,
    input: SessionRuntimeIdleUpdate<'_>,
    now: &str,
) -> Result<bool, String> {
    let changed = mark_session_idle_row_count(conn, input, now)?;
    Ok(row_changed(changed))
}

fn mark_session_idle_row_count(
    conn: &Connection,
    input: SessionRuntimeIdleUpdate<'_>,
    now: &str,
) -> Result<usize, String> {
    conn.execute(
        "UPDATE session_runtime
         SET run_state = 'idle',
             updated_at = ?3,
             pty_control_path = NULL,
             running_invocation_uuid = NULL,
             running_os_pid = NULL,
             running_os_boot_id = NULL,
             running_os_pid_starttime_ticks = NULL,
             turn_ended_at = ?3,
             last_exit_code = ?4
         WHERE session_id = ?1
           AND running_invocation_uuid = ?2",
        params![
            input.session_id,
            input.invocation_uuid,
            now,
            input.last_exit_code,
        ],
    )
    .map_err(|err| format!("Failed to mark session runtime idle: {err}"))
}

fn row_changed(changed: usize) -> bool {
    changed > 0
}

fn session_runtime_row(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<SessionRuntimeRow>, String> {
    conn.query_row(
        "SELECT session_id, mode, invocation_uuid, provider_name, model_name,
                pty_control_path, updated_at, run_state, running_invocation_uuid,
                running_os_pid, running_os_boot_id, running_os_pid_starttime_ticks,
                turn_started_at, turn_ended_at, turn_start_max_mailbox_seq,
                last_exit_code, models_dir, effective_cwd, auto_wake_count
         FROM session_runtime
         WHERE session_id = ?1",
        params![session_id],
        map_session_runtime_row,
    )
    .optional()
    .map_err(|err| format!("Failed to read session runtime row: {err}"))
}

fn validate_session_runtime_row(row: Option<&SessionRuntimeRow>) -> Result<(), String> {
    let Some(row) = row else {
        return Ok(());
    };
    validate_run_state(&row.run_state)
}

fn validate_running_run_state() -> Result<(), String> {
    validate_run_state("running")
}

fn validate_idle_run_state() -> Result<(), String> {
    validate_run_state("idle")
}

fn runtime_row_is_idle(row: &SessionRuntimeRow) -> bool {
    row.run_state != "running"
}

fn live_process_identity_for_runtime(
    recorded: &ProcessIdentity,
) -> Result<Option<ProcessIdentity>, String> {
    pid_identity::read_live_process_identity(recorded.os_pid)
}

fn runtime_identity_is_live(live: Option<&ProcessIdentity>, recorded: &ProcessIdentity) -> bool {
    live.is_some_and(|live| live == recorded)
}

struct RuntimeLivenessEvidence {
    invocation_uuid: Option<String>,
    recorded: Option<ProcessIdentity>,
    live: Option<ProcessIdentity>,
}

fn session_runtime_liveness_decision(
    row: Option<&SessionRuntimeRow>,
) -> Result<SessionRuntimeLivenessDecision, String> {
    let Some(row) = row else {
        return Ok(SessionRuntimeLivenessDecision::Idle);
    };
    if runtime_row_is_idle(row) {
        return Ok(SessionRuntimeLivenessDecision::Idle);
    }
    let evidence = runtime_liveness_evidence(row)?;
    Ok(session_runtime_liveness_from_evidence(evidence))
}

fn classify_session_runtime_row_read_only(
    row: Option<&SessionRuntimeRow>,
) -> Result<SessionRuntimeReadOnlyLiveness, String> {
    let Some(row) = row else {
        return Ok(SessionRuntimeReadOnlyLiveness::Idle);
    };
    if runtime_row_is_idle(row) {
        return Ok(SessionRuntimeReadOnlyLiveness::Idle);
    }
    let evidence = runtime_liveness_evidence(row)?;
    Ok(read_only_liveness_from_evidence(evidence))
}

fn runtime_liveness_evidence(row: &SessionRuntimeRow) -> Result<RuntimeLivenessEvidence, String> {
    let invocation_uuid = row.running_invocation_uuid.clone();
    let recorded = runtime_liveness_recorded_identity(row, invocation_uuid.as_ref());
    let live = live_process_identity_for_evidence(recorded.as_ref())?;
    Ok(runtime_liveness_evidence_from_parts(
        invocation_uuid,
        recorded,
        live,
    ))
}

fn runtime_liveness_recorded_identity(
    row: &SessionRuntimeRow,
    invocation_uuid: Option<&String>,
) -> Option<ProcessIdentity> {
    invocation_uuid.and_then(|_| runtime_row_identity(row))
}

fn live_process_identity_for_evidence(
    recorded: Option<&ProcessIdentity>,
) -> Result<Option<ProcessIdentity>, String> {
    match recorded {
        Some(recorded) => live_process_identity_for_runtime(recorded),
        None => Ok(None),
    }
}

fn runtime_liveness_evidence_from_parts(
    invocation_uuid: Option<String>,
    recorded: Option<ProcessIdentity>,
    live: Option<ProcessIdentity>,
) -> RuntimeLivenessEvidence {
    RuntimeLivenessEvidence {
        invocation_uuid,
        recorded,
        live,
    }
}

fn session_runtime_liveness_from_evidence(
    evidence: RuntimeLivenessEvidence,
) -> SessionRuntimeLivenessDecision {
    if liveness_evidence_missing_invocation(&evidence) {
        return stale_liveness_decision(None);
    };
    let recorded_missing = liveness_evidence_missing_recorded(&evidence);
    let invocation_uuid = evidence.invocation_uuid.expect("invocation checked above");
    if recorded_missing {
        return stale_liveness_decision(Some(invocation_uuid));
    };
    let recorded = evidence.recorded.expect("recorded identity checked above");
    if liveness_evidence_is_busy(&evidence.live, &recorded) {
        return SessionRuntimeLivenessDecision::Busy;
    }
    stale_liveness_decision(Some(invocation_uuid))
}

fn liveness_evidence_missing_invocation(evidence: &RuntimeLivenessEvidence) -> bool {
    evidence.invocation_uuid.is_none()
}

fn liveness_evidence_missing_recorded(evidence: &RuntimeLivenessEvidence) -> bool {
    evidence.recorded.is_none()
}

fn liveness_evidence_is_busy(live: &Option<ProcessIdentity>, recorded: &ProcessIdentity) -> bool {
    runtime_identity_is_live(live.as_ref(), recorded)
}

fn stale_liveness_decision(
    running_invocation_uuid: Option<String>,
) -> SessionRuntimeLivenessDecision {
    SessionRuntimeLivenessDecision::Stale {
        running_invocation_uuid,
    }
}

fn read_only_liveness_from_evidence(
    evidence: RuntimeLivenessEvidence,
) -> SessionRuntimeReadOnlyLiveness {
    if liveness_evidence_missing_recorded(&evidence) {
        return read_only_missing_liveness(evidence.invocation_uuid.as_deref());
    };
    if read_only_liveness_evidence_missing_live(&evidence) {
        return SessionRuntimeReadOnlyLiveness::StaleDead;
    };
    let recorded = evidence.recorded.expect("recorded identity checked above");
    let live = evidence.live.expect("live identity checked above");
    read_only_liveness_from_live_identity(&live, &recorded)
}

fn read_only_liveness_evidence_missing_live(evidence: &RuntimeLivenessEvidence) -> bool {
    evidence.live.is_none()
}

fn read_only_liveness_from_live_identity(
    live: &ProcessIdentity,
    recorded: &ProcessIdentity,
) -> SessionRuntimeReadOnlyLiveness {
    if live == recorded {
        SessionRuntimeReadOnlyLiveness::Busy
    } else {
        SessionRuntimeReadOnlyLiveness::StalePidReused
    }
}

fn read_only_missing_liveness(invocation_uuid: Option<&str>) -> SessionRuntimeReadOnlyLiveness {
    if invocation_uuid.is_some() {
        SessionRuntimeReadOnlyLiveness::StaleMissingIdentity
    } else {
        SessionRuntimeReadOnlyLiveness::StaleMissingInvocation
    }
}

fn session_liveness_from_decision(decision: &SessionRuntimeLivenessDecision) -> SessionLiveness {
    match decision {
        SessionRuntimeLivenessDecision::Busy => SessionLiveness::Busy,
        SessionRuntimeLivenessDecision::Idle | SessionRuntimeLivenessDecision::Stale { .. } => {
            SessionLiveness::Idle
        }
    }
}

fn fresh_in_flight_wake_claim(
    tx: &rusqlite::Transaction<'_>,
    existing: Option<WakeClaimRow>,
    stale_after_seconds: i64,
    renew_token: Option<&str>,
) -> Result<Option<WakeClaimRow>, String> {
    let Some(existing) = existing else {
        return Ok(None);
    };
    let renews_existing = renew_token == Some(existing.claim_token.as_str());
    if renews_existing || wake_claim_is_reclaimable(tx, &existing, stale_after_seconds)? {
        return Ok(None);
    }
    Ok(Some(existing))
}

fn wake_claim_is_reclaimable(
    conn: &Connection,
    claim: &WakeClaimRow,
    stale_after_seconds: i64,
) -> Result<bool, String> {
    if claim.wake_pid.is_some() {
        return wake_claim_pid_is_reclaimable(conn, claim);
    }
    Ok(claim_is_stale(claim, stale_after_seconds))
}

fn wake_claim_pid_is_reclaimable(conn: &Connection, claim: &WakeClaimRow) -> Result<bool, String> {
    let Some(wake_pid) = claim.wake_pid else {
        return Ok(false);
    };
    wake_claim_pid_is_live_identity_matched(conn, claim, wake_pid).map(|matched| !matched)
}

fn wake_claim_pid_is_live_identity_matched(
    conn: &Connection,
    claim: &WakeClaimRow,
    wake_pid: i64,
) -> Result<bool, String> {
    let Some(live) = wake_claim_live_process_identity(wake_pid)? else {
        return Ok(false);
    };
    wake_claim_live_identity_has_matching_sidecar_row(conn, claim, &live)
}

fn wake_claim_live_process_identity(wake_pid: i64) -> Result<Option<ProcessIdentity>, String> {
    pid_identity::read_live_process_identity(wake_pid)
}

fn wake_claim_live_identity_has_matching_sidecar_row(
    conn: &Connection,
    claim: &WakeClaimRow,
    live: &ProcessIdentity,
) -> Result<bool, String> {
    let exists = wake_claim_live_identity_matching_sidecar_exists(conn, claim, live)?;
    Ok(sqlite_exists_value_to_bool(exists))
}

fn wake_claim_live_identity_matching_sidecar_exists(
    conn: &Connection,
    claim: &WakeClaimRow,
    live: &ProcessIdentity,
) -> Result<i64, String> {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1
            FROM pid_identity
            WHERE os_pid = ?1
              AND os_boot_id = ?2
              AND os_pid_starttime_ticks = ?3
              AND invocation_uuid = ?4
              AND session_id = ?5
        )",
        params![
            live.os_pid,
            &live.os_boot_id,
            live.os_pid_starttime_ticks,
            &claim.claim_token,
            &claim.session_id,
        ],
        |row| row.get::<_, i64>(0),
    )
    .map_err(|err| format!("Failed to verify wake PID sidecar identity: {err}"))
}

fn sqlite_exists_value_to_bool(value: i64) -> bool {
    value != 0
}

fn acquire_wake_claim_tx(
    tx: &rusqlite::Transaction<'_>,
    input: WakeClaimRequest<'_>,
    now: &str,
    min_seq: i64,
    max_seq: i64,
) -> Result<WakeClaimRow, String> {
    upsert_wake_claim_tx(tx, input, now, min_seq, max_seq)?;
    update_session_runtime_auto_wake_count_tx(tx, input.session_id, input.auto_wake_count)?;
    read_acquired_wake_claim_tx(tx, input.session_id)
}

fn upsert_wake_claim_tx(
    tx: &rusqlite::Transaction<'_>,
    input: WakeClaimRequest<'_>,
    now: &str,
    min_seq: i64,
    max_seq: i64,
) -> Result<(), String> {
    tx.execute(
        "INSERT INTO session_wake_claim (
            session_id,
            claim_token,
            claimed_at,
            wake_pid,
            wake_invocation_uuid,
            reason,
            auto_wake_count,
            min_pending_seq_at_claim,
            max_pending_seq_at_claim
         ) VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(session_id)
         DO UPDATE SET
            claim_token = excluded.claim_token,
            claimed_at = excluded.claimed_at,
            wake_pid = excluded.wake_pid,
            wake_invocation_uuid = excluded.wake_invocation_uuid,
            reason = excluded.reason,
            auto_wake_count = excluded.auto_wake_count,
            min_pending_seq_at_claim = excluded.min_pending_seq_at_claim,
            max_pending_seq_at_claim = excluded.max_pending_seq_at_claim",
        params![
            input.session_id,
            input.claim_token,
            now,
            input.wake_invocation_uuid,
            input.reason,
            input.auto_wake_count,
            min_seq,
            max_seq,
        ],
    )
    .map_err(format_acquire_wake_claim_error)?;
    Ok(())
}

fn format_acquire_wake_claim_error(err: rusqlite::Error) -> String {
    format!("Failed to acquire wake claim: {err}")
}

fn read_acquired_wake_claim_tx(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
) -> Result<WakeClaimRow, String> {
    wake_claim_tx(tx, session_id)?
        .ok_or_else(|| "Wake claim missing immediately after acquisition".to_string())
}

fn update_session_runtime_auto_wake_count_tx(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    auto_wake_count: i64,
) -> Result<(), String> {
    tx.execute(
        "UPDATE session_runtime
         SET auto_wake_count = MAX(auto_wake_count, ?2)
         WHERE session_id = ?1",
        params![session_id, auto_wake_count],
    )
    .map_err(|err| format!("Failed to update session auto wake count: {err}"))?;
    Ok(())
}

fn wake_claim_matches_child(claim: &WakeClaimRow, claim_token: &str) -> bool {
    claim.claim_token == claim_token
}

fn wake_claim_is_valid_for_child(claim: Option<&WakeClaimRow>, claim_token: &str) -> bool {
    claim.is_some_and(|claim| wake_claim_matches_child(claim, claim_token))
}

fn query_mailbox_by_kind_handle_tx(
    tx: &rusqlite::Transaction<'_>,
    kind: &str,
    handle: &str,
) -> Result<Option<MailboxRow>, String> {
    tx.query_row(
        "SELECT seq, session_id, kind, handle, payload_json, enqueued_at,
                delivered_at, delivered_by_invocation_uuid, delivery_attempts,
                delivery_error, owner_invocation_uuid, matched_os_pid,
                matched_os_boot_id, matched_os_pid_starttime_ticks,
                matched_chain_index, state_dir, meta_path, log_path, rc_path, rc
         FROM mailbox
         WHERE kind = ?1 AND handle = ?2",
        params![kind, handle],
        map_mailbox_row,
    )
    .optional()
    .map_err(|err| format!("Failed to read mailbox row by handle: {err}"))
}

fn ensure_parent_dir(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("Failed to create PID mailbox sidecar directory: {err}"))?;
    }
    Ok(())
}

fn set_wal_mode(conn: &Connection) -> Result<(), String> {
    conn.execute_batch("PRAGMA journal_mode=WAL;")
        .map_err(|err| format!("Failed to set PID mailbox sidecar WAL mode: {err}"))
}

fn ensure_mailbox_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS mailbox (
            seq                          INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id                   TEXT    NOT NULL,
            kind                         TEXT    NOT NULL,
            handle                       TEXT    NOT NULL,
            payload_json                 TEXT    NOT NULL,
            enqueued_at                  TEXT    NOT NULL,
            delivered_at                 TEXT,
            delivered_by_invocation_uuid TEXT,
            delivery_attempts            INTEGER NOT NULL DEFAULT 0,
            delivery_error               TEXT,
            owner_invocation_uuid        TEXT,
            matched_os_pid               INTEGER,
            matched_os_boot_id           TEXT,
            matched_os_pid_starttime_ticks INTEGER,
            matched_chain_index          INTEGER,
            state_dir                    TEXT    NOT NULL,
            meta_path                    TEXT    NOT NULL,
            log_path                     TEXT    NOT NULL,
            rc_path                      TEXT    NOT NULL,
            rc                           INTEGER NOT NULL,
            UNIQUE(kind, handle)
        );

        CREATE INDEX IF NOT EXISTS idx_mailbox_pending
            ON mailbox(session_id, delivered_at, seq);

        CREATE TABLE IF NOT EXISTS session_runtime (
            session_id                       TEXT PRIMARY KEY,
            mode                             TEXT NOT NULL CHECK(mode IN ('headless', 'pty_interactive')),
            invocation_uuid                  TEXT,
            provider_name                    TEXT,
            model_name                       TEXT,
            pty_control_path                 TEXT,
            updated_at                       TEXT NOT NULL,
            run_state                        TEXT NOT NULL DEFAULT 'idle',
            running_invocation_uuid          TEXT,
            running_os_pid                   INTEGER,
            running_os_boot_id               TEXT,
            running_os_pid_starttime_ticks   INTEGER,
            turn_started_at                  TEXT,
            turn_ended_at                    TEXT,
            turn_start_max_mailbox_seq       INTEGER,
            last_exit_code                   INTEGER,
            models_dir                       TEXT,
            effective_cwd                    TEXT,
            auto_wake_count                  INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS session_wake_claim (
            session_id                       TEXT PRIMARY KEY,
            claim_token                      TEXT NOT NULL,
            claimed_at                       TEXT NOT NULL,
            wake_pid                         INTEGER,
            wake_invocation_uuid             TEXT,
            reason                           TEXT NOT NULL,
            auto_wake_count                  INTEGER NOT NULL,
            min_pending_seq_at_claim         INTEGER,
            max_pending_seq_at_claim         INTEGER
        );

        CREATE INDEX IF NOT EXISTS idx_session_wake_claim_claimed_at
            ON session_wake_claim(claimed_at);",
    )
    .map_err(|err| format!("Failed to ensure PID mailbox sidecar schema: {err}"))?;
    ensure_session_runtime_columns(conn)
}

fn ensure_session_runtime_columns(conn: &Connection) -> Result<(), String> {
    let columns = table_columns(conn, "session_runtime")?;
    for (name, definition) in missing_session_runtime_columns(&columns) {
        add_session_runtime_column(conn, name, definition)?;
    }
    Ok(())
}

fn session_runtime_column_additions() -> [(&'static str, &'static str); 12] {
    [
        ("run_state", "TEXT NOT NULL DEFAULT 'idle'"),
        ("running_invocation_uuid", "TEXT"),
        ("running_os_pid", "INTEGER"),
        ("running_os_boot_id", "TEXT"),
        ("running_os_pid_starttime_ticks", "INTEGER"),
        ("turn_started_at", "TEXT"),
        ("turn_ended_at", "TEXT"),
        ("turn_start_max_mailbox_seq", "INTEGER"),
        ("last_exit_code", "INTEGER"),
        ("models_dir", "TEXT"),
        ("effective_cwd", "TEXT"),
        ("auto_wake_count", "INTEGER NOT NULL DEFAULT 0"),
    ]
}

fn missing_session_runtime_columns(columns: &[String]) -> Vec<(&'static str, &'static str)> {
    session_runtime_column_additions()
        .into_iter()
        .filter(|(name, _)| !columns.iter().any(|column| column == name))
        .collect()
}

fn add_session_runtime_column(
    conn: &Connection,
    name: &str,
    definition: &str,
) -> Result<(), String> {
    conn.execute_batch(&session_runtime_add_column_sql(name, definition))
        .map_err(|err| format!("Failed to add session_runtime.{name}: {err}"))
}

fn session_runtime_add_column_sql(name: &str, definition: &str) -> String {
    format!("ALTER TABLE session_runtime ADD COLUMN {name} {definition};")
}

fn table_columns(conn: &Connection, table: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|err| format!("Failed to inspect {table} columns: {err}"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|err| format!("Failed to query {table} columns: {err}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("Failed to read {table} column: {err}"))
}

fn validate_run_state(run_state: &str) -> Result<(), String> {
    match run_state {
        "idle" | "running" => Ok(()),
        other => Err(format!("Invalid session_runtime.run_state value: {other}")),
    }
}

fn runtime_row_identity(row: &SessionRuntimeRow) -> Option<ProcessIdentity> {
    Some(ProcessIdentity {
        os_pid: row.running_os_pid?,
        os_boot_id: row.running_os_boot_id.clone()?,
        os_pid_starttime_ticks: row.running_os_pid_starttime_ticks?,
    })
}

fn pending_seq_bounds_tx(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
) -> Result<Option<(i64, i64)>, String> {
    tx.query_row(
        "SELECT MIN(seq), MAX(seq)
         FROM mailbox
         WHERE session_id = ?1
           AND delivered_at IS NULL
           AND (delivery_error IS NULL OR delivery_error != ?2)",
        params![session_id, WAKE_SWEEP_ABANDONED_ERROR],
        |row| {
            let min_seq: Option<i64> = row.get(0)?;
            let max_seq: Option<i64> = row.get(1)?;
            Ok(min_seq.zip(max_seq))
        },
    )
    .map_err(|err| format!("Failed to read pending mailbox seq bounds: {err}"))
}

fn pending_seq_bounds_on(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<(i64, i64)>, String> {
    conn.query_row(
        "SELECT MIN(seq), MAX(seq)
         FROM mailbox
         WHERE session_id = ?1
           AND delivered_at IS NULL
           AND (delivery_error IS NULL OR delivery_error != ?2)",
        params![session_id, WAKE_SWEEP_ABANDONED_ERROR],
        |row| {
            let min_seq: Option<i64> = row.get(0)?;
            let max_seq: Option<i64> = row.get(1)?;
            Ok(min_seq.zip(max_seq))
        },
    )
    .map_err(|err| format!("Failed to read pending mailbox seq bounds: {err}"))
}

fn wake_claim(conn: &Connection, session_id: &str) -> Result<Option<WakeClaimRow>, String> {
    conn.query_row(
        "SELECT session_id, claim_token, claimed_at, wake_pid, wake_invocation_uuid,
                reason, auto_wake_count, min_pending_seq_at_claim, max_pending_seq_at_claim
         FROM session_wake_claim
         WHERE session_id = ?1",
        params![session_id],
        map_wake_claim_row,
    )
    .optional()
    .map_err(|err| format!("Failed to read wake claim row: {err}"))
}

fn wake_claim_tx(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
) -> Result<Option<WakeClaimRow>, String> {
    tx.query_row(
        "SELECT session_id, claim_token, claimed_at, wake_pid, wake_invocation_uuid,
                reason, auto_wake_count, min_pending_seq_at_claim, max_pending_seq_at_claim
         FROM session_wake_claim
         WHERE session_id = ?1",
        params![session_id],
        map_wake_claim_row,
    )
    .optional()
    .map_err(|err| format!("Failed to read wake claim row: {err}"))
}

fn claim_is_stale(claim: &WakeClaimRow, stale_after_seconds: i64) -> bool {
    let Some(claimed_at) = parse_claimed_at(&claim.claimed_at) else {
        return true;
    };
    claim_age_exceeds_stale_after(claimed_at, stale_after_seconds)
}

fn parse_claimed_at(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn claim_age_exceeds_stale_after(claimed_at: DateTime<Utc>, stale_after_seconds: i64) -> bool {
    let age = Utc::now().signed_duration_since(claimed_at);
    age > Duration::seconds(stale_after_seconds)
}

fn map_session_runtime_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRuntimeRow> {
    Ok(SessionRuntimeRow {
        session_id: row.get(0)?,
        mode: row.get(1)?,
        invocation_uuid: row.get(2)?,
        provider_name: row.get(3)?,
        model_name: row.get(4)?,
        pty_control_path: row.get(5)?,
        updated_at: row.get(6)?,
        run_state: row.get(7)?,
        running_invocation_uuid: row.get(8)?,
        running_os_pid: row.get(9)?,
        running_os_boot_id: row.get(10)?,
        running_os_pid_starttime_ticks: row.get(11)?,
        turn_started_at: row.get(12)?,
        turn_ended_at: row.get(13)?,
        turn_start_max_mailbox_seq: row.get(14)?,
        last_exit_code: row.get(15)?,
        models_dir: row.get(16)?,
        effective_cwd: row.get(17)?,
        auto_wake_count: row.get(18)?,
    })
}

fn map_wake_claim_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WakeClaimRow> {
    Ok(WakeClaimRow {
        session_id: row.get(0)?,
        claim_token: row.get(1)?,
        claimed_at: row.get(2)?,
        wake_pid: row.get(3)?,
        wake_invocation_uuid: row.get(4)?,
        reason: row.get(5)?,
        auto_wake_count: row.get(6)?,
        min_pending_seq_at_claim: row.get(7)?,
        max_pending_seq_at_claim: row.get(8)?,
    })
}

fn map_mailbox_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MailboxRow> {
    Ok(MailboxRow {
        seq: row.get(0)?,
        session_id: row.get(1)?,
        kind: row.get(2)?,
        handle: row.get(3)?,
        payload_json: row.get(4)?,
        enqueued_at: row.get(5)?,
        delivered_at: row.get(6)?,
        delivered_by_invocation_uuid: row.get(7)?,
        delivery_attempts: row.get(8)?,
        delivery_error: row.get(9)?,
        owner_invocation_uuid: row.get(10)?,
        matched_os_pid: row.get(11)?,
        matched_os_boot_id: row.get(12)?,
        matched_os_pid_starttime_ticks: row.get(13)?,
        matched_chain_index: row.get(14)?,
        state_dir: row.get(15)?,
        meta_path: row.get(16)?,
        log_path: row.get(17)?,
        rc_path: row.get(18)?,
        rc: row.get(19)?,
    })
}

fn collect_rows<F>(rows: rusqlite::MappedRows<'_, F>) -> Result<Vec<MailboxRow>, String>
where
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<MailboxRow>,
{
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("Failed to read mailbox row: {err}"))
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StateDb;

    fn input<'a>(handle: &'a str, session_id: &'a str) -> AgentBashCompleteEnqueue<'a> {
        AgentBashCompleteEnqueue {
            session_id,
            handle,
            payload_json: r#"{"schema_version":1}"#,
            owner_invocation_uuid: Some("11111111-1111-4111-8111-111111111111"),
            matched_os_pid: Some(4242),
            matched_os_boot_id: Some("boot-a"),
            matched_os_pid_starttime_ticks: Some(99),
            matched_chain_index: Some(0),
            state_dir: "/tmp/state",
            meta_path: "/tmp/state/meta.json",
            log_path: "/tmp/state/log",
            rc_path: "/tmp/state/rc",
            rc: 0,
        }
    }

    #[test]
    fn enqueue_transaction_rollback_has_no_partial_row() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();

        let err = db
            .enqueue_agent_bash_complete_then_rollback(&input("handle-a", "session-a"))
            .unwrap_err();

        assert_eq!(err, "forced rollback before commit");
        assert!(db.list_mailbox("session-a", true).unwrap().is_empty());
    }

    #[test]
    fn mark_delivered_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));

        db.mark_delivered("session-a", &[row.seq], "resume-1")
            .unwrap();
        let first = db.list_mailbox("session-a", true).unwrap().remove(0);
        db.mark_delivered("session-a", &[row.seq], "resume-2")
            .unwrap();
        let second = db.list_mailbox("session-a", true).unwrap().remove(0);

        assert_eq!(second.delivered_at, first.delivered_at);
        assert_eq!(
            second.delivered_by_invocation_uuid.as_deref(),
            Some("resume-1")
        );
        assert_eq!(second.delivery_attempts, 1);
    }

    #[test]
    fn mark_delivery_failed_records_attempt_without_delivery() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));

        db.mark_delivery_failed("session-a", &[row.seq], "mailbox_delivery_unconfirmed")
            .unwrap();
        let failed = db.list_mailbox("session-a", true).unwrap().remove(0);

        assert!(failed.delivered_at.is_none());
        assert_eq!(failed.delivery_attempts, 1);
        assert_eq!(
            failed.delivery_error.as_deref(),
            Some("mailbox_delivery_unconfirmed")
        );
    }

    #[test]
    fn list_pending_excludes_delivered() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let delivered =
            inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));
        let pending = inserted_row(db.enqueue_agent_bash_complete(&input("handle-b", "session-a")));

        db.mark_delivered("session-a", &[delivered.seq], "resume-1")
            .unwrap();

        let pending_rows = db.list_pending("session-a").unwrap();
        assert_eq!(pending_rows.len(), 1);
        assert_eq!(pending_rows[0].seq, pending.seq);
        assert_eq!(pending_rows[0].handle, "handle-b");
    }

    #[test]
    fn pending_list_does_not_mutate_rows_for_crash_redelivery() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));

        let first = db.list_pending("session-a").unwrap();
        let second = db.list_pending("session-a").unwrap();

        assert_eq!(first[0].seq, row.seq);
        assert_eq!(second[0].seq, row.seq);
        assert!(second[0].delivered_at.is_none());
        assert_eq!(second[0].delivery_attempts, 0);
    }

    #[test]
    fn runtime_mark_running_records_pid_identity() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let identity = current_identity();

        db.mark_session_running(SessionRuntimeRunningUpdate {
            session_id: "session-a",
            mode: "headless",
            invocation_uuid: "invocation-a",
            provider_name: Some("provider-a"),
            model_name: Some("model-a"),
            identity: &identity,
            pty_control_path: None,
            turn_start_max_mailbox_seq: Some(7),
            models_dir: Some("/tmp/models"),
            effective_cwd: Some("/tmp/work"),
        })
        .unwrap();

        let row = db.session_runtime("session-a").unwrap().unwrap();
        assert_eq!(row.run_state, "running");
        assert_eq!(row.mode, "headless");
        assert_eq!(row.invocation_uuid.as_deref(), Some("invocation-a"));
        assert_eq!(row.running_invocation_uuid.as_deref(), Some("invocation-a"));
        assert_eq!(row.provider_name.as_deref(), Some("provider-a"));
        assert_eq!(row.model_name.as_deref(), Some("model-a"));
        assert_eq!(row.running_os_pid, Some(identity.os_pid));
        assert_eq!(
            row.running_os_boot_id.as_deref(),
            Some(identity.os_boot_id.as_str())
        );
        assert_eq!(
            row.running_os_pid_starttime_ticks,
            Some(identity.os_pid_starttime_ticks)
        );
        assert_eq!(row.turn_start_max_mailbox_seq, Some(7));
        assert_eq!(row.models_dir.as_deref(), Some("/tmp/models"));
        assert_eq!(row.effective_cwd.as_deref(), Some("/tmp/work"));
        assert!(row.turn_started_at.is_some());
        assert!(row.turn_ended_at.is_none());
    }

    #[test]
    fn runtime_mark_running_records_pty_control_path_without_schema_change() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let identity = current_identity();

        db.mark_session_running(SessionRuntimeRunningUpdate {
            session_id: "session-a",
            mode: "pty_interactive",
            invocation_uuid: "invocation-a",
            provider_name: Some("provider-a"),
            model_name: Some("model-a"),
            identity: &identity,
            pty_control_path: Some("/tmp/oulipoly-a.sock"),
            turn_start_max_mailbox_seq: None,
            models_dir: None,
            effective_cwd: None,
        })
        .unwrap();

        let row = db.session_runtime("session-a").unwrap().unwrap();
        assert_eq!(row.mode, "pty_interactive");
        assert_eq!(
            row.pty_control_path.as_deref(),
            Some("/tmp/oulipoly-a.sock")
        );
    }

    #[test]
    fn runtime_mark_idle_is_invocation_guarded() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let identity = current_identity();

        db.mark_session_running(SessionRuntimeRunningUpdate {
            session_id: "session-a",
            mode: "headless",
            invocation_uuid: "new-invocation",
            provider_name: Some("provider-a"),
            model_name: Some("model-a"),
            identity: &identity,
            pty_control_path: Some("/tmp/oulipoly-test.sock"),
            turn_start_max_mailbox_seq: None,
            models_dir: None,
            effective_cwd: None,
        })
        .unwrap();

        assert!(
            !db.mark_session_idle(SessionRuntimeIdleUpdate {
                session_id: "session-a",
                invocation_uuid: "old-invocation",
                last_exit_code: Some(0),
            })
            .unwrap()
        );
        assert_eq!(
            db.session_runtime("session-a").unwrap().unwrap().run_state,
            "running"
        );

        assert!(
            db.mark_session_idle(SessionRuntimeIdleUpdate {
                session_id: "session-a",
                invocation_uuid: "new-invocation",
                last_exit_code: Some(0),
            })
            .unwrap()
        );
        let row = db.session_runtime("session-a").unwrap().unwrap();
        assert_eq!(row.run_state, "idle");
        assert_eq!(row.last_exit_code, Some(0));
        assert!(row.turn_ended_at.is_some());
        assert!(row.running_invocation_uuid.is_none());
        assert!(row.running_os_pid.is_none());
        assert!(row.pty_control_path.is_none());
    }

    #[test]
    fn liveness_live_matching_identity_is_busy() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let identity = current_identity();
        db.mark_session_running(SessionRuntimeRunningUpdate {
            session_id: "session-a",
            mode: "headless",
            invocation_uuid: "invocation-a",
            provider_name: None,
            model_name: None,
            identity: &identity,
            pty_control_path: None,
            turn_start_max_mailbox_seq: None,
            models_dir: None,
            effective_cwd: None,
        })
        .unwrap();

        assert_eq!(
            db.session_liveness("session-a").unwrap(),
            SessionLiveness::Busy
        );
        assert_eq!(
            db.session_runtime("session-a").unwrap().unwrap().run_state,
            "running"
        );
    }

    #[test]
    fn liveness_dead_or_reused_identity_is_idle_and_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let mut identity = current_identity();
        identity.os_pid_starttime_ticks += 1;
        db.mark_session_running(SessionRuntimeRunningUpdate {
            session_id: "session-a",
            mode: "headless",
            invocation_uuid: "invocation-a",
            provider_name: None,
            model_name: None,
            identity: &identity,
            pty_control_path: Some("/tmp/stale.sock"),
            turn_start_max_mailbox_seq: None,
            models_dir: None,
            effective_cwd: None,
        })
        .unwrap();

        assert_eq!(
            db.session_liveness("session-a").unwrap(),
            SessionLiveness::Idle
        );
        let row = db.session_runtime("session-a").unwrap().unwrap();
        assert_eq!(row.run_state, "idle");
        assert!(row.running_invocation_uuid.is_none());
        assert!(row.running_os_pid.is_none());
        assert!(row.pty_control_path.is_none());
    }

    #[test]
    fn wake_idle_pending_acquires_claim() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        db.enqueue_agent_bash_complete(&input("handle-b", "session-a"))
            .unwrap();

        let result = db
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-a",
                reason: "notify_idle",
                auto_wake_count: 1,
                wake_invocation_uuid: Some("wake-a"),
                stale_after_seconds: 600,
            })
            .unwrap();

        let WakeClaimAcquireResult::Acquired(claim) = result else {
            panic!("expected acquired claim, got {result:?}");
        };
        assert_eq!(claim.session_id, "session-a");
        assert_eq!(claim.claim_token, "token-a");
        assert_eq!(claim.reason, "notify_idle");
        assert_eq!(claim.auto_wake_count, 1);
        assert_eq!(claim.min_pending_seq_at_claim, Some(1));
        assert_eq!(claim.max_pending_seq_at_claim, Some(2));
    }

    #[test]
    fn wake_claim_count_persists_on_session_runtime_after_claim_release() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.upsert_session_runtime(SessionRuntimeUpsert {
            session_id: "session-a",
            mode: "headless",
            invocation_uuid: None,
            provider_name: Some("provider-a"),
            model_name: Some("model-a"),
            pty_control_path: None,
            models_dir: Some("/tmp/models"),
            effective_cwd: None,
        })
        .unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();

        let result = db
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-a",
                reason: "notify_idle",
                auto_wake_count: 5,
                wake_invocation_uuid: Some("wake-a"),
                stale_after_seconds: 600,
            })
            .unwrap();

        assert!(matches!(result, WakeClaimAcquireResult::Acquired(_)));
        assert_eq!(
            db.session_runtime("session-a")
                .unwrap()
                .unwrap()
                .auto_wake_count,
            5
        );
        db.release_wake_claim("session-a", Some("token-a")).unwrap();
        let candidates = db.wake_sweep_candidates(600, 10).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].auto_wake_count, 6);
    }

    #[test]
    fn wake_busy_pending_skips_claim() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let identity = current_identity();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        db.mark_session_running(SessionRuntimeRunningUpdate {
            session_id: "session-a",
            mode: "headless",
            invocation_uuid: "invocation-a",
            provider_name: None,
            model_name: None,
            identity: &identity,
            pty_control_path: None,
            turn_start_max_mailbox_seq: None,
            models_dir: None,
            effective_cwd: None,
        })
        .unwrap();

        assert!(matches!(
            db.try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-a",
                reason: "notify_idle",
                auto_wake_count: 1,
                wake_invocation_uuid: None,
                stale_after_seconds: 600,
            })
            .unwrap(),
            WakeClaimAcquireResult::Busy
        ));
        assert!(db.wake_claim("session-a").unwrap().is_none());
    }

    #[test]
    fn wake_existing_claim_is_single_flight() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        let first = db
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-a",
                reason: "notify_idle",
                auto_wake_count: 1,
                wake_invocation_uuid: None,
                stale_after_seconds: 600,
            })
            .unwrap();
        assert!(matches!(first, WakeClaimAcquireResult::Acquired(_)));

        let second = db
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-b",
                reason: "notify_idle",
                auto_wake_count: 1,
                wake_invocation_uuid: None,
                stale_after_seconds: 600,
            })
            .unwrap();

        let WakeClaimAcquireResult::AlreadyInFlight(claim) = second else {
            panic!("expected already in flight, got {second:?}");
        };
        assert_eq!(claim.claim_token, "token-a");
    }

    #[test]
    fn wake_stale_claim_can_be_stolen() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        assert!(matches!(
            db.try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-a",
                reason: "notify_idle",
                auto_wake_count: 1,
                wake_invocation_uuid: None,
                stale_after_seconds: 600,
            })
            .unwrap(),
            WakeClaimAcquireResult::Acquired(_)
        ));
        db.force_wake_claim_age_for_test("session-a", 601).unwrap();

        let stolen = db
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-b",
                reason: "turn_end_recheck",
                auto_wake_count: 2,
                wake_invocation_uuid: None,
                stale_after_seconds: 600,
            })
            .unwrap();

        let WakeClaimAcquireResult::Acquired(claim) = stolen else {
            panic!("expected stolen claim, got {stolen:?}");
        };
        assert_eq!(claim.claim_token, "token-b");
        assert_eq!(claim.reason, "turn_end_recheck");
        assert_eq!(claim.auto_wake_count, 2);
    }

    #[test]
    fn wake_dead_pid_claim_can_be_stolen_before_ttl() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        assert!(matches!(
            db.try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-a",
                reason: "notify_idle",
                auto_wake_count: 1,
                wake_invocation_uuid: None,
                stale_after_seconds: 600,
            })
            .unwrap(),
            WakeClaimAcquireResult::Acquired(_)
        ));
        db.record_wake_claim_pid("session-a", "token-a", 999_999_999)
            .unwrap();

        let stolen = db
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-b",
                reason: "wake_reclaim_sweep",
                auto_wake_count: 2,
                wake_invocation_uuid: None,
                stale_after_seconds: 600,
            })
            .unwrap();

        let WakeClaimAcquireResult::Acquired(claim) = stolen else {
            panic!("expected dead PID claim to be stolen, got {stolen:?}");
        };
        assert_eq!(claim.claim_token, "token-b");
    }

    #[test]
    fn wake_live_identity_matched_claim_is_not_stolen_after_ttl() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        assert!(matches!(
            db.try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-a",
                reason: "notify_idle",
                auto_wake_count: 1,
                wake_invocation_uuid: None,
                stale_after_seconds: 600,
            })
            .unwrap(),
            WakeClaimAcquireResult::Acquired(_)
        ));
        let identity = current_identity();
        let sidecar = pid_identity::PidIdentityDb::open(db.path()).unwrap();
        sidecar
            .record_identity(pid_identity::PidIdentityRecord {
                identity: &identity,
                os_pgid: None,
                invocation_uuid: "token-a",
                session_id: Some("session-a"),
                provider_name: Some("wake"),
                model_name: Some("model-a"),
                recorded_at: "2026-06-08T00:00:00Z",
            })
            .unwrap();
        db.record_wake_claim_pid("session-a", "token-a", identity.os_pid)
            .unwrap();
        db.force_wake_claim_age_for_test("session-a", 601).unwrap();

        let result = db
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-b",
                reason: "wake_reclaim_sweep",
                auto_wake_count: 2,
                wake_invocation_uuid: None,
                stale_after_seconds: 600,
            })
            .unwrap();

        let WakeClaimAcquireResult::AlreadyInFlight(claim) = result else {
            panic!("expected live identity-matched claim to remain in flight, got {result:?}");
        };
        assert_eq!(claim.claim_token, "token-a");
    }

    #[test]
    fn mailbox_operations_do_not_change_state_db_schema() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("state.db");
        let state = StateDb::open(&state_path).unwrap();
        let baseline_version = user_version(state.connection());
        let baseline_columns = invocation_columns(state.connection());
        drop(state);

        let mut mailbox = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row =
            inserted_row(mailbox.enqueue_agent_bash_complete(&input("handle-a", "session-a")));
        mailbox
            .mark_delivered("session-a", &[row.seq], "resume-1")
            .unwrap();
        let identity = current_identity();
        mailbox
            .mark_session_running(SessionRuntimeRunningUpdate {
                session_id: "session-a",
                mode: "headless",
                invocation_uuid: "invocation-a",
                provider_name: Some("provider-a"),
                model_name: Some("model-a"),
                identity: &identity,
                pty_control_path: None,
                turn_start_max_mailbox_seq: None,
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();
        mailbox
            .mark_session_idle(SessionRuntimeIdleUpdate {
                session_id: "session-a",
                invocation_uuid: "invocation-a",
                last_exit_code: Some(0),
            })
            .unwrap();
        assert!(matches!(
            mailbox
                .try_acquire_wake_claim(WakeClaimRequest {
                    session_id: "session-a",
                    claim_token: "token-a",
                    reason: "notify_idle",
                    auto_wake_count: 1,
                    wake_invocation_uuid: None,
                    stale_after_seconds: 600,
                })
                .unwrap(),
            WakeClaimAcquireResult::NoPending
        ));
        drop(mailbox);

        let state = StateDb::open(&state_path).unwrap();
        assert_eq!(user_version(state.connection()), baseline_version);
        assert_eq!(invocation_columns(state.connection()), baseline_columns);
    }

    fn inserted_row(result: Result<EnqueueResult, String>) -> MailboxRow {
        let result = result.unwrap();
        assert_inserted_result(&result);
        inserted_result_row(result)
    }

    fn assert_inserted_result(result: &EnqueueResult) {
        if !matches!(result, EnqueueResult::Inserted(_)) {
            panic!("expected inserted row, got {result:?}");
        }
    }

    fn inserted_result_row(result: EnqueueResult) -> MailboxRow {
        let EnqueueResult::Inserted(row) = result else {
            unreachable!("inserted result validated above");
        };
        row
    }

    fn current_identity() -> ProcessIdentity {
        expect_current_identity(read_current_identity().unwrap())
    }

    fn read_current_identity() -> Result<Option<ProcessIdentity>, String> {
        pid_identity::read_live_process_identity(std::process::id().into())
    }

    fn expect_current_identity(identity: Option<ProcessIdentity>) -> ProcessIdentity {
        identity.expect("test process should have a live identity")
    }

    fn user_version(conn: &Connection) -> i64 {
        conn.query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap()
    }

    fn invocation_columns(conn: &Connection) -> Vec<String> {
        read_invocation_columns(conn).unwrap()
    }

    fn read_invocation_columns(conn: &Connection) -> rusqlite::Result<Vec<String>> {
        let mut stmt = conn.prepare("PRAGMA table_info(invocations)").unwrap();
        stmt.query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
    }
}
