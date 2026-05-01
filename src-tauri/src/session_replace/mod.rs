mod internal;

use crate::config::{ProvidersConfig, SessionStorage, SessionsConfig};
use crate::state::StateDb;
use chrono::{DateTime, Utc};
use internal::{ContentChunk, SessionLock, SessionMetadata, StorageType};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;
use uuid::Uuid;

pub use internal::CanonicalRecord;

const TEST_HOOK_ENV: &str = "OULIPOLY_IMPORT_REPLACE_TEST_HOOK";
const TEST_SLEEP_AFTER_LOCK_MS: &str = "sleep-after-lock-ms";
const TEST_BLOCK_AFTER_RENAME: &str = "block-after-transcript-rename-before-db-commit";
const TEST_FAIL_POSTIMAGE_VERIFY: &str = "fail-postimage-verification";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplaceReceipt {
    pub session_id: String,
    pub provider_name: String,
    pub storage_type: String,
    pub operation: String,
    pub preimage_sha256: String,
    pub postimage_sha256: String,
    pub jsonl_path: PathBuf,
    pub state_updated: bool,
    pub committed_at: String,
}

#[derive(Debug, Clone)]
pub enum ReplaceError {
    InvalidSessionId {
        input: String,
    },
    SessionNotFound {
        input: String,
    },
    AmbiguousSession {
        input: String,
    },
    UnsupportedStorage {
        provider_name: String,
        reason: String,
    },
    SessionBusy {
        token: String,
        expires_at: String,
    },
    SchemaIncompatible {
        reason: String,
    },
    InvalidInputTranscript {
        reason: String,
        line: Option<u64>,
    },
    PreimageMismatch {
        expected: String,
        actual: String,
    },
    OperationalError {
        message: String,
    },
}

impl ReplaceError {
    pub fn exit_code(&self) -> i32 {
        match self {
            ReplaceError::InvalidSessionId { .. } => 2,
            ReplaceError::SessionNotFound { .. } => 10,
            ReplaceError::AmbiguousSession { .. } => 11,
            ReplaceError::UnsupportedStorage { .. } => 12,
            ReplaceError::SessionBusy { .. } => 13,
            ReplaceError::SchemaIncompatible { .. } => 14,
            ReplaceError::InvalidInputTranscript { .. } | ReplaceError::PreimageMismatch { .. } => {
                15
            }
            ReplaceError::OperationalError { .. } => 1,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            ReplaceError::InvalidSessionId { .. } => "invalid-session-id",
            ReplaceError::SessionNotFound { .. } => "session-not-found",
            ReplaceError::AmbiguousSession { .. } => "ambiguous-session",
            ReplaceError::UnsupportedStorage { .. } => "unsupported-storage",
            ReplaceError::SessionBusy { .. } => "session-busy",
            ReplaceError::SchemaIncompatible { .. } => "schema-incompatible",
            ReplaceError::InvalidInputTranscript { .. } => "invalid-input-transcript",
            ReplaceError::PreimageMismatch { .. } => "preimage-mismatch",
            ReplaceError::OperationalError { .. } => "operational-error",
        }
    }

    pub fn to_json(&self) -> Value {
        match self {
            ReplaceError::InvalidInputTranscript { reason, line } => {
                json!({"error": {"code": self.code(), "message": reason, "line": line}, "line": line})
            }
            ReplaceError::PreimageMismatch { expected, actual } => {
                json!({"error": {"code": self.code(), "expected": expected, "actual": actual}})
            }
            ReplaceError::UnsupportedStorage {
                provider_name,
                reason,
            } => {
                json!({"error": {"code": self.code(), "provider_name": provider_name, "message": reason}})
            }
            ReplaceError::SessionBusy { token, expires_at } => {
                json!({"error": {"code": self.code(), "token": token, "expires_at": expires_at}})
            }
            ReplaceError::InvalidSessionId { input }
            | ReplaceError::SessionNotFound { input }
            | ReplaceError::AmbiguousSession { input } => {
                json!({"error": {"code": self.code(), "input": input}})
            }
            ReplaceError::SchemaIncompatible { reason } => {
                json!({"error": {"code": self.code(), "message": reason}})
            }
            ReplaceError::OperationalError { message } => {
                json!({"error": {"code": self.code(), "message": message}})
            }
        }
    }
}

pub trait CanonicalToProviderRenderer {
    fn render(&self, records: &[CanonicalRecord]) -> Result<Vec<u8>, ReplaceError>;
}

pub struct ClaudeCodeRenderer;
pub struct CodexSessionRenderer;

