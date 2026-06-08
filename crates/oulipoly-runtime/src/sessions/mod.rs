//! CLI session log ingestion via user-provided adapter scripts.
//!
//! ## Declared roles
//!
//! `orchestration`, `accessor`, `filter`, `parser`, `validator`, `mapper`, `formatter`, `predicate`
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/sessions/mod.rs
//!     role: intrinsic-surface
//!     Domain: session_turn_ingest
//!     Owns:
//!       - adapter-script scan/ingest
//!       - ScanReport.new_turns
//! ```
//!
//! Mirrors the `quota_script` pattern in `providers.toml`: the application
//! is ignorant of any specific CLI's storage format. Each provider declares
//! a `turn_script` that knows how to enumerate session turns for that CLI
//! and emits them as a normalized JSONL stream.
//!
//! Script contract:
//!   - Run as `sh -c <turn_script>` with `STATE_DIR` env set to a writable dir
//!     the script may use for incremental cursor bookkeeping.
//!   - Stdout: one JSON object per line per turn (in any order):
//!     `{"session_id":"...","turn_id":"...","timestamp":"<ISO8601>","role":"user"|"assistant","parent_turn_id":"...","is_sidechain":true}`
//!   - Empty stdout = no new turns. Non-zero exit = error.
//!   - Idempotent: re-running with no source changes outputs nothing.
//!
//! The unified `session_turns` table stores everything across CLIs. The
//! UNIQUE constraint on `(provider, session_id, turn_id)` makes ingestion
//! safe even if a script over-emits (e.g. doesn't honor its cursor).

use chrono::{DateTime, Utc};
use oulipoly_config::{SessionSourceEntry, SessionsConfig};
use oulipoly_state::{SessionTurnIngest, StateDb};
use serde::Deserialize;
use serde_json::Value;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
use std::thread::JoinHandle;

// Turn scripts run on the pre-dispatch metrics path. Ninety seconds is long
// enough for slow CLI/API startup but prevents provider selection from being
// wedged by a broken adapter or an unexpectedly large history scan.
const SCRIPT_TIMEOUT_SECS: u64 = 90;

/// A turn as emitted by an adapter script.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ScriptTurn {
    pub session_id: String,
    pub turn_id: String,
    pub timestamp: String,
    pub role: String,
    #[serde(default)]
    pub parent_turn_id: Option<String>,
    #[serde(default)]
    pub is_sidechain: Option<bool>,
    #[serde(default)]
    pub is_compaction_boundary: Option<bool>,
    #[serde(default)]
    pub body: Option<Value>,
}

/// Outcome of scanning one provider's session source.
#[derive(Debug, Clone, Default)]
pub struct ScanReport {
    pub new_turns: u64,
    pub script_lines: u64,
    pub errors: Vec<String>,
}

pub(crate) fn is_canonical_body_shape(body: &Value) -> bool {
    let Value::Array(chunks) = body else {
        return false;
    };
    chunks.iter().all(is_canonical_body_chunk)
}

fn is_canonical_body_chunk(chunk: &Value) -> bool {
    let Value::Object(map) = chunk else {
        return false;
    };
    map.get("type").is_some_and(Value::is_string)
        && map.get("text").map(Value::is_string).unwrap_or(true)
}

/// Run the adapter script for `provider_name` and ingest every turn it emits.
///
/// Failure modes (script timeout, non-zero exit, malformed JSON) are
/// captured in `report.errors` rather than propagated — the balancer should
/// degrade gracefully when a script is broken, not abort the request.
pub fn scan_provider(
    provider_name: &str,
    sessions_cfg: &SessionsConfig,
    db: &StateDb,
) -> ScanReport {
    scan_provider_with_timeout(provider_name, sessions_cfg, db, SCRIPT_TIMEOUT_SECS)
}

/// Run one provider's adapter script for a specific session and ingest every
/// turn it emits. The configured script receives `SESSION_ID` in its env.
pub fn scan_provider_session(
    provider_name: &str,
    sessions_cfg: &SessionsConfig,
    db: &StateDb,
    session_id: &str,
) -> ScanReport {
    scan_provider_session_with_timeout(
        provider_name,
        sessions_cfg,
        db,
        session_id,
        SCRIPT_TIMEOUT_SECS,
    )
}

fn scan_provider_with_timeout(
    provider_name: &str,
    sessions_cfg: &SessionsConfig,
    db: &StateDb,
    timeout_secs: u64,
) -> ScanReport {
    let mut report = ScanReport::default();
    let Some(entry) = provider_session_source(sessions_cfg, provider_name) else {
        return report;
    };

    let state_dir = resolve_state_dir(provider_name, entry);
    if let Err(error) = create_session_state_dir(&state_dir) {
        return scan_report_with_error(report, error);
    }

    let stdout = match run_turn_script(&entry.turn_script, &state_dir, timeout_secs) {
        Ok(stdout) => stdout,
        Err(error) => return scan_report_with_error(report, error),
    };

    let batch = collect_turn_script_batch(provider_name, &stdout, &mut report);
    persist_scanned_turns(provider_name, db, &batch, &mut report);
    report
}

