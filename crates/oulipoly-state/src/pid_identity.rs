//! ## Declared roles
//!
//! `accessor`, `formatter`, `mapper`, `orchestration`, `parser`, `validator`
//!
//! PID identity sidecar storage. This module owns the independent
//! `pid-identity.db` schema and Linux `/proc` identity reads used to guard
//! against PID reuse.

use chrono::{SecondsFormat, Utc};
use rusqlite::{Connection, OpenFlags, params};
use serde::Serialize;
use std::path::{Path, PathBuf};

const SIDECAR_DB_NAME: &str = "pid-identity.db";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProcessIdentity {
    pub os_pid: i64,
    pub os_boot_id: String,
    pub os_pid_starttime_ticks: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PidIdentityRow {
    pub os_pid: i64,
    pub os_boot_id: String,
    pub os_pid_starttime_ticks: i64,
    pub os_pgid: Option<i64>,
    pub invocation_uuid: String,
    pub session_id: Option<String>,
    pub provider_name: Option<String>,
    pub model_name: Option<String>,
    pub recorded_at: String,
}

impl PidIdentityRow {
    pub fn identity(&self) -> ProcessIdentity {
        ProcessIdentity {
            os_pid: self.os_pid,
            os_boot_id: self.os_boot_id.clone(),
            os_pid_starttime_ticks: self.os_pid_starttime_ticks,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PidIdentityRecord<'a> {
    pub identity: &'a ProcessIdentity,
    pub os_pgid: Option<i64>,
    pub invocation_uuid: &'a str,
    pub session_id: Option<&'a str>,
    pub provider_name: Option<&'a str>,
    pub model_name: Option<&'a str>,
    pub recorded_at: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct LiveProcessIdentityRecord<'a> {
    pub os_pid: i64,
    pub invocation_uuid: &'a str,
    pub session_id: Option<&'a str>,
    pub provider_name: Option<&'a str>,
    pub model_name: Option<&'a str>,
}

pub struct PidIdentityDb {
    conn: Connection,
    path: PathBuf,
}

impl PidIdentityDb {
    pub fn default_path() -> Result<PathBuf, String> {
        default_path()
    }

    pub fn open_default() -> Result<Self, String> {
        let path = Self::default_path()?;
        Self::open(&path)
    }

    pub fn open(path: &Path) -> Result<Self, String> {
        ensure_parent_dir(path)?;
        let conn = Connection::open(path)
            .map_err(|err| format!("Failed to open PID identity sidecar: {err}"))?;
        set_wal_mode(&conn)?;
        ensure_identity_schema(&conn)?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
    }

    pub fn open_default_read_only() -> Result<Self, String> {
        let path = Self::default_path()?;
        Self::open_read_only(&path)
    }

    pub fn open_read_only(path: &Path) -> Result<Self, String> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|err| format!("Failed to open PID identity sidecar read-only: {err}"))?;
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

    pub fn record_identity(&self, record: PidIdentityRecord<'_>) -> Result<PidIdentityRow, String> {
        upsert_pid_identity_row(&self.conn, record)?;
        let row = self.lookup_by_identity(record.identity)?;
        require_recorded_identity(row)
    }

    pub fn set_session_id(
        &self,
        identity: &ProcessIdentity,
        session_id: &str,
    ) -> Result<bool, String> {
        let changed = self
            .conn
            .execute(
                "UPDATE pid_identity
                 SET session_id = ?4
                 WHERE os_pid = ?1
                   AND os_boot_id = ?2
                   AND os_pid_starttime_ticks = ?3",
                params![
                    identity.os_pid,
                    &identity.os_boot_id,
                    identity.os_pid_starttime_ticks,
                    session_id,
                ],
            )
            .map_err(|err| format!("Failed to set PID identity session_id: {err}"))?;
        Ok(changed > 0)
    }

    pub fn lookup_by_identity(
        &self,
        identity: &ProcessIdentity,
    ) -> Result<Option<PidIdentityRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT os_pid, os_boot_id, os_pid_starttime_ticks, os_pgid,
                        invocation_uuid, session_id, provider_name, model_name, recorded_at
                 FROM pid_identity
                 WHERE os_pid = ?1
                   AND os_boot_id = ?2
                   AND os_pid_starttime_ticks = ?3
                 LIMIT 2",
            )
            .map_err(|err| format!("Failed to prepare PID identity lookup: {err}"))?;
        let rows = stmt
            .query_map(
                params![
                    identity.os_pid,
                    &identity.os_boot_id,
                    identity.os_pid_starttime_ticks,
                ],
                map_pid_identity_row,
            )
            .map_err(|err| format!("Failed to query PID identity: {err}"))?;
        single_or_ambiguous(collect_rows(rows)?)
    }

    pub fn lookup_by_invocation_uuid(
        &self,
        invocation_uuid: &str,
    ) -> Result<Vec<PidIdentityRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT os_pid, os_boot_id, os_pid_starttime_ticks, os_pgid,
                        invocation_uuid, session_id, provider_name, model_name, recorded_at
                 FROM pid_identity
                 WHERE invocation_uuid = ?1
                 ORDER BY recorded_at DESC, os_pid DESC",
            )
            .map_err(|err| format!("Failed to prepare invocation PID lookup: {err}"))?;
        let rows = stmt
            .query_map(params![invocation_uuid], map_pid_identity_row)
            .map_err(|err| format!("Failed to query invocation PID identities: {err}"))?;
        collect_rows(rows)
    }
}