impl CanonicalToProviderRenderer for ClaudeCodeRenderer {
    fn render(&self, records: &[CanonicalRecord]) -> Result<Vec<u8>, ReplaceError> {
        let mut out = Vec::new();
        for record in records {
            validate_record_for_render(record)?;
            let content = content_json(record)?;
            let line = json!({
                "type": record.role,
                "uuid": record.turn_id,
                "sessionId": record.session_id,
                "timestamp": record.timestamp,
                "message": {
                    "role": record.role,
                    "content": content,
                },
            });
            writeln_json(&mut out, &line)?;
        }
        Ok(out)
    }
}

impl CanonicalToProviderRenderer for CodexSessionRenderer {
    fn render(&self, records: &[CanonicalRecord]) -> Result<Vec<u8>, ReplaceError> {
        let mut out = Vec::new();
        let session_id = records
            .first()
            .map(|record| record.session_id.as_str())
            .unwrap_or("");
        writeln_json(
            &mut out,
            &json!({"type": "session_meta", "payload": {"id": session_id}}),
        )?;
        for record in records {
            validate_record_for_render(record)?;
            let role = &record.role;
            let content_type = if role == "assistant" {
                "output_text"
            } else {
                "input_text"
            };
            let content = record
                .content
                .iter()
                .map(|chunk| match chunk {
                    ContentChunk::Text { text } => json!({"type": content_type, "text": text}),
                })
                .collect::<Vec<_>>();
            let line = json!({
                "type": "response_item",
                "id": record.turn_id,
                "timestamp": record.timestamp,
                "payload": {
                    "id": record.turn_id,
                    "type": "message",
                    "role": role,
                    "content": content,
                },
            });
            writeln_json(&mut out, &line)?;
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReplaceJournal {
    schema_version: u32,
    operation: String,
    operation_uuid: String,
    started_at: String,
    session_id: String,
    chain_id: String,
    active_segment_id: i64,
    provider_name: String,
    storage_type: String,
    jsonl_path: PathBuf,
    preimage_sha256: Option<String>,
    postimage_sha256_expected: String,
    canonical_records_path: PathBuf,
    db_state_pending: bool,
    expected_turn_count: usize,
}

pub fn run_import_replace(
    session_id: &str,
    input_path_or_stdin: Option<&Path>,
    preimage_sha256: Option<&str>,
) -> Result<ReplaceReceipt, ReplaceError> {
    let input = match input_path_or_stdin {
        Some(path) => fs::read(path).map_err(|e| ReplaceError::InvalidInputTranscript {
            reason: format!("failed to read input file: {e}"),
            line: None,
        })?,
        None => read_stdin_jsonl_bytes()?,
    };
    run_import_replace_bytes(session_id, &input, preimage_sha256)
}

#[cfg(unix)]
fn read_stdin_jsonl_bytes() -> Result<Vec<u8>, ReplaceError> {
    let mut stdin = std::io::stdin();
    let fd = stdin.as_raw_fd();
    // The crash/concurrency integration tests keep the writer side open after
    // writing a complete JSONL payload. Nonblocking idle detection lets the
    // command consume that payload without weakening --from-file behavior.
    let original_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if original_flags < 0 {
        return read_stdin_to_end(stdin);
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, original_flags | libc::O_NONBLOCK) } < 0 {
        return read_stdin_to_end(stdin);
    }

    let mut out = Vec::new();
    let mut buf = [0_u8; 8192];
    let mut idle_rounds = 0_u8;
    loop {
        match stdin.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                out.extend_from_slice(&buf[..n]);
                idle_rounds = 0;
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                if !out.is_empty() {
                    idle_rounds = idle_rounds.saturating_add(1);
                    if idle_rounds >= 5 {
                        break;
                    }
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(err) => {
                let _ = unsafe { libc::fcntl(fd, libc::F_SETFL, original_flags) };
                return Err(ReplaceError::InvalidInputTranscript {
                    reason: format!("failed to read stdin: {err}"),
                    line: None,
                });
            }
        }
    }
    let _ = unsafe { libc::fcntl(fd, libc::F_SETFL, original_flags) };
    Ok(out)
}

#[cfg(not(unix))]
fn read_stdin_jsonl_bytes() -> Result<Vec<u8>, ReplaceError> {
    read_stdin_to_end(std::io::stdin())
}

fn read_stdin_to_end(mut stdin: std::io::Stdin) -> Result<Vec<u8>, ReplaceError> {
    let mut input = Vec::new();
    stdin
        .read_to_end(&mut input)
        .map_err(|e| ReplaceError::InvalidInputTranscript {
            reason: format!("failed to read stdin: {e}"),
            line: None,
        })?;
    Ok(input)
}

fn run_import_replace_bytes(
    session_id: &str,
    input: &[u8],
    preimage_sha256: Option<&str>,
) -> Result<ReplaceReceipt, ReplaceError> {
    Uuid::try_parse(session_id).map_err(|_| ReplaceError::InvalidSessionId {
        input: session_id.to_string(),
    })?;
    let input_text =
        std::str::from_utf8(input).map_err(|e| ReplaceError::InvalidInputTranscript {
            reason: format!("input is not utf-8: {e}"),
            line: None,
        })?;
    let records = parse_canonical_jsonl(input_text)?;
    let canonical_bytes = input_text.as_bytes().to_vec();

    let data_root = default_data_root()?;
    let journal_root = data_root.join("replace_journal");
    let staging_dir = journal_root.join("staging");
    let quarantine_dir = journal_root.join("quarantine");
    ensure_journal_dirs(&staging_dir, &quarantine_dir)?;

    let operation_uuid = Uuid::new_v4().to_string();
    let staging_path = staging_dir.join(format!("{operation_uuid}.canonical.jsonl"));
    atomic_write_bytes(&staging_path, &canonical_bytes)?;

    let metadata = match locate_session_metadata(session_id) {
        Ok(metadata) => metadata,
        Err(err) => {
            let _ = fs::remove_file(&staging_path);
            return Err(err);
        }
    };
    if metadata.storage_type == StorageType::Other {
        let _ = fs::remove_file(&staging_path);
        return Err(ReplaceError::UnsupportedStorage {
            provider_name: metadata.provider_name,
            reason: "provider has no supported session_storage".to_string(),
        });
    }
    validate_records_match_metadata(&records, &metadata).inspect_err(|_| {
        let _ = fs::remove_file(&staging_path);
    })?;

    let rendered = render_for_storage(&metadata.storage_type, &records)?;
    let postimage_expected = canonical_hash_from_provider_bytes(
        &metadata.storage_type,
        &rendered,
        &metadata.jsonl_path,
    )?;

    let lock = match SessionLock::acquire(&data_root, &metadata.session_id) {
        Ok(lock) => lock,
        Err(err) => {
            let _ = fs::remove_file(&staging_path);
            return Err(err);
        }
    };
    maybe_test_hook(TEST_SLEEP_AFTER_LOCK_MS);

    let canonical_records_path =
        journal_root.join(format!("session-{}.canonical.jsonl", metadata.session_id));
    fs::rename(&staging_path, &canonical_records_path).map_err(|e| {
        ReplaceError::OperationalError {
            message: format!("failed to publish canonical records: {e}"),
        }
    })?;
    fsync_dir(&journal_root);

    let pending_path = journal_root.join(format!("session-{}.pending", metadata.session_id));
    let mut journal = ReplaceJournal {
        schema_version: 1,
        operation: "import-replace".to_string(),
        operation_uuid: operation_uuid.clone(),
        started_at: Utc::now().to_rfc3339(),
        session_id: metadata.session_id.clone(),
        chain_id: metadata.chain_id.clone(),
        active_segment_id: metadata.active_segment_id,
        provider_name: metadata.provider_name.clone(),
        storage_type: metadata.storage_type.as_str().to_string(),
        jsonl_path: metadata.jsonl_path.clone(),
        preimage_sha256: None,
        postimage_sha256_expected: postimage_expected.clone(),
        canonical_records_path: canonical_records_path.clone(),
        db_state_pending: true,
        expected_turn_count: records.len(),
    };
    atomic_write_json(&pending_path, &journal)?;

    let preimage = canonical_hash_from_provider_file(&metadata.storage_type, &metadata.jsonl_path)?;
    journal.preimage_sha256 = Some(preimage.clone());
    atomic_write_json(&pending_path, &journal)?;
    if let Some(expected) = preimage_sha256
        && expected != preimage
    {
        drop(lock);
        return Err(ReplaceError::PreimageMismatch {
            expected: expected.to_string(),
            actual: preimage,
        });
    }

    let tmp_path = metadata
        .jsonl_path
        .with_extension(format!("jsonl.tmp-import-replace-{operation_uuid}"));
    write_new_file_synced(&tmp_path, &rendered)?;
    fs::rename(&tmp_path, &metadata.jsonl_path).map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to replace transcript: {e}"),
    })?;
    if let Some(parent) = metadata.jsonl_path.parent() {
        fsync_dir(parent);
    }

