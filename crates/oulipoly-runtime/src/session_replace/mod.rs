//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/session_replace/mod.rs
//!     role: intrinsic-surface
//!     Domain: session import-replace host lifecycle
//!     Owns:
//!       - canonical replacement input parsing and renderability validation
//!       - built-in canonical-to-native transcript rendering
//!       - import-replace journal, lock, recovery, hash verification, and SQLite mutation
//!       - external-provider write-ahead preimage snapshots and recovery reconcile
//!       - external-provider postimage artifact hash accessors and validators
//! ```

use crate::session_export::{self as export, ContentChunk, ExportError, ExportSessionMetadata};
use crate::session_lock::{Lease, LockError, SessionLock};
use crate::session_metadata::{
    MetadataError, SessionMetadata, SessionStorageType, TranscriptState, locate_session_metadata,
};
use chrono::{DateTime, Utc};
use oulipoly_config::{ProvidersConfig, SessionsConfig, load_models};
use oulipoly_state::{
    SessionTurnReplacement, SessionTurnRestoreRow, SessionTurnsReplacement, SessionTurnsRestore,
    StateDb, StateReadConnection,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;
use uuid::Uuid;

pub use crate::session_export::CanonicalRecord;

/// Source for replacement transcript data in session import-replace operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplaceSource {
    /// Read replacement data from the specified file path.
    File(PathBuf),
    /// Read replacement data from standard input.
    Stdin,
}

const TEST_HOOK_ENV: &str = "OULIPOLY_IMPORT_REPLACE_TEST_HOOK";
const TEST_SLEEP_AFTER_LOCK_MS: &str = "sleep-after-lock-ms";
const TEST_BLOCK_AFTER_RENAME: &str = "block-after-transcript-rename-before-db-commit";
const TEST_FAIL_POSTIMAGE_VERIFY: &str = "fail-postimage-verification";

static RESOLVE_REPLACE_METADATA_CALLS: AtomicUsize = AtomicUsize::new(0);
static CANONICAL_RECORDS_FROM_PROVIDER_FILE_CALLS: AtomicUsize = AtomicUsize::new(0);
static RENDER_FOR_STORAGE_CALLS: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForbiddenHelperCallCounts {
    pub resolve_replace_metadata: usize,
    pub canonical_records_from_provider_file: usize,
    pub render_for_storage: usize,
}

pub fn reset_forbidden_helper_recorder() {
    RESOLVE_REPLACE_METADATA_CALLS.store(0, Ordering::SeqCst);
    CANONICAL_RECORDS_FROM_PROVIDER_FILE_CALLS.store(0, Ordering::SeqCst);
    RENDER_FOR_STORAGE_CALLS.store(0, Ordering::SeqCst);
}

