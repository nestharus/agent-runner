use crate::config::{
    FilesystemModelConfigRepository, FilesystemProviderConfigSource,
    FilesystemSessionsConfigSource, ModelConfigRepository, ProviderConfigSource,
    SessionsConfigSource,
};
use crate::process::{OsProcessRunner, ProcessRunner};
use crate::runtime::{DefaultRuntimePaths, RuntimePaths};
use crate::session_export::{self as export, ContentChunk, ExportError, ExportSessionMetadata};
use crate::session_lock::{FilesystemSessionLockProvider, Lease, LockError, SessionLockProvider};
use crate::session_metadata::{
    MetadataError, SessionMetadata, SessionStorageType, TranscriptState, locate_session_metadata,
};
use crate::state::{
    DefaultStateDbOpener, SessionTurnReplacement, SessionTurnReplacementTurn,
    SessionTurnRepository, StateDbOpener,
};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
use uuid::Uuid;

pub use crate::session_export::CanonicalRecord;

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
    InvalidArgument {
        message: String,
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
            ReplaceError::InvalidSessionId { .. } | ReplaceError::InvalidArgument { .. } => 2,
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
            ReplaceError::InvalidArgument { .. } => "invalid-argument",
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
                json!({"error": {"code": self.code(), "message": reason, "line": line}})
            }
            ReplaceError::PreimageMismatch { expected, actual } => {
                json!({"error": {"code": self.code(), "expected": expected, "actual": actual}})
            }
            ReplaceError::InvalidArgument { message } => {
                json!({"error": {"code": self.code(), "message": message}})
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

fn map_lock_error(err: LockError) -> ReplaceError {
    match err {
        LockError::Busy {
            expires_at,
            token_hash,
        } => ReplaceError::SessionBusy {
            token: token_hash
                .as_deref()
                .and_then(|value| value.strip_prefix("sha256:"))
                .or(token_hash.as_deref())
                .unwrap_or_default()
                .to_string(),
            expires_at,
        },
        LockError::TokenInvalid => ReplaceError::OperationalError {
            message: "lock token became invalid during import-replace".to_string(),
        },
        LockError::LockExpired => ReplaceError::OperationalError {
            message: "lock expired during import-replace".to_string(),
        },
        LockError::Operational { message } => ReplaceError::OperationalError { message },
    }
}

fn map_metadata_error(err: MetadataError) -> ReplaceError {
    match err {
        MetadataError::InvalidSessionId { input } => ReplaceError::InvalidSessionId { input },
        MetadataError::SessionNotFound { input } => ReplaceError::SessionNotFound { input },
        MetadataError::AmbiguousSession { input } => ReplaceError::AmbiguousSession { input },
        MetadataError::UnsupportedStorage {
            provider_name,
            reason,
        } => ReplaceError::UnsupportedStorage {
            provider_name,
            reason,
        },
        MetadataError::Operational { message } => ReplaceError::OperationalError { message },
    }
}

pub trait CanonicalToProviderRenderer {
    fn render(&self, records: &[CanonicalRecord]) -> Result<Vec<u8>, ReplaceError>;
}

pub struct ImportReplaceDeps<'a> {
    pub paths: &'a dyn RuntimePaths,
    pub state_opener: &'a dyn StateDbOpener,
    pub model_repo: &'a dyn ModelConfigRepository,
    pub provider_source: &'a dyn ProviderConfigSource,
    pub sessions_source: &'a dyn SessionsConfigSource,
    pub locator_runner: &'a dyn ProcessRunner,
    pub lock_provider: &'a dyn SessionLockProvider,
}

struct ImportReplaceLease<'a> {
    lock_provider: &'a dyn SessionLockProvider,
    lock_dir: PathBuf,
    session_id: String,
    lease: Option<Lease>,
}

impl<'a> ImportReplaceLease<'a> {
    fn commit(mut self) -> Result<(), ReplaceError> {
        if let Some(lease) = self.lease.take() {
            self.lock_provider
                .release(&self.lock_dir, &self.session_id, &lease.token)
                .map_err(map_lock_error)?;
        }
        Ok(())
    }
}