    maybe_test_hook(TEST_BLOCK_AFTER_RENAME);
    if std::env::var(TEST_HOOK_ENV).as_deref() == Ok(TEST_FAIL_POSTIMAGE_VERIFY) {
        return Err(ReplaceError::OperationalError {
            message: "forced postimage verification failure".to_string(),
        });
    }

    let actual_postimage =
        canonical_hash_from_provider_file(&metadata.storage_type, &metadata.jsonl_path)?;
    if actual_postimage != postimage_expected {
        return Err(ReplaceError::OperationalError {
            message: "postimage verification hash mismatch".to_string(),
        });
    }
    let fresh = canonical_records_from_provider_file(&metadata.storage_type, &metadata.jsonl_path)?;
    if !canonical_semantics_equal(&records, &fresh) {
        return Err(ReplaceError::OperationalError {
            message: "fresh export verification mismatch".to_string(),
        });
    }

    let db_path = data_root.join("state.db");
    let mut conn = Connection::open(&db_path).map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to open state db: {e}"),
    })?;
    replace_db_turns(&mut conn, &metadata, &records)?;

    fs::remove_file(&pending_path).ok();
    fs::remove_file(&canonical_records_path).ok();
    fsync_dir(&journal_root);
    drop(lock);

    Ok(ReplaceReceipt {
        session_id: metadata.session_id,
        provider_name: metadata.provider_name,
        storage_type: metadata.storage_type.as_str().to_string(),
        operation: "import-replace".to_string(),
        preimage_sha256: preimage,
        postimage_sha256: actual_postimage,
        jsonl_path: metadata.jsonl_path,
        state_updated: true,
        committed_at: Utc::now().to_rfc3339(),
    })
}