pub fn forbidden_helper_call_counts() -> ForbiddenHelperCallCounts {
    ForbiddenHelperCallCounts {
        resolve_replace_metadata: RESOLVE_REPLACE_METADATA_CALLS.load(Ordering::SeqCst),
        canonical_records_from_provider_file: CANONICAL_RECORDS_FROM_PROVIDER_FILE_CALLS
            .load(Ordering::SeqCst),
        render_for_storage: RENDER_FOR_STORAGE_CALLS.load(Ordering::SeqCst),
    }
}

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
pub(crate) struct ProviderReplaceDbTarget {
    pub(crate) provider_name: String,
    pub(crate) session_id: String,
    pub(crate) chain_id: String,
    pub(crate) active_segment_id: i64,
    pub(crate) source_file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ProviderReplaceDbPreimage {
    pub(crate) session_turns: Value,
    pub(crate) last_turn_id: Option<String>,
    pub(crate) last_used_at: String,
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

struct ImportReplaceLease<'a> {
    lock: &'a SessionLock,
    session_id: String,
    lease: Option<Lease>,
}

impl<'a> ImportReplaceLease<'a> {
    fn commit(mut self) -> Result<(), ReplaceError> {
        if let Some(lease) = self.lease.take() {
            self.lock
                .release(&self.session_id, &lease.token)
                .map_err(map_lock_error)?;
        }
        Ok(())
    }
}

impl Drop for ImportReplaceLease<'_> {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            let _ = self.lock.release(&self.session_id, &lease.token);
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
    #[serde(default, alias = "postimage_sha256_expected")]
    postimage_sha256: Option<String>,
    canonical_records_path: PathBuf,
    #[serde(default)]
    preimage_snapshot_path: Option<PathBuf>,
    db_state_pending: bool,
    expected_turn_count: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct ReplaceJournalHeader {
    schema_version: u32,
    operation: String,
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

fn probe_state_schema_compatible(data_root: &Path, session_id: &str) -> Result<(), ReplaceError> {
    let db_path = data_root.join("state.db");
    if !db_path.exists() {
        return Err(ReplaceError::SessionNotFound {
            input: session_id.to_string(),
        });
    }
    StateDb::open(&db_path)
        .map(|_| ())
        .map_err(|e| ReplaceError::SchemaIncompatible {
            reason: format!("state db schema is incompatible: {e}"),
        })
}

pub(crate) fn run_import_replace_bytes(
    session_id: &str,
    input: &[u8],
    preimage_sha256: Option<&str>,
) -> Result<ReplaceReceipt, ReplaceError> {
    run_import_replace_with_postimage(
        session_id,
        input,
        preimage_sha256,
        ReplacePostimageAuthority::BuiltInRender,
    )
}

enum ReplacePostimageAuthority {
    BuiltInRender,
}

struct ReplaceInput {
    records: Vec<CanonicalRecord>,
    canonical_bytes: Vec<u8>,
}

struct ReplaceJournalWorkspace {
    journal_root: PathBuf,
    staging_path: PathBuf,
    operation_uuid: String,
}

struct PublishedReplaceJournal {
    pending_path: PathBuf,
    canonical_records_path: PathBuf,
    preimage_snapshot_path: Option<PathBuf>,
}

struct ReplacePostimagePlan {
    rendered: Option<Vec<u8>>,
    expected_sha256: String,
}

fn run_import_replace_with_postimage(
    session_id: &str,
    input: &[u8],
    preimage_sha256: Option<&str>,
    postimage_authority: ReplacePostimageAuthority,
) -> Result<ReplaceReceipt, ReplaceError> {
    let replace_input = prepare_replace_input(session_id, input)?;
    let data_root = prepare_replace_data_root(session_id)?;
    let workspace = stage_replace_journal_input(&data_root, &replace_input.canonical_bytes)?;
    let metadata =
        resolve_replace_metadata_for_staged_input(session_id, &replace_input.records, &workspace)?;
    let postimage_plan =
        map_replace_postimage_authority(&postimage_authority, &metadata, &replace_input.records)
            .inspect_err(|_| remove_staged_replace_input(&workspace))?;
    let lock = initialize_replace_session_lock(&data_root)?;
    let lease = acquire_import_replace_lease(&lock, &metadata, &workspace)?;
    maybe_test_hook(TEST_SLEEP_AFTER_LOCK_MS);

    let preimage = resolve_replace_preimage(&postimage_authority, &metadata, &workspace)?;
    let published = publish_replace_journal(
        &workspace,
        &metadata,
        replace_input.records.len(),
        &preimage,
        &postimage_plan.expected_sha256,
    )?;
    validate_requested_preimage(preimage_sha256, &preimage)?;
    orchestrate_replace_transcript_write(&metadata, &postimage_plan, &workspace.operation_uuid)?;

    maybe_test_hook(TEST_BLOCK_AFTER_RENAME);
    let actual_postimage = verify_replace_postimage(
        &metadata,
        &replace_input.records,
        &postimage_plan.expected_sha256,
    )?;
    apply_replace_sqlite(&metadata, &replace_input.records)?;
    cleanup_replace_journal_publication(&workspace, &published)?;
    lease.commit()?;

    Ok(format_replace_receipt(
        &metadata,
        preimage,
        actual_postimage,
    ))
}

fn prepare_replace_input(session_id: &str, input: &[u8]) -> Result<ReplaceInput, ReplaceError> {
    Uuid::try_parse(session_id).map_err(|_| ReplaceError::InvalidSessionId {
        input: session_id.to_string(),
    })?;
    Ok(ReplaceInput {
        records: parse_and_validate_canonical_input(input)?,
        canonical_bytes: input.to_vec(),
    })
}

fn prepare_replace_data_root(session_id: &str) -> Result<PathBuf, ReplaceError> {
    let data_root = default_data_root()?;
    probe_state_schema_compatible(&data_root, session_id)?;
    Ok(data_root)
}

fn stage_replace_journal_input(
    data_root: &Path,
    canonical_bytes: &[u8],
) -> Result<ReplaceJournalWorkspace, ReplaceError> {
    let journal_root = data_root.join("replace_journal");
    let staging_dir = journal_root.join("staging");
    let quarantine_dir = journal_root.join("quarantine");
    ensure_journal_dirs(&staging_dir, &quarantine_dir)?;

    let operation_uuid = Uuid::new_v4().to_string();
    let staging_path = staging_dir.join(format!("{operation_uuid}.canonical.jsonl"));
    atomic_write_bytes(&staging_path, canonical_bytes)?;

    Ok(ReplaceJournalWorkspace {
        journal_root,
        staging_path,
        operation_uuid,
    })
}

fn remove_staged_replace_input(workspace: &ReplaceJournalWorkspace) {
    let _ = fs::remove_file(&workspace.staging_path);
}

fn resolve_replace_metadata_for_staged_input(
    session_id: &str,
    records: &[CanonicalRecord],
    workspace: &ReplaceJournalWorkspace,
) -> Result<SessionMetadata, ReplaceError> {
    let metadata = resolve_replace_metadata(session_id).inspect_err(|_| {
        remove_staged_replace_input(workspace);
    })?;
    validate_replace_metadata_for_input(&metadata, records).inspect_err(|_| {
        remove_staged_replace_input(workspace);
    })?;
    Ok(metadata)
}

fn validate_replace_metadata_for_input(
    metadata: &SessionMetadata,
    records: &[CanonicalRecord],
) -> Result<(), ReplaceError> {
    if metadata.storage_type == SessionStorageType::Other {
        return Err(ReplaceError::UnsupportedStorage {
            provider_name: metadata.provider_name.clone(),
            reason: "provider has no supported session_storage".to_string(),
        });
    }
    validate_records_match_metadata(records, metadata)
}

fn map_replace_postimage_authority(
    authority: &ReplacePostimageAuthority,
    metadata: &SessionMetadata,
    records: &[CanonicalRecord],
) -> Result<ReplacePostimagePlan, ReplaceError> {
    match authority {
        ReplacePostimageAuthority::BuiltInRender => {
            map_builtin_render_postimage_authority(metadata, records)
        }
    }
}

fn map_builtin_render_postimage_authority(
    metadata: &SessionMetadata,
    records: &[CanonicalRecord],
) -> Result<ReplacePostimagePlan, ReplaceError> {
    let rendered = render_for_storage(&metadata.storage_type, records)?;
    let expected_sha256 = canonical_hash_from_provider_bytes(metadata, &rendered)?;
    Ok(ReplacePostimagePlan {
        rendered: Some(rendered),
        expected_sha256,
    })
}

fn initialize_replace_session_lock(data_root: &Path) -> Result<SessionLock, ReplaceError> {
    let lock_dir = data_root.join("locks");
    SessionLock::new(&lock_dir).map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to initialize session lock: {e}"),
    })
}