impl Drop for ImportReplaceLease<'_> {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            let _ = self
                .lock_provider
                .release(&self.lock_dir, &self.session_id, &lease.token);
        }
    }
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
                .map(|chunk| {
                    let item_type = if chunk.r#type == "text" {
                        content_type
                    } else {
                        chunk.r#type.as_str()
                    };
                    json!({"type": item_type, "text": chunk.text.as_deref().unwrap_or("")})
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
    #[serde(alias = "postimage_sha256_expected")]
    postimage_sha256: String,
    canonical_records_path: PathBuf,
    db_state_pending: bool,
    expected_turn_count: usize,
}

pub fn run_import_replace(
    session_id: &str,
    input_path_or_stdin: Option<&Path>,
    preimage_sha256: Option<&str>,
) -> Result<ReplaceReceipt, ReplaceError> {
    let paths = DefaultRuntimePaths::new();
    let opener = DefaultStateDbOpener;
    let models = FilesystemModelConfigRepository::new(paths.models_dir());
    let providers = FilesystemProviderConfigSource::new(paths.providers_path());
    let sessions = FilesystemSessionsConfigSource::new(paths.sessions_path());
    let runner = OsProcessRunner;
    let locks = FilesystemSessionLockProvider;
    let deps = ImportReplaceDeps {
        paths: &paths,
        state_opener: &opener,
        model_repo: &models,
        provider_source: &providers,
        sessions_source: &sessions,
        locator_runner: &runner,
        lock_provider: &locks,
    };
    run_import_replace_with_deps(&deps, session_id, input_path_or_stdin, preimage_sha256)
}

pub fn run_import_replace_with_deps(
    deps: &ImportReplaceDeps<'_>,
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
    run_import_replace_bytes_with_deps(deps, session_id, &input, preimage_sha256)
}

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

fn probe_state_schema_compatible(db_path: &Path, session_id: &str) -> Result<(), ReplaceError> {
    if !db_path.exists() {
        return Err(ReplaceError::SessionNotFound {
            input: session_id.to_string(),
        });
    }
    let conn =
        Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| {
            ReplaceError::SchemaIncompatible {
                reason: format!("state db is not readable without mutation: {e}"),
            }
        })?;
    require_columns(
        &conn,
        "session_chain_segments",
        &[
            "id",
            "chain_id",
            "provider_name",
            "session_id",
            "started_at",
            "ended_at",
            "last_turn_id",
        ],
    )?;
    require_columns(
        &conn,
        "session_chains",
        &["chain_id", "created_at", "last_used_at", "model_name"],
    )?;
    require_columns(
        &conn,
        "session_turns",
        &[
            "provider_name",
            "session_id",
            "turn_id",
            "timestamp",
            "role",
            "parent_turn_id",
            "is_sidechain",
            "is_compaction_boundary",
            "source_file",
            "ingested_at",
        ],
    )
}

fn require_columns(conn: &Connection, table: &str, required: &[&str]) -> Result<(), ReplaceError> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| ReplaceError::SchemaIncompatible {
            reason: format!("failed to inspect {table} schema: {e}"),
        })?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| ReplaceError::SchemaIncompatible {
            reason: format!("failed to read {table} schema: {e}"),
        })?;
    let mut columns = Vec::new();
    for row in rows {
        columns.push(row.map_err(|e| ReplaceError::SchemaIncompatible {
            reason: format!("failed to read {table} column: {e}"),
        })?);
    }
    if columns.is_empty() {
        return Err(ReplaceError::SchemaIncompatible {
            reason: format!("missing required table {table}"),
        });
    }
    for column in required {
        if !columns.iter().any(|existing| existing == column) {
            return Err(ReplaceError::SchemaIncompatible {
                reason: format!("missing required column {table}.{column}"),
            });
        }
    }
    Ok(())
}