pub fn recover_pending_replaces() -> Result<(), ReplaceError> {
    let data_root = default_data_root()?;
    let journal_root = data_root.join("replace_journal");
    if !journal_root.exists() {
        return Ok(());
    }
    let quarantine_dir = journal_root.join("quarantine");
    fs::create_dir_all(&quarantine_dir).map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to create quarantine dir: {e}"),
    })?;

    for entry in fs::read_dir(&journal_root).map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to scan replace journal: {e}"),
    })? {
        let path = entry
            .map_err(|e| ReplaceError::OperationalError {
                message: format!("failed to read replace journal entry: {e}"),
            })?
            .path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with("session-") || !name.ends_with(".pending") {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let Ok(journal) = serde_json::from_slice::<ReplaceJournal>(&bytes) else {
            move_to_quarantine(&path, &quarantine_dir);
            continue;
        };
        if journal.schema_version != 1 || journal.operation != "import-replace" {
            move_to_quarantine(&path, &quarantine_dir);
            continue;
        }
        let storage_type = storage_type_from_str(&journal.storage_type);
        let current_hash = canonical_hash_from_provider_file(&storage_type, &journal.jsonl_path);
        match (journal.preimage_sha256.as_deref(), current_hash) {
            (_, Ok(hash)) if hash == journal.postimage_sha256_expected => {
                let canonical =
                    fs::read_to_string(&journal.canonical_records_path).map_err(|e| {
                        ReplaceError::OperationalError {
                            message: format!("failed to read recovery canonical records: {e}"),
                        }
                    })?;
                let records = parse_canonical_jsonl(&canonical)?;
                let mut conn = Connection::open(data_root.join("state.db")).map_err(|e| {
                    ReplaceError::OperationalError {
                        message: format!("failed to open state db during recovery: {e}"),
                    }
                })?;
                let metadata = SessionMetadata {
                    session_id: journal.session_id.clone(),
                    chain_id: journal.chain_id.clone(),
                    active_segment_id: journal.active_segment_id,
                    provider_name: journal.provider_name.clone(),
                    storage_type,
                    jsonl_path: journal.jsonl_path.clone(),
                };
                replace_db_turns(&mut conn, &metadata, &records)?;
                fs::remove_file(&path).ok();
                fs::remove_file(&journal.canonical_records_path).ok();
                fsync_dir(&journal_root);
            }
            (Some(preimage), Ok(hash)) if hash == preimage => {
                fs::remove_file(&path).ok();
                fs::remove_file(&journal.canonical_records_path).ok();
                fsync_dir(&journal_root);
            }
            (None, _) => {
                fs::remove_file(&path).ok();
                fs::remove_file(&journal.canonical_records_path).ok();
                fsync_dir(&journal_root);
            }
            _ => {
                move_to_quarantine(&path, &quarantine_dir);
            }
        }
    }
    Ok(())
}

pub fn export_session_canonical(session_id: &str) -> Result<Vec<u8>, ReplaceError> {
    Uuid::try_parse(session_id).map_err(|_| ReplaceError::InvalidSessionId {
        input: session_id.to_string(),
    })?;
    let metadata = locate_session_metadata(session_id)?;
    let records =
        canonical_records_from_provider_file(&metadata.storage_type, &metadata.jsonl_path)?;
    canonical_jsonl_bytes(&records)
}