fn acquire_import_replace_lease<'a>(
    lock: &'a SessionLock,
    metadata: &SessionMetadata,
    workspace: &ReplaceJournalWorkspace,
) -> Result<ImportReplaceLease<'a>, ReplaceError> {
    let lease = match lock.acquire(
        &metadata.session_id,
        &metadata.provider_name,
        Duration::from_secs(300),
    ) {
        Ok(lease) => lease,
        Err(err) => {
            remove_staged_replace_input(workspace);
            return Err(map_lock_error(err));
        }
    };
    Ok(ImportReplaceLease {
        lock,
        session_id: metadata.session_id.clone(),
        lease: Some(lease),
    })
}

fn resolve_replace_preimage(
    authority: &ReplacePostimageAuthority,
    metadata: &SessionMetadata,
    workspace: &ReplaceJournalWorkspace,
) -> Result<String, ReplaceError> {
    match authority {
        ReplacePostimageAuthority::BuiltInRender => canonical_hash_from_provider_file(metadata)
            .inspect_err(|_| {
                remove_staged_replace_input(workspace);
            }),
    }
}

fn publish_replace_journal(
    workspace: &ReplaceJournalWorkspace,
    metadata: &SessionMetadata,
    expected_turn_count: usize,
    preimage_sha256: &str,
    postimage_sha256: &str,
) -> Result<PublishedReplaceJournal, ReplaceError> {
    let canonical_records_path = workspace
        .journal_root
        .join(format!("session-{}.canonical.jsonl", metadata.session_id));
    fs::rename(&workspace.staging_path, &canonical_records_path).map_err(|e| {
        remove_staged_replace_input(workspace);
        ReplaceError::OperationalError {
            message: format!("failed to publish canonical records: {e}"),
        }
    })?;
    fsync_dir(&workspace.journal_root)?;

    let pending_path = workspace
        .journal_root
        .join(format!("session-{}.pending", metadata.session_id));
    let journal = ReplaceJournal {
        schema_version: 1,
        operation: "import-replace".to_string(),
        operation_uuid: workspace.operation_uuid.clone(),
        started_at: Utc::now().to_rfc3339(),
        session_id: metadata.session_id.clone(),
        chain_id: metadata.chain_id.clone(),
        active_segment_id: metadata.active_segment_id,
        provider_name: metadata.provider_name.clone(),
        storage_type: storage_type_as_str(metadata.storage_type).to_string(),
        jsonl_path: metadata.jsonl_path.clone(),
        preimage_sha256: Some(preimage_sha256.to_string()),
        postimage_sha256: Some(postimage_sha256.to_string()),
        canonical_records_path: canonical_records_path.clone(),
        preimage_snapshot_path: None,
        db_state_pending: true,
        expected_turn_count,
    };
    atomic_write_json(&pending_path, &journal)?;
    Ok(PublishedReplaceJournal {
        pending_path,
        canonical_records_path,
        preimage_snapshot_path: None,
    })
}

fn validate_requested_preimage(
    expected_preimage: Option<&str>,
    actual_preimage: &str,
) -> Result<(), ReplaceError> {
    if let Some(expected) = expected_preimage
        && expected != actual_preimage
    {
        return Err(ReplaceError::PreimageMismatch {
            expected: expected.to_string(),
            actual: actual_preimage.to_string(),
        });
    }
    Ok(())
}

fn orchestrate_replace_transcript_write(
    metadata: &SessionMetadata,
    postimage_plan: &ReplacePostimagePlan,
    operation_uuid: &str,
) -> Result<(), ReplaceError> {
    let Some(rendered) = postimage_plan.rendered.as_deref() else {
        return Ok(());
    };
    let tmp_path = metadata
        .jsonl_path
        .with_extension(format!("jsonl.tmp-import-replace-{operation_uuid}"));
    write_new_file_synced(&tmp_path, rendered)?;
    if let Err(e) = fs::rename(&tmp_path, &metadata.jsonl_path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(ReplaceError::OperationalError {
            message: format!("failed to replace transcript: {e}"),
        });
    }
    if let Some(parent) = metadata.jsonl_path.parent() {
        fsync_dir(parent)?;
    }
    Ok(())
}

fn verify_replace_postimage(
    metadata: &SessionMetadata,
    records: &[CanonicalRecord],
    expected_postimage: &str,
) -> Result<String, ReplaceError> {
    if std::env::var(TEST_HOOK_ENV).as_deref() == Ok(TEST_FAIL_POSTIMAGE_VERIFY) {
        return Err(ReplaceError::OperationalError {
            message: "forced postimage verification failure".to_string(),
        });
    }

    let actual_postimage = canonical_hash_from_provider_file(metadata)?;
    if actual_postimage != expected_postimage {
        return Err(ReplaceError::OperationalError {
            message: "postimage verification hash mismatch".to_string(),
        });
    }
    let fresh = canonical_records_from_provider_file(metadata)?;
    if !canonical_semantics_equal(records, &fresh) {
        return Err(ReplaceError::OperationalError {
            message: "fresh export verification mismatch".to_string(),
        });
    }
    Ok(actual_postimage)
}

fn apply_replace_sqlite(
    metadata: &SessionMetadata,
    records: &[CanonicalRecord],
) -> Result<(), ReplaceError> {
    let mut state = StateDb::open_default().map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to open state db: {e}"),
    })?;
    let input = replacement_from_metadata(metadata, records)?;
    state
        .replace_session_turns(&input)
        .map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to update state db: {e}"),
        })
}