fn scan_provider_session_with_timeout(
    provider_name: &str,
    sessions_cfg: &SessionsConfig,
    db: &StateDb,
    session_id: &str,
    timeout_secs: u64,
) -> ScanReport {
    let mut report = ScanReport::default();
    let Some(entry) = provider_session_source(sessions_cfg, provider_name) else {
        return report;
    };

    let state_dir = resolve_state_dir(provider_name, entry);
    if let Err(error) = create_session_state_dir(&state_dir) {
        return scan_report_with_error(report, error);
    }

    let stdout = match run_session_script_with_timeout(
        &entry.turn_script,
        &state_dir,
        Some(session_id),
        "turn script",
        timeout_secs,
    ) {
        Ok(stdout) => stdout,
        Err(error) => return scan_report_with_error(report, error),
    };

    let batch = collect_turn_script_batch(provider_name, &stdout, &mut report);
    persist_scanned_turns(provider_name, db, &batch, &mut report);
    report
}

fn provider_session_source<'a>(
    sessions_cfg: &'a SessionsConfig,
    provider_name: &str,
) -> Option<&'a SessionSourceEntry> {
    sessions_cfg.get(provider_name)
}

fn scan_report_with_error(mut report: ScanReport, error: String) -> ScanReport {
    report.errors.push(error);
    report
}

fn create_session_state_dir(state_dir: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(state_dir)
        .map_err(|error| format_state_dir_create_error(state_dir, error))
}

fn format_state_dir_create_error(state_dir: &std::path::Path, error: std::io::Error) -> String {
    format!(
        "could not create state_dir {}: {error}",
        state_dir.display()
    )
}

fn collect_turn_script_batch(
    provider_name: &str,
    stdout: &str,
    report: &mut ScanReport,
) -> Vec<SessionTurnIngest> {
    let mut batch = Vec::new();
    for line in non_empty_script_lines(stdout) {
        let line_number = record_script_line_seen(report);
        if let Some(error) = degraded_marker_error(line) {
            record_scan_error(report, error);
            continue;
        }
        match script_line_to_ingest(provider_name, line, line_number) {
            Ok(parsed) => {
                record_optional_scan_error(report, parsed.body_error);
                push_script_turn_ingest(&mut batch, parsed.ingest);
            }
            Err(error) => record_scan_error(report, error),
        }
    }
    batch
}

fn degraded_marker_error(trimmed: &str) -> Option<String> {
    let marker = parse_degraded_marker_jsonl(trimmed)?;
    if !is_degraded_marker(&marker) {
        return None;
    }
    Some(format_degraded_marker_error(degraded_marker_count(&marker)))
}

fn parse_degraded_marker_jsonl(trimmed: &str) -> Option<Value> {
    serde_json::from_str(trimmed).ok()
}

fn is_degraded_marker(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.get("degraded").and_then(Value::as_bool) == Some(true),
        _ => false,
    }
}

fn degraded_marker_count(value: &Value) -> u64 {
    match value {
        Value::Object(map) => map.get("count").and_then(Value::as_u64).unwrap_or(0),
        _ => 0,
    }
}

fn format_degraded_marker_error(count: u64) -> String {
    format!("turn script degraded before completing scan; best_count={count}")
}

fn non_empty_script_lines(stdout: &str) -> Vec<&str> {
    non_empty_trimmed_lines(trimmed_script_lines(stdout))
}

fn trimmed_script_lines(stdout: &str) -> Vec<&str> {
    stdout.lines().map(str::trim).collect()
}

fn non_empty_trimmed_lines(lines: Vec<&str>) -> Vec<&str> {
    lines.into_iter().filter(|line| !line.is_empty()).collect()
}

fn record_script_line_seen(report: &mut ScanReport) -> u64 {
    report.script_lines += 1;
    report.script_lines
}

fn record_optional_scan_error(report: &mut ScanReport, error: Option<String>) {
    if let Some(error) = error {
        record_scan_error(report, error);
    }
}

fn record_scan_error(report: &mut ScanReport, error: String) {
    report.errors.push(error);
}

fn push_script_turn_ingest(batch: &mut Vec<SessionTurnIngest>, ingest: SessionTurnIngest) {
    batch.push(ingest);
}

fn script_line_to_ingest(
    provider_name: &str,
    trimmed: &str,
    line_number: u64,
) -> Result<ParsedScriptTurnIngest, String> {
    let turn = parse_script_turn_line(trimmed)?;
    let timestamp = parse_script_turn_timestamp(&turn.timestamp)?;
    let body_validation = validate_script_turn_body(provider_name, line_number, &turn.body);
    let body = serialize_selected_script_turn_body(
        provider_name,
        line_number,
        selected_script_turn_body(&body_validation),
    )?;
    let body_error = script_turn_body_error(&body_validation);
    Ok(script_turn_ingest_from_parts(
        turn, timestamp, body, body_error,
    ))
}

struct ParsedScriptTurnIngest {
    ingest: SessionTurnIngest,
    body_error: Option<String>,
}