pub fn default_path() -> Result<PathBuf, String> {
    Ok(crate::paths::data_dir()?.join(SIDECAR_DB_NAME))
}

pub fn record_live_process_identity(
    input: LiveProcessIdentityRecord<'_>,
) -> Result<Option<PidIdentityRow>, String> {
    let Some(identity) = read_live_process_identity(input.os_pid)? else {
        return Ok(None);
    };
    let recorded_at = recorded_at_now();
    let db = PidIdentityDb::open_default()?;
    db.record_identity(pid_identity_record_from_live(
        input,
        &identity,
        &recorded_at,
    ))
    .map(Some)
}

pub fn read_live_process_identity(os_pid: i64) -> Result<Option<ProcessIdentity>, String> {
    read_live_process_identity_impl(os_pid)
}

fn ensure_parent_dir(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("Failed to create PID identity sidecar directory: {err}"))?;
    }
    Ok(())
}

fn set_wal_mode(conn: &Connection) -> Result<(), String> {
    conn.execute_batch("PRAGMA journal_mode=WAL;")
        .map_err(|err| format!("Failed to set PID identity sidecar WAL mode: {err}"))
}

pub(crate) fn ensure_identity_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS pid_identity (
            os_pid                  INTEGER NOT NULL,
            os_boot_id              TEXT    NOT NULL,
            os_pid_starttime_ticks  INTEGER NOT NULL,
            os_pgid                 INTEGER,
            invocation_uuid         TEXT    NOT NULL,
            session_id              TEXT,
            provider_name           TEXT,
            model_name              TEXT,
            recorded_at             TEXT    NOT NULL,
            PRIMARY KEY (os_pid, os_boot_id, os_pid_starttime_ticks)
        );",
    )
    .map_err(|err| format!("Failed to ensure PID identity sidecar schema: {err}"))
}

fn map_pid_identity_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PidIdentityRow> {
    Ok(PidIdentityRow {
        os_pid: row.get(0)?,
        os_boot_id: row.get(1)?,
        os_pid_starttime_ticks: row.get(2)?,
        os_pgid: row.get(3)?,
        invocation_uuid: row.get(4)?,
        session_id: row.get(5)?,
        provider_name: row.get(6)?,
        model_name: row.get(7)?,
        recorded_at: row.get(8)?,
    })
}

fn collect_rows<F>(rows: rusqlite::MappedRows<'_, F>) -> Result<Vec<PidIdentityRow>, String>
where
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<PidIdentityRow>,
{
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("Failed to read PID identity row: {err}"))
}