pub(crate) fn apply_provider_owned_replace_sqlite(
    target: &ProviderReplaceDbTarget,
    records: &[CanonicalRecord],
) -> Result<(), ReplaceError> {
    let mut state = StateDb::open_default().map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to open state db: {e}"),
    })?;
    let input = replacement_for_target(target, records)?;
    state
        .replace_session_turns(&input)
        .map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to update state db: {e}"),
        })
}

pub(crate) fn restore_provider_owned_db_preimage(
    target: &ProviderReplaceDbTarget,
    preimage: &ProviderReplaceDbPreimage,
) -> Result<(), ReplaceError> {
    let mut state = StateDb::open_default().map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to open state db: {e}"),
    })?;
    let input = restoration_for_target(target, preimage)?;
    state
        .restore_session_turns(&input)
        .map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to restore state db: {e}"),
        })
}

pub(crate) fn strict_provider_replace_db_identity(
    provider_name: &str,
    session_id: &str,
    source_file: String,
) -> Result<ProviderReplaceDbTarget, ReplaceError> {
    let data_root = default_data_root()?;
    let db_path = data_root.join("state.db");
    let state = StateDb::open(&db_path).map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to open state db: {e}"),
    })?;
    let conn = state.connection();
    let mut stmt = conn
        .prepare(
            "SELECT chain_id, id
             FROM session_chain_segments
             WHERE provider_name = ?1 AND session_id = ?2 AND ended_at IS NULL
             ORDER BY id",
        )
        .map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to prepare provider DB identity lookup: {e}"),
        })?;
    let rows = stmt
        .query_map(params![provider_name, session_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to query provider DB identity: {e}"),
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to read provider DB identity: {e}"),
        })?;
    match rows.as_slice() {
        [] => Err(ReplaceError::OperationalError {
            message: "provider_db_identity_missing".to_string(),
        }),
        [(chain_id, active_segment_id)] => Ok(ProviderReplaceDbTarget {
            provider_name: provider_name.to_string(),
            session_id: session_id.to_string(),
            chain_id: chain_id.clone(),
            active_segment_id: *active_segment_id,
            source_file,
        }),
        _ => Err(ReplaceError::OperationalError {
            message: "provider_db_identity_ambiguous".to_string(),
        }),
    }
}

pub(crate) fn provider_replace_db_preimage(
    target: &ProviderReplaceDbTarget,
) -> Result<ProviderReplaceDbPreimage, ReplaceError> {
    let data_root = default_data_root()?;
    let db_path = data_root.join("state.db");
    let state = StateDb::open(&db_path).map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to open state db: {e}"),
    })?;
    let conn = state.connection();
    db_preimage_from_conn(&conn, target)
}

fn cleanup_replace_journal_publication(
    workspace: &ReplaceJournalWorkspace,
    published: &PublishedReplaceJournal,
) -> Result<(), ReplaceError> {
    fs::remove_file(&published.pending_path).ok();
    fs::remove_file(&published.canonical_records_path).ok();
    if let Some(path) = &published.preimage_snapshot_path {
        fs::remove_file(path).ok();
    }
    fsync_dir(&workspace.journal_root)
}

fn format_replace_receipt(
    metadata: &SessionMetadata,
    preimage_sha256: String,
    postimage_sha256: String,
) -> ReplaceReceipt {
    ReplaceReceipt {
        session_id: metadata.session_id.clone(),
        provider_name: metadata.provider_name.clone(),
        storage_type: storage_type_as_str(metadata.storage_type).to_string(),
        operation: "import-replace".to_string(),
        preimage_sha256,
        postimage_sha256,
        jsonl_path: metadata.jsonl_path.clone(),
        state_updated: true,
        committed_at: Utc::now().to_rfc3339(),
    }
}

fn parse_and_validate_canonical_input(input: &[u8]) -> Result<Vec<CanonicalRecord>, ReplaceError> {
    let input_text =
        std::str::from_utf8(input).map_err(|e| ReplaceError::InvalidInputTranscript {
            reason: format!("input is not utf-8: {e}"),
            line: None,
        })?;
    let records = parse_canonical_jsonl(input_text)?;
    for record in &records {
        validate_record_for_render(record)?;
    }
    Ok(records)
}

pub(crate) fn parse_provider_owned_canonical_input_for_session(
    session_id: &str,
    input: &[u8],
) -> Result<Vec<CanonicalRecord>, ReplaceError> {
    Uuid::try_parse(session_id).map_err(|_| ReplaceError::OperationalError {
        message: format!("invalid_session_id: {session_id}"),
    })?;
    let input_text =
        std::str::from_utf8(input).map_err(|e| ReplaceError::InvalidInputTranscript {
            reason: format!("input is not utf-8: {e}"),
            line: None,
        })?;
    let records = parse_canonical_jsonl(input_text)?;
    if records.iter().any(|record| record.session_id != session_id) {
        return Err(ReplaceError::OperationalError {
            message: "canonical_session_id_mismatch".to_string(),
        });
    }
    Ok(records)
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
        let Ok(header) = serde_json::from_slice::<ReplaceJournalHeader>(&bytes) else {
            move_to_quarantine(&path, &quarantine_dir);
            continue;
        };
        if header.schema_version == 2 && header.operation == "provider-owned-import-replace" {
            continue;
        }
        if header.schema_version != 1 || header.operation != "import-replace" {
            move_to_quarantine(&path, &quarantine_dir);
            continue;
        }
        let Ok(journal) = serde_json::from_slice::<ReplaceJournal>(&bytes) else {
            move_to_quarantine(&path, &quarantine_dir);
            continue;
        };
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
        recover_replace_journal_entry(&journal_root, &quarantine_dir, &path, &journal, &metadata)?;
    }
    cleanup_orphan_canonical_records(&data_root, &journal_root, &quarantine_dir)?;
    Ok(())
}

