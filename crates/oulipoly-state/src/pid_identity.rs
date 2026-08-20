//! ## Declared roles
//!
//! `accessor`, `formatter`, `mapper`, `orchestration`, `parser`, `validator`
//!
//! PID identity sidecar storage. This module owns the PID identity table and
//! platform process-creation identity reads used to guard against PID reuse.

use chrono::{SecondsFormat, Utc};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SIDECAR_DB_NAME: &str = "pid-identity.db";
// Shell owner lookup has a 60-second caller bound and can wait through a short
// writer burst. Cancellable monitor snapshots keep the shared 250 ms policy.
const DIRECT_LOOKUP_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProcessIdentity {
    pub os_pid: i64,
    pub os_boot_id: String,
    pub os_pid_starttime_ticks: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessIdentityObservation {
    ExactLive(ProcessIdentity),
    Dead,
    Unsupported,
    ReadError(String),
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

/// External callers use typed identity operations rather than raw writable SQL.
///
/// ```compile_fail
/// use oulipoly_state::pid_identity::PidIdentityDb;
///
/// fn raw_write_capability(identity: &PidIdentityDb) {
///     let _ = identity.connection();
/// }
/// ```
pub struct PidIdentityDb {
    conn: Connection,
    path: PathBuf,
    _read_only_snapshot: Option<crate::read_only_snapshot::ReadOnlySnapshot>,
    _namespace_authority: Option<crate::mailbox::MailboxAuthorityFence>,
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
        let authority = crate::mailbox::MailboxAuthorityFence::acquire(path)
            .map_err(|error| error.to_string())?;
        crate::rebuild_recovery::ensure_writable_open_allowed(authority.path())?;
        let mut db = Self::open_with_authority(&authority)?;
        db._namespace_authority = Some(authority);
        Ok(db)
    }

    pub(crate) fn open_with_authority(
        authority: &crate::mailbox::MailboxAuthorityFence,
    ) -> Result<Self, String> {
        let path = authority.path();
        let conn = Connection::open(path)
            .map_err(|err| format!("Failed to open PID identity sidecar: {err}"))?;
        authority.validate_opened_target()?;
        crate::mailbox::configure_writable_sidecar_connection(&conn)?;
        crate::mailbox::set_wal_mode(&conn)?;
        ensure_identity_schema(&conn)?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
            _read_only_snapshot: None,
            _namespace_authority: None,
        })
    }

    pub fn open_default_read_only() -> Result<Self, String> {
        let path = Self::default_path()?;
        Self::open_read_only(&path)
    }

    pub fn open_read_only(path: &Path) -> Result<Self, String> {
        Self::open_read_only_with_retry_timeout(path, DIRECT_LOOKUP_SNAPSHOT_TIMEOUT, &|| false)
    }

    pub fn open_read_only_with_cancel(
        path: &Path,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Self, String> {
        let snapshot =
            crate::read_only_snapshot::ReadOnlySnapshot::create_with_cancel(path, is_cancelled)
                .map_err(|err| format!("Failed to open PID identity sidecar read-only: {err}"))?;
        Self::open_snapshot(path, snapshot)
    }

    fn open_read_only_with_retry_timeout(
        path: &Path,
        retry_timeout: Duration,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Self, String> {
        let snapshot = crate::read_only_snapshot::ReadOnlySnapshot::create_with_retry_timeout(
            path,
            retry_timeout,
            is_cancelled,
        )
        .map_err(|err| format!("Failed to open PID identity sidecar read-only: {err}"))?;
        Self::open_snapshot(path, snapshot)
    }

    fn open_snapshot(
        path: &Path,
        snapshot: crate::read_only_snapshot::ReadOnlySnapshot,
    ) -> Result<Self, String> {
        let conn =
            Connection::open_with_flags(snapshot.path(), OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|err| format!("Failed to open PID identity sidecar read-only: {err}"))?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
            _read_only_snapshot: Some(snapshot),
            _namespace_authority: None,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(test)]
    pub(crate) fn connection(&self) -> &Connection {
        &self.conn
    }

    pub fn record_identity(&self, record: PidIdentityRecord<'_>) -> Result<PidIdentityRow, String> {
        if !bind_identity_on(&self.conn, record)? {
            return Err(
                "Exact PID identity is already bound to another invocation or session".to_string(),
            );
        }
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
                   AND os_pid_starttime_ticks = ?3
                   AND (session_id IS NULL OR session_id = ?4)",
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

pub fn observe_live_process_identity(os_pid: i64) -> ProcessIdentityObservation {
    observe_live_process_identity_impl(os_pid)
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
fn observe_live_process_identity_impl(os_pid: i64) -> ProcessIdentityObservation {
    match read_live_process_identity_impl(os_pid) {
        Ok(Some(identity)) => ProcessIdentityObservation::ExactLive(identity),
        Ok(None) => ProcessIdentityObservation::Dead,
        Err(err) => ProcessIdentityObservation::ReadError(err),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn observe_live_process_identity_impl(_os_pid: i64) -> ProcessIdentityObservation {
    ProcessIdentityObservation::Unsupported
}

pub(crate) fn ensure_identity_schema(conn: &Connection) -> Result<(), String> {
    const RETRY_INTERVAL: Duration = Duration::from_millis(10);
    const TIMEOUT: Duration = Duration::from_secs(5);
    const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS pid_identity (
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
        );";

    let deadline = Instant::now() + TIMEOUT;
    loop {
        match conn.execute_batch(SCHEMA) {
            Ok(()) => return Ok(()),
            Err(error)
                if matches!(
                    error.sqlite_error_code(),
                    Some(rusqlite::ffi::ErrorCode::DatabaseBusy)
                        | Some(rusqlite::ffi::ErrorCode::DatabaseLocked)
                ) && Instant::now() < deadline =>
            {
                std::thread::sleep(RETRY_INTERVAL);
            }
            Err(error) => {
                return Err(format!(
                    "Failed to ensure PID identity sidecar schema: {error}"
                ));
            }
        }
    }
}

pub(crate) fn bind_identity_on(
    conn: &Connection,
    record: PidIdentityRecord<'_>,
) -> Result<bool, String> {
    upsert_pid_identity_row(conn, record)
}

pub(crate) fn attach_identity_session_on(
    conn: &Connection,
    identity: &ProcessIdentity,
    session_id: &str,
) -> Result<bool, String> {
    let Some(existing) = lookup_by_identity_on(conn, identity)? else {
        return Ok(false);
    };
    if existing
        .session_id
        .as_deref()
        .is_some_and(|existing| existing != session_id)
    {
        return Ok(false);
    }
    conn.execute(
        "UPDATE pid_identity
         SET session_id = ?4
         WHERE os_pid = ?1
           AND os_boot_id = ?2
           AND os_pid_starttime_ticks = ?3
           AND (session_id IS NULL OR session_id = ?4)",
        params![
            identity.os_pid,
            &identity.os_boot_id,
            identity.os_pid_starttime_ticks,
            session_id,
        ],
    )
    .map_err(|err| format!("Failed to attach PID identity session_id: {err}"))?;
    Ok(true)
}

fn lookup_by_identity_on(
    conn: &Connection,
    identity: &ProcessIdentity,
) -> Result<Option<PidIdentityRow>, String> {
    conn.query_row(
        "SELECT os_pid, os_boot_id, os_pid_starttime_ticks, os_pgid,
                invocation_uuid, session_id, provider_name, model_name, recorded_at
         FROM pid_identity
         WHERE os_pid = ?1
           AND os_boot_id = ?2
           AND os_pid_starttime_ticks = ?3",
        params![
            identity.os_pid,
            &identity.os_boot_id,
            identity.os_pid_starttime_ticks,
        ],
        map_pid_identity_row,
    )
    .optional()
    .map_err(|err| format!("Failed to query exact PID identity: {err}"))
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

fn upsert_pid_identity_row(
    conn: &Connection,
    record: PidIdentityRecord<'_>,
) -> Result<bool, String> {
    let changed = conn
        .execute(
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
            os_pgid = COALESCE(excluded.os_pgid, pid_identity.os_pgid),
            session_id = COALESCE(pid_identity.session_id, excluded.session_id),
            provider_name = COALESCE(excluded.provider_name, pid_identity.provider_name),
            model_name = COALESCE(excluded.model_name, pid_identity.model_name),
            recorded_at = excluded.recorded_at
          WHERE pid_identity.invocation_uuid = excluded.invocation_uuid
            AND (pid_identity.session_id IS NULL
              OR excluded.session_id IS NULL
              OR pid_identity.session_id = excluded.session_id)",
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
    Ok(changed > 0)
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

#[cfg(target_os = "macos")]
fn read_live_process_identity_impl(os_pid: i64) -> Result<Option<ProcessIdentity>, String> {
    let Ok(pid) = libc::pid_t::try_from(os_pid) else {
        return Ok(None);
    };
    if pid <= 0 {
        return Ok(None);
    }
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let expected = std::mem::size_of::<libc::proc_bsdinfo>();
    let read = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            expected as libc::c_int,
        )
    };
    if read == 0 {
        let error = std::io::Error::last_os_error();
        return match error.raw_os_error() {
            Some(libc::ESRCH | libc::ENOENT) => Ok(None),
            _ => Err(format!(
                "Failed to read macOS process identity for PID {os_pid}: {error}"
            )),
        };
    }
    if read < 0 || read as usize != expected {
        return Err(format!(
            "Failed to read complete macOS process identity for PID {os_pid}: expected {expected} bytes, read {read}"
        ));
    }
    let info = unsafe { info.assume_init() };
    let start_micros = info
        .pbi_start_tvsec
        .checked_mul(1_000_000)
        .and_then(|value| value.checked_add(info.pbi_start_tvusec))
        .and_then(|value| i64::try_from(value).ok())
        .ok_or_else(|| format!("macOS process start time overflowed for PID {os_pid}"))?;
    Ok(Some(ProcessIdentity {
        os_pid,
        os_boot_id: "macos-absolute-process-start-v1".to_string(),
        os_pid_starttime_ticks: start_micros,
    }))
}

#[cfg(windows)]
fn read_live_process_identity_impl(os_pid: i64) -> Result<Option<ProcessIdentity>, String> {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let Ok(pid) = u32::try_from(os_pid) else {
        return Ok(None);
    };
    if pid == 0 {
        return Ok(None);
    }
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        let error = std::io::Error::last_os_error();
        return if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
            Ok(None)
        } else {
            Err(format!(
                "Failed to open Windows process identity for PID {os_pid}: {error}"
            ))
        };
    }
    let mut creation = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exit = creation;
    let mut kernel = creation;
    let mut user = creation;
    let read = unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };
    unsafe {
        CloseHandle(handle);
    }
    if read == 0 {
        return Err(format!(
            "Failed to read Windows process identity for PID {os_pid}: {}",
            std::io::Error::last_os_error()
        ));
    }
    if exit.dwLowDateTime != 0 || exit.dwHighDateTime != 0 {
        return Ok(None);
    }
    let start_filetime =
        (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
    let start_filetime = i64::try_from(start_filetime)
        .map_err(|_| format!("Windows process start time overflowed for PID {os_pid}"))?;
    Ok(Some(ProcessIdentity {
        os_pid,
        os_boot_id: "windows-absolute-process-start-v1".to_string(),
        os_pid_starttime_ticks: start_filetime,
    }))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
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
    use crate::mailbox::{
        BindRuntimeGenerationRunning, CreateRuntimeGeneration, MailboxDb, RuntimeGenerationFence,
        RuntimeGenerationId, RuntimeLifecycleState, SessionLiveness,
    };
    use std::sync::mpsc;
    use std::time::Duration;

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
    fn direct_read_only_lookup_waits_through_a_sidecar_write_burst() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let writer_db = PidIdentityDb::open(&path).unwrap();
        let target_identity = identity(99, "boot-write-burst");
        let target_pid = target_identity.os_pid;
        writer_db
            .record_identity(record(&target_identity, Some("session-write-burst")))
            .unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let writer = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(1);
            let mut revision = 0_u64;
            while Instant::now() < deadline {
                writer_db
                    .connection()
                    .execute(
                        "UPDATE pid_identity SET recorded_at = ?1 WHERE os_pid = ?2",
                        params![format!("revision-{revision}"), target_pid],
                    )
                    .unwrap();
                revision += 1;
            }
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        let result = PidIdentityDb::open_read_only(&path);
        writer.join().unwrap();
        let reader = result.unwrap();
        assert!(
            reader
                .lookup_by_identity(&target_identity)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn sidecar_creation_waits_for_canonical_namespace_authority() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let authority = crate::mailbox::MailboxAuthorityFence::acquire_exclusive(&path).unwrap();
        let (opened_tx, opened_rx) = mpsc::channel();
        let opener_path = path.clone();
        let opener = std::thread::spawn(move || {
            opened_tx.send(PidIdentityDb::open(&opener_path)).unwrap();
        });

        assert!(
            opened_rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "sidecar creation must wait behind canonical namespace authority"
        );
        assert!(!path.exists());

        drop(authority);
        opened_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        opener.join().unwrap();
        assert!(path.exists());
    }

    #[test]
    fn sidecar_open_reuses_canonical_namespace_authority() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let authority = crate::mailbox::MailboxAuthorityFence::acquire(&path).unwrap();

        let db = PidIdentityDb::open_with_authority(&authority).unwrap();

        assert_eq!(db.path(), authority.path().canonicalize().unwrap());
    }

    #[test]
    fn nested_shared_authority_acquisition_avoids_same_process_deadlock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let _authority = crate::mailbox::MailboxAuthorityFence::acquire(&path).unwrap();

        let nested = crate::mailbox::MailboxAuthorityFence::acquire(&path).unwrap();
        drop(nested);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_sidecar_reuses_canonical_namespace_authority() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        PidIdentityDb::open(&path).unwrap();
        let alias = dir.path().join("pid-identity-alias.db");
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        let _authority = crate::mailbox::MailboxAuthorityFence::acquire(&path).unwrap();

        let alias_authority = crate::mailbox::MailboxAuthorityFence::acquire(&alias).unwrap();
        assert_eq!(alias_authority.path(), path.canonicalize().unwrap());
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
    fn exact_process_identity_rejects_invocation_and_session_reassignment() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let db = PidIdentityDb::open(&path).unwrap();
        let identity = identity(100, "boot-binding");
        db.record_identity(record(&identity, Some("session-a")))
            .unwrap();

        let error = db
            .record_identity(PidIdentityRecord {
                identity: &identity,
                os_pgid: Some(4242),
                invocation_uuid: "22222222-2222-2222-2222-222222222222",
                session_id: Some("session-b"),
                provider_name: Some("replacement-provider"),
                model_name: Some("replacement~high"),
                recorded_at: "2026-06-04T12:01:00Z",
            })
            .unwrap_err();

        assert!(error.contains("already bound"), "{error}");
        assert!(!db.set_session_id(&identity, "session-b").unwrap());
        let row = db.lookup_by_identity(&identity).unwrap().unwrap();
        assert_eq!(row.invocation_uuid, INVOCATION_UUID);
        assert_eq!(row.session_id.as_deref(), Some("session-a"));
        assert_eq!(row.provider_name.as_deref(), Some("fixture-provider"));
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

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    #[test]
    fn live_process_identity_is_stable_and_observable() {
        let pid = i64::from(std::process::id());
        let first = read_live_process_identity(pid).unwrap().unwrap();
        let second = read_live_process_identity(pid).unwrap().unwrap();

        assert_eq!(first, second);
        assert_eq!(first.os_pid, pid);
        assert_eq!(
            observe_live_process_identity(pid),
            ProcessIdentityObservation::ExactLive(first)
        );
        assert!(read_live_process_identity(i64::MAX).unwrap().is_none());
        assert_eq!(
            observe_live_process_identity(i64::MAX),
            ProcessIdentityObservation::Dead
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    #[test]
    fn dead_child_identity_reconciles_runtime_generation() {
        let mut child = spawn_sleeping_child();
        let os_pid = i64::from(child.id());
        let identity = read_live_process_identity(os_pid).unwrap().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut mailbox = MailboxDb::open(&directory.path().join("pid-identity.db")).unwrap();
        let generation_id =
            RuntimeGenerationId::parse("11111111-1111-4111-8111-111111111111").unwrap();
        mailbox
            .runtime_lifecycle()
            .create_runtime_generation(CreateRuntimeGeneration {
                generation_id: &generation_id,
                spawn_invocation_uuid: INVOCATION_UUID,
                session_id: Some("session-crash-recovery"),
                runtime_mode: "headless",
                provider_name: "fixture-provider",
                model_name: Some("fixture~high"),
                pty_control_path: None,
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();
        mailbox
            .runtime_lifecycle()
            .bind_runtime_generation_running(BindRuntimeGenerationRunning {
                fence: RuntimeGenerationFence {
                    generation_id: &generation_id,
                    spawn_invocation_uuid: INVOCATION_UUID,
                },
                spawned_os_pid: os_pid,
                exact_process_identity: Some(&identity),
                os_pgid: None,
            })
            .unwrap();
        assert_eq!(
            mailbox
                .runtime_lifecycle()
                .reconcile_session_liveness("session-crash-recovery")
                .unwrap(),
            SessionLiveness::Busy
        );

        child.kill().unwrap();
        child.wait().unwrap();

        assert_eq!(
            observe_live_process_identity(os_pid),
            ProcessIdentityObservation::Dead
        );
        assert_eq!(
            mailbox
                .runtime_lifecycle()
                .reconcile_session_liveness("session-crash-recovery")
                .unwrap(),
            SessionLiveness::Idle
        );
        assert_eq!(
            mailbox
                .runtime_lifecycle_reader()
                .runtime_generation(&generation_id)
                .unwrap()
                .unwrap()
                .lifecycle_state,
            RuntimeLifecycleState::Exited
        );
    }

    #[cfg(unix)]
    fn spawn_sleeping_child() -> std::process::Child {
        std::process::Command::new("sh")
            .args(["-c", "sleep 30"])
            .spawn()
            .unwrap()
    }

    #[cfg(windows)]
    fn spawn_sleeping_child() -> std::process::Child {
        std::process::Command::new("cmd")
            .args(["/C", "ping 127.0.0.1 -n 30 >NUL"])
            .spawn()
            .unwrap()
    }

    #[test]
    fn sidecar_operations_do_not_change_state_db_schema() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("state.db");
        let state = StateDb::open(&state_path).unwrap();
        let baseline_version = user_version(state.raw_connection());
        let baseline_columns = invocation_columns(state.raw_connection());
        drop(state);

        let sidecar_path = dir.path().join("pid-identity.db");
        let sidecar = PidIdentityDb::open(&sidecar_path).unwrap();
        let identity = identity(101, "boot-e");
        sidecar.record_identity(record(&identity, None)).unwrap();
        sidecar.set_session_id(&identity, "session-e").unwrap();
        drop(sidecar);

        let state = StateDb::open(&state_path).unwrap();
        assert_eq!(user_version(state.raw_connection()), baseline_version);
        assert_eq!(invocation_columns(state.raw_connection()), baseline_columns);
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