pub(crate) fn run_import_replace_bytes_with_deps(
    deps: &ImportReplaceDeps<'_>,
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
    for record in &records {
        validate_record_for_render(record)?;
    }
    let canonical_bytes = input_text.as_bytes().to_vec();

    let state_db_path = deps
        .paths
        .state_db_path()
        .map_err(|message| ReplaceError::OperationalError { message })?;
    probe_state_schema_compatible(&state_db_path, session_id)?;
    let journal_root = deps
        .paths
        .replace_journal_dir()
        .map_err(|message| ReplaceError::OperationalError { message })?;
    let staging_dir = journal_root.join("staging");
    let quarantine_dir = journal_root.join("quarantine");
    ensure_journal_dirs(&staging_dir, &quarantine_dir)?;

    let operation_uuid = Uuid::new_v4().to_string();
    let staging_path = staging_dir.join(format!("{operation_uuid}.canonical.jsonl"));
    atomic_write_bytes(&staging_path, &canonical_bytes)?;

    let metadata = match resolve_replace_metadata_with_deps(deps, session_id) {
        Ok(metadata) => metadata,
        Err(err) => {
            let _ = fs::remove_file(&staging_path);
            return Err(err);
        }
    };
    if metadata.storage_type == SessionStorageType::Other {
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
    let postimage_expected = canonical_hash_from_provider_bytes(&metadata, &rendered)?;

    let lock_dir = deps
        .paths
        .lock_dir()
        .map_err(|message| ReplaceError::OperationalError { message })?;
    let lease = match deps.lock_provider.acquire(
        &lock_dir,
        &metadata.session_id,
        &metadata.provider_name,
        Duration::from_secs(300),
    ) {
        Ok(lease) => lease,
        Err(err) => {
            let _ = fs::remove_file(&staging_path);
            return Err(map_lock_error(err));
        }
    };
    let lease = ImportReplaceLease {
        lock_provider: deps.lock_provider,
        lock_dir,
        session_id: metadata.session_id.clone(),
        lease: Some(lease),
    };
    maybe_test_hook(TEST_SLEEP_AFTER_LOCK_MS);

    let preimage = match canonical_hash_from_provider_file(&metadata) {
        Ok(preimage) => preimage,
        Err(err) => {
            let _ = fs::remove_file(&staging_path);
            return Err(err);
        }
    };

    let canonical_records_path =
        journal_root.join(format!("session-{}.canonical.jsonl", metadata.session_id));
    fs::rename(&staging_path, &canonical_records_path).map_err(|e| {
        let _ = fs::remove_file(&staging_path);
        ReplaceError::OperationalError {
            message: format!("failed to publish canonical records: {e}"),
        }
    })?;
    fsync_dir(&journal_root)?;

    let pending_path = journal_root.join(format!("session-{}.pending", metadata.session_id));
    let journal = ReplaceJournal {
        schema_version: 1,
        operation: "import-replace".to_string(),
        operation_uuid: operation_uuid.clone(),
        started_at: Utc::now().to_rfc3339(),
        session_id: metadata.session_id.clone(),
        chain_id: metadata.chain_id.clone(),
        active_segment_id: metadata.active_segment_id,
        provider_name: metadata.provider_name.clone(),
        storage_type: storage_type_as_str(metadata.storage_type).to_string(),
        jsonl_path: metadata.jsonl_path.clone(),
        preimage_sha256: Some(preimage.clone()),
        postimage_sha256: postimage_expected.clone(),
        canonical_records_path: canonical_records_path.clone(),
        db_state_pending: true,
        expected_turn_count: records.len(),
    };
    atomic_write_json(&pending_path, &journal)?;
    if let Some(expected) = preimage_sha256
        && expected != preimage
    {
        return Err(ReplaceError::PreimageMismatch {
            expected: expected.to_string(),
            actual: preimage,
        });
    }

    let tmp_path = metadata
        .jsonl_path
        .with_extension(format!("jsonl.tmp-import-replace-{operation_uuid}"));
    write_new_file_synced(&tmp_path, &rendered)?;
    if let Err(e) = fs::rename(&tmp_path, &metadata.jsonl_path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(ReplaceError::OperationalError {
            message: format!("failed to replace transcript: {e}"),
        });
    }
    if let Some(parent) = metadata.jsonl_path.parent() {
        fsync_dir(parent)?;
    }

    maybe_test_hook(TEST_BLOCK_AFTER_RENAME);
    if std::env::var(TEST_HOOK_ENV).as_deref() == Ok(TEST_FAIL_POSTIMAGE_VERIFY) {
        return Err(ReplaceError::OperationalError {
            message: "forced postimage verification failure".to_string(),
        });
    }

    let actual_postimage = canonical_hash_from_provider_file(&metadata)?;
    if actual_postimage != postimage_expected {
        return Err(ReplaceError::OperationalError {
            message: "postimage verification hash mismatch".to_string(),
        });
    }
    let fresh = canonical_records_from_provider_file(&metadata)?;
    if !canonical_semantics_equal(&records, &fresh) {
        return Err(ReplaceError::OperationalError {
            message: "fresh export verification mismatch".to_string(),
        });
    }

    let state =
        deps.state_opener
            .open(&state_db_path)
            .map_err(|e| ReplaceError::OperationalError {
                message: format!("failed to open state db: {e}"),
            })?;
    let turn_repo: &dyn SessionTurnRepository = &state;
    replace_db_turns_with_repo(turn_repo, &metadata, &records)?;

    fs::remove_file(&pending_path).ok();
    fs::remove_file(&canonical_records_path).ok();
    fsync_dir(&journal_root)?;
    lease.commit()?;

    Ok(ReplaceReceipt {
        session_id: metadata.session_id,
        provider_name: metadata.provider_name,
        storage_type: storage_type_as_str(metadata.storage_type).to_string(),
        operation: "import-replace".to_string(),
        preimage_sha256: preimage,
        postimage_sha256: actual_postimage,
        jsonl_path: metadata.jsonl_path,
        state_updated: true,
        committed_at: Utc::now().to_rfc3339(),
    })
}

pub fn recover_pending_replaces() -> Result<(), ReplaceError> {
    let paths = DefaultRuntimePaths::new();
    let opener = DefaultStateDbOpener;
    let models = FilesystemModelConfigRepository::new(paths.models_dir());
    let providers = FilesystemProviderConfigSource::new(paths.providers_path());
    let sessions = FilesystemSessionsConfigSource::new(paths.sessions_path());
    let runner = OsProcessRunner;
    let locks = FilesystemSessionLockProvider;
    let deps = ImportReplaceDeps {
        paths: &paths,
        state_opener: &opener,
        model_repo: &models,
        provider_source: &providers,
        sessions_source: &sessions,
        locator_runner: &runner,
        lock_provider: &locks,
    };
    recover_pending_replaces_with_deps(&deps)
}

pub fn recover_pending_replaces_with_deps(
    deps: &ImportReplaceDeps<'_>,
) -> Result<(), ReplaceError> {
    let journal_root = deps
        .paths
        .replace_journal_dir()
        .map_err(|message| ReplaceError::OperationalError { message })?;
    let state_db_path = deps
        .paths
        .state_db_path()
        .map_err(|message| ReplaceError::OperationalError { message })?;
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
        let metadata = SessionMetadata {
            session_id: journal.session_id.clone(),
            chain_id: journal.chain_id.clone(),
            active_segment_id: journal.active_segment_id,
            provider_name: journal.provider_name.clone(),
            storage_type,
            jsonl_path: journal.jsonl_path.clone(),
            workspace_root: PathBuf::new(),
            transcript_state: TranscriptState::Available,
            mutable: true,
        };
        let current_hash = canonical_hash_from_provider_file(&metadata);
        match (journal.preimage_sha256.as_deref(), current_hash) {
            (_, Ok(hash)) if hash == journal.postimage_sha256 => {
                let Ok(canonical) = fs::read_to_string(&journal.canonical_records_path) else {
                    move_to_quarantine(&path, &quarantine_dir);
                    continue;
                };
                let Ok(records) = parse_canonical_jsonl(&canonical) else {
                    move_to_quarantine(&path, &quarantine_dir);
                    continue;
                };
                let state = deps.state_opener.open(&state_db_path).map_err(|e| {
                    ReplaceError::OperationalError {
                        message: format!("failed to open state db during recovery: {e}"),
                    }
                })?;
                let turn_repo: &dyn SessionTurnRepository = &state;
                replace_db_turns_with_repo(turn_repo, &metadata, &records)?;
                fs::remove_file(&path).ok();
                fs::remove_file(&journal.canonical_records_path).ok();
                fsync_dir(&journal_root)?;
            }
            (Some(preimage), Ok(hash)) if hash == preimage => {
                fs::remove_file(&path).ok();
                fs::remove_file(&journal.canonical_records_path).ok();
                fsync_dir(&journal_root)?;
            }
            (None, _) => {
                fs::remove_file(&path).ok();
                fs::remove_file(&journal.canonical_records_path).ok();
                fsync_dir(&journal_root)?;
            }
            _ => {
                move_to_quarantine(&path, &quarantine_dir);
            }
        }
    }
    cleanup_orphan_canonical_records(deps, &journal_root, &quarantine_dir)?;
    Ok(())
}