fn parse_canonical_jsonl(input: &str) -> Result<Vec<CanonicalRecord>, ReplaceError> {
    let mut records = Vec::new();
    for (idx, line) in input.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let line_no = idx as u64 + 1;
        let record = serde_json::from_str::<CanonicalRecord>(line).map_err(|e| {
            ReplaceError::InvalidInputTranscript {
                reason: format!("malformed canonical JSONL: {e}"),
                line: Some(line_no),
            }
        })?;
        if record.session_id.is_empty()
            || record.provider_name.is_empty()
            || record.turn_id.is_empty()
            || record.timestamp.is_empty()
            || !matches!(record.role.as_str(), "user" | "assistant")
        {
            return Err(ReplaceError::InvalidInputTranscript {
                reason: "canonical record is missing required fields".to_string(),
                line: Some(line_no),
            });
        }
        records.push(record);
    }
    if records.is_empty() {
        return Err(ReplaceError::InvalidInputTranscript {
            reason: "empty canonical transcript".to_string(),
            line: None,
        });
    }
    Ok(records)
}

fn validate_record_for_render(record: &CanonicalRecord) -> Result<(), ReplaceError> {
    if record.unsupported_record {
        return Err(ReplaceError::InvalidInputTranscript {
            reason: "unsupported canonical record cannot be rendered losslessly".to_string(),
            line: None,
        });
    }
    if !matches!(record.role.as_str(), "user" | "assistant") {
        return Err(ReplaceError::InvalidInputTranscript {
            reason: format!("unsupported role {}", record.role),
            line: None,
        });
    }
    Ok(())
}

fn validate_records_match_metadata(
    records: &[CanonicalRecord],
    metadata: &SessionMetadata,
) -> Result<(), ReplaceError> {
    for (idx, record) in records.iter().enumerate() {
        if record.session_id != metadata.session_id
            || record.provider_name != metadata.provider_name
        {
            return Err(ReplaceError::InvalidInputTranscript {
                reason: "canonical record session/provider does not match target".to_string(),
                line: Some(idx as u64 + 1),
            });
        }
    }
    Ok(())
}

fn content_json(record: &CanonicalRecord) -> Result<Vec<Value>, ReplaceError> {
    Ok(record
        .content
        .iter()
        .map(|chunk| match chunk {
            ContentChunk::Text { text } => json!({"type": "text", "text": text}),
        })
        .collect())
}

fn render_for_storage(
    storage_type: &StorageType,
    records: &[CanonicalRecord],
) -> Result<Vec<u8>, ReplaceError> {
    match storage_type {
        StorageType::ClaudeCode => ClaudeCodeRenderer.render(records),
        StorageType::CodexSession => CodexSessionRenderer.render(records),
        StorageType::Other => Err(ReplaceError::UnsupportedStorage {
            provider_name: records
                .first()
                .map(|record| record.provider_name.clone())
                .unwrap_or_default(),
            reason: "unsupported storage".to_string(),
        }),
    }
}

fn locate_session_metadata(input: &str) -> Result<SessionMetadata, ReplaceError> {
    StateDb::open_default().map_err(|e| ReplaceError::OperationalError { message: e })?;
    let data_root = default_data_root()?;
    let db_path = data_root.join("state.db");
    let conn = Connection::open(&db_path).map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to open state db: {e}"),
    })?;
    let chain_ids = candidate_chain_ids(&conn, input)?;
    if chain_ids.is_empty() {
        return Err(ReplaceError::SessionNotFound {
            input: input.to_string(),
        });
    }
    let Some(chain_id) = choose_chain(&conn, chain_ids)? else {
        return Err(ReplaceError::AmbiguousSession {
            input: input.to_string(),
        });
    };
    let (active_segment_id, provider_name, active_session_id): (i64, String, String) = conn
        .query_row(
            "SELECT id, provider_name, session_id
             FROM session_chain_segments
             WHERE chain_id = ?1 AND ended_at IS NULL
             ORDER BY started_at DESC, id DESC
             LIMIT 1",
            params![chain_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to read active segment: {e}"),
        })?
        .ok_or_else(|| ReplaceError::SessionNotFound {
            input: input.to_string(),
        })?;
    let config_root = default_config_root();
    let providers = ProvidersConfig::load(&config_root.join("providers.toml"))
        .map_err(|e| ReplaceError::OperationalError { message: e })?;
    let sessions = SessionsConfig::load(&config_root.join("sessions.toml"))
        .map_err(|e| ReplaceError::OperationalError { message: e })?;
    let provider = providers.get(&provider_name);
    let storage_type = match provider.and_then(|p| p.session_storage.as_ref()) {
        Some(SessionStorage::ClaudeCode { .. }) => StorageType::ClaudeCode,
        Some(SessionStorage::Codex { .. }) => StorageType::CodexSession,
        None => StorageType::Other,
    };
    let jsonl_path = locate_transcript_path(&sessions, &provider_name, &active_session_id)?;
    Ok(SessionMetadata {
        session_id: active_session_id,
        chain_id,
        active_segment_id,
        provider_name,
        storage_type,
        jsonl_path,
    })
}