fn upsert_pid_identity_row(conn: &Connection, record: PidIdentityRecord<'_>) -> Result<(), String> {
    conn.execute(
        "INSERT INTO pid_identity (
            os_pid,
            os_boot_id,
            os_pid_starttime_ticks,
            os_pgid,
            invocation_uuid,
            session_id,
            provider_name,
            model_name,
            recorded_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(os_pid, os_boot_id, os_pid_starttime_ticks)
         DO UPDATE SET
            os_pgid = excluded.os_pgid,
            invocation_uuid = excluded.invocation_uuid,
            session_id = COALESCE(excluded.session_id, pid_identity.session_id),
            provider_name = COALESCE(excluded.provider_name, pid_identity.provider_name),
            model_name = COALESCE(excluded.model_name, pid_identity.model_name),
            recorded_at = excluded.recorded_at",
        params![
            record.identity.os_pid,
            &record.identity.os_boot_id,
            record.identity.os_pid_starttime_ticks,
            record.os_pgid,
            record.invocation_uuid,
            record.session_id,
            record.provider_name,
            record.model_name,
            record.recorded_at,
        ],
    )
    .map_err(|err| format!("Failed to record PID identity: {err}"))?;
    Ok(())
}

fn require_recorded_identity(row: Option<PidIdentityRow>) -> Result<PidIdentityRow, String> {
    row.ok_or_else(|| "Failed to reread PID identity immediately after recording".to_string())
}

fn recorded_at_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn pid_identity_record_from_live<'a>(
    input: LiveProcessIdentityRecord<'a>,
    identity: &'a ProcessIdentity,
    recorded_at: &'a str,
) -> PidIdentityRecord<'a> {
    PidIdentityRecord {
        identity,
        os_pgid: process_group_id(input.os_pid),
        invocation_uuid: input.invocation_uuid,
        session_id: input.session_id,
        provider_name: input.provider_name,
        model_name: input.model_name,
        recorded_at,
    }
}

fn single_or_ambiguous(rows: Vec<PidIdentityRow>) -> Result<Option<PidIdentityRow>, String> {
    match rows.len() {
        0 => Ok(None),
        1 => Ok(rows.into_iter().next()),
        _ => Err("Ambiguous PID identity rows for verified process identity".to_string()),
    }
}

#[cfg(target_os = "linux")]
fn read_live_process_identity_impl(os_pid: i64) -> Result<Option<ProcessIdentity>, String> {
    if !valid_os_pid(os_pid) {
        return Ok(None);
    }
    let Some(os_pid_starttime_ticks) = read_proc_starttime_ticks(os_pid)? else {
        return Ok(None);
    };
    let Some(os_boot_id) = read_boot_id()? else {
        return Ok(None);
    };
    Ok(Some(process_identity(
        os_pid,
        os_boot_id,
        os_pid_starttime_ticks,
    )))
}

#[cfg(target_os = "linux")]
fn valid_os_pid(os_pid: i64) -> bool {
    os_pid > 0
}

#[cfg(target_os = "linux")]
fn process_identity(
    os_pid: i64,
    os_boot_id: String,
    os_pid_starttime_ticks: i64,
) -> ProcessIdentity {
    ProcessIdentity {
        os_pid,
        os_boot_id,
        os_pid_starttime_ticks,
    }
}

#[cfg(not(target_os = "linux"))]
fn read_live_process_identity_impl(_os_pid: i64) -> Result<Option<ProcessIdentity>, String> {
    Ok(None)
}

#[cfg(target_os = "linux")]
fn read_proc_starttime_ticks(os_pid: i64) -> Result<Option<i64>, String> {
    let path = proc_stat_path(os_pid);
    let Some(stat) = read_optional_file_to_string(&path, "proc stat")? else {
        return Ok(None);
    };
    Ok(parse_proc_stat_starttime_ticks(&stat))
}

#[cfg(target_os = "linux")]
fn proc_stat_path(os_pid: i64) -> PathBuf {
    PathBuf::from("/proc").join(os_pid.to_string()).join("stat")
}

#[cfg(target_os = "linux")]
fn parse_proc_stat_starttime_ticks(stat: &str) -> Option<i64> {
    let close = stat.rfind(") ")?;
    let after_comm = &stat[close + 2..];
    after_comm.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(target_os = "linux")]
fn read_boot_id() -> Result<Option<String>, String> {
    let Some(value) =
        read_optional_file_to_string(Path::new("/proc/sys/kernel/random/boot_id"), "OS boot id")?
    else {
        return Ok(None);
    };
    Ok(non_empty_trimmed(value))
}

#[cfg(target_os = "linux")]
fn read_optional_file_to_string(path: &Path, description: &str) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(value) => Ok(Some(value)),
        Err(err) if optional_file_read_error(&err) => Ok(None),
        Err(err) => Err(file_read_error(path, description, &err)),
    }
}