fn cleanup_orphan_canonical_records(
    deps: &ImportReplaceDeps<'_>,
    journal_root: &Path,
    quarantine_dir: &Path,
) -> Result<(), ReplaceError> {
    for entry in fs::read_dir(journal_root).map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to scan replace journal for orphans: {e}"),
    })? {
        let path = entry
            .map_err(|e| ReplaceError::OperationalError {
                message: format!("failed to read replace journal orphan entry: {e}"),
            })?
            .path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(session_part) = name.strip_suffix(".canonical.jsonl") else {
            continue;
        };
        if !session_part.starts_with("session-") {
            continue;
        }
        let session_id = session_part.trim_start_matches("session-");
        let lock_dir = deps
            .paths
            .lock_dir()
            .map_err(|message| ReplaceError::OperationalError { message })?;
        if deps
            .lock_provider
            .any_active_for_session(&lock_dir, session_id)
            .map_err(map_lock_error)?
        {
            continue;
        }
        let pending = journal_root.join(format!("{session_part}.pending"));
        let quarantined_pending = quarantine_dir.join(format!("{session_part}.pending"));
        if !pending.exists() && !quarantined_pending.exists() {
            fs::remove_file(&path).map_err(|e| ReplaceError::OperationalError {
                message: format!("failed to remove orphan canonical records: {e}"),
            })?;
            fsync_dir(journal_root)?;
        }
    }
    Ok(())
}