fn recover_replace_journal_entry(
    journal_root: &Path,
    quarantine_dir: &Path,
    pending_path: &Path,
    journal: &ReplaceJournal,
    metadata: &SessionMetadata,
) -> Result<(), ReplaceError> {
    let current_hash = canonical_hash_from_provider_file(metadata);
    match (
        journal.preimage_sha256.as_deref(),
        journal.postimage_sha256.as_deref(),
        current_hash,
    ) {
        (_, Some(postimage), Ok(hash)) if hash == postimage => roll_forward_replace_journal(
            journal_root,
            quarantine_dir,
            pending_path,
            journal,
            metadata,
        ),
        (Some(preimage), _, Ok(hash)) if hash == preimage => {
            cleanup_recovered_replace_journal(journal_root, pending_path, journal)
        }
        (Some(_), _, _) if journal.preimage_snapshot_path.is_some() => {
            roll_back_external_replace_journal(journal_root, pending_path, journal, metadata)
        }
        (None, _, _) => cleanup_recovered_replace_journal(journal_root, pending_path, journal),
        _ => {
            move_to_quarantine(pending_path, quarantine_dir);
            Ok(())
        }
    }
}

fn roll_forward_replace_journal(
    journal_root: &Path,
    quarantine_dir: &Path,
    pending_path: &Path,
    journal: &ReplaceJournal,
    metadata: &SessionMetadata,
) -> Result<(), ReplaceError> {
    let Ok(canonical) = fs::read_to_string(&journal.canonical_records_path) else {
        move_to_quarantine(pending_path, quarantine_dir);
        return Ok(());
    };
    let Ok(records) = parse_canonical_jsonl(&canonical) else {
        move_to_quarantine(pending_path, quarantine_dir);
        return Ok(());
    };
    let mut state = StateDb::open_default().map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to open state db during recovery: {e}"),
    })?;
    let input = replacement_from_metadata(metadata, &records)?;
    state
        .replace_session_turns(&input)
        .map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to update state db during recovery: {e}"),
        })?;
    cleanup_recovered_replace_journal(journal_root, pending_path, journal)
}

fn roll_back_external_replace_journal(
    journal_root: &Path,
    pending_path: &Path,
    journal: &ReplaceJournal,
    metadata: &SessionMetadata,
) -> Result<(), ReplaceError> {
    let Some(snapshot_path) = &journal.preimage_snapshot_path else {
        return cleanup_recovered_replace_journal(journal_root, pending_path, journal);
    };
    let bytes = fs::read(snapshot_path).map_err(|e| ReplaceError::OperationalError {
        message: format!(
            "failed to read external replace recovery snapshot {}: {e}",
            snapshot_path.display()
        ),
    })?;
    atomic_write_bytes(&metadata.jsonl_path, &bytes)?;
    cleanup_recovered_replace_journal(journal_root, pending_path, journal)
}

fn cleanup_recovered_replace_journal(
    journal_root: &Path,
    pending_path: &Path,
    journal: &ReplaceJournal,
) -> Result<(), ReplaceError> {
    fs::remove_file(pending_path).ok();
    fs::remove_file(&journal.canonical_records_path).ok();
    if let Some(path) = &journal.preimage_snapshot_path {
        fs::remove_file(path).ok();
    }
    fsync_dir(journal_root)
}

fn cleanup_orphan_canonical_records(
    data_root: &Path,
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
            if let Some(session_part) = name.strip_suffix(".preimage") {
                cleanup_orphan_preimage_snapshot(
                    journal_root,
                    quarantine_dir,
                    &path,
                    session_part,
                )?;
            }
            continue;
        };
        if !session_part.starts_with("session-") {
            continue;
        }
        let session_id = session_part.trim_start_matches("session-");
        let lock_dir = data_root.join("locks");
        if crate::session_lock::any_active_for_session(&lock_dir, session_id)
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

fn cleanup_orphan_preimage_snapshot(
    journal_root: &Path,
    quarantine_dir: &Path,
    path: &Path,
    session_part: &str,
) -> Result<(), ReplaceError> {
    if !session_part.starts_with("session-") {
        return Ok(());
    }
    let pending = journal_root.join(format!("{session_part}.pending"));
    let quarantined_pending = quarantine_dir.join(format!("{session_part}.pending"));
    if !pending.exists() && !quarantined_pending.exists() {
        fs::remove_file(path).map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to remove orphan preimage snapshot: {e}"),
        })?;
        fsync_dir(journal_root)?;
    }
    Ok(())
}