#[cfg(target_os = "linux")]
fn optional_file_read_error(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
    )
}

#[cfg(target_os = "linux")]
fn file_read_error(path: &Path, description: &str, err: &std::io::Error) -> String {
    format!("Failed to read {description} at {}: {err}", path.display())
}

#[cfg(target_os = "linux")]
fn non_empty_trimmed(value: String) -> Option<String> {
    let trimmed = value.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

#[cfg(unix)]
fn process_group_id(os_pid: i64) -> Option<i64> {
    let pid = libc::pid_t::try_from(os_pid).ok()?;
    let pgid = unsafe { libc::getpgid(pid) };
    (pgid >= 0).then_some(i64::from(pgid))
}

#[cfg(not(unix))]
fn process_group_id(_os_pid: i64) -> Option<i64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StateDb;

    const INVOCATION_UUID: &str = "11111111-1111-1111-1111-111111111111";

    fn identity(starttime: i64, boot_id: &str) -> ProcessIdentity {
        ProcessIdentity {
            os_pid: 4242,
            os_boot_id: boot_id.to_string(),
            os_pid_starttime_ticks: starttime,
        }
    }

    fn record<'a>(
        identity: &'a ProcessIdentity,
        session_id: Option<&'a str>,
    ) -> PidIdentityRecord<'a> {
        PidIdentityRecord {
            identity,
            os_pgid: Some(4242),
            invocation_uuid: INVOCATION_UUID,
            session_id,
            provider_name: Some("fixture-provider"),
            model_name: Some("fixture~high"),
            recorded_at: "2026-06-04T12:00:00Z",
        }
    }

    #[test]
    fn sidecar_round_trip_and_reopen_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let db = PidIdentityDb::open(&path).unwrap();
        let mode: String = db
            .connection()
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_ascii_lowercase(), "wal");

        let identity = identity(99, "boot-a");
        let row = db
            .record_identity(record(&identity, Some("session-a")))
            .unwrap();
        assert_eq!(row.identity(), identity);
        assert_eq!(row.session_id.as_deref(), Some("session-a"));
        drop(db);

        let db = PidIdentityDb::open_read_only(&path).unwrap();
        let reopened = db.lookup_by_identity(&identity).unwrap().unwrap();
        assert_eq!(reopened.invocation_uuid, INVOCATION_UUID);
        assert_eq!(reopened.provider_name.as_deref(), Some("fixture-provider"));
        assert_eq!(reopened.model_name.as_deref(), Some("fixture~high"));
    }

    #[test]
    fn set_session_id_fills_existing_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let db = PidIdentityDb::open(&path).unwrap();
        let identity = identity(100, "boot-b");
        db.record_identity(record(&identity, None)).unwrap();

        assert!(db.set_session_id(&identity, "session-late").unwrap());

        let row = db.lookup_by_identity(&identity).unwrap().unwrap();
        assert_eq!(row.session_id.as_deref(), Some("session-late"));
    }

    #[test]
    fn lookup_guards_recycled_pid_and_cross_boot_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let db = PidIdentityDb::open(&path).unwrap();
        let original = identity(100, "boot-c");
        db.record_identity(record(&original, Some("session-c")))
            .unwrap();

        let recycled = identity(101, "boot-c");
        let cross_boot = identity(100, "boot-d");
        assert!(db.lookup_by_identity(&recycled).unwrap().is_none());
        assert!(db.lookup_by_identity(&cross_boot).unwrap().is_none());
    }

    #[test]
    fn sidecar_operations_do_not_change_state_db_schema() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("state.db");
        let state = StateDb::open(&state_path).unwrap();
        let baseline_version = user_version(state.connection());
        let baseline_columns = invocation_columns(state.connection());
        drop(state);

        let sidecar_path = dir.path().join("pid-identity.db");
        let sidecar = PidIdentityDb::open(&sidecar_path).unwrap();
        let identity = identity(101, "boot-e");
        sidecar.record_identity(record(&identity, None)).unwrap();
        sidecar.set_session_id(&identity, "session-e").unwrap();
        drop(sidecar);

        let state = StateDb::open(&state_path).unwrap();
        assert_eq!(user_version(state.connection()), baseline_version);
        assert_eq!(invocation_columns(state.connection()), baseline_columns);
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