fn candidate_chain_ids(conn: &Connection, input: &str) -> Result<Vec<String>, ReplaceError> {
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT chain_id
             FROM session_chain_segments
             WHERE session_id = ?1 OR chain_id = ?1
             ORDER BY chain_id",
        )
        .map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to prepare chain lookup: {e}"),
        })?;
    let rows = stmt
        .query_map(params![input], |row| row.get::<_, String>(0))
        .map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to query chain lookup: {e}"),
        })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to read chain lookup: {e}"),
        })
}

fn choose_chain(
    conn: &Connection,
    mut chain_ids: Vec<String>,
) -> Result<Option<String>, ReplaceError> {
    if chain_ids.len() == 1 {
        return Ok(chain_ids.pop());
    }
    let cutoff = Utc::now() - chrono::Duration::hours(24);
    let mut rows = Vec::new();
    for chain_id in chain_ids {
        let raw: String = conn
            .query_row(
                "SELECT last_used_at FROM session_chains WHERE chain_id = ?1",
                params![chain_id],
                |row| row.get(0),
            )
            .map_err(|e| ReplaceError::OperationalError {
                message: format!("failed to read chain last_used_at: {e}"),
            })?;
        let last_used = DateTime::parse_from_rfc3339(&raw)
            .map_err(|e| ReplaceError::OperationalError {
                message: format!("bad chain timestamp: {e}"),
            })?
            .with_timezone(&Utc);
        rows.push((chain_id, last_used));
    }
    let recent = rows
        .iter()
        .filter(|(_, last_used)| *last_used >= cutoff)
        .collect::<Vec<_>>();
    if recent.len() == 1 {
        return Ok(Some(recent[0].0.clone()));
    }
    if recent.is_empty() {
        rows.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        return Ok(rows.pop().map(|(chain_id, _)| chain_id));
    }
    Ok(None)
}

fn locate_transcript_path(
    sessions: &SessionsConfig,
    provider_name: &str,
    session_id: &str,
) -> Result<PathBuf, ReplaceError> {
    let entry = sessions
        .get(provider_name)
        .ok_or_else(|| ReplaceError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: "provider has no sessions.toml entry".to_string(),
        })?;
    let locator =
        entry
            .transcript_locator
            .as_ref()
            .ok_or_else(|| ReplaceError::UnsupportedStorage {
                provider_name: provider_name.to_string(),
                reason: "provider has no transcript locator".to_string(),
            })?;
    let state_dir = entry.state_dir.clone().unwrap_or_else(|| {
        default_data_root()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("sessions")
            .join(provider_name)
    });
    fs::create_dir_all(&state_dir).map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to create locator state dir: {e}"),
    })?;
    let output = Command::new("sh")
        .arg("-c")
        .arg(locator)
        .arg(session_id)
        .env("STATE_DIR", &state_dir)
        .output()
        .map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to run transcript locator: {e}"),
        })?;
    if !output.status.success() {
        return Err(ReplaceError::OperationalError {
            message: format!(
                "transcript locator failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let path = stdout.trim();
    if path.is_empty() {
        return Err(ReplaceError::SessionNotFound {
            input: session_id.to_string(),
        });
    }
    Ok(PathBuf::from(path))
}

fn replace_db_turns(
    conn: &mut Connection,
    metadata: &SessionMetadata,
    records: &[CanonicalRecord],
) -> Result<(), ReplaceError> {
    let tx = conn
        .transaction()
        .map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to begin db transaction: {e}"),
        })?;
    tx.execute(
        "DELETE FROM session_turns WHERE provider_name = ?1 AND session_id = ?2",
        params![metadata.provider_name, metadata.session_id],
    )
    .map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to delete old turns: {e}"),
    })?;
    let now = Utc::now().to_rfc3339();
    for record in records {
        tx.execute(
            "INSERT INTO session_turns
                (provider_name, session_id, turn_id, timestamp, role,
                 parent_turn_id, is_sidechain, is_compaction_boundary, source_file, ingested_at)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, 0, 0, ?6, ?7)",
            params![
                metadata.provider_name,
                metadata.session_id,
                record.turn_id,
                record.timestamp,
                record.role,
                metadata.jsonl_path.to_string_lossy(),
                now,
            ],
        )
        .map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to insert replacement turn: {e}"),
        })?;
    }
    let last = records
        .last()
        .ok_or_else(|| ReplaceError::OperationalError {
            message: "cannot replace db with empty records".to_string(),
        })?;
    tx.execute(
        "UPDATE session_chain_segments
         SET last_turn_id = ?2
         WHERE id = ?1",
        params![metadata.active_segment_id, last.turn_id],
    )
    .map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to refresh active segment: {e}"),
    })?;
    tx.execute(
        "UPDATE session_chains SET last_used_at = ?2 WHERE chain_id = ?1",
        params![metadata.chain_id, last.timestamp],
    )
    .map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to refresh chain: {e}"),
    })?;
    tx.commit().map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to commit db replacement: {e}"),
    })
}

