//! ## Declared roles
//!
//! `accessor`, `orchestration`, `formatter`, `validator`
//!
//! Resume-backed notification mailbox storage in the PID identity sidecar DB.
//! This module deliberately owns only additive sidecar tables and never touches
//! the versioned `state.db` schema.

use chrono::{SecondsFormat, Utc};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::pid_identity;

pub const AGENT_BASH_COMPLETE_KIND: &str = "agent_bash_complete";

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
}

pub struct MailboxDb {
    conn: Connection,
    path: PathBuf,
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
                    updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(session_id)
                 DO UPDATE SET
                    mode = excluded.mode,
                    invocation_uuid = excluded.invocation_uuid,
                    provider_name = excluded.provider_name,
                    model_name = excluded.model_name,
                    pty_control_path = excluded.pty_control_path,
                    updated_at = excluded.updated_at",
                params![
                    input.session_id,
                    input.mode,
                    input.invocation_uuid,
                    input.provider_name,
                    input.model_name,
                    input.pty_control_path,
                    &now,
                ],
            )
            .map_err(|err| format!("Failed to upsert session runtime row: {err}"))?;
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
}

fn enqueue_agent_bash_complete_in_tx(
    tx: &rusqlite::Transaction<'_>,
    input: &AgentBashCompleteEnqueue<'_>,
    now: &str,
) -> Result<EnqueueResult, String> {
    let changed = tx
        .execute(
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
        .map_err(|err| format!("Failed to insert mailbox row: {err}"))?;

    let row = query_mailbox_by_kind_handle_tx(tx, AGENT_BASH_COMPLETE_KIND, input.handle)?
        .ok_or_else(|| "Mailbox row missing after enqueue conflict check".to_string())?;
    if changed > 0 {
        return Ok(EnqueueResult::Inserted(row));
    }
    if row.session_id == input.session_id {
        Ok(EnqueueResult::AlreadyEnqueued(row))
    } else {
        Ok(EnqueueResult::Conflict { existing: row })
    }
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
            session_id       TEXT PRIMARY KEY,
            mode             TEXT NOT NULL CHECK(mode IN ('headless', 'pty_interactive')),
            invocation_uuid  TEXT,
            provider_name    TEXT,
            model_name       TEXT,
            pty_control_path TEXT,
            updated_at       TEXT NOT NULL
        );",
    )
    .map_err(|err| format!("Failed to ensure PID mailbox sidecar schema: {err}"))
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
        drop(mailbox);

        let state = StateDb::open(&state_path).unwrap();
        assert_eq!(user_version(state.connection()), baseline_version);
        assert_eq!(invocation_columns(state.connection()), baseline_columns);
    }

    fn inserted_row(result: Result<EnqueueResult, String>) -> MailboxRow {
        match result.unwrap() {
            EnqueueResult::Inserted(row) => row,
            other => panic!("expected inserted row, got {other:?}"),
        }
    }

    fn user_version(conn: &Connection) -> i64 {
        conn.query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap()
    }

    fn invocation_columns(conn: &Connection) -> Vec<String> {
        let mut stmt = conn.prepare("PRAGMA table_info(invocations)").unwrap();
        stmt.query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }
}