enum ValidatedScriptTurnBody<'a> {
    Accepted(Option<&'a Value>),
    Rejected(String),
}

fn parsed_script_turn_ingest(
    ingest: SessionTurnIngest,
    body_error: Option<String>,
) -> ParsedScriptTurnIngest {
    ParsedScriptTurnIngest { ingest, body_error }
}

fn parse_script_turn_line(trimmed: &str) -> Result<ScriptTurn, String> {
    serde_json::from_str(trimmed).map_err(|error| format_malformed_turn_line(error, trimmed))
}

fn format_malformed_turn_line(error: serde_json::Error, trimmed: &str) -> String {
    format!("malformed turn line ({error}): {trimmed}")
}

fn parse_script_turn_timestamp(timestamp: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(timestamp)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|error| format_bad_timestamp(timestamp, error))
}

fn format_bad_timestamp(timestamp: &str, error: chrono::ParseError) -> String {
    format!("bad timestamp {timestamp}: {error}")
}

fn validate_script_turn_body<'a>(
    provider_name: &str,
    line_number: u64,
    body: &'a Option<Value>,
) -> ValidatedScriptTurnBody<'a> {
    let Some(body) = body else {
        return ValidatedScriptTurnBody::Accepted(None);
    };
    match validate_script_turn_body_shape(provider_name, line_number, body) {
        Ok(()) => ValidatedScriptTurnBody::Accepted(Some(body)),
        Err(error) => ValidatedScriptTurnBody::Rejected(error),
    }
}

fn validate_script_turn_body_shape(
    provider_name: &str,
    line_number: u64,
    body: &Value,
) -> Result<(), String> {
    if is_canonical_body_shape(body) {
        Ok(())
    } else {
        Err(format_invalid_body_shape(provider_name, line_number))
    }
}

fn format_invalid_body_shape(provider_name: &str, line_number: u64) -> String {
    format!(
        "invalid body shape for provider {provider_name} line {line_number}: expected canonical content chunk array"
    )
}

fn serialize_script_turn_body(
    provider_name: &str,
    line_number: u64,
    body: &Value,
) -> Result<Option<String>, String> {
    serde_json::to_string(body)
        .map(Some)
        .map_err(|error| format_body_serialize_error(provider_name, line_number, error))
}

fn selected_script_turn_body<'a>(body: &'a ValidatedScriptTurnBody<'a>) -> Option<&'a Value> {
    match body {
        ValidatedScriptTurnBody::Accepted(Some(body)) => Some(body),
        ValidatedScriptTurnBody::Accepted(None) | ValidatedScriptTurnBody::Rejected(_) => None,
    }
}

fn serialize_selected_script_turn_body(
    provider_name: &str,
    line_number: u64,
    body: Option<&Value>,
) -> Result<Option<String>, String> {
    body.map(|body| serialize_script_turn_body(provider_name, line_number, body))
        .transpose()
        .map(Option::flatten)
}

fn format_body_serialize_error(
    provider_name: &str,
    line_number: u64,
    error: serde_json::Error,
) -> String {
    format!("failed to serialize body for provider {provider_name} line {line_number}: {error}")
}

fn script_turn_to_ingest(
    turn: ScriptTurn,
    timestamp: DateTime<Utc>,
    body: Option<String>,
) -> SessionTurnIngest {
    SessionTurnIngest {
        session_id: turn.session_id,
        turn_id: turn.turn_id,
        timestamp,
        role: turn.role,
        parent_turn_id: turn.parent_turn_id,
        is_sidechain: turn.is_sidechain.unwrap_or(false),
        is_compaction_boundary: turn.is_compaction_boundary.unwrap_or(false),
        body,
    }
}

fn script_turn_ingest_from_parts(
    turn: ScriptTurn,
    timestamp: DateTime<Utc>,
    body: Option<String>,
    body_error: Option<String>,
) -> ParsedScriptTurnIngest {
    parsed_script_turn_ingest(script_turn_to_ingest(turn, timestamp, body), body_error)
}

fn script_turn_body_error(body_validation: &ValidatedScriptTurnBody<'_>) -> Option<String> {
    match body_validation {
        ValidatedScriptTurnBody::Accepted(_) => None,
        ValidatedScriptTurnBody::Rejected(error) => Some(error.clone()),
    }
}

fn persist_scanned_turns(
    provider_name: &str,
    db: &StateDb,
    batch: &[SessionTurnIngest],
    report: &mut ScanReport,
) {
    match db.ingest_session_turns_batch(provider_name, batch) {
        Ok(new_turns) => persist_imported_chains(provider_name, db, batch, report, new_turns),
        Err(error) => report.errors.push(error),
    }
}

fn persist_imported_chains(
    provider_name: &str,
    db: &StateDb,
    batch: &[SessionTurnIngest],
    report: &mut ScanReport,
    new_turns: u64,
) {
    report.new_turns = new_turns;
    for turn in batch {
        if let Err(error) = db.mint_imported_chain_if_absent(
            provider_name,
            &turn.session_id,
            &turn.timestamp,
            "<unknown>",
        ) {
            report.errors.push(error);
        }
    }
}