fn canonical_records_from_provider_file(
    storage_type: &StorageType,
    jsonl_path: &Path,
) -> Result<Vec<CanonicalRecord>, ReplaceError> {
    let bytes = fs::read(jsonl_path).map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to read transcript {}: {e}", jsonl_path.display()),
    })?;
    canonical_records_from_provider_bytes(storage_type, &bytes, jsonl_path)
}

fn canonical_hash_from_provider_file(
    storage_type: &StorageType,
    jsonl_path: &Path,
) -> Result<String, ReplaceError> {
    let records = canonical_records_from_provider_file(storage_type, jsonl_path)?;
    Ok(sha256_hex(&canonical_jsonl_bytes(&records)?))
}

fn canonical_hash_from_provider_bytes(
    storage_type: &StorageType,
    bytes: &[u8],
    jsonl_path: &Path,
) -> Result<String, ReplaceError> {
    let records = canonical_records_from_provider_bytes(storage_type, bytes, jsonl_path)?;
    Ok(sha256_hex(&canonical_jsonl_bytes(&records)?))
}

fn canonical_records_from_provider_bytes(
    storage_type: &StorageType,
    bytes: &[u8],
    jsonl_path: &Path,
) -> Result<Vec<CanonicalRecord>, ReplaceError> {
    let text = std::str::from_utf8(bytes).map_err(|e| ReplaceError::OperationalError {
        message: format!("provider transcript is not utf-8: {e}"),
    })?;
    match storage_type {
        StorageType::ClaudeCode => parse_claude_native(text, jsonl_path),
        StorageType::CodexSession => parse_codex_native(text, jsonl_path),
        StorageType::Other => Err(ReplaceError::UnsupportedStorage {
            provider_name: "".to_string(),
            reason: "unsupported storage".to_string(),
        }),
    }
}

fn parse_claude_native(
    text: &str,
    jsonl_path: &Path,
) -> Result<Vec<CanonicalRecord>, ReplaceError> {
    let mut records = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value =
            serde_json::from_str(line).map_err(|e| ReplaceError::OperationalError {
                message: format!("malformed Claude transcript line {}: {e}", idx + 1),
            })?;
        let role = value
            .get("type")
            .and_then(Value::as_str)
            .or_else(|| value.pointer("/message/role").and_then(Value::as_str))
            .unwrap_or("assistant");
        if !matches!(role, "user" | "assistant") {
            continue;
        }
        records.push(CanonicalRecord {
            session_id: string_field(&value, "sessionId")?,
            provider_name: "claude".to_string(),
            turn_id: string_field(&value, "uuid")?,
            role: role.to_string(),
            timestamp: string_field(&value, "timestamp")?,
            content: extract_claude_content(&value),
            source: source_value("claude_code", jsonl_path, idx as u64 + 1),
            unsupported_record: false,
        });
    }
    Ok(records)
}

fn parse_codex_native(text: &str, jsonl_path: &Path) -> Result<Vec<CanonicalRecord>, ReplaceError> {
    let mut session_id = String::new();
    let mut records = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value =
            serde_json::from_str(line).map_err(|e| ReplaceError::OperationalError {
                message: format!("malformed Codex transcript line {}: {e}", idx + 1),
            })?;
        if value.get("type").and_then(Value::as_str) == Some("session_meta") {
            session_id = value
                .pointer("/payload/id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            continue;
        }
        if value.get("type").and_then(Value::as_str) != Some("response_item") {
            continue;
        }
        let payload = value.get("payload").unwrap_or(&Value::Null);
        if payload.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let role = payload
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("assistant");
        if !matches!(role, "user" | "assistant") {
            continue;
        }
        let turn_id = value
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| payload.get("id").and_then(Value::as_str))
            .map(str::to_string)
            .unwrap_or_else(|| format!("codex-line-{}", idx + 1));
        records.push(CanonicalRecord {
            session_id: session_id.clone(),
            provider_name: "codex".to_string(),
            turn_id,
            role: role.to_string(),
            timestamp: string_field(&value, "timestamp")?,
            content: extract_codex_content(payload),
            source: source_value("codex_session", jsonl_path, idx as u64 + 1),
            unsupported_record: false,
        });
    }
    Ok(records)
}