pub fn export_session_canonical(session_id: &str) -> Result<Vec<u8>, ReplaceError> {
    Uuid::try_parse(session_id).map_err(|_| ReplaceError::InvalidSessionId {
        input: session_id.to_string(),
    })?;
    let paths = DefaultRuntimePaths::new();
    let opener = DefaultStateDbOpener;
    let models = FilesystemModelConfigRepository::new(paths.models_dir());
    let providers = FilesystemProviderConfigSource::new(paths.providers_path());
    let sessions = FilesystemSessionsConfigSource::new(paths.sessions_path());
    let runner = OsProcessRunner;
    let locks = FilesystemSessionLockProvider;
    let deps = ImportReplaceDeps {
        paths: &paths,
        state_opener: &opener,
        model_repo: &models,
        provider_source: &providers,
        sessions_source: &sessions,
        locator_runner: &runner,
        lock_provider: &locks,
    };
    let metadata = resolve_replace_metadata_with_deps(&deps, session_id)?;
    let records = canonical_records_from_provider_file(&metadata)?;
    canonical_jsonl_bytes(&records)
}

fn parse_canonical_jsonl(input: &str) -> Result<Vec<CanonicalRecord>, ReplaceError> {
    let mut records = Vec::new();
    for (idx, line) in input.lines().enumerate() {
        let line_no = idx as u64 + 1;
        if line.trim().is_empty() {
            return Err(ReplaceError::InvalidInputTranscript {
                reason: "blank line in canonical JSONL".to_string(),
                line: Some(line_no),
            });
        }
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
            || (!record.unsupported_record && !matches!(record.role.as_str(), "user" | "assistant"))
        {
            return Err(ReplaceError::InvalidInputTranscript {
                reason: "canonical record is missing required fields".to_string(),
                line: Some(line_no),
            });
        }
        DateTime::parse_from_rfc3339(&record.timestamp).map_err(|e| {
            ReplaceError::InvalidInputTranscript {
                reason: format!("invalid canonical record timestamp: {e}"),
                line: Some(line_no),
            }
        })?;
        records.push(record);
    }
    if records.is_empty() {
        return Err(ReplaceError::InvalidInputTranscript {
            reason: "empty canonical transcript".to_string(),
            line: None,
        });
    }
    if records.iter().all(|record| record.unsupported_record) {
        return Err(ReplaceError::InvalidInputTranscript {
            reason: "unsupported record class: canonical transcript has no replaceable records"
                .to_string(),
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
    for chunk in &record.content {
        if chunk.text.is_none() {
            return Err(ReplaceError::InvalidInputTranscript {
                reason: format!(
                    "content chunk type {} cannot be rendered losslessly without text",
                    chunk.r#type
                ),
                line: None,
            });
        }
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
        .map(|chunk| json!({"type": chunk.r#type, "text": chunk.text.as_deref().unwrap_or("")}))
        .collect())
}

fn render_for_storage(
    storage_type: &SessionStorageType,
    records: &[CanonicalRecord],
) -> Result<Vec<u8>, ReplaceError> {
    match storage_type {
        SessionStorageType::ClaudeCode => ClaudeCodeRenderer.render(records),
        SessionStorageType::CodexSession => CodexSessionRenderer.render(records),
        SessionStorageType::Other => Err(ReplaceError::UnsupportedStorage {
            provider_name: records
                .first()
                .map(|record| record.provider_name.clone())
                .unwrap_or_default(),
            reason: "unsupported storage".to_string(),
        }),
    }
}

fn resolve_replace_metadata_with_deps(
    deps: &ImportReplaceDeps<'_>,
    session_id: &str,
) -> Result<SessionMetadata, ReplaceError> {
    let state_db_path = deps
        .paths
        .state_db_path()
        .map_err(|message| ReplaceError::OperationalError { message })?;
    let state = deps
        .state_opener
        .open(&state_db_path)
        .map_err(|message| ReplaceError::OperationalError { message })?;
    locate_session_metadata(
        &state,
        deps.model_repo,
        deps.provider_source,
        deps.sessions_source,
        deps.locator_runner,
        session_id,
    )
    .map_err(map_metadata_error)
}

fn replace_db_turns_with_repo(
    turn_repo: &dyn SessionTurnRepository,
    metadata: &SessionMetadata,
    records: &[CanonicalRecord],
) -> Result<(), ReplaceError> {
    let replacement = SessionTurnReplacement {
        provider_name: metadata.provider_name.clone(),
        session_id: metadata.session_id.clone(),
        chain_id: metadata.chain_id.clone(),
        active_segment_id: metadata.active_segment_id,
        source_file: metadata.jsonl_path.clone(),
        turns: records
            .iter()
            .map(|record| SessionTurnReplacementTurn {
                turn_id: record.turn_id.clone(),
                timestamp: record.timestamp.clone(),
                role: record.role.clone(),
            })
            .collect(),
    };
    turn_repo
        .replace_session_turns(&replacement)
        .map_err(|message| ReplaceError::OperationalError { message })
}

fn canonical_records_from_provider_file(
    metadata: &SessionMetadata,
) -> Result<Vec<CanonicalRecord>, ReplaceError> {
    let bytes = fs::read(&metadata.jsonl_path).map_err(|e| ReplaceError::OperationalError {
        message: format!(
            "failed to read transcript {}: {e}",
            metadata.jsonl_path.display()
        ),
    })?;
    canonical_records_from_provider_bytes(metadata, &bytes)
}

fn canonical_hash_from_provider_file(metadata: &SessionMetadata) -> Result<String, ReplaceError> {
    let records = canonical_records_from_provider_file(metadata)?;
    Ok(sha256_hex(&canonical_jsonl_bytes(&records)?))
}

fn canonical_hash_from_provider_bytes(
    metadata: &SessionMetadata,
    bytes: &[u8],
) -> Result<String, ReplaceError> {
    let records = canonical_records_from_provider_bytes(metadata, bytes)?;
    Ok(sha256_hex(&canonical_jsonl_bytes(&records)?))
}

fn canonical_records_from_provider_bytes(
    metadata: &SessionMetadata,
    bytes: &[u8],
) -> Result<Vec<CanonicalRecord>, ReplaceError> {
    let export_metadata = export_metadata_for(metadata);
    export::read_canonical_transcript_from_bytes(&export_metadata, bytes).map_err(map_export_error)
}

fn export_metadata_for(metadata: &SessionMetadata) -> ExportSessionMetadata {
    ExportSessionMetadata {
        session_id: metadata.session_id.clone(),
        chain_id: metadata.chain_id.clone(),
        provider_name: metadata.provider_name.clone(),
        storage_type: storage_type_to_export(metadata.storage_type),
        jsonl_path: metadata.jsonl_path.clone(),
    }
}

fn map_export_error(err: ExportError) -> ReplaceError {
    match err {
        ExportError::InvalidSessionId { input } => ReplaceError::InvalidSessionId { input },
        ExportError::SessionNotFound { input } => ReplaceError::SessionNotFound { input },
        ExportError::AmbiguousSession { input } => ReplaceError::AmbiguousSession { input },
        ExportError::UnsupportedStorage {
            provider_name,
            reason,
        } => ReplaceError::UnsupportedStorage {
            provider_name,
            reason,
        },
        ExportError::MalformedTranscript { line, reason, .. } => {
            ReplaceError::InvalidInputTranscript {
                reason,
                line: if line == 0 { None } else { Some(line) },
            }
        }
        ExportError::Operational { message } => ReplaceError::OperationalError { message },
    }
}

fn canonical_jsonl_bytes(records: &[CanonicalRecord]) -> Result<Vec<u8>, ReplaceError> {
    export::canonical_jsonl_bytes(records).map_err(map_export_error)
}

fn canonical_semantics_equal(left: &[CanonicalRecord], right: &[CanonicalRecord]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(a, b)| {
            a.session_id == b.session_id
                && a.provider_name == b.provider_name
                && a.turn_id == b.turn_id
                && a.role == b.role
                && a.timestamp == b.timestamp
                && content_chunks_equal(&a.content, &b.content)
                && a.unsupported_record == b.unsupported_record
        })
}

fn content_chunks_equal(left: &[ContentChunk], right: &[ContentChunk]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(a, b)| a.r#type == b.r#type && a.text == b.text)
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
        fsync_dir(parent)?;
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

fn fsync_dir(path: &Path) -> Result<(), ReplaceError> {
    let file = File::open(path).map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to open directory {} for fsync: {e}", path.display()),
    })?;
    file.sync_all().map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to fsync directory {}: {e}", path.display()),
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
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

fn storage_type_from_str(raw: &str) -> SessionStorageType {
    match raw {
        "claude_code" => SessionStorageType::ClaudeCode,
        "codex_session" | "codex" => SessionStorageType::CodexSession,
        _ => SessionStorageType::Other,
    }
}

fn storage_type_as_str(storage_type: SessionStorageType) -> &'static str {
    match storage_type {
        SessionStorageType::ClaudeCode => "claude_code",
        SessionStorageType::CodexSession => "codex_session",
        SessionStorageType::Other => "other",
    }
}

fn storage_type_to_export(
    storage_type: SessionStorageType,
) -> crate::session_export::SessionStorageType {
    match storage_type {
        SessionStorageType::ClaudeCode => crate::session_export::SessionStorageType::ClaudeCode,
        SessionStorageType::CodexSession => crate::session_export::SessionStorageType::CodexSession,
        SessionStorageType::Other => crate::session_export::SessionStorageType::Other,
    }
}

fn default_models_dir() -> PathBuf {
    dirs::config_dir()
        .map(|dir| dir.join("oulipoly-agent-runner").join("models"))
        .unwrap_or_else(|| PathBuf::from("models"))
}

fn maybe_test_hook(name: &str) {
    if std::env::var(TEST_HOOK_ENV).as_deref() != Ok(name) {
        return;
    }
    eprintln!("import-replace-test-hook:{name}");
    let _ = std::io::stderr().flush();
    match name {
        TEST_SLEEP_AFTER_LOCK_MS => {
            let millis = std::env::var("OULIPOLY_IMPORT_REPLACE_TEST_SLEEP_MS")
                .ok()
                .and_then(|raw| raw.parse::<u64>().ok())
                .unwrap_or(1500);
            thread::sleep(Duration::from_millis(millis));
        }
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