/// Scan every provider listed in `sessions_cfg`. Failures in one provider
/// don't abort the others.
pub fn scan_all(sessions_cfg: &SessionsConfig, db: &StateDb) -> Vec<(String, ScanReport)> {
    let mut out = Vec::new();
    for name in sessions_cfg.entries.keys() {
        let report = scan_provider(name, sessions_cfg, db);
        out.push((name.clone(), report));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn resolve_state_dir(provider_name: &str, entry: &SessionSourceEntry) -> PathBuf {
    if let Some(dir) = &entry.state_dir {
        return dir.clone();
    }
    let base = default_app_data_dir().join("sessions");
    base.join(provider_name)
}

fn default_app_data_dir() -> PathBuf {
    oulipoly_state::paths::data_dir()
        .unwrap_or_else(|_| PathBuf::from(".").join(oulipoly_state::paths::APP_DATA_DIR_NAME))
}

fn run_turn_script(
    script: &str,
    state_dir: &std::path::Path,
    timeout_secs: u64,
) -> Result<String, String> {
    run_session_script_with_timeout(script, state_dir, None, "turn script", timeout_secs)
}

pub fn locate_transcript(
    sessions_cfg: &SessionsConfig,
    provider_name: &str,
    session_id: &str,
) -> Result<Option<PathBuf>, String> {
    let Some(entry) = session_source_entry(sessions_cfg, provider_name) else {
        return Ok(None);
    };
    let Some(locator) = transcript_locator_script(entry) else {
        return Ok(None);
    };

    let state_dir = resolve_state_dir(provider_name, entry);
    create_session_state_dir(&state_dir)?;

    let stdout = run_session_script(locator, &state_dir, Some(session_id), "transcript locator")?;
    let lines = non_empty_script_lines(&stdout);
    let line = single_transcript_stdout_line(&lines)?;
    Ok(Some(transcript_path_from_line(line)))
}

fn session_source_entry<'a>(
    sessions_cfg: &'a SessionsConfig,
    provider_name: &str,
) -> Option<&'a SessionSourceEntry> {
    sessions_cfg.get(provider_name)
}

fn transcript_locator_script(entry: &SessionSourceEntry) -> Option<&str> {
    entry.transcript_locator.as_deref()
}

fn single_transcript_stdout_line<'a>(lines: &'a [&'a str]) -> Result<&'a str, String> {
    match lines {
        [] => Err("transcript locator returned empty stdout".to_string()),
        [line] => Ok(line),
        _ => Err("transcript locator stdout was not a single line".to_string()),
    }
}

fn transcript_path_from_line(line: &str) -> PathBuf {
    PathBuf::from(line)
}

fn run_session_script(
    script: &str,
    state_dir: &std::path::Path,
    session_id: Option<&str>,
    script_kind: &str,
) -> Result<String, String> {
    run_session_script_with_timeout(
        script,
        state_dir,
        session_id,
        script_kind,
        SCRIPT_TIMEOUT_SECS,
    )
}

fn run_session_script_with_timeout(
    script: &str,
    state_dir: &std::path::Path,
    session_id: Option<&str>,
    script_kind: &str,
    timeout_secs: u64,
) -> Result<String, String> {
    let mut cmd = session_script_command(script, state_dir, session_id);
    let mut child = spawn_session_script_child(&mut cmd, script_kind)?;

    // Drain stdout/stderr concurrently. A naive `try_wait` loop deadlocks
    // for scripts that produce more than ~64KB on stdout: the kernel pipe
    // fills up, the script blocks on write, the loop waits for exit forever.
    let stdout_handle = spawn_stdout_reader(take_child_stdout(&mut child));
    let stderr_handle = spawn_stderr_reader(take_child_stderr(&mut child));
    let status = wait_for_session_script(&mut child, script_kind, timeout_secs)?;
    let stdout_text = join_script_reader(stdout_handle);
    let stderr_text = join_script_reader(stderr_handle);

    validate_session_script_success(script_kind, status, &stderr_text)?;
    Ok(stdout_text)
}

fn validate_session_script_success(
    script_kind: &str,
    status: ExitStatus,
    stderr_text: &str,
) -> Result<(), String> {
    if status.success() {
        Ok(())
    } else {
        Err(format_session_script_nonzero(
            script_kind,
            status,
            stderr_text,
        ))
    }
}

fn session_script_command(
    script: &str,
    state_dir: &std::path::Path,
    session_id: Option<&str>,
) -> Command {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(script);
    cmd.env("STATE_DIR", state_dir);
    if let Some(session_id) = session_id {
        cmd.env("SESSION_ID", session_id);
    }
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    configure_session_script_process_group(&mut cmd);
    cmd
}

#[cfg(unix)]
fn configure_session_script_process_group(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;

    cmd.process_group(0);
}

#[cfg(not(unix))]
fn configure_session_script_process_group(_cmd: &mut Command) {}

fn spawn_session_script_child(cmd: &mut Command, script_kind: &str) -> Result<Child, String> {
    cmd.spawn()
        .map_err(|error| format_session_script_spawn_error(script_kind, error))
}

fn format_session_script_spawn_error(script_kind: &str, error: std::io::Error) -> String {
    format!("Failed to spawn {script_kind}: {error}")
}

fn take_child_stdout(child: &mut Child) -> ChildStdout {
    child.stdout.take().expect("piped")
}

fn take_child_stderr(child: &mut Child) -> ChildStderr {
    child.stderr.take().expect("piped")
}

fn spawn_stdout_reader(stdout: ChildStdout) -> JoinHandle<String> {
    spawn_script_reader(stdout)
}

fn spawn_stderr_reader(stderr: ChildStderr) -> JoinHandle<String> {
    spawn_script_reader(stderr)
}

fn spawn_script_reader<R>(mut reader: R) -> JoinHandle<String>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || drain_script_reader_to_string(&mut reader))
}

