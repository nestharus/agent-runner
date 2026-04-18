//! CLI session log ingestion via user-provided adapter scripts.
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

use crate::config::{SessionSourceEntry, SessionsConfig};
use crate::state::{SessionTurnIngest, StateDb};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::path::PathBuf;
use std::process::{Command, Stdio};

// Generous to accommodate first-run scans across thousands of historical
// session files. Subsequent scans are bounded by the script's mtime cursor
// and finish in seconds.
const SCRIPT_TIMEOUT_SECS: u64 = 600;

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
}

/// Outcome of scanning one provider's session source.
#[derive(Debug, Clone, Default)]
pub struct ScanReport {
    pub new_turns: u64,
    pub script_lines: u64,
    pub errors: Vec<String>,
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
    let mut report = ScanReport::default();
    let Some(entry) = sessions_cfg.get(provider_name) else {
        return report;
    };

    let state_dir = resolve_state_dir(provider_name, entry);
    if let Err(e) = std::fs::create_dir_all(&state_dir) {
        report.errors.push(format!(
            "could not create state_dir {}: {e}",
            state_dir.display()
        ));
        return report;
    }

    let stdout = match run_turn_script(&entry.turn_script, &state_dir) {
        Ok(s) => s,
        Err(e) => {
            report.errors.push(e);
            return report;
        }
    };

    // Collect all turns first, then batch-insert in one transaction. For
    // the bootstrap scan (hundreds of thousands of rows), per-row inserts
    // are 100x slower than a transactional batch.
    let mut batch: Vec<SessionTurnIngest> = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        report.script_lines += 1;
        let turn: ScriptTurn = match serde_json::from_str(trimmed) {
            Ok(t) => t,
            Err(e) => {
                report
                    .errors
                    .push(format!("malformed turn line ({e}): {trimmed}"));
                continue;
            }
        };
        let timestamp = match DateTime::parse_from_rfc3339(&turn.timestamp) {
            Ok(t) => t.with_timezone(&Utc),
            Err(e) => {
                report
                    .errors
                    .push(format!("bad timestamp {}: {e}", turn.timestamp));
                continue;
            }
        };
        batch.push(SessionTurnIngest {
            session_id: turn.session_id,
            turn_id: turn.turn_id,
            timestamp,
            role: turn.role,
            parent_turn_id: turn.parent_turn_id,
            is_sidechain: turn.is_sidechain.unwrap_or(false),
        });
    }
    match db.ingest_session_turns_batch(provider_name, &batch) {
        Ok(n) => report.new_turns = n,
        Err(e) => report.errors.push(e),
    }
    report
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
    let base = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("oulipoly-agent-runner")
        .join("sessions");
    base.join(provider_name)
}

fn run_turn_script(script: &str, state_dir: &std::path::Path) -> Result<String, String> {
    run_session_script(script, state_dir, None, "turn script")
}

pub fn locate_transcript(
    sessions_cfg: &SessionsConfig,
    provider_name: &str,
    session_id: &str,
) -> Result<Option<PathBuf>, String> {
    let Some(entry) = sessions_cfg.get(provider_name) else {
        return Ok(None);
    };
    let Some(locator) = entry.transcript_locator.as_deref() else {
        return Ok(None);
    };

    let state_dir = resolve_state_dir(provider_name, entry);
    std::fs::create_dir_all(&state_dir)
        .map_err(|e| format!("could not create state_dir {}: {e}", state_dir.display()))?;

    let stdout = run_session_script(locator, &state_dir, Some(session_id), "transcript locator")?;
    let lines = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();

    match lines.as_slice() {
        [] => Err("transcript locator returned empty stdout".to_string()),
        [line] => Ok(Some(PathBuf::from(line))),
        _ => Err("transcript locator stdout was not a single line".to_string()),
    }
}

fn run_session_script(
    script: &str,
    state_dir: &std::path::Path,
    session_id: Option<&str>,
    script_kind: &str,
) -> Result<String, String> {
    use std::io::Read;

    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(script);
    cmd.env("STATE_DIR", state_dir);
    if let Some(session_id) = session_id {
        cmd.env("SESSION_ID", session_id);
    }
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn {script_kind}: {e}"))?;

    // Drain stdout/stderr concurrently. A naive `try_wait` loop deadlocks
    // for scripts that produce more than ~64KB on stdout: the kernel pipe
    // fills up, the script blocks on write, the loop waits for exit forever.
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let stdout_handle = std::thread::spawn(move || {
        let mut buf = String::new();
        let mut s = stdout;
        s.read_to_string(&mut buf).ok();
        buf
    });
    let stderr_handle = std::thread::spawn(move || {
        let mut buf = String::new();
        let mut s = stderr;
        s.read_to_string(&mut buf).ok();
        buf
    });

    let timeout = std::time::Duration::from_secs(SCRIPT_TIMEOUT_SECS);
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    return Err(format!(
                        "{} timed out after {SCRIPT_TIMEOUT_SECS}s",
                        capitalize_script_kind(script_kind)
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => {
                return Err(format!(
                    "{} wait failed: {e}",
                    capitalize_script_kind(script_kind)
                ));
            }
        }
    };

    let stdout_text = stdout_handle.join().unwrap_or_default();
    let stderr_text = stderr_handle.join().unwrap_or_default();

    if !status.success() {
        return Err(format!(
            "{} exited {}: {}",
            capitalize_script_kind(script_kind),
            status.code().unwrap_or(-1),
            stderr_text.trim()
        ));
    }
    Ok(stdout_text)
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
}
