use super::SessionStorageType;
use oulipoly_config::{ScriptSessionStorageType, SessionSourceEntry, SessionStorage};
use std::io::ErrorKind as StdIoErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

mod claude;
mod codex;

pub use claude::ClaudeStorageLocator;
pub use codex::CodexStorageLocator;

pub type ProviderName<'a> = &'a str;
pub type SessionId = String;
pub type IoErrorKind = StdIoErrorKind;

pub trait TranscriptLocator {
    fn locate_jsonl(&self, request: &TranscriptRequest) -> Result<LocatedTranscript, LocatorError>;
}

#[derive(Debug, Clone)]
pub struct TranscriptRequest<'a> {
    pub provider: ProviderName<'a>,
    pub session_id: SessionId,
    pub storage: Option<&'a SessionStorage>,
    pub sessions_config_locator: Option<&'a SessionSourceEntry>,
    pub mode: TranscriptLookupMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptLookupMode {
    RequireExisting,
    AllowMissing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocatedTranscript {
    pub path: PathBuf,
    pub storage_classification: SessionStorageType,
    pub require_existing_observed: bool,
}

#[derive(Debug, Clone)]
pub enum LocatorError {
    UnsupportedStorage {
        reason: UnsupportedStorageReason,
    },
    Io {
        kind: IoErrorKind,
        path: PathBuf,
        message: String,
    },
    NotFound {
        source: LocatorSource,
        session_id: SessionId,
    },
    Ambiguous {
        source: LocatorSource,
        candidates: Vec<PathBuf>,
    },
}

#[derive(Debug, Clone)]
pub enum UnsupportedStorageReason {
    NoTranscriptScriptForScriptStorage,
    NoLocatorForUnknownStorage,
    MissingJsonlPath {
        path: PathBuf,
    },
    RelativeJsonlPath {
        path: PathBuf,
    },
    NonUtf8JsonlPath,
    TranscriptScriptEmptyStdout,
    TranscriptScriptStdoutNotSingleLine {
        lines: usize,
    },
    TranscriptScriptSpawnFailed {
        kind: ScriptKind,
        message: String,
    },
    TranscriptScriptExit {
        kind: ScriptKind,
        code: i32,
        stderr: String,
    },
    ClaudeProjectsDirUnavailable {
        path: PathBuf,
        io_error: String,
    },
    ClaudeStorageScanAmbiguous {
        candidates: Vec<PathBuf>,
    },
    CodexSessionsDirUnavailable {
        path: PathBuf,
        io_error: String,
    },
    CodexStorageScanNotFound,
    CodexStorageScanAmbiguous {
        candidates: Vec<PathBuf>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocatorSource {
    Claude,
    Codex,
    TranscriptScript,
    SessionsLocator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptKind {
    TranscriptScript,
    SessionsTranscriptLocator,
}

pub fn unsupported_storage_reason_to_stem(reason: UnsupportedStorageReason) -> String {
    match reason {
        UnsupportedStorageReason::NoTranscriptScriptForScriptStorage => {
            "no_transcript_script_for_script_storage".to_string()
        }
        UnsupportedStorageReason::NoLocatorForUnknownStorage => "no_locator".to_string(),
        UnsupportedStorageReason::MissingJsonlPath { path } => {
            format!("missing_jsonl_path: {}", path.display())
        }
        UnsupportedStorageReason::RelativeJsonlPath { path } => {
            format!("relative_jsonl_path: {}", path.display())
        }
        UnsupportedStorageReason::NonUtf8JsonlPath => "non_utf8_jsonl_path".to_string(),
        UnsupportedStorageReason::TranscriptScriptEmptyStdout => {
            "transcript_script_empty_stdout".to_string()
        }
        UnsupportedStorageReason::TranscriptScriptStdoutNotSingleLine { .. } => {
            "transcript_script_stdout_not_single_line".to_string()
        }
        UnsupportedStorageReason::TranscriptScriptSpawnFailed { kind, message } => match kind {
            ScriptKind::TranscriptScript => format!("transcript_script_spawn_failed: {message}"),
            ScriptKind::SessionsTranscriptLocator => {
                format!("locator_error: Failed to spawn transcript locator: {message}")
            }
        },
        UnsupportedStorageReason::TranscriptScriptExit { kind, code, stderr } => match kind {
            ScriptKind::TranscriptScript => format!("transcript_script_exit_{code}: {stderr}"),
            ScriptKind::SessionsTranscriptLocator => {
                format!("locator_error: Transcript locator exited {code}: {stderr}")
            }
        },
        UnsupportedStorageReason::ClaudeProjectsDirUnavailable { path, io_error } => {
            format!(
                "claude_projects_dir_unavailable: {}: {io_error}",
                path.display()
            )
        }
        UnsupportedStorageReason::ClaudeStorageScanAmbiguous { .. } => {
            "claude_storage_scan_ambiguous".to_string()
        }
        UnsupportedStorageReason::CodexSessionsDirUnavailable { path, io_error } => {
            format!(
                "codex_sessions_dir_unavailable: {}: {io_error}",
                path.display()
            )
        }
        UnsupportedStorageReason::CodexStorageScanNotFound => {
            "codex_storage_scan_not_found".to_string()
        }
        UnsupportedStorageReason::CodexStorageScanAmbiguous { .. } => {
            "codex_storage_scan_ambiguous".to_string()
        }
    }
}

pub fn locator_error_to_stem(error: LocatorError) -> String {
    match error {
        LocatorError::UnsupportedStorage { reason } => unsupported_storage_reason_to_stem(reason),
        LocatorError::Io { message, .. } => format!("locator_error: {message}"),
        LocatorError::NotFound { source, .. } => match source {
            LocatorSource::Claude => "claude_storage_scan_not_found".to_string(),
            LocatorSource::Codex => "codex_storage_scan_not_found".to_string(),
            LocatorSource::TranscriptScript | LocatorSource::SessionsLocator => {
                "no_locator".to_string()
            }
        },
        LocatorError::Ambiguous { source, candidates } => match source {
            LocatorSource::Claude => unsupported_storage_reason_to_stem(
                UnsupportedStorageReason::ClaudeStorageScanAmbiguous { candidates },
            ),
            LocatorSource::Codex => unsupported_storage_reason_to_stem(
                UnsupportedStorageReason::CodexStorageScanAmbiguous { candidates },
            ),
            LocatorSource::TranscriptScript | LocatorSource::SessionsLocator => {
                "locator_error: ambiguous transcript locator output".to_string()
            }
        },
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct TranscriptScriptLocator;

impl TranscriptLocator for TranscriptScriptLocator {
    fn locate_jsonl(&self, request: &TranscriptRequest) -> Result<LocatedTranscript, LocatorError> {
        let script_storage = require_script_storage(request.storage)?;
        let script = require_transcript_script(script_storage.transcript_script)?;
        let stdout =
            run_transcript_script(script, &request.session_id, ScriptKind::TranscriptScript)?;
        let path = parse_transcript_script_stdout(&stdout)?;
        let path = validate_jsonl_path(path, request.mode)?;
        Ok(located(
            path,
            script_storage_type(script_storage.storage_type),
            request.mode,
        ))
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SessionsConfigLocator;

impl TranscriptLocator for SessionsConfigLocator {
    fn locate_jsonl(&self, request: &TranscriptRequest) -> Result<LocatedTranscript, LocatorError> {
        let entry = require_sessions_config_entry(request)?;
        let locator = require_sessions_transcript_locator(entry, &request.session_id)?;
        let state_dir = resolve_state_dir(entry, request.provider);
        create_state_dir(&state_dir)?;
        let stdout = run_sessions_transcript_locator(locator, &state_dir, &request.session_id)?;
        let path = parse_sessions_locator_stdout(&stdout)?;
        let path = validate_jsonl_path(path, request.mode)?;
        Ok(located(
            path,
            storage_classification(request.storage),
            request.mode,
        ))
    }
}

struct ScriptStorageFields<'a> {
    transcript_script: Option<&'a str>,
    storage_type: Option<ScriptSessionStorageType>,
}

fn require_script_storage(
    storage: Option<&SessionStorage>,
) -> Result<ScriptStorageFields<'_>, LocatorError> {
    script_storage_fields(storage).ok_or_else(no_locator)
}

fn script_storage_fields(storage: Option<&SessionStorage>) -> Option<ScriptStorageFields<'_>> {
    match storage {
        Some(SessionStorage::Script {
            transcript_script,
            storage_type,
            ..
        }) => Some(ScriptStorageFields {
            transcript_script: transcript_script.as_deref(),
            storage_type: *storage_type,
        }),
        _ => None,
    }
}

fn require_transcript_script(script: Option<&str>) -> Result<&str, LocatorError> {
    script.ok_or(LocatorError::UnsupportedStorage {
        reason: UnsupportedStorageReason::NoTranscriptScriptForScriptStorage,
    })
}

fn require_sessions_config_entry<'a>(
    request: &'a TranscriptRequest,
) -> Result<&'a SessionSourceEntry, LocatorError> {
    request
        .sessions_config_locator
        .ok_or_else(|| sessions_locator_not_found(&request.session_id))
}

fn require_sessions_transcript_locator<'a>(
    entry: &'a SessionSourceEntry,
    session_id: &str,
) -> Result<&'a str, LocatorError> {
    entry
        .transcript_locator
        .as_deref()
        .ok_or_else(|| sessions_locator_not_found(session_id))
}

fn sessions_locator_not_found(session_id: &str) -> LocatorError {
    LocatorError::NotFound {
        source: LocatorSource::SessionsLocator,
        session_id: session_id.to_string(),
    }
}

fn create_state_dir(state_dir: &Path) -> Result<(), LocatorError> {
    std::fs::create_dir_all(state_dir).map_err(|error| create_state_dir_error(state_dir, error))
}

fn create_state_dir_error(state_dir: &Path, error: std::io::Error) -> LocatorError {
    LocatorError::Io {
        kind: error.kind(),
        path: state_dir.to_path_buf(),
        message: create_state_dir_error_message(state_dir, &error),
    }
}

fn create_state_dir_error_message(state_dir: &Path, error: &std::io::Error) -> String {
    format!(
        "could not create state_dir {}: {error}",
        state_dir.display()
    )
}

pub(super) fn no_locator() -> LocatorError {
    LocatorError::UnsupportedStorage {
        reason: UnsupportedStorageReason::NoLocatorForUnknownStorage,
    }
}

pub(super) fn located(
    path: PathBuf,
    storage_classification: SessionStorageType,
    mode: TranscriptLookupMode,
) -> LocatedTranscript {
    LocatedTranscript {
        path,
        storage_classification,
        require_existing_observed: matches!(mode, TranscriptLookupMode::RequireExisting),
    }
}

fn script_storage_type(storage_type: Option<ScriptSessionStorageType>) -> SessionStorageType {
    match storage_type {
        Some(ScriptSessionStorageType::ClaudeCode) => SessionStorageType::ClaudeCode,
        Some(ScriptSessionStorageType::CodexSession) => SessionStorageType::CodexSession,
        None => SessionStorageType::Other,
    }
}

fn storage_classification(storage: Option<&SessionStorage>) -> SessionStorageType {
    match storage {
        Some(SessionStorage::Script { storage_type, .. }) => script_storage_type(*storage_type),
        Some(SessionStorage::ClaudeCode { .. }) => SessionStorageType::ClaudeCode,
        Some(SessionStorage::Codex { .. }) => SessionStorageType::CodexSession,
        None => SessionStorageType::Other,
    }
}

pub(super) fn validate_jsonl_path(
    path: PathBuf,
    mode: TranscriptLookupMode,
) -> Result<PathBuf, LocatorError> {
    if !path.is_absolute() {
        return Err(LocatorError::UnsupportedStorage {
            reason: UnsupportedStorageReason::RelativeJsonlPath { path },
        });
    }
    if !path.exists() {
        if matches!(mode, TranscriptLookupMode::AllowMissing) {
            if path.to_str().is_none() {
                return Err(LocatorError::UnsupportedStorage {
                    reason: UnsupportedStorageReason::NonUtf8JsonlPath,
                });
            }
            return Ok(path);
        }
        return Err(LocatorError::UnsupportedStorage {
            reason: UnsupportedStorageReason::MissingJsonlPath { path },
        });
    }
    let canonical = path
        .canonicalize()
        .map_err(|e| LocatorError::UnsupportedStorage {
            reason: canonicalization_failure_reason(&path, &e),
        })?;
    if canonical.to_str().is_none() {
        return Err(LocatorError::UnsupportedStorage {
            reason: UnsupportedStorageReason::NonUtf8JsonlPath,
        });
    }
    Ok(canonical)
}

fn canonicalization_failure_reason(
    path: &Path,
    error: &std::io::Error,
) -> UnsupportedStorageReason {
    UnsupportedStorageReason::MissingJsonlPath {
        path: PathBuf::from(format!("{}: {error}", path.display())),
    }
}

fn parse_transcript_script_stdout(stdout: &str) -> Result<PathBuf, LocatorError> {
    let lines = parse_stdout_lines(stdout);
    let line = single_transcript_stdout_line(&lines)?;
    Ok(stdout_line_path(line))
}

fn single_transcript_stdout_line<'a>(lines: &'a [&'a str]) -> Result<&'a str, LocatorError> {
    match lines {
        [] => Err(LocatorError::UnsupportedStorage {
            reason: UnsupportedStorageReason::TranscriptScriptEmptyStdout,
        }),
        [line] => Ok(line),
        _ => Err(LocatorError::UnsupportedStorage {
            reason: UnsupportedStorageReason::TranscriptScriptStdoutNotSingleLine {
                lines: lines.len(),
            },
        }),
    }
}

fn parse_sessions_locator_stdout(stdout: &str) -> Result<PathBuf, LocatorError> {
    let lines = parse_stdout_lines(stdout);
    let line = single_sessions_locator_stdout_line(&lines)?;
    Ok(stdout_line_path(line))
}

fn single_sessions_locator_stdout_line<'a>(lines: &'a [&'a str]) -> Result<&'a str, LocatorError> {
    match lines {
        [] => Err(LocatorError::Io {
            kind: IoErrorKind::InvalidData,
            path: PathBuf::new(),
            message: empty_sessions_locator_stdout_message(),
        }),
        [line] => Ok(line),
        _ => Err(LocatorError::Io {
            kind: IoErrorKind::InvalidData,
            path: PathBuf::new(),
            message: multiline_sessions_locator_stdout_message(),
        }),
    }
}

fn stdout_line_path(line: &str) -> PathBuf {
    PathBuf::from(line)
}

fn empty_sessions_locator_stdout_message() -> String {
    "transcript locator returned empty stdout".to_string()
}

fn multiline_sessions_locator_stdout_message() -> String {
    "transcript locator stdout was not a single line".to_string()
}

fn parse_stdout_lines(stdout: &str) -> Vec<&str> {
    keep_nonempty_lines(trim_lines(stdout))
}

fn trim_lines(stdout: &str) -> Vec<&str> {
    stdout.lines().map(str::trim).collect()
}

fn keep_nonempty_lines(lines: Vec<&str>) -> Vec<&str> {
    lines.into_iter().filter(|line| !line.is_empty()).collect()
}

fn run_transcript_script(
    script: &str,
    session_id: &str,
    kind: ScriptKind,
) -> Result<String, LocatorError> {
    let mut command = transcript_script_command(script, session_id);
    let output = run_script_process(&mut command, kind)?;
    validate_script_output(&output, kind)?;
    Ok(decode_stdout(&output))
}

fn run_sessions_transcript_locator(
    script: &str,
    state_dir: &Path,
    session_id: &str,
) -> Result<String, LocatorError> {
    let kind = ScriptKind::SessionsTranscriptLocator;
    let mut command = sessions_transcript_locator_command(script, state_dir, session_id);
    let output = run_script_process(&mut command, kind)?;
    validate_script_output(&output, kind)?;
    Ok(decode_stdout(&output))
}

fn transcript_script_command(script: &str, session_id: &str) -> Command {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(transcript_script_command_text(script))
        .arg("oulipoly-session-script")
        .arg(session_id)
        .env("SESSION_ID", session_id)
        .stdin(Stdio::null());
    command
}

fn transcript_script_command_text(script: &str) -> String {
    format!("{script} \"$1\"")
}

fn sessions_transcript_locator_command(
    script: &str,
    state_dir: &Path,
    session_id: &str,
) -> Command {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(script)
        .env("STATE_DIR", state_dir)
        .env("SESSION_ID", session_id)
        .stdin(Stdio::null());
    command
}

fn run_script_process(command: &mut Command, kind: ScriptKind) -> Result<Output, LocatorError> {
    script_process_output(command).map_err(|error| script_spawn_error(kind, error))
}

fn script_process_output(command: &mut Command) -> std::io::Result<Output> {
    command.output()
}

fn validate_script_output(output: &Output, kind: ScriptKind) -> Result<(), LocatorError> {
    if output.status.success() {
        Ok(())
    } else {
        Err(script_exit_error(kind, output))
    }
}

fn script_spawn_error(kind: ScriptKind, error: std::io::Error) -> LocatorError {
    LocatorError::UnsupportedStorage {
        reason: UnsupportedStorageReason::TranscriptScriptSpawnFailed {
            kind,
            message: error.to_string(),
        },
    }
}

fn script_exit_error(kind: ScriptKind, output: &Output) -> LocatorError {
    LocatorError::UnsupportedStorage {
        reason: UnsupportedStorageReason::TranscriptScriptExit {
            kind,
            code: script_exit_code(output),
            stderr: decode_stderr(output).trim().to_string(),
        },
    }
}

fn script_exit_code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

fn decode_stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn decode_stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn resolve_state_dir(entry: &SessionSourceEntry, provider_name: &str) -> PathBuf {
    configured_state_dir(entry).unwrap_or_else(|| default_state_dir(provider_name))
}

fn configured_state_dir(entry: &SessionSourceEntry) -> Option<PathBuf> {
    entry.state_dir.clone()
}

fn default_state_dir(provider_name: &str) -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("oulipoly-agent-runner")
        .join("sessions")
        .join(provider_name)
}

pub(super) fn single_jsonl_match(
    source: LocatorSource,
    session_id: &str,
    mut matches: Vec<PathBuf>,
) -> Result<PathBuf, LocatorError> {
    match match_cardinality(&matches) {
        MatchCardinality::One => Ok(matches.remove(0)),
        MatchCardinality::None => Err(not_found_error(source, session_id)),
        MatchCardinality::Many => Err(ambiguous_error(source, matches)),
    }
}

enum MatchCardinality {
    None,
    One,
    Many,
}

fn match_cardinality(matches: &[PathBuf]) -> MatchCardinality {
    match matches.len() {
        0 => MatchCardinality::None,
        1 => MatchCardinality::One,
        _ => MatchCardinality::Many,
    }
}

fn not_found_error(source: LocatorSource, session_id: &str) -> LocatorError {
    LocatorError::NotFound {
        source,
        session_id: session_id.to_string(),
    }
}

fn ambiguous_error(source: LocatorSource, candidates: Vec<PathBuf>) -> LocatorError {
    LocatorError::Ambiguous { source, candidates }
}