fn drain_script_reader_to_string<R>(reader: &mut R) -> String
where
    R: Read,
{
    let mut buf = String::new();
    reader.read_to_string(&mut buf).ok();
    buf
}

fn wait_for_session_script(
    child: &mut Child,
    script_kind: &str,
    timeout_secs: u64,
) -> Result<ExitStatus, String> {
    let timeout = std::time::Duration::from_secs(timeout_secs);
    let start = std::time::Instant::now();
    loop {
        match poll_session_script(child) {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {
                wait_for_pending_session_script(child, start, timeout, script_kind, timeout_secs)?
            }
            Err(error) => return Err(format_session_script_wait_error(script_kind, error)),
        }
    }
}

fn poll_session_script(child: &mut Child) -> Result<Option<ExitStatus>, std::io::Error> {
    child.try_wait()
}

fn wait_for_pending_session_script(
    child: &mut Child,
    start: std::time::Instant,
    timeout: std::time::Duration,
    script_kind: &str,
    timeout_secs: u64,
) -> Result<(), String> {
    if pending_session_script_timed_out(start, timeout) {
        return fail_timed_out_pending_session_script(child, script_kind, timeout_secs);
    }
    sleep_before_next_session_script_poll();
    Ok(())
}

fn pending_session_script_timed_out(
    start: std::time::Instant,
    timeout: std::time::Duration,
) -> bool {
    start.elapsed() >= timeout
}

fn fail_timed_out_pending_session_script(
    child: &mut Child,
    script_kind: &str,
    timeout_secs: u64,
) -> Result<(), String> {
    kill_timed_out_pending_session_script(child);
    Err(format_pending_session_script_timeout(
        script_kind,
        timeout_secs,
    ))
}

fn kill_timed_out_pending_session_script(child: &mut Child) {
    kill_session_script_process_group(child);
}