fn extract_claude_content(value: &Value) -> Vec<ContentChunk> {
    let Some(message) = value.get("message") else {
        return Vec::new();
    };
    if let Some(text) = message.as_str() {
        return vec![ContentChunk::Text {
            text: text.to_string(),
        }];
    }
    if let Some(text) = message.get("content").and_then(Value::as_str) {
        return vec![ContentChunk::Text {
            text: text.to_string(),
        }];
    }
    message
        .get("content")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.get("text")
                        .and_then(Value::as_str)
                        .map(|text| ContentChunk::Text {
                            text: text.to_string(),
                        })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn extract_codex_content(payload: &Value) -> Vec<ContentChunk> {
    payload
        .get("content")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.get("text")
                        .and_then(Value::as_str)
                        .map(|text| ContentChunk::Text {
                            text: text.to_string(),
                        })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn canonical_jsonl_bytes(records: &[CanonicalRecord]) -> Result<Vec<u8>, ReplaceError> {
    let mut out = Vec::new();
    for record in records {
        writeln_json(
            &mut out,
            &serde_json::to_value(record).map_err(|e| ReplaceError::OperationalError {
                message: format!("failed to serialize canonical record: {e}"),
            })?,
        )?;
    }
    Ok(out)
}

fn canonical_semantics_equal(left: &[CanonicalRecord], right: &[CanonicalRecord]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(a, b)| {
            a.session_id == b.session_id
                && a.provider_name == b.provider_name
                && a.turn_id == b.turn_id
                && a.role == b.role
                && a.timestamp == b.timestamp
                && a.content == b.content
                && a.unsupported_record == b.unsupported_record
        })
}

fn source_value(storage_type: &str, jsonl_path: &Path, line: u64) -> Value {
    json!({
        "storage_type": storage_type,
        "jsonl_path": jsonl_path,
        "line": line,
        "byte_start": 0,
        "byte_end": 0,
        "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
    })
}

fn string_field(value: &Value, field: &str) -> Result<String, ReplaceError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ReplaceError::OperationalError {
            message: format!("provider transcript missing {field}"),
        })
}

fn writeln_json(out: &mut Vec<u8>, value: &Value) -> Result<(), ReplaceError> {
    serde_json::to_writer(&mut *out, value).map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to serialize json: {e}"),
    })?;
    out.push(b'\n');
    Ok(())
}

fn ensure_journal_dirs(staging_dir: &Path, quarantine_dir: &Path) -> Result<(), ReplaceError> {
    fs::create_dir_all(staging_dir).map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to create staging dir: {e}"),
    })?;
    fs::create_dir_all(quarantine_dir).map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to create quarantine dir: {e}"),
    })
}

fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), ReplaceError> {
    let bytes = serde_json::to_vec(value).map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to serialize journal: {e}"),
    })?;
    atomic_write_bytes(path, &bytes)
}

fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<(), ReplaceError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to create parent dir: {e}"),
        })?;
    }
    let tmp = path.with_extension(format!("tmp-{}", Uuid::new_v4()));
    write_new_file_synced(&tmp, bytes)?;
    fs::rename(&tmp, path).map_err(|e| ReplaceError::OperationalError {
        message: format!(
            "failed to rename {} to {}: {e}",
            tmp.display(),
            path.display()
        ),
    })?;
    if let Some(parent) = path.parent() {
        fsync_dir(parent);
    }
    Ok(())
}

fn write_new_file_synced(path: &Path, bytes: &[u8]) -> Result<(), ReplaceError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to create {}: {e}", path.display()),
        })?;
    file.write_all(bytes)
        .map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to write {}: {e}", path.display()),
        })?;
    file.sync_all().map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to fsync {}: {e}", path.display()),
    })
}

fn fsync_dir(path: &Path) {
    if let Ok(file) = File::open(path) {
        let _ = file.sync_all();
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn default_data_root() -> Result<PathBuf, ReplaceError> {
    dirs::data_dir()
        .map(|dir| dir.join("oulipoly-agent-runner"))
        .ok_or_else(|| ReplaceError::OperationalError {
            message: "could not determine data directory".to_string(),
        })
}

fn default_config_root() -> PathBuf {
    dirs::config_dir()
        .map(|dir| dir.join("oulipoly-agent-runner"))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn storage_type_from_str(raw: &str) -> StorageType {
    match raw {
        "claude_code" => StorageType::ClaudeCode,
        "codex_session" | "codex" => StorageType::CodexSession,
        _ => StorageType::Other,
    }
}

fn maybe_test_hook(name: &str) {
    if std::env::var(TEST_HOOK_ENV).as_deref() != Ok(name) {
        return;
    }
    eprintln!("import-replace-test-hook:{name}");
    let _ = std::io::stderr().flush();
    match name {
        TEST_SLEEP_AFTER_LOCK_MS => thread::sleep(Duration::from_millis(1500)),
        TEST_BLOCK_AFTER_RENAME => loop {
            thread::sleep(Duration::from_secs(60));
        },
        _ => {}
    }
}

fn move_to_quarantine(path: &Path, quarantine_dir: &Path) {
    let Some(name) = path.file_name() else {
        return;
    };
    let dest = quarantine_dir.join(name);
    let _ = fs::rename(path, dest);
}
