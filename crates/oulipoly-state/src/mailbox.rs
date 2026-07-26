//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `parser`,
//! `predicate`, `validator`
//!
//! Resume-backed notification mailbox storage in the PID identity sidecar DB.
//! This module deliberately owns only additive sidecar tables and never touches
//! the versioned `state.db` schema.

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::pid_identity::{self, ProcessIdentity};

pub const AGENT_BASH_COMPLETE_KIND: &str = "agent_bash_complete";
pub const MAILBOX_DELIVERY_UNCONFIRMED_ERROR: &str = "mailbox_delivery_unconfirmed";
pub const MAX_UNCONFIRMED_DELIVERY_ATTEMPTS: i64 = 2;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailboxDeliveryAttemptDisposition {
    Pending,
    Resolved,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxDeliveryWindow {
    pub attempt_id: String,
    pub session_id: String,
    pub delivery_invocation_uuid: String,
    pub acknowledged_at: Option<String>,
    pub resolved_at: Option<String>,
    pub rows: Vec<MailboxRow>,
    pub remaining_count: usize,
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

    pub fn notifications_paused(&self, session_id: &str) -> Result<bool, String> {
        self.conn
            .query_row(
                "SELECT paused FROM mailbox_notification_control WHERE session_id = ?1",
                params![session_id],
                |row| row.get::<_, bool>(0),
            )
            .optional()
            .map(|paused| paused.unwrap_or(false))
            .map_err(|err| format!("Failed to query mailbox notification control: {err}"))
    }

    pub fn set_notifications_paused(
        &mut self,
        session_id: &str,
        paused: bool,
    ) -> Result<(), String> {
        let now = now_rfc3339();
        self.conn
            .execute(
                "INSERT INTO mailbox_notification_control (session_id, paused, updated_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(session_id) DO UPDATE SET
                    paused = excluded.paused,
                    updated_at = excluded.updated_at",
                params![session_id, paused, &now],
            )
            .map(|_| ())
            .map_err(|err| format!("Failed to update mailbox notification control: {err}"))
    }

    pub fn acknowledge_range(
        &mut self,
        session_id: &str,
        from_seq: i64,
        to_seq: i64,
        delivered_by: &str,
    ) -> Result<usize, String> {
        if from_seq > to_seq {
            return Err(format!(
                "Mailbox acknowledgement range is reversed: {from_seq} > {to_seq}"
            ));
        }
        let now = now_rfc3339();
        let tx = self.conn.transaction().map_err(|err| {
            format!("Failed to start mailbox range acknowledgement transaction: {err}")
        })?;
        let changed = tx
            .execute(
                "UPDATE mailbox
                 SET delivered_at = ?4,
                     delivered_by_invocation_uuid = ?5,
                     delivery_attempts = delivery_attempts + 1,
                     delivery_error = NULL
                 WHERE session_id = ?1
                   AND seq >= ?2
                   AND seq <= ?3
                   AND delivered_at IS NULL",
                params![session_id, from_seq, to_seq, &now, delivered_by],
            )
            .map_err(|err| format!("Failed to acknowledge mailbox range: {err}"))?;
        resolve_completed_delivery_attempts(&tx, session_id, &now, None)?;
        tx.commit().map_err(|err| {
            format!("Failed to commit mailbox range acknowledgement transaction: {err}")
        })?;
        Ok(changed)
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
        resolve_completed_delivery_attempts(&tx, session_id, &now, None)?;
        tx.commit()
            .map_err(|err| format!("Failed to commit mailbox delivery transaction: {err}"))
    }

    pub fn register_delivery_attempt(
        &mut self,
        attempt_id: &str,
        session_id: &str,
        delivery_invocation_uuid: &str,
        seqs: &[i64],
        remaining_count: usize,
    ) -> Result<(), String> {
        if seqs.is_empty() {
            return Err("Cannot register an empty mailbox delivery attempt".to_string());
        }
        let now = now_rfc3339();
        let tx = self.conn.transaction().map_err(|err| {
            format!("Failed to start mailbox delivery attempt transaction: {err}")
        })?;
        tx.execute(
            "UPDATE mailbox_delivery_attempts
             SET resolved_at = ?3
             WHERE session_id = ?1
               AND delivery_invocation_uuid != ?2
               AND acknowledged_at IS NULL
               AND resolved_at IS NULL",
            params![session_id, delivery_invocation_uuid, &now],
        )
        .map_err(|err| {
            format!("Failed to resolve prior unacknowledged mailbox deliveries: {err}")
        })?;
        tx.execute(
            "INSERT INTO mailbox_delivery_attempts (
                attempt_id, session_id, delivery_invocation_uuid, created_at,
                prepared_remaining_count
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                attempt_id,
                session_id,
                delivery_invocation_uuid,
                &now,
                remaining_count as i64
            ],
        )
        .map_err(|err| format!("Failed to insert mailbox delivery attempt: {err}"))?;
        for seq in seqs {
            let belongs_to_session = tx
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM mailbox
                        WHERE seq = ?1 AND session_id = ?2
                     )",
                    params![seq, session_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(|err| format!("Failed to validate mailbox delivery item: {err}"))?;
            if !belongs_to_session {
                return Err(format!(
                    "Mailbox delivery item {seq} does not belong to session {session_id}"
                ));
            }
            tx.execute(
                "INSERT INTO mailbox_delivery_attempt_items (attempt_id, mailbox_seq)
                 VALUES (?1, ?2)",
                params![attempt_id, seq],
            )
            .map_err(|err| format!("Failed to insert mailbox delivery attempt item: {err}"))?;
        }
        resolve_completed_delivery_attempts(&tx, session_id, &now, None)?;
        tx.commit()
            .map_err(|err| format!("Failed to commit mailbox delivery attempt: {err}"))
    }

    pub fn register_or_reuse_delivery_attempt(
        &mut self,
        attempt_id: &str,
        session_id: &str,
        delivery_invocation_uuid: &str,
        seqs: &[i64],
        remaining_count: usize,
    ) -> Result<String, String> {
        if seqs.is_empty() {
            return Err("Cannot register an empty mailbox delivery attempt".to_string());
        }
        let now = now_rfc3339();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|err| format!("Failed to start mailbox delivery claim transaction: {err}"))?;
        resolve_completed_delivery_attempts(&tx, session_id, &now, None)?;
        let existing = tx
            .query_row(
                "SELECT attempt_id
                 FROM mailbox_delivery_attempts
                 WHERE session_id = ?1
                   AND delivery_invocation_uuid = ?2
                   AND resolved_at IS NULL
                 ORDER BY created_at, attempt_id
                 LIMIT 1",
                params![session_id, delivery_invocation_uuid],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|err| format!("Failed to query reusable mailbox delivery attempt: {err}"))?;
        if let Some(existing) = existing {
            tx.commit().map_err(|err| {
                format!("Failed to commit reused mailbox delivery attempt: {err}")
            })?;
            return Ok(existing);
        }
        tx.execute(
            "UPDATE mailbox_delivery_attempts
             SET resolved_at = ?3
             WHERE session_id = ?1
               AND delivery_invocation_uuid != ?2
               AND acknowledged_at IS NULL
               AND resolved_at IS NULL",
            params![session_id, delivery_invocation_uuid, &now],
        )
        .map_err(|err| {
            format!("Failed to resolve prior unacknowledged mailbox deliveries: {err}")
        })?;
        tx.execute(
            "INSERT INTO mailbox_delivery_attempts (
                attempt_id, session_id, delivery_invocation_uuid, created_at,
                prepared_remaining_count
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                attempt_id,
                session_id,
                delivery_invocation_uuid,
                &now,
                remaining_count as i64
            ],
        )
        .map_err(|err| format!("Failed to insert mailbox delivery attempt: {err}"))?;
        for seq in seqs {
            let belongs_to_session = tx
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM mailbox
                        WHERE seq = ?1 AND session_id = ?2
                     )",
                    params![seq, session_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(|err| format!("Failed to validate mailbox delivery item: {err}"))?;
            if !belongs_to_session {
                return Err(format!(
                    "Mailbox delivery item {seq} does not belong to session {session_id}"
                ));
            }
            tx.execute(
                "INSERT INTO mailbox_delivery_attempt_items (attempt_id, mailbox_seq)
                 VALUES (?1, ?2)",
                params![attempt_id, seq],
            )
            .map_err(|err| format!("Failed to insert mailbox delivery attempt item: {err}"))?;
        }
        tx.commit()
            .map_err(|err| format!("Failed to commit mailbox delivery claim: {err}"))?;
        Ok(attempt_id.to_string())
    }

    pub fn delivery_attempt_disposition(
        &self,
        attempt_id: &str,
    ) -> Result<Option<MailboxDeliveryAttemptDisposition>, String> {
        let Some((total, pending)) = self
            .conn
            .query_row(
                "SELECT COUNT(*),
                        SUM(CASE WHEN mailbox.delivered_at IS NULL THEN 1 ELSE 0 END)
                 FROM mailbox_delivery_attempts AS attempts
                 JOIN mailbox_delivery_attempt_items AS items
                   ON items.attempt_id = attempts.attempt_id
                 JOIN mailbox ON mailbox.seq = items.mailbox_seq
                 WHERE attempts.attempt_id = ?1
                 GROUP BY attempts.attempt_id",
                params![attempt_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(|err| format!("Failed to query mailbox delivery attempt: {err}"))?
        else {
            return Ok(None);
        };
        let disposition = if pending == 0 {
            MailboxDeliveryAttemptDisposition::Resolved
        } else if pending == total {
            MailboxDeliveryAttemptDisposition::Pending
        } else {
            MailboxDeliveryAttemptDisposition::Stale
        };
        Ok(Some(disposition))
    }

    pub fn delivery_attempt_window(
        &self,
        attempt_id: &str,
    ) -> Result<Option<MailboxDeliveryWindow>, String> {
        let Some((session_id, delivery_invocation_uuid, acknowledged_at, resolved_at)) = self
            .conn
            .query_row(
                "SELECT session_id, delivery_invocation_uuid, acknowledged_at, resolved_at
                 FROM mailbox_delivery_attempts WHERE attempt_id = ?1",
                params![attempt_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|err| format!("Failed to query mailbox delivery window owner: {err}"))?
        else {
            return Ok(None);
        };
        let mut stmt = self
            .conn
            .prepare(
                "SELECT mailbox.seq, mailbox.session_id, mailbox.kind, mailbox.handle,
                        mailbox.payload_json, mailbox.enqueued_at, mailbox.delivered_at,
                        mailbox.delivered_by_invocation_uuid, mailbox.delivery_attempts,
                        mailbox.delivery_error, mailbox.owner_invocation_uuid,
                        mailbox.matched_os_pid, mailbox.matched_os_boot_id,
                        mailbox.matched_os_pid_starttime_ticks, mailbox.matched_chain_index,
                        mailbox.state_dir, mailbox.meta_path, mailbox.log_path,
                        mailbox.rc_path, mailbox.rc
                 FROM mailbox_delivery_attempt_items AS items
                 JOIN mailbox ON mailbox.seq = items.mailbox_seq
                 WHERE items.attempt_id = ?1 AND mailbox.delivered_at IS NULL
                 ORDER BY mailbox.seq",
            )
            .map_err(|err| format!("Failed to prepare mailbox delivery window query: {err}"))?;
        let rows = stmt
            .query_map(params![attempt_id], map_mailbox_row)
            .map_err(|err| format!("Failed to query mailbox delivery window: {err}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("Failed to read mailbox delivery window: {err}"))?
            .into_iter()
            .filter(mailbox_row_is_deliverable_pending)
            .collect::<Vec<_>>();
        let pending_count = self
            .list_pending(&session_id)?
            .into_iter()
            .filter(mailbox_row_is_deliverable_pending)
            .count();
        Ok(Some(MailboxDeliveryWindow {
            attempt_id: attempt_id.to_string(),
            session_id,
            delivery_invocation_uuid,
            acknowledged_at,
            resolved_at,
            remaining_count: pending_count.saturating_sub(rows.len()),
            rows,
        }))
    }

    pub fn accepted_delivery_attempt_windows(
        &self,
        session_id: &str,
    ) -> Result<Vec<MailboxDeliveryWindow>, String> {
        let oldest_deliverable_seq = self
            .list_pending(session_id)?
            .into_iter()
            .find(mailbox_row_is_deliverable_pending)
            .map(|row| row.seq);
        let Some(oldest_deliverable_seq) = oldest_deliverable_seq else {
            return Ok(Vec::new());
        };
        let mut stmt = self
            .conn
            .prepare(
                "SELECT attempts.attempt_id
                 FROM mailbox_delivery_attempts AS attempts
                 WHERE attempts.session_id = ?1
                   AND attempts.acknowledged_at IS NOT NULL
                   AND attempts.resolved_at IS NULL
                    AND EXISTS (
                        SELECT 1
                        FROM mailbox_delivery_attempt_items AS prefix_items
                        WHERE prefix_items.attempt_id = attempts.attempt_id
                          AND prefix_items.mailbox_seq = ?2
                    )
                  ORDER BY attempts.acknowledged_at, attempts.created_at, attempts.attempt_id",
            )
            .map_err(|err| format!("Failed to prepare accepted delivery attempt query: {err}"))?;
        let attempt_ids = stmt
            .query_map(params![session_id, oldest_deliverable_seq], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|err| format!("Failed to query accepted delivery attempts: {err}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("Failed to read accepted delivery attempts: {err}"))?;
        drop(stmt);
        attempt_ids
            .into_iter()
            .map(|attempt_id| {
                self.delivery_attempt_window(&attempt_id)?.ok_or_else(|| {
                    format!("Accepted mailbox delivery attempt {attempt_id} disappeared")
                })
            })
            .collect()
    }

    pub fn record_delivery_attempt_transport_ack(
        &mut self,
        attempt_id: &str,
    ) -> Result<bool, String> {
        let now = now_rfc3339();
        self.conn
            .execute(
                "UPDATE mailbox_delivery_attempts
                 SET acknowledged_at = COALESCE(acknowledged_at, ?2)
                 WHERE attempt_id = ?1
                   AND resolved_at IS NULL",
                params![attempt_id, &now],
            )
            .map(|changed| changed > 0)
            .map_err(|err| format!("Failed to record mailbox delivery transport ACK: {err}"))
    }

    pub fn resolve_unacknowledged_delivery_attempt(
        &mut self,
        attempt_id: &str,
    ) -> Result<bool, String> {
        let now = now_rfc3339();
        self.conn
            .execute(
                "UPDATE mailbox_delivery_attempts
                 SET resolved_at = ?2
                 WHERE attempt_id = ?1
                   AND acknowledged_at IS NULL
                   AND resolved_at IS NULL",
                params![attempt_id, &now],
            )
            .map(|changed| changed > 0)
            .map_err(|err| format!("Failed to resolve unacknowledged mailbox delivery: {err}"))
    }

    pub fn confirm_delivery_attempt(&mut self, attempt_id: &str) -> Result<bool, String> {
        let now = now_rfc3339();
        let tx = self.conn.transaction().map_err(|err| {
            format!("Failed to start mailbox delivery confirmation transaction: {err}")
        })?;
        let Some((session_id, delivery_invocation_uuid)) = tx
            .query_row(
                "SELECT session_id, delivery_invocation_uuid
                 FROM mailbox_delivery_attempts
                 WHERE attempt_id = ?1 AND acknowledged_at IS NOT NULL",
                params![attempt_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|err| format!("Failed to query confirmed delivery attempt owner: {err}"))?
        else {
            return Ok(false);
        };
        tx.execute(
            "UPDATE mailbox
             SET delivered_at = ?3,
                 delivered_by_invocation_uuid = ?4,
                 delivery_attempts = delivery_attempts + 1,
                 delivery_error = NULL
             WHERE session_id = ?1
               AND delivered_at IS NULL
               AND seq IN (
                   SELECT mailbox_seq
                   FROM mailbox_delivery_attempt_items
                   WHERE attempt_id = ?2
               )",
            params![&session_id, attempt_id, &now, &delivery_invocation_uuid],
        )
        .map_err(|err| format!("Failed to confirm mailbox delivery items: {err}"))?;
        resolve_completed_delivery_attempts(&tx, &session_id, &now, Some(attempt_id))?;
        tx.commit()
            .map_err(|err| format!("Failed to commit mailbox delivery confirmation: {err}"))?;
        Ok(true)
    }

    pub fn fail_unobserved_delivery_attempt(
        &mut self,
        attempt_id: &str,
        delivery_error: &str,
    ) -> Result<bool, String> {
        let now = now_rfc3339();
        let tx = self.conn.transaction().map_err(|err| {
            format!("Failed to start unobserved mailbox delivery transaction: {err}")
        })?;
        let session_id = tx
            .query_row(
                "SELECT session_id
                 FROM mailbox_delivery_attempts
                 WHERE attempt_id = ?1
                   AND acknowledged_at IS NOT NULL
                   AND resolved_at IS NULL",
                params![attempt_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|err| format!("Failed to query unobserved mailbox delivery: {err}"))?;
        let Some(session_id) = session_id else {
            return Ok(false);
        };
        tx.execute(
            "UPDATE mailbox
             SET delivery_attempts = delivery_attempts + 1,
                 delivery_error = ?3
             WHERE session_id = ?1
               AND delivered_at IS NULL
               AND seq IN (
                   SELECT mailbox_seq
                   FROM mailbox_delivery_attempt_items
                   WHERE attempt_id = ?2
               )",
            params![&session_id, attempt_id, delivery_error],
        )
        .map_err(|err| format!("Failed to mark unobserved mailbox delivery rows: {err}"))?;
        tx.execute(
            "UPDATE mailbox_delivery_attempts
             SET resolved_at = ?2
             WHERE attempt_id = ?1 AND resolved_at IS NULL",
            params![attempt_id, &now],
        )
        .map_err(|err| format!("Failed to resolve unobserved mailbox delivery: {err}"))?;
        tx.commit()
            .map_err(|err| format!("Failed to commit unobserved mailbox delivery: {err}"))?;
        Ok(true)
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

pub fn mailbox_row_is_deliverable_pending(row: &MailboxRow) -> bool {
    row.delivered_at.is_none()
        && row.delivery_error.as_deref() != Some(WAKE_SWEEP_ABANDONED_ERROR)
        && (row.delivery_error.as_deref() != Some(MAILBOX_DELIVERY_UNCONFIRMED_ERROR)
            || row.delivery_attempts < MAX_UNCONFIRMED_DELIVERY_ATTEMPTS)
}

fn resolve_completed_delivery_attempts(
    tx: &Transaction<'_>,
    session_id: &str,
    resolved_at: &str,
    resolved_by_attempt_id: Option<&str>,
) -> Result<(), String> {
    tx.execute(
        "UPDATE mailbox_delivery_attempts AS attempt
         SET resolved_at = COALESCE(resolved_at, ?2),
             resolved_by_attempt_id = COALESCE(resolved_by_attempt_id, ?3)
         WHERE session_id = ?1
           AND resolved_at IS NULL
           AND NOT EXISTS (
                 SELECT 1
                 FROM mailbox_delivery_attempt_items AS unresolved
                 JOIN mailbox ON mailbox.seq = unresolved.mailbox_seq
                 WHERE unresolved.attempt_id = attempt.attempt_id
                   AND mailbox.delivered_at IS NULL
             )",
        params![session_id, resolved_at, resolved_by_attempt_id],
    )
    .map(|_| ())
    .map_err(|err| format!("Failed to resolve completed mailbox delivery attempts: {err}"))
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

        CREATE TABLE IF NOT EXISTS mailbox_delivery_attempts (
            attempt_id                    TEXT PRIMARY KEY,
            session_id                    TEXT NOT NULL,
            delivery_invocation_uuid      TEXT NOT NULL,
            created_at                    TEXT NOT NULL,
            prepared_remaining_count      INTEGER NOT NULL,
            acknowledged_at               TEXT,
            resolved_at                   TEXT,
            resolved_by_attempt_id        TEXT
        );

        CREATE TABLE IF NOT EXISTS mailbox_delivery_attempt_items (
            attempt_id                    TEXT NOT NULL,
            mailbox_seq                   INTEGER NOT NULL,
            PRIMARY KEY(attempt_id, mailbox_seq),
            FOREIGN KEY(attempt_id) REFERENCES mailbox_delivery_attempts(attempt_id),
            FOREIGN KEY(mailbox_seq) REFERENCES mailbox(seq)
        );

        CREATE INDEX IF NOT EXISTS idx_mailbox_delivery_attempt_items_seq
            ON mailbox_delivery_attempt_items(mailbox_seq, attempt_id);

        CREATE TABLE IF NOT EXISTS mailbox_notification_control (
            session_id                    TEXT PRIMARY KEY,
            paused                       INTEGER NOT NULL DEFAULT 0,
            updated_at                   TEXT NOT NULL
        );

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
    fn late_attempt_confirmation_contracts_overlapping_delivery_window() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let rows = (1..=6)
            .map(|index| {
                inserted_row(
                    db.enqueue_agent_bash_complete(&input(&format!("handle-{index}"), "session-a")),
                )
            })
            .collect::<Vec<_>>();
        let first_window = rows[..3].iter().map(|row| row.seq).collect::<Vec<_>>();
        let expanded_window = rows.iter().map(|row| row.seq).collect::<Vec<_>>();
        db.register_delivery_attempt("attempt-1", "session-a", "invocation-a", &first_window, 3)
            .unwrap();
        db.register_delivery_attempt(
            "attempt-2",
            "session-a",
            "invocation-a",
            &expanded_window,
            0,
        )
        .unwrap();
        assert!(
            db.record_delivery_attempt_transport_ack("attempt-1")
                .unwrap()
        );
        assert!(db.confirm_delivery_attempt("attempt-1").unwrap());

        assert_eq!(db.list_pending("session-a").unwrap().len(), 3);
        assert_eq!(
            db.delivery_attempt_disposition("attempt-1").unwrap(),
            Some(MailboxDeliveryAttemptDisposition::Resolved)
        );
        assert_eq!(
            db.delivery_attempt_disposition("attempt-2").unwrap(),
            Some(MailboxDeliveryAttemptDisposition::Stale)
        );
        let contracted = db.delivery_attempt_window("attempt-2").unwrap().unwrap();
        assert_eq!(contracted.rows.len(), 3);
        assert_eq!(contracted.rows[0].handle, "handle-4");
        assert_eq!(contracted.remaining_count, 0);
        assert!(db.confirm_delivery_attempt("attempt-1").unwrap());
        assert_eq!(db.list_pending("session-a").unwrap().len(), 3);
        assert!(
            db.list_mailbox("session-a", true)
                .unwrap()
                .iter()
                .take(3)
                .all(|row| row.delivery_attempts == 1)
        );
        assert!(
            db.list_mailbox("session-a", true)
                .unwrap()
                .iter()
                .take(3)
                .all(|row| row.delivered_by_invocation_uuid.as_deref() == Some("invocation-a"))
        );
    }

    #[test]
    fn confirmation_from_any_retry_resolves_notification_roots_and_siblings() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let rows = ["handle-a", "handle-b"].map(|handle| {
            inserted_row(db.enqueue_agent_bash_complete(&input(handle, "session-a")))
        });
        let seqs = rows.iter().map(|row| row.seq).collect::<Vec<_>>();
        db.register_delivery_attempt("attempt-1", "session-a", "invocation-a", &seqs, 0)
            .unwrap();
        db.register_delivery_attempt("attempt-2", "session-a", "invocation-a", &seqs, 0)
            .unwrap();
        assert!(
            db.record_delivery_attempt_transport_ack("attempt-2")
                .unwrap()
        );
        assert!(db.confirm_delivery_attempt("attempt-2").unwrap());

        assert!(db.list_pending("session-a").unwrap().is_empty());
        assert_eq!(
            db.delivery_attempt_disposition("attempt-1").unwrap(),
            Some(MailboxDeliveryAttemptDisposition::Resolved)
        );
        assert_eq!(
            db.delivery_attempt_disposition("attempt-2").unwrap(),
            Some(MailboxDeliveryAttemptDisposition::Resolved)
        );
    }

    #[test]
    fn retry_registration_after_late_confirmation_is_resolved_without_redelivery() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));
        let seqs = [row.seq];
        db.register_delivery_attempt("attempt-1", "session-a", "invocation-a", &seqs, 0)
            .unwrap();
        db.record_delivery_attempt_transport_ack("attempt-1")
            .unwrap();
        db.confirm_delivery_attempt("attempt-1").unwrap();

        db.register_delivery_attempt("attempt-2", "session-a", "invocation-a", &seqs, 0)
            .unwrap();

        let window = db.delivery_attempt_window("attempt-2").unwrap().unwrap();
        assert!(window.rows.is_empty());
        assert_eq!(
            db.delivery_attempt_disposition("attempt-2").unwrap(),
            Some(MailboxDeliveryAttemptDisposition::Resolved)
        );
        assert!(
            !db.record_delivery_attempt_transport_ack("attempt-2")
                .unwrap()
        );
        let delivered = db.list_mailbox("session-a", true).unwrap().remove(0);
        assert_eq!(delivered.delivery_attempts, 1);
        assert_eq!(
            delivered.delivered_by_invocation_uuid.as_deref(),
            Some("invocation-a")
        );
    }

    #[test]
    fn transport_acknowledgement_is_nonterminal_and_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));
        db.register_delivery_attempt("attempt-1", "session-a", "invocation-a", &[row.seq], 0)
            .unwrap();

        assert!(
            db.record_delivery_attempt_transport_ack("attempt-1")
                .unwrap()
        );
        let first_ack = db
            .delivery_attempt_window("attempt-1")
            .unwrap()
            .unwrap()
            .acknowledged_at;
        assert!(first_ack.is_some());
        assert!(
            db.record_delivery_attempt_transport_ack("attempt-1")
                .unwrap()
        );
        let window = db.delivery_attempt_window("attempt-1").unwrap().unwrap();
        assert_eq!(window.acknowledged_at, first_ack);
        assert_eq!(window.rows, vec![row.clone()]);
        let persisted = db.list_mailbox("session-a", true).unwrap().remove(0);
        assert!(persisted.delivered_at.is_none());
        assert!(persisted.delivered_by_invocation_uuid.is_none());
        assert_eq!(persisted.delivery_attempts, 0);
        assert!(persisted.delivery_error.is_none());
        let (resolved_at, resolved_by): (Option<String>, Option<String>) = db
            .connection()
            .query_row(
                "SELECT resolved_at, resolved_by_attempt_id
                 FROM mailbox_delivery_attempts WHERE attempt_id = 'attempt-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(resolved_at.is_none());
        assert!(resolved_by.is_none());
        assert!(
            !db.record_delivery_attempt_transport_ack("missing-attempt")
                .unwrap()
        );
    }

    #[test]
    fn protocol_retry_reuses_unresolved_delivery_attempt() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));

        let first = db
            .register_or_reuse_delivery_attempt(
                "attempt-1",
                "session-a",
                "invocation-a",
                &[row.seq],
                0,
            )
            .unwrap();
        let retry = db
            .register_or_reuse_delivery_attempt(
                "attempt-2",
                "session-a",
                "invocation-a",
                &[row.seq],
                0,
            )
            .unwrap();

        assert_eq!(first, "attempt-1");
        assert_eq!(retry, first);
        assert!(db.delivery_attempt_window("attempt-2").unwrap().is_none());
        assert!(
            db.delivery_attempt_window(&retry)
                .unwrap()
                .is_some_and(|window| window.resolved_at.is_none())
        );
    }

    #[test]
    fn unacknowledged_attempt_resolution_is_terminal_but_never_resolves_an_ack() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));
        db.register_delivery_attempt("attempt-1", "session-a", "invocation-a", &[row.seq], 0)
            .unwrap();

        assert!(
            db.resolve_unacknowledged_delivery_attempt("attempt-1")
                .unwrap()
        );
        assert!(
            !db.resolve_unacknowledged_delivery_attempt("attempt-1")
                .unwrap()
        );
        assert!(
            !db.record_delivery_attempt_transport_ack("attempt-1")
                .unwrap()
        );

        db.register_delivery_attempt("attempt-2", "session-a", "invocation-a", &[row.seq], 0)
            .unwrap();
        db.record_delivery_attempt_transport_ack("attempt-2")
            .unwrap();
        assert!(
            !db.resolve_unacknowledged_delivery_attempt("attempt-2")
                .unwrap()
        );
        assert_eq!(
            db.accepted_delivery_attempt_windows("session-a")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn new_invocation_registration_resolves_only_prior_unacknowledged_attempts() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));
        db.register_delivery_attempt("old-unacked", "session-a", "old-invocation", &[row.seq], 0)
            .unwrap();
        db.register_delivery_attempt("old-acked", "session-a", "old-invocation", &[row.seq], 0)
            .unwrap();
        db.record_delivery_attempt_transport_ack("old-acked")
            .unwrap();

        db.register_delivery_attempt(
            "current-first",
            "session-a",
            "current-invocation",
            &[row.seq],
            0,
        )
        .unwrap();
        db.register_delivery_attempt(
            "current-second",
            "session-a",
            "current-invocation",
            &[row.seq],
            0,
        )
        .unwrap();

        let resolved_at = |attempt_id: &str| -> Option<String> {
            db.connection()
                .query_row(
                    "SELECT resolved_at FROM mailbox_delivery_attempts WHERE attempt_id = ?1",
                    params![attempt_id],
                    |row| row.get(0),
                )
                .unwrap()
        };
        assert!(resolved_at("old-unacked").is_some());
        assert!(resolved_at("old-acked").is_none());
        assert!(resolved_at("current-first").is_none());
        assert!(resolved_at("current-second").is_none());
    }

    #[test]
    fn provider_confirmation_marks_only_pending_items_and_resolves_attempts() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let rows = ["handle-a", "handle-b"].map(|handle| {
            inserted_row(db.enqueue_agent_bash_complete(&input(handle, "session-a")))
        });
        let seqs = rows.iter().map(|row| row.seq).collect::<Vec<_>>();
        db.register_delivery_attempt("attempt-1", "session-a", "invocation-a", &seqs, 0)
            .unwrap();
        assert!(!db.confirm_delivery_attempt("attempt-1").unwrap());
        db.mark_delivered("session-a", &[rows[0].seq], "sibling-invocation")
            .unwrap();
        db.record_delivery_attempt_transport_ack("attempt-1")
            .unwrap();

        assert!(db.confirm_delivery_attempt("attempt-1").unwrap());
        assert!(db.confirm_delivery_attempt("attempt-1").unwrap());
        let delivered = db.list_mailbox("session-a", true).unwrap();
        assert_eq!(delivered[0].delivery_attempts, 1);
        assert_eq!(
            delivered[0].delivered_by_invocation_uuid.as_deref(),
            Some("sibling-invocation")
        );
        assert_eq!(delivered[1].delivery_attempts, 1);
        assert_eq!(
            delivered[1].delivered_by_invocation_uuid.as_deref(),
            Some("invocation-a")
        );
        let (resolved_at, resolved_by): (Option<String>, Option<String>) = db
            .connection()
            .query_row(
                "SELECT resolved_at, resolved_by_attempt_id
                 FROM mailbox_delivery_attempts WHERE attempt_id = 'attempt-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(resolved_at.is_some());
        assert_eq!(resolved_by.as_deref(), Some("attempt-1"));
    }

    #[test]
    fn unobserved_delivery_failure_releases_owner_and_records_one_retry() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));
        db.register_delivery_attempt("attempt-1", "session-a", "invocation-a", &[row.seq], 0)
            .unwrap();

        assert!(
            !db.fail_unobserved_delivery_attempt("attempt-1", MAILBOX_DELIVERY_UNCONFIRMED_ERROR)
                .unwrap()
        );
        db.record_delivery_attempt_transport_ack("attempt-1")
            .unwrap();
        assert!(
            db.fail_unobserved_delivery_attempt("attempt-1", MAILBOX_DELIVERY_UNCONFIRMED_ERROR)
                .unwrap()
        );
        assert!(
            !db.fail_unobserved_delivery_attempt("attempt-1", MAILBOX_DELIVERY_UNCONFIRMED_ERROR)
                .unwrap()
        );

        let pending = db.list_pending("session-a").unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].delivery_attempts, 1);
        assert_eq!(
            pending[0].delivery_error.as_deref(),
            Some(MAILBOX_DELIVERY_UNCONFIRMED_ERROR)
        );
        assert!(
            db.accepted_delivery_attempt_windows("session-a")
                .unwrap()
                .is_empty()
        );
        let resolved_at: Option<String> = db
            .connection()
            .query_row(
                "SELECT resolved_at FROM mailbox_delivery_attempts WHERE attempt_id = 'attempt-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(resolved_at.is_some());
    }

    #[test]
    fn accepted_attempt_owner_requires_transport_ack_and_oldest_pending_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let rows = ["handle-a", "handle-b"].map(|handle| {
            inserted_row(db.enqueue_agent_bash_complete(&input(handle, "session-a")))
        });
        db.register_delivery_attempt(
            "prefix-attempt",
            "session-a",
            "invocation-a",
            &[rows[0].seq],
            1,
        )
        .unwrap();
        db.register_delivery_attempt(
            "suffix-attempt",
            "session-a",
            "invocation-a",
            &[rows[1].seq],
            0,
        )
        .unwrap();
        db.record_delivery_attempt_transport_ack("suffix-attempt")
            .unwrap();
        assert!(
            db.accepted_delivery_attempt_windows("session-a")
                .unwrap()
                .is_empty()
        );

        db.record_delivery_attempt_transport_ack("prefix-attempt")
            .unwrap();
        let owners = db.accepted_delivery_attempt_windows("session-a").unwrap();
        assert_eq!(owners.len(), 1);
        assert_eq!(owners[0].attempt_id, "prefix-attempt");
        assert_eq!(owners[0].rows, vec![rows[0].clone()]);
    }

    #[test]
    fn accepted_attempt_owner_skips_undeliverable_older_rows() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let abandoned =
            inserted_row(db.enqueue_agent_bash_complete(&input("abandoned", "session-a")));
        let exhausted =
            inserted_row(db.enqueue_agent_bash_complete(&input("exhausted", "session-a")));
        let deliverable =
            inserted_row(db.enqueue_agent_bash_complete(&input("deliverable", "session-a")));
        db.mark_pending_abandoned("session-a", WAKE_SWEEP_ABANDONED_ERROR, 1)
            .unwrap();
        for _ in 0..MAX_UNCONFIRMED_DELIVERY_ATTEMPTS {
            db.mark_delivery_failed(
                "session-a",
                &[exhausted.seq],
                MAILBOX_DELIVERY_UNCONFIRMED_ERROR,
            )
            .unwrap();
        }
        db.register_delivery_attempt(
            "deliverable-attempt",
            "session-a",
            "invocation-a",
            &[deliverable.seq],
            0,
        )
        .unwrap();
        db.record_delivery_attempt_transport_ack("deliverable-attempt")
            .unwrap();

        let owners = db.accepted_delivery_attempt_windows("session-a").unwrap();
        assert_eq!(owners.len(), 1);
        assert_eq!(owners[0].attempt_id, "deliverable-attempt");
        assert_eq!(owners[0].rows, vec![deliverable]);
        assert_eq!(owners[0].remaining_count, 0);
        let abandoned = db
            .list_pending("session-a")
            .unwrap()
            .into_iter()
            .find(|row| row.seq == abandoned.seq)
            .unwrap();
        assert!(!mailbox_row_is_deliverable_pending(&abandoned));
    }

    #[test]
    fn late_transport_ack_after_sibling_confirmation_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));
        for attempt_id in ["attempt-1", "attempt-2"] {
            db.register_delivery_attempt(attempt_id, "session-a", "invocation-a", &[row.seq], 0)
                .unwrap();
        }
        db.record_delivery_attempt_transport_ack("attempt-2")
            .unwrap();
        db.confirm_delivery_attempt("attempt-2").unwrap();
        let before = db.list_mailbox("session-a", true).unwrap().remove(0);

        assert!(
            !db.record_delivery_attempt_transport_ack("attempt-1")
                .unwrap()
        );
        let after = db.list_mailbox("session-a", true).unwrap().remove(0);
        assert_eq!(after, before);
        let late = db.delivery_attempt_window("attempt-1").unwrap().unwrap();
        assert!(late.acknowledged_at.is_none());
        assert!(late.rows.is_empty());
    }

    #[test]
    fn deployed_sidecar_attempt_rows_reopen_without_schema_or_historical_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let mut db = MailboxDb::open(&path).unwrap();
        let old = inserted_row(db.enqueue_agent_bash_complete(&input("old-handle", "session-a")));
        db.register_delivery_attempt("old-attempt", "session-a", "old-invocation", &[old.seq], 0)
            .unwrap();
        db.record_delivery_attempt_transport_ack("old-attempt")
            .unwrap();
        db.confirm_delivery_attempt("old-attempt").unwrap();
        let old_mailbox = db.list_mailbox("session-a", true).unwrap();
        let old_attempt: (Option<String>, Option<String>, Option<String>) = db
            .connection()
            .query_row(
                "SELECT acknowledged_at, resolved_at, resolved_by_attempt_id
                 FROM mailbox_delivery_attempts WHERE attempt_id = 'old-attempt'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let attempt_ddl: String = db
            .connection()
            .query_row(
                "SELECT sql FROM sqlite_master
                 WHERE type = 'table' AND name = 'mailbox_delivery_attempts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        drop(db);

        let mut reopened = MailboxDb::open(&path).unwrap();
        let new =
            inserted_row(reopened.enqueue_agent_bash_complete(&input("new-handle", "session-a")));
        reopened
            .register_delivery_attempt("new-attempt", "session-a", "new-invocation", &[new.seq], 0)
            .unwrap();
        reopened
            .record_delivery_attempt_transport_ack("new-attempt")
            .unwrap();
        assert!(reopened.list_pending("session-a").unwrap().contains(&new));
        assert_eq!(
            reopened
                .list_mailbox("session-a", true)
                .unwrap()
                .into_iter()
                .filter(|row| row.handle == "old-handle")
                .collect::<Vec<_>>(),
            old_mailbox
        );
        let reopened_old_attempt: (Option<String>, Option<String>, Option<String>) = reopened
            .connection()
            .query_row(
                "SELECT acknowledged_at, resolved_at, resolved_by_attempt_id
                 FROM mailbox_delivery_attempts WHERE attempt_id = 'old-attempt'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(reopened_old_attempt, old_attempt);
        let reopened_ddl: String = reopened
            .connection()
            .query_row(
                "SELECT sql FROM sqlite_master
                 WHERE type = 'table' AND name = 'mailbox_delivery_attempts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(reopened_ddl, attempt_ddl);
    }

    #[test]
    fn manual_range_ack_leaves_newer_rows_and_contracts_attempts() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let rows = ["handle-a", "handle-b", "handle-c"].map(|handle| {
            inserted_row(db.enqueue_agent_bash_complete(&input(handle, "session-a")))
        });
        let seqs = rows.iter().map(|row| row.seq).collect::<Vec<_>>();
        db.register_delivery_attempt("attempt-1", "session-a", "invocation-a", &seqs, 0)
            .unwrap();

        let changed = db
            .acknowledge_range("session-a", rows[0].seq, rows[1].seq, "manual-test")
            .unwrap();

        assert_eq!(changed, 2);
        let pending = db.list_pending("session-a").unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].seq, rows[2].seq);
        let window = db.delivery_attempt_window("attempt-1").unwrap().unwrap();
        assert_eq!(window.rows.len(), 1);
        assert_eq!(window.rows[0].seq, rows[2].seq);
        assert_eq!(
            db.delivery_attempt_disposition("attempt-1").unwrap(),
            Some(MailboxDeliveryAttemptDisposition::Stale)
        );
    }

    #[test]
    fn notification_pause_state_defaults_false_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();

        assert!(!db.notifications_paused("session-a").unwrap());
        db.set_notifications_paused("session-a", true).unwrap();
        assert!(db.notifications_paused("session-a").unwrap());
        db.set_notifications_paused("session-a", false).unwrap();
        assert!(!db.notifications_paused("session-a").unwrap());
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