#[cfg(unix)]
fn kill_session_script_process_group(child: &mut Child) {
    let pgid = -(child.id() as libc::pid_t);
    // SAFETY: `pgid` targets the process group created with `process_group(0)`
    // for this child. Killing the group prevents shell grandchildren from
    // continuing to run or holding stdout/stderr pipes after timeout.
    let _ = unsafe { libc::kill(pgid, libc::SIGKILL) };
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(not(unix))]
fn kill_session_script_process_group(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn sleep_before_next_session_script_poll() {
    std::thread::sleep(std::time::Duration::from_millis(50));
}

fn format_pending_session_script_timeout(script_kind: &str, timeout_secs: u64) -> String {
    format!("script_timeout: {script_kind} timed out after {timeout_secs}s")
}

fn format_session_script_wait_error(script_kind: &str, error: std::io::Error) -> String {
    format!(
        "{} wait failed: {error}",
        capitalize_script_kind(script_kind)
    )
}

fn join_script_reader(handle: JoinHandle<String>) -> String {
    handle.join().unwrap_or_default()
}

fn format_session_script_nonzero(
    script_kind: &str,
    status: ExitStatus,
    stderr_text: &str,
) -> String {
    format!(
        "{} exited {}: {}",
        capitalize_script_kind(script_kind),
        status.code().unwrap_or(-1),
        stderr_text.trim()
    )
}

fn capitalize_script_kind(kind: &str) -> String {
    let mut chars = kind.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => kind.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    fn db() -> StateDb {
        StateDb::open(std::path::Path::new(":memory:")).unwrap()
    }

    /// Tempdir + executable script inside it. Returning the dir keeps the
    /// path alive; writing via `std::fs::write` releases the file handle so
    /// `sh` can exec it without "Text file busy".
    struct Fixture {
        _dir: tempfile::TempDir,
        path: std::path::PathBuf,
    }

    fn fixture_script(body: &str) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("turn-script.sh");
        std::fs::write(&path, format!("#!/usr/bin/env bash\n{body}\n")).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        Fixture { _dir: dir, path }
    }

    fn cfg_with(provider: &str, script_path: &std::path::Path) -> SessionsConfig {
        let mut entries = HashMap::new();
        entries.insert(
            provider.to_string(),
            SessionSourceEntry {
                turn_script: script_path.to_string_lossy().into_owned(),
                transcript_locator: None,
                state_dir: None,
            },
        );
        SessionsConfig { entries }
    }

    fn cfg_with_locator(
        provider: &str,
        locator_path: &std::path::Path,
        state_dir: Option<std::path::PathBuf>,
    ) -> SessionsConfig {
        let mut entries = HashMap::new();
        entries.insert(
            provider.to_string(),
            SessionSourceEntry {
                turn_script: "true".to_string(),
                transcript_locator: Some(locator_path.to_string_lossy().into_owned()),
                state_dir,
            },
        );
        SessionsConfig { entries }
    }

    #[test]
    fn ingests_assistant_turns_and_advances_count() {
        let db = db();
        let script = fixture_script(
            r#"cat <<EOF
{"session_id":"S1","turn_id":"t1","timestamp":"2026-04-17T08:00:00Z","role":"user"}
{"session_id":"S1","turn_id":"t2","timestamp":"2026-04-17T08:00:01Z","role":"assistant"}
{"session_id":"S1","turn_id":"t3","timestamp":"2026-04-17T08:00:02Z","role":"user"}
{"session_id":"S1","turn_id":"t4","timestamp":"2026-04-17T08:00:03Z","role":"assistant"}
EOF"#,
        );
        let cfg = cfg_with("p", &script.path);
        let r = scan_provider("p", &cfg, &db);
        assert_eq!(r.errors, Vec::<String>::new());
        assert_eq!(r.new_turns, 4);
        assert_eq!(db.count_assistant_turns_since("p", None).unwrap(), 2);
    }

    #[test]
    fn duplicate_turns_are_idempotent_per_unique_constraint() {
        let db = db();
        let script = fixture_script(
            r#"cat <<EOF
{"session_id":"S1","turn_id":"t1","timestamp":"2026-04-17T08:00:00Z","role":"assistant"}
{"session_id":"S1","turn_id":"t1","timestamp":"2026-04-17T08:00:00Z","role":"assistant"}
EOF"#,
        );
        let cfg = cfg_with("p", &script.path);
        let r = scan_provider("p", &cfg, &db);
        assert_eq!(r.new_turns, 1, "second emission deduped by UNIQUE");
        assert_eq!(r.script_lines, 2);
    }

    #[test]
    fn script_turn_legacy_json_deserializes_with_none_defaults() {
        let turn: ScriptTurn = serde_json::from_str(
            r#"{"session_id":"S1","turn_id":"t1","timestamp":"2026-04-17T08:00:00Z","role":"assistant"}"#,
        )
        .unwrap();

        assert_eq!(turn.session_id, "S1");
        assert_eq!(turn.turn_id, "t1");
        assert_eq!(turn.role, "assistant");
        assert_eq!(turn.parent_turn_id, None);
        assert_eq!(turn.is_sidechain, None);
        assert_eq!(turn.body, None);
    }

    #[test]
    fn script_turn_full_json_deserializes_parent_and_sidechain_fields() {
        let turn: ScriptTurn = serde_json::from_str(
            r#"{"session_id":"S1","turn_id":"t2","timestamp":"2026-04-17T08:00:01Z","role":"assistant","parent_turn_id":"t1","is_sidechain":true}"#,
        )
        .unwrap();

        assert_eq!(turn.session_id, "S1");
        assert_eq!(turn.turn_id, "t2");
        assert_eq!(turn.parent_turn_id.as_deref(), Some("t1"));
        assert_eq!(turn.is_sidechain, Some(true));
    }

    #[test]
    fn scan_provider_persists_body_encoding_edge_cases() {
        // risk: encoding regression; level: particular-integration; source: contract §4 T6 / proposal A5,A7.
        let db = db();
        let script = fixture_script(
            r#"cat <<'EOF'
{"session_id":"S1","turn_id":"edge-body","timestamp":"2026-04-17T08:00:00Z","role":"assistant","body":[{"type":"text","text":"line one\n日本語\n{\"escaped\":true}\u0007"}]}
EOF"#,
        );
        let cfg = cfg_with("p", &script.path);

        let report = scan_provider("p", &cfg, &db);

        assert_eq!(report.errors, Vec::<String>::new());
        assert_eq!(report.new_turns, 1);
        let raw_body: String = db
            .connection()
            .query_row(
                "SELECT body FROM session_turns
                 WHERE provider_name = 'p' AND session_id = 'S1' AND turn_id = 'edge-body'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&raw_body).unwrap();
        assert_eq!(
            body,
            serde_json::json!([{"type":"text","text":"line one\n日本語\n{\"escaped\":true}\u{0007}"}])
        );
    }

    #[test]
    fn scan_provider_rejects_non_canonical_body_shape() {
        // risk: invalid adapter body poisons downstream canonical export; level: unit; source: CodeRabbit R4-F05.
        let db = db();
        let script = fixture_script(
            r#"cat <<'EOF'
{"session_id":"S1","turn_id":"bad-body","timestamp":"2026-04-17T08:00:00Z","role":"assistant","body":{"type":"text","text":"not an array"}}
{"session_id":"S1","turn_id":"bad-text","timestamp":"2026-04-17T08:00:01Z","role":"assistant","body":[{"type":"text","text":7}]}
EOF"#,
        );
        let cfg = cfg_with("p", &script.path);

        let report = scan_provider("p", &cfg, &db);

        assert_eq!(report.new_turns, 2);
        assert_eq!(report.errors.len(), 2);
        assert!(report.errors.iter().all(|error| {
            error.contains("invalid body shape")
                && error.contains("expected canonical content chunk array")
        }));
        let stored_bodies: Vec<Option<String>> = db
            .connection()
            .prepare(
                "SELECT body FROM session_turns
                 WHERE provider_name = 'p' AND session_id = 'S1'
                 ORDER BY turn_id",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(stored_bodies, vec![None, None]);
    }

    #[test]
    fn malformed_lines_collect_as_errors_but_dont_abort() {
        let db = db();
        let script = fixture_script(
            r#"cat <<EOF
not-json
{"session_id":"S1","turn_id":"t1","timestamp":"2026-04-17T08:00:00Z","role":"assistant"}
EOF"#,
        );
        let cfg = cfg_with("p", &script.path);
        let r = scan_provider("p", &cfg, &db);
        assert_eq!(r.new_turns, 1);
        assert_eq!(r.errors.len(), 1);
        assert!(r.errors[0].contains("malformed"));
    }

    #[test]
    fn nonzero_exit_is_an_error() {
        let db = db();
        let script = fixture_script("echo something-bad >&2; exit 7");
        let cfg = cfg_with("p", &script.path);
        let r = scan_provider("p", &cfg, &db);
        assert_eq!(r.new_turns, 0);
        assert_eq!(r.errors.len(), 1);
        assert!(r.errors[0].contains("exited 7"));
    }

    #[test]
    fn turn_script_timeout_is_classified_and_does_not_persist_turns() {
        let db = db();
        let script = fixture_script("sleep 60");
        let cfg = cfg_with("p", &script.path);

        let r = scan_provider_with_timeout("p", &cfg, &db, 1);

        assert_eq!(r.new_turns, 0);
        assert_eq!(db.count_assistant_turns_since("p", None).unwrap(), 0);
        assert_eq!(r.errors.len(), 1);
        assert!(r.errors[0].contains("script_timeout"), "{:?}", r.errors);
        assert!(r.errors[0].contains("turn script"), "{:?}", r.errors);
    }

    #[cfg(unix)]
    #[test]
    fn turn_script_timeout_kills_process_group_children() {
        let db = db();
        let dir = tempfile::tempdir().unwrap();
        let leaked_marker = dir.path().join("leaked");
        let script = fixture_script(&format!(
            "(sleep 2; printf leaked > {}) & wait",
            leaked_marker.display()
        ));
        let cfg = cfg_with("p", &script.path);

        let r = scan_provider_with_timeout("p", &cfg, &db, 1);
        std::thread::sleep(Duration::from_secs(3));

        assert_eq!(r.new_turns, 0);
        assert_eq!(r.errors.len(), 1);
        assert!(r.errors[0].contains("script_timeout"), "{:?}", r.errors);
        assert!(
            !leaked_marker.exists(),
            "timed-out turn script left a process-group child running"
        );
    }

    #[test]
    fn degraded_marker_is_reported_without_malformed_turn_error() {
        let db = db();
        let script = fixture_script(r#"printf '%s\n' '{"degraded":true,"count":1}'"#);
        let cfg = cfg_with("p", &script.path);

        let r = scan_provider("p", &cfg, &db);

        assert_eq!(r.new_turns, 0);
        assert_eq!(r.errors.len(), 1);
        assert!(r.errors[0].contains("degraded"), "{:?}", r.errors);
        assert!(!r.errors[0].contains("malformed"), "{:?}", r.errors);
    }

    #[test]
    fn script_receives_state_dir_env() {
        let db = db();
        let script = fixture_script(
            r#"echo "{\"session_id\":\"S\",\"turn_id\":\"t1\",\"timestamp\":\"2026-04-17T08:00:00Z\",\"role\":\"assistant\"}"
echo "STATE_DIR=$STATE_DIR" > "$STATE_DIR/marker.txt""#,
        );
        let tempdir = tempfile::tempdir().unwrap();
        let mut entries = HashMap::new();
        entries.insert(
            "p".to_string(),
            SessionSourceEntry {
                turn_script: script.path.to_string_lossy().into_owned(),
                transcript_locator: None,
                state_dir: Some(tempdir.path().to_path_buf()),
            },
        );
        let cfg = SessionsConfig { entries };
        let r = scan_provider("p", &cfg, &db);
        assert_eq!(r.errors, Vec::<String>::new());
        assert_eq!(r.new_turns, 1);
        let marker = std::fs::read_to_string(tempdir.path().join("marker.txt")).unwrap();
        assert!(marker.contains(tempdir.path().to_str().unwrap()));
    }

    #[test]
    fn scan_provider_session_sets_session_id_env() {
        let db = db();
        let script = fixture_script(
            r#"printf '{"session_id":"%s","turn_id":"t1","timestamp":"2026-04-17T08:00:00Z","role":"user"}\n' "$SESSION_ID"
printf '%s\n' "$SESSION_ID" > "$STATE_DIR/seen-session.txt""#,
        );
        let tempdir = tempfile::tempdir().unwrap();
        let mut entries = HashMap::new();
        entries.insert(
            "p".to_string(),
            SessionSourceEntry {
                turn_script: script.path.to_string_lossy().into_owned(),
                transcript_locator: None,
                state_dir: Some(tempdir.path().to_path_buf()),
            },
        );
        let cfg = SessionsConfig { entries };

        let r = scan_provider_session("p", &cfg, &db, "session-123");

        assert_eq!(r.errors, Vec::<String>::new());
        assert_eq!(r.new_turns, 1);
        assert_eq!(db.count_session_turns("p", "session-123").unwrap().total, 1);
        let seen = std::fs::read_to_string(tempdir.path().join("seen-session.txt")).unwrap();
        assert_eq!(seen.trim(), "session-123");
    }

    #[test]
    fn locate_transcript_returns_none_when_no_locator_is_configured() {
        let cfg = cfg_with("p", std::path::Path::new("/bin/true"));

        let path = locate_transcript(&cfg, "p", "session-123").unwrap();

        assert_eq!(path, None);
    }

    #[test]
    fn locate_transcript_returns_script_stdout_path() {
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("session.jsonl");
        std::fs::write(&transcript, "{\"type\":\"system\"}\n").unwrap();
        let script = fixture_script(&format!(
            r#"printf '%s\n' "$SESSION_ID" > "$STATE_DIR/seen-session.txt"
printf '%s\n' "{}""#,
            transcript.display()
        ));
        let cfg = cfg_with_locator("p", &script.path, Some(dir.path().join("state")));

        let path = locate_transcript(&cfg, "p", "session-123")
            .unwrap()
            .expect("locator should return a path");

        assert_eq!(path, transcript);
        let seen =
            std::fs::read_to_string(dir.path().join("state").join("seen-session.txt")).unwrap();
        assert_eq!(seen.trim(), "session-123");
    }

    #[test]
    fn locate_transcript_returns_error_on_nonzero_exit() {
        let script = fixture_script("echo missing >&2; exit 9");
        let cfg = cfg_with_locator("p", &script.path, None);

        let err = locate_transcript(&cfg, "p", "session-123").unwrap_err();

        assert!(err.contains("exited 9"), "{err}");
    }

    /// Per contract: a locator that exits 0 but emits nothing on stdout
    /// is malformed (the script's contract requires a single line). The
    /// runner must surface this as Err so trace can show the user a
    /// degraded state, not silently succeed with an empty PathBuf.
    #[test]
    fn locate_transcript_returns_error_on_empty_stdout() {
        let script = fixture_script("# emit nothing; exit 0");
        let cfg = cfg_with_locator("p", &script.path, None);

        let err = locate_transcript(&cfg, "p", "session-123").unwrap_err();

        assert!(
            err.to_lowercase().contains("empty") || err.to_lowercase().contains("no path"),
            "expected 'empty' or 'no path' in error, got: {err}"
        );
    }

    fn cfg_from_adapter_fixture(provider: &str, fixture_name: &str) -> (Fixture, SessionsConfig) {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("runtime crate should live under <repo>/crates/oulipoly-runtime")
            .join("src-tauri")
            .join("tests")
            .join("fixtures")
            .join("jsonl")
            .join("adapter")
            .join(fixture_name);
        let script = fixture_script(&format!(r#"cat "{}""#, path.display()));
        let cfg = cfg_with(provider, &script.path);
        (script, cfg)
    }

    // risk: is_compaction_boundary ingest plumbing; level: particular-integration; source: proposal §11.1 is_compaction_boundary ingest plumbing / A3, A6.
    #[test]
    fn turn_script_optional_compaction_field_defaults_false() {
        let db = db();
        let (_script, cfg) = cfg_from_adapter_fixture("claude", "without_compaction.jsonl");

        let result = scan_provider("claude", &cfg, &db);

        assert_eq!(result.errors, Vec::<String>::new());
        assert_eq!(result.new_turns, 1);
        assert_eq!(
            db.latest_compaction_boundary("claude", "11111111-1111-4111-8111-111111111111")
                .unwrap(),
            None
        );
    }

    // risk: is_compaction_boundary ingest plumbing; level: particular-integration; source: proposal §11.1 is_compaction_boundary ingest plumbing / A3, A6.
    #[test]
    fn turn_script_compaction_field_propagates_to_session_turns() {
        let db = db();
        let (_script, cfg) = cfg_from_adapter_fixture("claude", "with_compaction.jsonl");

        let result = scan_provider("claude", &cfg, &db);

        assert_eq!(result.errors, Vec::<String>::new());
        assert_eq!(result.new_turns, 1);
        let boundary = db
            .latest_compaction_boundary("claude", "11111111-1111-4111-8111-111111111111")
            .unwrap()
            .expect("boundary turn should be persisted");
        assert_eq!(boundary.0, "boundary-1");
    }
}