pub fn export_session_canonical(session_id: &str) -> Result<Vec<u8>, ReplaceError> {
    Uuid::try_parse(session_id).map_err(|_| ReplaceError::InvalidSessionId {
        input: session_id.to_string(),
    })?;
    let metadata = resolve_replace_metadata(session_id)?;
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
    RENDER_FOR_STORAGE_CALLS.fetch_add(1, Ordering::SeqCst);
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

fn resolve_replace_metadata(session_id: &str) -> Result<SessionMetadata, ReplaceError> {
    RESOLVE_REPLACE_METADATA_CALLS.fetch_add(1, Ordering::SeqCst);
    let state =
        StateDb::open_default().map_err(|e| ReplaceError::OperationalError { message: e })?;
    let providers =
        ProvidersConfig::load(&default_config_root().join("providers.toml")).map_err(|e| {
            ReplaceError::OperationalError {
                message: e.to_string(),
            }
        })?;
    let models = load_models(&default_models_dir(), Some(&providers)).map_err(|e| {
        ReplaceError::OperationalError {
            message: e.to_string(),
        }
    })?;
    let sessions = SessionsConfig::load(&default_config_root().join("sessions.toml"))
        .map_err(|e| ReplaceError::OperationalError { message: e })?;
    locate_session_metadata(&state, &models, &providers, &sessions, session_id)
        .map_err(map_metadata_error)
}

fn replacement_from_metadata(
    metadata: &SessionMetadata,
    records: &[CanonicalRecord],
) -> Result<SessionTurnsReplacement, ReplaceError> {
    let target = ProviderReplaceDbTarget {
        provider_name: metadata.provider_name.clone(),
        session_id: metadata.session_id.clone(),
        chain_id: metadata.chain_id.clone(),
        active_segment_id: metadata.active_segment_id,
        source_file: metadata.jsonl_path.to_string_lossy().to_string(),
    };
    replacement_for_target(&target, records)
}

fn replacement_for_target(
    target: &ProviderReplaceDbTarget,
    records: &[CanonicalRecord],
) -> Result<SessionTurnsReplacement, ReplaceError> {
    let turns = records
        .iter()
        .map(|record| {
            Ok(SessionTurnReplacement {
                turn_id: record.turn_id.clone(),
                timestamp: record.timestamp.clone(),
                role: record.role.clone(),
                body: replacement_body_json(&record.content)?,
            })
        })
        .collect::<Result<Vec<_>, ReplaceError>>()?;
    Ok(SessionTurnsReplacement {
        provider_name: target.provider_name.clone(),
        session_id: target.session_id.clone(),
        chain_id: target.chain_id.clone(),
        active_segment_id: target.active_segment_id,
        source_file: target.source_file.clone(),
        turns,
    })
}

fn replacement_body_json(content: &[ContentChunk]) -> Result<String, ReplaceError> {
    let mut chunks = Vec::with_capacity(content.len());
    for chunk in content {
        let chunk_type =
            serde_json::to_string(&chunk.r#type).map_err(|e| ReplaceError::OperationalError {
                message: format!("failed to serialize replacement body: {e}"),
            })?;
        let mut object = format!("{{\"type\":{chunk_type}");
        if let Some(text) = &chunk.text {
            let text = serde_json::to_string(text).map_err(|e| ReplaceError::OperationalError {
                message: format!("failed to serialize replacement body: {e}"),
            })?;
            object.push_str(",\"text\":");
            object.push_str(&text);
        }
        object.push('}');
        chunks.push(object);
    }
    Ok(format!("[{}]", chunks.join(",")))
}

fn restoration_for_target(
    target: &ProviderReplaceDbTarget,
    preimage: &ProviderReplaceDbPreimage,
) -> Result<SessionTurnsRestore, ReplaceError> {
    let Some(rows) = preimage.session_turns.as_array() else {
        return Err(ReplaceError::OperationalError {
            message: "invalid_provider_owned_db_preimage".to_string(),
        });
    };
    let turns = rows
        .iter()
        .map(|row| {
            let Some(values) = row.as_array() else {
                return Err(ReplaceError::OperationalError {
                    message: "invalid_provider_owned_db_preimage".to_string(),
                });
            };
            Ok(SessionTurnRestoreRow {
                provider_name: value_str(values, 0)?.to_string(),
                session_id: value_str(values, 1)?.to_string(),
                turn_id: value_str(values, 2)?.to_string(),
                timestamp: value_str(values, 3)?.to_string(),
                role: value_str(values, 4)?.to_string(),
                parent_turn_id: values.get(5).and_then(Value::as_str).map(str::to_string),
                is_sidechain: value_i64(values, 6)?,
                is_compaction_boundary: value_i64(values, 7)?,
                source_file: value_str(values, 8)?.to_string(),
                body: values.get(9).and_then(Value::as_str).map(str::to_string),
            })
        })
        .collect::<Result<Vec<_>, ReplaceError>>()?;
    Ok(SessionTurnsRestore {
        provider_name: target.provider_name.clone(),
        session_id: target.session_id.clone(),
        chain_id: target.chain_id.clone(),
        active_segment_id: target.active_segment_id,
        last_turn_id: preimage.last_turn_id.clone(),
        last_used_at: preimage.last_used_at.clone(),
        turns,
    })
}

fn db_preimage_from_conn(
    conn: &StateReadConnection<'_>,
    target: &ProviderReplaceDbTarget,
) -> Result<ProviderReplaceDbPreimage, ReplaceError> {
    let mut turns = conn
        .prepare(
            "SELECT provider_name, session_id, turn_id, timestamp, role,
                    parent_turn_id, is_sidechain, is_compaction_boundary, source_file, body
             FROM session_turns
             WHERE provider_name = ?1 AND session_id = ?2
             ORDER BY timestamp, turn_id",
        )
        .map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to prepare DB preimage turn query: {e}"),
        })?;
    let session_turns = turns
        .query_map(params![target.provider_name, target.session_id], |row| {
            Ok(json!([
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                "<session-transcript>",
                row.get::<_, Option<String>>(9)?,
            ]))
        })
        .map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to query DB preimage turns: {e}"),
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to read DB preimage turns: {e}"),
        })?;
    let (last_turn_id, last_used_at) = conn
        .query_row(
            "SELECT s.last_turn_id, c.last_used_at
             FROM session_chain_segments s
             JOIN session_chains c ON c.chain_id = s.chain_id
             WHERE s.id = ?1",
            params![target.active_segment_id],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to read DB preimage chain state: {e}"),
        })?;
    Ok(ProviderReplaceDbPreimage {
        session_turns: Value::Array(session_turns),
        last_turn_id,
        last_used_at,
    })
}

fn value_str(values: &[Value], index: usize) -> Result<&str, ReplaceError> {
    values
        .get(index)
        .and_then(Value::as_str)
        .ok_or_else(|| ReplaceError::OperationalError {
            message: "invalid_provider_owned_db_preimage".to_string(),
        })
}

fn value_i64(values: &[Value], index: usize) -> Result<i64, ReplaceError> {
    values
        .get(index)
        .and_then(Value::as_i64)
        .ok_or_else(|| ReplaceError::OperationalError {
            message: "invalid_provider_owned_db_preimage".to_string(),
        })
}

fn canonical_records_from_provider_file(
    metadata: &SessionMetadata,
) -> Result<Vec<CanonicalRecord>, ReplaceError> {
    CANONICAL_RECORDS_FROM_PROVIDER_FILE_CALLS.fetch_add(1, Ordering::SeqCst);
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
    oulipoly_state::paths::data_dir().map_err(|_| ReplaceError::OperationalError {
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::session_export::RecordSource;
    use crate::test_support::lock_env;
    use rusqlite::params;
    use std::os::unix::fs::PermissionsExt;

    const SESSION_ID: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
    const CHAIN_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const PROVIDER: &str = "codex";
    const MODEL: &str = "codex-high";

    fn with_homes<T>(config_home: &Path, data_home: &Path, test: impl FnOnce() -> T) -> T {
        let _guard = lock_env();
        let data_root = data_home.join(oulipoly_state::paths::APP_DATA_DIR_NAME);
        let old_data_dir = std::env::var_os("OULIPOLY_DATA_DIR");
        let old_config = std::env::var_os("XDG_CONFIG_HOME");
        let old_data = std::env::var_os("XDG_DATA_HOME");
        let old_path = std::env::var_os("PATH");
        let scripts_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("scripts");
        let path = std::env::join_paths(
            std::iter::once(scripts_dir)
                .chain(std::env::split_paths(&old_path.clone().unwrap_or_default())),
        )
        .unwrap();
        unsafe {
            std::env::set_var("OULIPOLY_DATA_DIR", &data_root);
            std::env::set_var("XDG_CONFIG_HOME", config_home);
            std::env::set_var("XDG_DATA_HOME", data_home);
            std::env::set_var("PATH", path);
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(test));
        match old_data_dir {
            Some(value) => unsafe {
                std::env::set_var("OULIPOLY_DATA_DIR", value);
            },
            None => unsafe {
                std::env::remove_var("OULIPOLY_DATA_DIR");
            },
        }
        match old_config {
            Some(value) => unsafe {
                std::env::set_var("XDG_CONFIG_HOME", value);
            },
            None => unsafe {
                std::env::remove_var("XDG_CONFIG_HOME");
            },
        }
        match old_data {
            Some(value) => unsafe {
                std::env::set_var("XDG_DATA_HOME", value);
            },
            None => unsafe {
                std::env::remove_var("XDG_DATA_HOME");
            },
        }
        match old_path {
            Some(value) => unsafe {
                std::env::set_var("PATH", value);
            },
            None => unsafe {
                std::env::remove_var("PATH");
            },
        }
        match result {
            Ok(value) => value,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    fn write_script(path: &Path, body: &str) {
        fs::write(
            path,
            format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
        )
        .unwrap();
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }

    fn sh_path(path: &Path) -> String {
        format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
    }

    fn replacement_records(jsonl_path: &Path) -> Vec<CanonicalRecord> {
        vec![
            CanonicalRecord {
                session_id: SESSION_ID.to_string(),
                provider_name: PROVIDER.to_string(),
                turn_id: "new-user".to_string(),
                role: "user".to_string(),
                timestamp: "2026-04-17T08:00:00Z".to_string(),
                content: vec![ContentChunk {
                    r#type: "text".to_string(),
                    text: Some("replacement user body".to_string()),
                }],
                source: RecordSource {
                    storage_type: "codex_session".to_string(),
                    jsonl_path: jsonl_path.to_path_buf(),
                    line: 2,
                    byte_start: 0,
                    byte_end: 0,
                    sha256: "fixture".to_string(),
                },
                unsupported_record: false,
            },
            CanonicalRecord {
                session_id: SESSION_ID.to_string(),
                provider_name: PROVIDER.to_string(),
                turn_id: "new-assistant".to_string(),
                role: "assistant".to_string(),
                timestamp: "2026-04-17T08:00:01Z".to_string(),
                content: vec![ContentChunk {
                    r#type: "text".to_string(),
                    text: Some("replacement assistant body".to_string()),
                }],
                source: RecordSource {
                    storage_type: "codex_session".to_string(),
                    jsonl_path: jsonl_path.to_path_buf(),
                    line: 3,
                    byte_start: 0,
                    byte_end: 0,
                    sha256: "fixture".to_string(),
                },
                unsupported_record: false,
            },
        ]
    }

    #[test]
    fn restoration_rejects_missing_or_non_integer_lineage_flags() {
        let target = ProviderReplaceDbTarget {
            provider_name: PROVIDER.to_string(),
            session_id: SESSION_ID.to_string(),
            chain_id: CHAIN_ID.to_string(),
            active_segment_id: 1,
            source_file: "<session-transcript>".to_string(),
        };
        let row = json!([
            PROVIDER,
            SESSION_ID,
            "turn-1",
            "2026-04-17T08:00:00Z",
            "assistant",
            null,
            1,
            1,
            "<session-transcript>",
            null
        ]);

        let mut invalid_sidechain = row.clone();
        invalid_sidechain.as_array_mut().unwrap()[6] = Value::String("invalid".to_string());
        let mut invalid_boundary = row.clone();
        invalid_boundary.as_array_mut().unwrap()[7] = Value::String("invalid".to_string());
        let mut missing_sidechain = row.clone();
        missing_sidechain.as_array_mut().unwrap().truncate(6);
        let mut missing_boundary = row;
        missing_boundary.as_array_mut().unwrap().truncate(7);

        for invalid_row in [
            invalid_sidechain,
            invalid_boundary,
            missing_sidechain,
            missing_boundary,
        ] {
            let preimage = ProviderReplaceDbPreimage {
                session_turns: Value::Array(vec![invalid_row]),
                last_turn_id: Some("turn-1".to_string()),
                last_used_at: "2026-04-17T08:00:00Z".to_string(),
            };

            assert!(matches!(
                restoration_for_target(&target, &preimage),
                Err(ReplaceError::OperationalError { message })
                    if message == "invalid_provider_owned_db_preimage"
            ));
        }
    }

    #[test]
    fn import_replace_round_trips_canonical_content_into_session_turn_bodies() {
        // risk: atomic-transaction regression; level: particular-integration; source: contract §4 T9 / proposal A4.
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let app_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_dir.join("models");
        let codex_sessions = dir.path().join("codex-sessions");
        let workspace = dir.path().join("workspace");
        fs::create_dir_all(&models_dir).unwrap();
        fs::create_dir_all(&codex_sessions).unwrap();
        fs::create_dir_all(&workspace).unwrap();
        let jsonl_path = codex_sessions.join("rollout-2026-04-17.jsonl");
        fs::write(
            &jsonl_path,
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{SESSION_ID}\",\"cwd\":\"{}\"}}}}\n\
                 {{\"type\":\"response_item\",\"timestamp\":\"2026-04-17T08:00:00Z\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"old user\"}}]}}}}\n\
                 {{\"type\":\"response_item\",\"timestamp\":\"2026-04-17T08:00:01Z\",\"payload\":{{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{{\"type\":\"output_text\",\"text\":\"old assistant\"}}]}}}}\n",
                workspace.display()
            ),
        )
        .unwrap();
        fs::write(
            models_dir.join(format!("{MODEL}.toml")),
            format!("[[providers]]\nname = \"{PROVIDER}\"\n"),
        )
        .unwrap();
        fs::write(
            app_dir.join("providers.toml"),
            format!(
                r#"[{PROVIDER}]
command = "codex"
args = []
prompt_mode = "arg"

[{PROVIDER}.resume]
kind = "subcommand"
subcommand = ["resume"]

[{PROVIDER}.session_storage]
kind = "codex"
sessions_dir = "{}"
"#,
                codex_sessions.display()
            ),
        )
        .unwrap();
        let locator_path = dir.path().join("locator.sh");
        write_script(
            &locator_path,
            &format!("printf '%s\\n' {}", sh_path(&jsonl_path)),
        );
        fs::write(
            app_dir.join("sessions.toml"),
            format!(
                "[{PROVIDER}]\nturn_script = \"true\"\ntranscript_locator = {:?}\nstate_dir = {:?}\n",
                locator_path.to_string_lossy(),
                dir.path().join("locator-state").to_string_lossy()
            ),
        )
        .unwrap();

        with_homes(&config_home, &data_home, || {
            let db = StateDb::open_default().unwrap();
            let connection = rusqlite::Connection::open(db.path()).unwrap();
            connection
                .execute(
                    "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
                     VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:00:01Z', ?2)",
                    params![CHAIN_ID, MODEL],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO session_chain_segments
                        (chain_id, provider_name, session_id, started_at, last_turn_id, transition_reason)
                     VALUES (?1, ?2, ?3, '2026-04-17T08:00:00Z', 'old-assistant', 'initial')",
                    params![CHAIN_ID, PROVIDER, SESSION_ID],
                )
                .unwrap();
            for (turn_id, role) in [("old-user", "user"), ("old-assistant", "assistant")] {
                connection
                    .execute(
                        "INSERT INTO session_turns
                            (provider_name, session_id, turn_id, timestamp, role,
                             parent_turn_id, is_sidechain, is_compaction_boundary, source_file, ingested_at)
                         VALUES (?1, ?2, ?3, '2026-04-17T08:00:00Z', ?4, NULL, 0, 0, ?5, '2026-04-17T08:00:00Z')",
                        params![PROVIDER, SESSION_ID, turn_id, role, jsonl_path.to_string_lossy()],
                    )
                    .unwrap();
            }

            let records = replacement_records(&jsonl_path);
            let input = canonical_jsonl_bytes(&records).unwrap();
            let receipt = run_import_replace_bytes(SESSION_ID, &input, None).unwrap();

            assert!(receipt.state_updated);
            assert_eq!(receipt.session_id, SESSION_ID);
            assert_eq!(receipt.provider_name, PROVIDER);
            let rows = db
                .connection()
                .prepare(
                    "SELECT turn_id, body FROM session_turns
                     WHERE provider_name = ?1 AND session_id = ?2
                     ORDER BY timestamp, id",
                )
                .unwrap()
                .query_map(params![PROVIDER, SESSION_ID], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0].0, "new-user");
            assert_eq!(
                rows[0].1,
                serde_json::to_string(&records[0].content).unwrap()
            );
            assert_eq!(rows[1].0, "new-assistant");
            assert_eq!(
                rows[1].1,
                serde_json::to_string(&records[1].content).unwrap()
            );
        });
    }
}
