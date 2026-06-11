//! ## Declared roles
//!
//! - accessor
//! - mapper
//! - formatter
//! - predicate
//! - validator
//! - parser
//! - orchestration
//! - filter
//!
//! Role set: { accessor, mapper, formatter, predicate, validator, parser, orchestration, filter }
//!
//! Per ACR-249/ACR-250 db.rs is a declared multi-role state-DB persistence adapter that owns
//! SQLite open/migration/schema behavior, marker parsing/formatting (re-exported from
//! `invocation_marker.rs`), sidecar identity classification (delegated to `db/sqlite_adapter.rs`),
//! lifecycle log sink integration (delegated to `db/lifecycle_log_adapter.rs`), and resume/quota
//! orchestration. Intrinsic-surface declarations cover the schema-version and chrono couplings;
//! see `the AGE-160 proposal § Intrinsic-surface declarations` for the canonical declaration.
//!
//! AGE-160 intrinsic schema-version carrier: `crate::schema` owns the schema-version constants and
//! compatibility classifier consumed by this StateDb open/migration boundary.
//!
//! AGE-160 intrinsic timestamp carrier: `chrono` owns the UTC timestamp and RFC3339 shapes persisted
//! and returned by this StateDb boundary.
//!
//! AGE-160 serde_json residual disposition: remaining JSON calls are DB-owned artifact/config payload
//! codecs and test assertions after marker and lifecycle JSON construction moved behind adapters.
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db.rs
//!     role: intrinsic-surface
//!     Domain: state_db_persistence
//!     Owns:
//!       - provider_quotas.exhausted_at
//!       - count_session_turns
//! ```

mod accounts;
mod cli_providers;
mod discovered_models;
mod discovery_types;
mod lifecycle_log_adapter;
mod model_parameters;
mod provider_quota_reads;
mod provider_quotas;
mod sqlite_adapter;

pub use self::discovery_types::{
    AccountRecord, AuthMethod, AuthStatus, CliMapping, CliProviderRecord, DiscoveredModel,
    ModelParameter, ParamType,
};
use self::provider_quotas::{
    MAX_LEARNABLE_BURN_RATE, MIN_LEARN_SAMPLE_CALLS, NEAR_EXHAUSTED_USED_PERCENT,
    QuotaAggregateProjection, QuotaWindowDelta,
};
pub use self::provider_quotas::{QuotaRecord, QuotaWindow, QuotaWindowInput};

use self::lifecycle_log_adapter as lc_log_adapter;
use self::sqlite_adapter as sqlite;
use self::sqlite_adapter::params;
use self::sqlite_adapter::{Connection, RusqliteOptionalExtension, Transaction};
#[cfg(test)]
use crate::invocation_marker::CompositeInvocationId;
use crate::lifecycle_log::{LifecycleEventSink, NoopLifecycleEventSink};
use crate::migrations;
use crate::result_envelope::{
    ResultEnvelopeFailureIdentity, ResultEnvelopeInput, result_envelope_payload,
};
use crate::schema::{
    CURRENT_SCHEMA_VERSION, MINIMUM_SUPPORTED_SCHEMA_VERSION, SchemaCompatibility,
};
use chrono::{DateTime, Utc};
use oulipoly_agent_messenger::ReturnedArtifactRef;
use oulipoly_config::{ModelConfig, load_models};
use oulipoly_core::TransitionReason;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use uuid::Uuid;

const OPENCODE_SESSION_PREFIX: &str = "ses_";
const OPENCODE_SESSION_MIN_SUFFIX_LEN: usize = 3;

pub struct StateDb {
    conn: sqlite::Connection,
    db_path: PathBuf,
    lifecycle_sink: Mutex<Box<dyn LifecycleEventSink + Send>>,
}

#[derive(Debug, Clone)]
pub enum ReadOnlyOpenError {
    Missing { path: PathBuf },
    NotADatabase { path: PathBuf, message: String },
    PermissionDenied { path: PathBuf },
    WalSidecarError { path: PathBuf, message: String },
    Operational { message: String },
}

fn classify_read_only_open_error(path: &Path, err: sqlite::Error) -> ReadOnlyOpenError {
    match sqlite::project_read_only_open_error(path, &err) {
        sqlite::ReadOnlyOpenFailure::WalSidecar { message }
        | sqlite::ReadOnlyOpenFailure::ShmSidecar { message } => {
            read_only_wal_sidecar_error(path.to_path_buf(), message)
        }
        sqlite::ReadOnlyOpenFailure::PlainDb { kind, message } => {
            read_only_plain_db_error(path, kind, message)
        }
        sqlite::ReadOnlyOpenFailure::Unknown { message } => {
            ReadOnlyOpenError::Operational { message }
        }
    }
}

fn read_only_plain_db_error(
    path: &Path,
    kind: sqlite::PlainDbKind,
    message: String,
) -> ReadOnlyOpenError {
    match kind {
        sqlite::PlainDbKind::NotDatabase | sqlite::PlainDbKind::Corrupt => {
            read_only_not_database(path.to_path_buf(), message)
        }
        sqlite::PlainDbKind::PermissionDenied => read_only_permission_denied(path.to_path_buf()),
        sqlite::PlainDbKind::ReadOnly
        | sqlite::PlainDbKind::CannotOpen
        | sqlite::PlainDbKind::SystemIo => ReadOnlyOpenError::Operational { message },
    }
}

fn read_only_not_database(path: PathBuf, message: String) -> ReadOnlyOpenError {
    ReadOnlyOpenError::NotADatabase { path, message }
}

fn read_only_permission_denied(path: PathBuf) -> ReadOnlyOpenError {
    ReadOnlyOpenError::PermissionDenied { path }
}

fn read_only_wal_sidecar_error(path: PathBuf, message: String) -> ReadOnlyOpenError {
    ReadOnlyOpenError::WalSidecarError { path, message }
}

fn wal_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}-wal", path.display()))
}

fn shm_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}-shm", path.display()))
}

#[cfg(unix)]
fn path_is_unreadable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    match std::fs::metadata(path) {
        Ok(metadata) => metadata.permissions().mode() & 0o444 == 0,
        Err(err) => err.kind() == std::io::ErrorKind::PermissionDenied,
    }
}

#[cfg(not(unix))]
fn path_is_unreadable(path: &Path) -> bool {
    match std::fs::File::open(path) {
        Ok(_) => false,
        Err(err) => err.kind() == std::io::ErrorKind::PermissionDenied,
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ProviderRecord {
    pub model_name: String,
    pub provider_name: String,
    pub invocation_count: i64,
    pub error_count: i64,
    pub last_error: Option<String>,
    pub last_error_at: Option<String>,
    pub last_invoked_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderColumn {
    name: String,
    data_type: String,
    notnull: i64,
    pk: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveChainSegmentSnapshot {
    pub chain_id: String,
    pub active_provider: String,
    pub active_session_id: String,
    pub active_started_at: String,
    pub active_ended_at: Option<String>,
    pub active_last_turn_id: Option<String>,
    pub latest_turn_at: Option<String>,
}

pub struct ChainSegmentRotationInput<'a> {
    pub chain_id: &'a str,
    pub source_provider_name: &'a str,
    pub source_session_id: &'a str,
    pub target_provider_name: &'a str,
    pub target_session_id: &'a str,
    pub changed_at: &'a DateTime<Utc>,
    pub reason: TransitionReason,
}

/// One turn ingested from a CLI session log. The unified store across
/// every CLI we know how to parse — Claude Code, Codex, etc.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SessionTurnRecord {
    pub provider_name: String,
    pub session_id: String,
    pub turn_id: String,
    pub timestamp: DateTime<Utc>,
    /// "user" or "assistant" — only "assistant" turns count toward quota.
    pub role: String,
    pub parent_turn_id: Option<String>,
    pub is_sidechain: bool,
    pub source_file: String,
}

/// One turn batched into `ingest_session_turns_batch`. Named struct
/// instead of a tuple so callers can't accidentally swap positional
/// fields (the role / parent_turn_id pair is otherwise easy to mix up).
#[derive(Debug, Clone)]
pub struct SessionTurnIngest {
    pub session_id: String,
    pub turn_id: String,
    pub timestamp: DateTime<Utc>,
    pub role: String,
    pub parent_turn_id: Option<String>,
    pub is_sidechain: bool,
    pub is_compaction_boundary: bool,
    pub body: Option<String>,
}

/// Oulipoly-owned compact-summary evidence projected from provider transcripts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedTurnEventRow {
    pub session_id: String,
    pub turn_uuid: String,
    pub is_compaction_boundary: bool,
    pub summary_metadata_json: Option<String>,
}

/// Test-visible owned turn/event row read from the state boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedTurnEvent {
    pub session_id: String,
    pub turn_uuid: String,
    pub is_compaction_boundary: bool,
    pub summary_metadata_json: Option<String>,
    pub ingested_at: String,
}

/// Compact-summary evidence consumed by `migrate-db`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactSummaryEvidence {
    pub session_id: String,
    pub compact_turn_uuids: Vec<String>,
}

mod owned_turn_event;

pub type ModelStore = std::collections::HashMap<String, ModelConfig>;
pub type DbError = String;

macro_rules! invocation_returned_artifacts_schema_sql {
    () => {
        "CREATE TABLE IF NOT EXISTS invocation_returned_artifacts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            invocation_id INTEGER NOT NULL REFERENCES invocations(id),
            ordinal INTEGER NOT NULL,
            version_id TEXT NOT NULL,
            name TEXT NOT NULL,
            workflow_run_id TEXT NOT NULL,
            artifact_name TEXT NOT NULL,
            version INTEGER NOT NULL CHECK(version > 0),
            sha256 TEXT NOT NULL CHECK(length(sha256) = 64),
            content_len INTEGER NOT NULL CHECK(content_len >= 0),
            format_hint TEXT NULL,
            verdict_line TEXT NULL,
            source_kind TEXT NOT NULL,
            source_json TEXT NOT NULL,
            returned_at TEXT NOT NULL,
            UNIQUE(invocation_id, ordinal),
            UNIQUE(invocation_id, version_id)
        );"
    };
}

fn returned_source_kind(source: &oulipoly_agent_messenger::ReturnedArtifactSource) -> &'static str {
    match source {
        oulipoly_agent_messenger::ReturnedArtifactSource::Scratchpad { .. } => "scratchpad",
        oulipoly_agent_messenger::ReturnedArtifactSource::InlineBytes => "inline_bytes",
    }
}

fn returned_artifact_producer_uuid(workflow_run_id: &str) -> sqlite::Result<Uuid> {
    let uuid_text = returned_artifact_workflow_uuid_text(workflow_run_id)?;
    parse_returned_artifact_uuid(uuid_text)
}

fn returned_artifact_workflow_uuid_text(workflow_run_id: &str) -> sqlite::Result<&str> {
    workflow_run_id
        .strip_prefix("return:")
        .ok_or_else(returned_artifact_workflow_namespace_error)
}

fn returned_artifact_workflow_namespace_error() -> sqlite::Error {
    sqlite::Error::FromSqlConversionFailure(
        2,
        sqlite::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "returned artifact workflow_run_id is not in return namespace",
        )),
    )
}

fn parse_returned_artifact_uuid(uuid_text: &str) -> sqlite::Result<Uuid> {
    Uuid::parse_str(uuid_text).map_err(|err| {
        sqlite::Error::FromSqlConversionFailure(2, sqlite::Type::Text, Box::new(err))
    })
}

fn returned_artifact_version_id(
    invocation_uuid: Uuid,
    artifact_name: &str,
    version: u64,
) -> String {
    let encoded_name = returned_artifact_encoded_name(artifact_name);
    format_returned_artifact_version_id(invocation_uuid, &encoded_name, version)
}

fn returned_artifact_encoded_name(artifact_name: &str) -> String {
    let mut encoded_name = String::new();
    for byte in artifact_name.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded_name.push(byte as char);
        } else {
            encoded_name.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded_name
}

fn format_returned_artifact_version_id(
    invocation_uuid: Uuid,
    encoded_name: &str,
    version: u64,
) -> String {
    format!("store://return/{invocation_uuid}/{encoded_name}/{version}")
}

fn returned_artifact_sql_integer(value: u64, field: &str) -> Result<i64, DbError> {
    validate_returned_artifact_sql_integer(value, field)?;
    Ok(map_returned_artifact_sql_integer(value))
}

fn validate_returned_artifact_sql_integer(value: u64, field: &str) -> Result<(), DbError> {
    if value > i64::MAX as u64 {
        Err(returned_artifact_sql_integer_overflow(field, value))
    } else {
        Ok(())
    }
}

fn map_returned_artifact_sql_integer(value: u64) -> i64 {
    value as i64
}

fn returned_artifact_sql_integer_overflow(field: &str, value: u64) -> DbError {
    format!("Returned artifact {field} exceeds SQLite INTEGER range: {value}")
}

#[derive(Debug, Clone)]
pub struct ResolvedResume {
    pub chain_id: String,
    pub model_name: Option<String>,
    pub model: Option<ModelConfig>,
    pub active_provider: String,
    pub active_session_id: String,
}

#[derive(Debug, Clone)]
pub enum ResumeError {
    InvalidUuid {
        input: String,
    },
    NoChainFound {
        input: String,
    },
    WrongIdKind {
        input: String,
        input_kind: WrongIdKindInput,
        provider_session_id: Option<String>,
        agent_runner_invocation_id: String,
        chain_id: Option<String>,
        provider_name: Option<String>,
    },
    Ambiguous {
        input: String,
        previews: Vec<ChainPreview>,
    },
    ProviderModelMismatch {
        model_name: String,
        active_provider: String,
        suggestions: Vec<String>,
    },
    ProviderNotConfigured {
        provider: String,
    },
    UnknownModel {
        model_name: String,
    },
    ActiveSegmentMissing {
        chain_id: String,
    },
    ProviderMissingResume {
        provider_name: String,
    },
    Db {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrongIdKindInput {
    AgentRunnerInvocationId,
}

#[derive(Debug, Clone)]
pub struct ChainPreview {
    pub chain_id: String,
    pub last_used_at: DateTime<Utc>,
    pub active_provider: String,
    pub active_session_id: String,
    pub turn_count: usize,
    pub recent_turns: Vec<TurnPreview>,
}

#[derive(Debug, Clone)]
pub struct TurnPreview {
    pub role: String,
    pub timestamp: DateTime<Utc>,
    pub snippet: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillReport {
    pub skipped_existing: bool,
    pub chains_inserted: u64,
    pub segments_inserted: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTurnCounts {
    pub total: u64,
    pub assistant: u64,
    pub sidechain: u64,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct InvocationRecord {
    pub id: i64,
    pub invocation_uuid: String,
    pub model_name: String,
    pub provider_name: Option<String>,
    pub provider_index: usize,
    pub parent_invocation_id: Option<i64>,
    pub status: InvocationStatus,
    pub success: Option<bool>,
    pub exit_code: Option<i32>,
    pub error_category: Option<String>,
    pub terminal_reason: Option<String>,
    pub session_id: Option<String>,
    pub session_capture_method: Option<String>,
    pub provider_session_id: Option<String>,
    pub resume_input_id: Option<String>,
    pub provider_session_capture_method: Option<String>,
    pub provider_session_resolved_account: Option<String>,
    pub resume_acceptance_status: Option<String>,
    pub resume_acceptance_evidence: Option<String>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct InvocationStart {
    pub invocation_uuid: String,
    pub model_name: String,
    pub provider_name: String,
    pub provider_index: usize,
    pub parent_invocation_id: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvocationStatus {
    Running,
    Succeeded,
    Failed,
    Legacy,
}

impl InvocationStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            InvocationStatus::Running => "running",
            InvocationStatus::Succeeded => "succeeded",
            InvocationStatus::Failed => "failed",
            InvocationStatus::Legacy => "legacy",
        }
    }

    /// Inherent `from_str` returning `Option<Self>` per the PR-A contract
    /// (`tmp/01-pr-a-contract.md` §"Struct contract"). The `FromStr` trait
    /// impl below provides the `Result`-returning idiomatic Rust surface;
    /// this inherent method is the contracted API caller-facing surface.
    /// Clippy's `should_implement_trait` lint flags the name collision —
    /// allowed here because both surfaces are intentional and the contract
    /// pins this specific shape.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        s.parse().ok()
    }
}

impl std::str::FromStr for InvocationStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "running" => Ok(InvocationStatus::Running),
            "succeeded" => Ok(InvocationStatus::Succeeded),
            "failed" => Ok(InvocationStatus::Failed),
            "legacy" => Ok(InvocationStatus::Legacy),
            _ => Err(format!("Unknown invocation status: {s}")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionMarkerPayload {
    pub agent_runner_invocation_id: String,
    pub provider_session_id: Option<String>,
    pub provider_name: Option<String>,
    pub agent_runner_chain_id: Option<String>,
    pub resume_input_id: Option<String>,
    pub legacy_id: String,
    pub legacy_session_id: Option<String>,
}

impl SessionMarkerPayload {
    pub fn stderr_line(&self) -> String {
        let payload = serde_json::json!({
            "id": self.legacy_id,
            "session_id": self.legacy_session_id,
            "agent_runner_invocation_id": self.agent_runner_invocation_id,
            "provider_session_id": self.provider_session_id,
            "provider_name": self.provider_name,
            "agent_runner_chain_id": self.agent_runner_chain_id,
            "resume_input_id": self.resume_input_id,
        });
        format!("OULIPOLY_SESSION={payload}\n")
    }
}

#[derive(Debug, Clone)]
pub struct ProviderSessionBinding {
    pub provider_session_id: String,
    pub capture_method: &'static str,
    pub resume_input_id: Option<String>,
    pub provider_session_resolved_account: Option<String>,
}

struct WrongIdKindInvocationMatch {
    invocation_uuid: String,
    provider_name: Option<String>,
    provider_session_id: Option<String>,
    chain_id: Option<String>,
}

struct WrongIdKindInvocationRow {
    invocation_uuid: String,
    provider_name: Option<String>,
    provider_session_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderSessionProjection {
    DualId,
    LegacySessionId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InvocationDualIdProjection {
    Current,
    CurrentWithoutResolvedAccount,
    Legacy,
}

impl InvocationDualIdProjection {
    fn select_columns(&self) -> (&'static str, &'static str, &'static str, &'static str) {
        match self {
            InvocationDualIdProjection::Current => (
                "provider_session_id",
                "resume_input_id",
                "provider_session_capture_method",
                "provider_session_resolved_account",
            ),
            InvocationDualIdProjection::CurrentWithoutResolvedAccount => (
                "provider_session_id",
                "resume_input_id",
                "provider_session_capture_method",
                "NULL AS provider_session_resolved_account",
            ),
            InvocationDualIdProjection::Legacy => (
                "NULL AS provider_session_id",
                "NULL AS resume_input_id",
                "NULL AS provider_session_capture_method",
                "NULL AS provider_session_resolved_account",
            ),
        }
    }
}

struct FinalizeInvocationRow {
    invocation_uuid: String,
    model_name: String,
    provider_name: Option<String>,
    provider_session_id: Option<String>,
    status: String,
}

type FinalizeInvocationRowColumns = (String, String, Option<String>, Option<String>, String);

enum InvocationsSchemaShape {
    Empty,
    Current,
    LegacyPreUuid,
    UnrecognizedPreUuid(Vec<String>),
}

enum ProvidersSchemaShape {
    Empty,
    Current,
    LegacyIndexKeyed,
    Unexpected(String),
}

struct ColumnRepair {
    column_name: &'static str,
    sql: &'static str,
    error_context: &'static str,
}

struct DropColumnRepair {
    column_name: &'static str,
    sql: &'static str,
    error_context: &'static str,
}

struct LegacyInvocationRow {
    model_name: String,
    provider_index: i64,
    success: i64,
    exit_code: i64,
    error_category: Option<String>,
    created_at: String,
}

struct LegacyInvocationInsert {
    invocation_uuid: String,
    model_name: String,
    provider_name: Option<String>,
    provider_index: i64,
    status: InvocationStatus,
    success: i64,
    exit_code: i64,
    error_category: Option<String>,
    created_at: String,
}

struct InvocationIdentity {
    row_id: i64,
    uuid: Uuid,
}

struct LifecycleInvocationRow {
    invocation_uuid: String,
    provider_name: Option<String>,
    session_id: Option<String>,
    provider_session_id: Option<String>,
    resume_input_id: Option<String>,
}

type OperationResult = &'static str;

#[derive(Debug, Clone, Copy)]
enum RawArtifactKind {
    Stdout,
    Stderr,
    Result,
    EventsJsonl,
}

struct FinalizeLifecycleInput<'a> {
    terminal_status_attempt: &'a str,
    exit_code: i32,
    error_category: Option<&'a str>,
    terminal_reason: Option<&'a str>,
    operation_result: OperationResult,
}

struct ReturnedArtifactRawRow {
    version_id: String,
    name: String,
    workflow_run_id: String,
    artifact_name: String,
    version: i64,
    sha256: String,
    content_len: i64,
    format_hint: Option<String>,
    verdict_line: Option<String>,
    source_json: String,
    returned_at_text: String,
}

struct ReturnedArtifactValidatedInputs {
    version: i64,
    content_len: i64,
}

struct ReturnedArtifactPayloadFields {
    source_json: String,
    returned_at: String,
}

struct ReturnedArtifactRowParams<'a> {
    invocation_row_id: i64,
    ordinal: i64,
    version_id: &'a str,
    name: &'a str,
    workflow_run_id: &'a str,
    artifact_name: &'a str,
    version: i64,
    sha256: &'a str,
    content_len: i64,
    format_hint: &'a Option<String>,
    verdict_line: &'a Option<String>,
    source_kind: &'static str,
    source_json: &'a str,
    returned_at: &'a str,
}

struct ParsedReturnedArtifactFieldValues {
    source: oulipoly_agent_messenger::ReturnedArtifactSource,
    returned_at: DateTime<Utc>,
    producer_invocation_uuid: Uuid,
    version: i64,
    content_len: i64,
}

struct ValidatedReturnedArtifactFieldValues {
    source: oulipoly_agent_messenger::ReturnedArtifactSource,
    returned_at: DateTime<Utc>,
    producer_invocation_uuid: Uuid,
    version: u64,
    content_len: u64,
}

enum ReturnedArtifactFieldError {
    SourceJson(serde_json::Error),
    ReturnedAt {
        raw: String,
        err: chrono::ParseError,
    },
    ProducerUuid(sqlite::Error),
    NegativeInteger {
        field: &'static str,
    },
}

struct SessionCaptureProjection<'a> {
    provider_session_id: Option<&'a str>,
    resume_input_id: Option<&'a str>,
    provider_session_capture_method: Option<&'a str>,
}

struct SessionTurnBindValues<'a> {
    session_id: &'a str,
    turn_id: &'a str,
    timestamp: String,
    role: &'a str,
    parent_turn_id: Option<&'a str>,
    is_sidechain: i64,
    is_compaction_boundary: i64,
    body: Option<&'a str>,
}

struct InvocationChainMintRow {
    model_name: String,
    provider_name: String,
    session_id: String,
    raw_ts: String,
}

struct RecentTurnRow {
    role: String,
    timestamp_raw: String,
}

struct ParsedTurnPreviewTimestamp {
    role: String,
    timestamp: DateTime<Utc>,
}

#[derive(Debug)]
struct SessionChainBackfillRow {
    provider: String,
    session: String,
    started_at: String,
    last_used_at: String,
    last_turn_id: String,
}

struct ResumeChainCandidate {
    chain_id: String,
    last_used_at: DateTime<Utc>,
    latest_segment_started_at: DateTime<Utc>,
}

struct InvocationWindowTurnRow {
    session_id: String,
    timestamp_raw: String,
}

fn lifecycle_terminal_status(success: bool) -> &'static str {
    if success { "success" } else { "failed" }
}

fn active_lifecycle_session_id(row: &LifecycleInvocationRow) -> Option<String> {
    row.provider_session_id
        .clone()
        .or_else(|| row.session_id.clone())
}

impl StateDb {
    pub fn open(path: &Path) -> Result<Self, String> {
        Self::open_with_sink(path, Box::new(NoopLifecycleEventSink))
    }

    pub fn open_with_sink(
        path: &Path,
        sink: Box<dyn LifecycleEventSink + Send>,
    ) -> Result<Self, String> {
        Self::ensure_state_parent_dir(path)?;
        let mut conn =
            sqlite::Connection::open(path).map_err(|e| format!("Failed to open state DB: {e}"))?;

        let ran_open_migrations = Self::run_open_migrations(path, &mut conn)?;
        Self::apply_current_schema_repairs(&mut conn, ran_open_migrations)?;
        let db = StateDb {
            conn,
            db_path: path.to_path_buf(),
            lifecycle_sink: Mutex::new(sink),
        };
        db.backfill_session_chains()
            .map_err(|e| format!("{e}; run `agents migrate-db` first"))?;

        Ok(db)
    }

    fn ensure_state_parent_dir(path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create state directory: {e}"))?;
        }
        Ok(())
    }

    fn run_open_migrations(path: &Path, conn: &mut sqlite::Connection) -> Result<bool, String> {
        let compatibility = migrations::classify(conn)?;
        let ran_open_migrations = Self::compatibility_runs_open_migrations(&compatibility);
        Self::dispatch_open_migration_plan(path, conn, compatibility)?;
        Ok(ran_open_migrations)
    }

    fn compatibility_runs_open_migrations(compatibility: &SchemaCompatibility) -> bool {
        matches!(
            compatibility,
            SchemaCompatibility::Fresh
                | SchemaCompatibility::Migratable { .. }
                | SchemaCompatibility::LegacyVersionless
        )
    }

    fn dispatch_open_migration_plan(
        path: &Path,
        conn: &mut sqlite::Connection,
        compatibility: SchemaCompatibility,
    ) -> Result<(), String> {
        match compatibility {
            SchemaCompatibility::Fresh => {
                Self::set_wal_mode(conn)?;
                Self::run_current_plan_from(path, conn, 0)
            }
            SchemaCompatibility::Current { .. } => Self::set_wal_mode(conn),
            SchemaCompatibility::Migratable { stored } => {
                Self::set_wal_mode(conn)?;
                let stored = Self::promote_existing_dual_id_schema5_if_present(conn, stored)?;
                Self::run_current_plan_from(path, conn, stored)
            }
            SchemaCompatibility::LegacyVersionless => {
                Self::validate_versionless_shape(path, conn)?;
                Self::set_wal_mode(conn)?;
                Self::run_current_plan_from(path, conn, MINIMUM_SUPPORTED_SCHEMA_VERSION)
            }
            SchemaCompatibility::Future { stored } => Err(Self::future_schema_error(path, stored)),
            SchemaCompatibility::UnrecognizedVersionless => {
                Err(Self::unrecognized_versionless_error(path))
            }
            SchemaCompatibility::Corrupt { reason } => {
                Err(Self::corrupt_schema_error(path, reason))
            }
        }
    }

    fn run_current_plan_from(
        path: &Path,
        conn: &mut sqlite::Connection,
        stored: i32,
    ) -> Result<(), String> {
        let plan = migrations::current_plan_from(stored).map_err(|e| e.to_string())?;
        migrations::run_with_db_path(conn, &plan, path.to_path_buf()).map_err(|e| e.to_string())
    }

    fn validate_versionless_shape(path: &Path, conn: &sqlite::Connection) -> Result<(), String> {
        if migrations::classify_versionless(conn)?.is_some() {
            Ok(())
        } else {
            Err(Self::unrecognized_versionless_error(path))
        }
    }

    fn future_schema_error(path: &Path, stored: i32) -> String {
        migrations::MigrationError::Incompatible {
            db_path: path.to_path_buf(),
            stored,
            current: CURRENT_SCHEMA_VERSION,
        }
        .to_string()
    }

    fn unrecognized_versionless_error(path: &Path) -> String {
        migrations::MigrationError::UnrecognizedShape {
            db_path: path.to_path_buf(),
        }
        .to_string()
    }

    fn corrupt_schema_error(path: &Path, reason: String) -> String {
        format!(
            "Corrupt schema ({reason}); run `agents migrate --rebuild`. db={}",
            path.display()
        )
    }

    fn apply_current_schema_repairs(
        conn: &mut sqlite::Connection,
        ran_open_migrations: bool,
    ) -> Result<(), String> {
        Self::validate_providers_schema(conn)?;
        Self::ensure_invocations_schema(conn)?;
        Self::ensure_providers_schema(conn)?;
        Self::ensure_provider_quotas_schema(conn)?;
        Self::ensure_provider_quotas_topology_schema(conn)?;
        Self::ensure_provider_quota_windows_schema(conn)?;
        Self::ensure_session_turns_schema(conn)?;
        if ran_open_migrations {
            Self::apply_returned_artifacts_schema(conn)?;
        }
        Ok(())
    }

    fn apply_returned_artifacts_schema(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch(invocation_returned_artifacts_schema_sql!())
            .map_err(|e| format!("Failed to ensure returned-artifacts schema: {e}"))
    }

    pub fn open_read_only(path: &Path) -> Result<Self, ReadOnlyOpenError> {
        Self::validate_read_only_paths(path)?;
        let conn = Self::open_read_only_connection(path)?;
        Self::probe_read_only_schema(path, &conn)?;

        Ok(Self {
            conn,
            db_path: path.to_path_buf(),
            lifecycle_sink: Mutex::new(Box::new(NoopLifecycleEventSink)),
        })
    }

    fn validate_read_only_paths(path: &Path) -> Result<(), ReadOnlyOpenError> {
        if !path.exists() {
            return Err(ReadOnlyOpenError::Missing {
                path: path.to_path_buf(),
            });
        }
        if path_is_unreadable(path) {
            return Err(ReadOnlyOpenError::PermissionDenied {
                path: path.to_path_buf(),
            });
        }
        Self::validate_read_only_sidecars(path)
    }

    fn validate_read_only_sidecars(path: &Path) -> Result<(), ReadOnlyOpenError> {
        for sidecar in [wal_path(path), shm_path(path)] {
            if sidecar.exists() && path_is_unreadable(&sidecar) {
                return Err(ReadOnlyOpenError::WalSidecarError {
                    path: path.to_path_buf(),
                    message: format!("SQLite sidecar is not readable: {}", sidecar.display()),
                });
            }
        }
        Ok(())
    }

    fn open_read_only_connection(path: &Path) -> Result<sqlite::Connection, ReadOnlyOpenError> {
        sqlite::Connection::open_with_flags(path, sqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|err| classify_read_only_open_error(path, err))
    }

    fn probe_read_only_schema(
        path: &Path,
        conn: &sqlite::Connection,
    ) -> Result<(), ReadOnlyOpenError> {
        conn.query_row("SELECT count(*) FROM sqlite_schema", [], |_row| Ok(()))
            .map_err(|err| classify_read_only_open_error(path, err))
    }

    pub fn open_default() -> Result<Self, String> {
        let db_path = Self::default_path()?;
        Self::open(&db_path)
    }

    pub fn open_for_memory(path: impl AsRef<Path>) -> Result<Self, String> {
        Self::open(path.as_ref())
    }

    pub fn default_path() -> Result<PathBuf, String> {
        Ok(crate::paths::data_dir()?.join("state.db"))
    }

    pub fn connection(&self) -> &sqlite::Connection {
        &self.conn
    }

    fn set_wal_mode(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(|e| format!("Failed to set WAL mode: {e}; run `agents migrate --rebuild`"))
    }

    pub fn with_write_txn<R, F>(&mut self, f: F) -> Result<R, String>
    where
        F: FnOnce(&mut Transaction<'_>) -> Result<R, String>,
    {
        let mut tx = self
            .conn
            .transaction()
            .map_err(|e| format!("Failed to begin state DB transaction: {e}"))?;
        match f(&mut tx) {
            Ok(value) => {
                tx.commit()
                    .map_err(|e| format!("Failed to commit state DB transaction: {e}"))?;
                Ok(value)
            }
            Err(err) => Err(err),
        }
    }

    fn strict_rfc3339_at(raw: &str, column_index: usize) -> sqlite::Result<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(raw)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| {
                sqlite::Error::FromSqlConversionFailure(
                    column_index,
                    sqlite::Type::Text,
                    Box::new(e),
                )
            })
    }

    fn strict_rfc3339_message(raw: &str, field_name: &str) -> Result<DateTime<Utc>, String> {
        DateTime::parse_from_rfc3339(raw)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| format!("Bad {field_name} {raw}: {e}"))
    }

    fn optional_forgiving_rfc3339(raw: Option<String>) -> Option<DateTime<Utc>> {
        raw.and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc))
    }

    fn fallback_now_rfc3339(raw: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(raw)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now())
    }

    fn table_column_names(
        conn: &sqlite::Connection,
        table_name: &str,
        inspect_context: &str,
        query_context: &str,
        read_context: &str,
    ) -> Result<Vec<String>, String> {
        let pragma = Self::pragma_table_info_sql(table_name);
        Self::query_table_column_names(conn, &pragma, inspect_context, query_context, read_context)
    }

    fn pragma_table_info_sql(table_name: &str) -> String {
        format!("PRAGMA table_info({table_name})")
    }

    fn query_table_column_names(
        conn: &sqlite::Connection,
        pragma: &str,
        inspect_context: &str,
        query_context: &str,
        read_context: &str,
    ) -> Result<Vec<String>, String> {
        let mut stmt = conn
            .prepare(pragma)
            .map_err(|e| Self::format_contextual_sqlite_error(inspect_context, e))?;
        let rows = stmt
            .query_map([], Self::column_name_row_mapper)
            .map_err(|e| Self::format_contextual_sqlite_error(query_context, e))?;
        Self::collect_table_column_rows(rows, read_context)
    }

    fn column_name_row_mapper(row: &sqlite::Row<'_>) -> sqlite::Result<String> {
        row.get::<_, String>(1)
    }

    fn collect_table_column_rows<I>(rows: I, read_context: &str) -> Result<Vec<String>, String>
    where
        I: IntoIterator<Item = sqlite::Result<String>>,
    {
        let mut columns = Vec::new();
        for row in rows {
            columns.push(row.map_err(|e| Self::format_contextual_sqlite_error(read_context, e))?);
        }
        Ok(columns)
    }

    fn format_contextual_sqlite_error(context: &str, err: sqlite::Error) -> String {
        format!("{context}: {err}")
    }

    fn has_column(columns: &[String], name: &str) -> bool {
        columns.iter().any(|column| column == name)
    }

    // Legacy repair allow-list only. Durable schema changes belong in
    // crates/oulipoly-state/migrations/ and schema.rs owns the version.
    fn ensure_invocations_schema(conn: &sqlite::Connection) -> Result<(), String> {
        let columns = Self::invocations_columns(conn)?;
        match Self::classify_invocations_schema(&columns) {
            InvocationsSchemaShape::Empty => Self::initialize_invocations_schema(conn),
            InvocationsSchemaShape::Current => {
                Self::repair_current_invocations_schema(conn, &columns)
            }
            InvocationsSchemaShape::LegacyPreUuid => Self::migrate_legacy_invocations(conn),
            InvocationsSchemaShape::UnrecognizedPreUuid(columns) => {
                Err(Self::unrecognized_invocations_shape_error(&columns))
            }
        }
    }

    fn classify_invocations_schema(columns: &[String]) -> InvocationsSchemaShape {
        if columns.is_empty() {
            InvocationsSchemaShape::Empty
        } else if Self::has_column(columns, "invocation_uuid") {
            InvocationsSchemaShape::Current
        } else if Self::legacy_invocations_shape_is_pre_uuid(columns) {
            InvocationsSchemaShape::LegacyPreUuid
        } else {
            InvocationsSchemaShape::UnrecognizedPreUuid(columns.to_vec())
        }
    }

    fn initialize_invocations_schema(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch(Self::invocations_schema_sql())
            .map_err(|e| format!("Failed to initialize invocations schema: {e}"))?;
        Self::ensure_invocations_row_version_support(conn)
    }

    fn repair_current_invocations_schema(
        conn: &sqlite::Connection,
        columns: &[String],
    ) -> Result<(), String> {
        Self::execute_column_repairs(conn, columns, Self::invocations_column_repairs().as_slice())?;
        let drop_repairs = [DropColumnRepair {
            column_name: "quota_tight_routing",
            sql: "ALTER TABLE invocations DROP COLUMN quota_tight_routing",
            error_context: "Failed to drop invocations.quota_tight_routing",
        }];
        Self::execute_drop_column_repairs(conn, columns, drop_repairs.as_slice())?;
        conn.execute_batch(Self::invocations_index_sql())
            .map_err(|e| format!("Failed to ensure invocation indexes: {e}"))?;
        Self::ensure_invocations_row_version_support(conn)
    }

    fn invocations_column_repairs() -> [ColumnRepair; 5] {
        [
            ColumnRepair {
                column_name: "session_id",
                sql: "ALTER TABLE invocations ADD COLUMN session_id TEXT",
                error_context: "Failed to add invocations.session_id",
            },
            ColumnRepair {
                column_name: "session_capture_method",
                sql: "ALTER TABLE invocations ADD COLUMN session_capture_method TEXT",
                error_context: "Failed to add invocations.session_capture_method",
            },
            ColumnRepair {
                column_name: "resume_acceptance_status",
                sql: "ALTER TABLE invocations ADD COLUMN resume_acceptance_status TEXT",
                error_context: "Failed to add invocations.resume_acceptance_status",
            },
            ColumnRepair {
                column_name: "resume_acceptance_evidence",
                sql: "ALTER TABLE invocations ADD COLUMN resume_acceptance_evidence TEXT",
                error_context: "Failed to add invocations.resume_acceptance_evidence",
            },
            ColumnRepair {
                column_name: "terminal_reason",
                sql: "ALTER TABLE invocations ADD COLUMN terminal_reason TEXT",
                error_context: "Failed to add invocations.terminal_reason",
            },
        ]
    }

    fn unrecognized_invocations_shape_error(columns: &[String]) -> String {
        format!(
            "Refusing to rebuild populated invocations table with unrecognized pre-UUID shape: {columns:?}"
        )
    }

    fn normalize_invocations_columns_excluding_maintenance(columns: &[String]) -> Vec<String> {
        let mut names = Self::invocation_columns_without_maintenance(columns);
        names.sort();
        names
    }

    fn invocation_columns_without_maintenance(columns: &[String]) -> Vec<String> {
        columns
            .iter()
            .filter(|column| {
                !matches!(
                    column.as_str(),
                    "row_version" | "provider_session_resolved_account"
                )
            })
            .cloned()
            .collect()
    }

    fn legacy_invocations_shape_is_pre_uuid(columns: &[String]) -> bool {
        Self::normalize_invocations_columns_excluding_maintenance(columns)
            == [
                "created_at",
                "error_category",
                "exit_code",
                "id",
                "model_name",
                "provider_index",
                "success",
            ]
    }

    fn ensure_invocations_row_version_support(conn: &sqlite::Connection) -> Result<(), String> {
        let columns = Self::invocations_columns(conn)?;
        Self::repair_invocations_row_version_column(conn, &columns)?;
        Self::install_invocations_row_version_triggers(conn)
    }

    fn repair_invocations_row_version_column(
        conn: &sqlite::Connection,
        columns: &[String],
    ) -> Result<(), String> {
        if Self::has_column(columns, "row_version") {
            return Ok(());
        }
        conn.execute(
            "ALTER TABLE invocations ADD COLUMN row_version INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .map_err(|e| format!("Failed to add invocations.row_version during repair: {e}"))?;
        Ok(())
    }

    fn install_invocations_row_version_triggers(conn: &sqlite::Connection) -> Result<(), String> {
        let registration = Self::invocations_row_version_registration()?;
        let trigger_sql = Self::row_version_trigger_sql(registration);
        conn.execute_batch(&trigger_sql)
            .map_err(|e| format!("Failed to install invocation row-version triggers: {e}"))
    }

    fn invocations_row_version_registration()
    -> Result<&'static crate::deployment::row_version::registry::TableRegistration, String> {
        crate::deployment::row_version::registry::lookup("invocations").ok_or_else(|| {
            "Missing row-version registry entry for invocations during repair".to_string()
        })
    }

    fn row_version_trigger_sql(
        registration: &crate::deployment::row_version::registry::TableRegistration,
    ) -> String {
        crate::deployment::row_version::triggers_sql::generate_triggers_for_table(registration)
    }

    fn invocations_columns(conn: &sqlite::Connection) -> Result<Vec<String>, String> {
        Self::table_column_names(
            conn,
            "invocations",
            "Failed to inspect invocations schema",
            "Failed to inspect invocations columns",
            "Failed to read invocations column",
        )
    }

    fn invocations_have_dual_id_columns(conn: &sqlite::Connection) -> Result<bool, String> {
        let columns = Self::invocations_columns(conn)?;
        Ok(Self::columns_have_dual_id_columns(&columns))
    }

    fn invocations_have_resolved_account_column(conn: &sqlite::Connection) -> Result<bool, String> {
        let columns = Self::invocations_columns(conn)?;
        Ok(columns
            .iter()
            .any(|column| column == "provider_session_resolved_account"))
    }

    fn columns_have_dual_id_columns(columns: &[String]) -> bool {
        Self::has_column(columns, "provider_session_id")
            && Self::has_column(columns, "resume_input_id")
            && Self::has_column(columns, "provider_session_capture_method")
    }

    fn promote_existing_dual_id_schema5_if_present(
        conn: &mut sqlite::Connection,
        stored: i32,
    ) -> Result<i32, String> {
        if stored >= 5 {
            return Ok(stored);
        }
        let columns = Self::invocations_columns(conn)?;
        if !Self::columns_have_dual_id_columns(&columns) {
            return Ok(stored);
        }
        Self::promote_existing_dual_id_schema5(conn)?;
        Ok(5)
    }

    fn promote_existing_dual_id_schema5(conn: &mut sqlite::Connection) -> Result<(), String> {
        conn.execute_batch(
            "BEGIN IMMEDIATE;
             UPDATE invocations
             SET provider_session_id = COALESCE(provider_session_id, session_id),
                 provider_session_capture_method = COALESCE(provider_session_capture_method, session_capture_method)
             WHERE session_id IS NOT NULL
               AND (session_capture_method IS NULL OR session_capture_method <> 'resumed');

             UPDATE invocations
             SET resume_input_id = COALESCE(resume_input_id, session_id)
             WHERE session_id IS NOT NULL
               AND session_capture_method = 'resumed';

             CREATE INDEX IF NOT EXISTS idx_invocations_provider_provider_session
               ON invocations(provider_name, provider_index, provider_session_id)
               WHERE provider_session_id IS NOT NULL;

             PRAGMA user_version = 5;
             COMMIT;",
        )
        .map_err(|e| {
            let _ = conn.execute_batch("ROLLBACK;");
            format!("Failed to promote existing dual-id invocation schema to version 5: {e}")
        })
    }

    fn provider_session_expr(
        conn: &sqlite::Connection,
        alias: Option<&str>,
    ) -> Result<String, String> {
        let projection = if Self::invocations_have_dual_id_columns(conn)? {
            ProviderSessionProjection::DualId
        } else {
            ProviderSessionProjection::LegacySessionId
        };
        Ok(Self::format_provider_session_expr(projection, alias))
    }

    fn format_provider_session_expr(
        projection: ProviderSessionProjection,
        alias: Option<&str>,
    ) -> String {
        let prefix = alias.unwrap_or_default();
        match projection {
            ProviderSessionProjection::DualId => {
                format!("COALESCE({prefix}provider_session_id, {prefix}session_id)")
            }
            ProviderSessionProjection::LegacySessionId => format!("{prefix}session_id"),
        }
    }

    fn invocation_record_select_sql(
        conn: &sqlite::Connection,
        tail_sql: &str,
    ) -> Result<String, String> {
        let projection = Self::invocation_dual_id_projection(conn)?;
        Ok(Self::format_invocation_record_select_sql(
            projection, tail_sql,
        ))
    }

    fn invocation_dual_id_projection(
        conn: &sqlite::Connection,
    ) -> Result<InvocationDualIdProjection, String> {
        if Self::invocations_have_dual_id_columns(conn)? {
            if Self::invocations_have_resolved_account_column(conn)? {
                Ok(InvocationDualIdProjection::Current)
            } else {
                Ok(InvocationDualIdProjection::CurrentWithoutResolvedAccount)
            }
        } else {
            Ok(InvocationDualIdProjection::Legacy)
        }
    }

    fn format_invocation_record_select_sql(
        projection: InvocationDualIdProjection,
        tail_sql: &str,
    ) -> String {
        let (
            provider_session_id,
            resume_input_id,
            provider_session_capture_method,
            provider_session_resolved_account,
        ) = projection.select_columns();
        format!(
            "SELECT id, invocation_uuid, model_name, provider_name, provider_index,
                    parent_invocation_id, status, success, exit_code, error_category,
                    terminal_reason, session_id, session_capture_method,
                    {provider_session_id}, {resume_input_id}, {provider_session_capture_method},
                    {provider_session_resolved_account},
                    resume_acceptance_status, resume_acceptance_evidence,
                    created_at, finished_at
             FROM invocations
             {tail_sql}"
        )
    }

    fn ensure_providers_schema(conn: &mut sqlite::Connection) -> Result<(), String> {
        let columns = Self::providers_columns(conn)?;
        match Self::classify_providers_schema(&columns) {
            ProvidersSchemaShape::Empty => Self::initialize_providers_schema(conn),
            ProvidersSchemaShape::Current => Ok(()),
            ProvidersSchemaShape::LegacyIndexKeyed => Self::migrate_legacy_providers_schema(conn),
            ProvidersSchemaShape::Unexpected(description) => {
                Err(Self::unexpected_providers_schema_error(&description))
            }
        }
    }

    fn classify_providers_schema(columns: &[ProviderColumn]) -> ProvidersSchemaShape {
        if columns.is_empty() {
            ProvidersSchemaShape::Empty
        } else if Self::providers_shape_is_post_fix(columns) {
            ProvidersSchemaShape::Current
        } else if Self::providers_shape_is_pre_fix(columns) {
            ProvidersSchemaShape::LegacyIndexKeyed
        } else {
            ProvidersSchemaShape::Unexpected(Self::describe_columns(columns))
        }
    }

    fn initialize_providers_schema(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch(Self::providers_schema_sql())
            .map_err(|e| format!("Failed to initialize providers schema: {e}"))
    }

    fn migrate_legacy_providers_schema(conn: &mut sqlite::Connection) -> Result<(), String> {
        let tx = conn
            .transaction()
            .map_err(|e| format!("Failed to begin providers migration: {e}"))?;
        Self::rename_legacy_providers_table(&tx)?;
        Self::create_migrated_providers_table(&tx)?;
        Self::rebuild_providers_aggregate(&tx)?;
        Self::rebuild_provider_error_metadata(&tx)?;
        Self::drop_legacy_providers_table(&tx)?;
        tx.commit()
            .map_err(|e| format!("Failed to commit providers migration: {e}"))
    }

    fn rename_legacy_providers_table(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch("ALTER TABLE providers RENAME TO providers_legacy_index_keyed;")
            .map_err(|e| format!("Failed to rename legacy providers table: {e}"))
    }

    fn create_migrated_providers_table(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch(Self::providers_schema_sql())
            .map_err(|e| format!("Failed to create migrated providers table: {e}"))
    }

    fn rebuild_providers_aggregate(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch(
            "INSERT INTO providers (
                model_name, provider_name,
                invocation_count, error_count,
                last_error, last_error_at, last_invoked_at
            )
            SELECT
                model_name,
                provider_name,
                COUNT(*) AS invocation_count,
                SUM(CASE WHEN success = 0 THEN 1 ELSE 0 END) AS error_count,
                NULL AS last_error,
                NULL AS last_error_at,
                MAX(finished_at) AS last_invoked_at
            FROM invocations
            WHERE provider_name IS NOT NULL
              AND status IN ('succeeded', 'failed')
              AND success IS NOT NULL
            GROUP BY model_name, provider_name;",
        )
        .map_err(|e| format!("Failed to rebuild providers aggregate: {e}"))
    }

    fn rebuild_provider_error_metadata(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch(
            "UPDATE providers
                SET last_error_at = (
                        SELECT i.finished_at
                          FROM invocations i
                         WHERE i.model_name = providers.model_name
                           AND i.provider_name = providers.provider_name
                           AND i.success = 0
                         ORDER BY i.finished_at DESC, i.id DESC
                         LIMIT 1
                    ),
                    last_error = (
                        SELECT i.error_category
                          FROM invocations i
                         WHERE i.model_name = providers.model_name
                           AND i.provider_name = providers.provider_name
                           AND i.success = 0
                         ORDER BY i.finished_at DESC, i.id DESC
                         LIMIT 1
                    )
              WHERE EXISTS (
                        SELECT 1
                          FROM invocations i
                         WHERE i.model_name = providers.model_name
                           AND i.provider_name = providers.provider_name
                           AND i.success = 0
                    );",
        )
        .map_err(|e| format!("Failed to rebuild provider error metadata: {e}"))
    }

    fn drop_legacy_providers_table(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch("DROP TABLE providers_legacy_index_keyed;")
            .map_err(|e| format!("Failed to drop legacy providers table: {e}"))
    }

    fn unexpected_providers_schema_error(description: &str) -> String {
        format!("Unexpected providers schema shape: {description}")
    }

    fn validate_providers_schema(conn: &sqlite::Connection) -> Result<(), String> {
        match Self::providers_object_type(conn)? {
            None => return Ok(()),
            Some(object_type) if object_type != "table" => {
                return Err(format!(
                    "Unexpected providers schema shape: object type={object_type}"
                ));
            }
            _ => {}
        }

        if Self::providers_has_foreign_keys(conn)? {
            return Err(
                "Unexpected providers schema shape: foreign-key constraints present".to_string(),
            );
        }

        let columns = Self::providers_columns(conn)?;
        if columns.is_empty()
            || Self::providers_shape_is_post_fix(&columns)
            || Self::providers_shape_is_pre_fix(&columns)
        {
            return Ok(());
        }

        Err(format!(
            "Unexpected providers schema shape: {}",
            Self::describe_columns(&columns)
        ))
    }

    fn providers_object_type(conn: &sqlite::Connection) -> Result<Option<String>, String> {
        conn.query_row(
            "SELECT type FROM sqlite_master WHERE name = 'providers'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| format!("Failed to inspect providers object type: {e}"))
    }

    fn providers_has_foreign_keys(conn: &sqlite::Connection) -> Result<bool, String> {
        let mut stmt = conn
            .prepare("PRAGMA foreign_key_list(providers)")
            .map_err(|e| format!("Failed to inspect providers foreign keys: {e}"))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| format!("Failed to inspect providers foreign keys: {e}"))?;
        Ok(rows
            .next()
            .map_err(|e| format!("Failed to read providers foreign keys: {e}"))?
            .is_some())
    }

    fn providers_columns(conn: &sqlite::Connection) -> Result<Vec<ProviderColumn>, String> {
        Self::query_provider_columns(conn)
    }

    fn query_provider_columns(conn: &sqlite::Connection) -> Result<Vec<ProviderColumn>, String> {
        let mut stmt = conn
            .prepare("PRAGMA table_info(providers)")
            .map_err(|e| Self::format_provider_column_error("inspect providers schema", e))?;
        let rows = stmt
            .query_map([], Self::provider_column_row_mapper)
            .map_err(|e| Self::format_provider_column_error("inspect providers columns", e))?;
        Self::collect_provider_columns(rows)
    }

    fn provider_column_row_mapper(row: &sqlite::Row<'_>) -> sqlite::Result<ProviderColumn> {
        Ok(ProviderColumn {
            name: row.get(1)?,
            data_type: row.get(2)?,
            notnull: row.get(3)?,
            pk: row.get(5)?,
        })
    }

    fn collect_provider_columns<I>(rows: I) -> Result<Vec<ProviderColumn>, String>
    where
        I: IntoIterator<Item = sqlite::Result<ProviderColumn>>,
    {
        let mut columns = Vec::new();
        for row in rows {
            columns.push(
                row.map_err(|e| Self::format_provider_column_error("read providers column", e))?,
            );
        }
        Ok(columns)
    }

    fn format_provider_column_error(operation: &str, err: sqlite::Error) -> String {
        format!("Failed to {operation}: {err}")
    }

    fn providers_shape_is_post_fix(columns: &[ProviderColumn]) -> bool {
        Self::columns_match_allowing_row_version(
            columns,
            &[
                ("model_name", "TEXT", 1, 1),
                ("provider_name", "TEXT", 1, 2),
                ("invocation_count", "INTEGER", 1, 0),
                ("error_count", "INTEGER", 1, 0),
                ("last_error", "TEXT", 0, 0),
                ("last_error_at", "TEXT", 0, 0),
                ("last_invoked_at", "TEXT", 0, 0),
            ],
        )
    }

    fn providers_shape_is_pre_fix(columns: &[ProviderColumn]) -> bool {
        Self::columns_match_allowing_row_version(
            columns,
            &[
                ("model_name", "TEXT", 1, 1),
                ("provider_index", "INTEGER", 1, 2),
                ("invocation_count", "INTEGER", 1, 0),
                ("error_count", "INTEGER", 1, 0),
                ("last_error", "TEXT", 0, 0),
                ("last_error_at", "TEXT", 0, 0),
                ("last_invoked_at", "TEXT", 0, 0),
            ],
        )
    }

    fn columns_match_allowing_row_version(
        columns: &[ProviderColumn],
        expected: &[(&str, &str, i64, i64)],
    ) -> bool {
        Self::columns_match(columns, expected)
            || columns.len() == expected.len() + 1
                && Self::columns_match(&columns[..expected.len()], expected)
                && Self::column_matches(&columns[expected.len()], "row_version", "INTEGER", 1, 0)
    }

    fn columns_match(columns: &[ProviderColumn], expected: &[(&str, &str, i64, i64)]) -> bool {
        columns.len() == expected.len()
            && columns.iter().zip(expected.iter()).all(
                |(column, (expected_name, expected_type, expected_notnull, expected_pk))| {
                    Self::column_matches(
                        column,
                        expected_name,
                        expected_type,
                        *expected_notnull,
                        *expected_pk,
                    )
                },
            )
    }

    fn column_matches(
        column: &ProviderColumn,
        expected_name: &str,
        expected_type: &str,
        expected_notnull: i64,
        expected_pk: i64,
    ) -> bool {
        column.name == expected_name
            && column.data_type.eq_ignore_ascii_case(expected_type)
            && column.notnull == expected_notnull
            && column.pk == expected_pk
    }

    fn describe_columns(columns: &[ProviderColumn]) -> String {
        Self::provider_column_descriptions(columns).join(", ")
    }

    fn provider_column_descriptions(columns: &[ProviderColumn]) -> Vec<String> {
        columns
            .iter()
            .map(Self::provider_column_description)
            .collect::<Vec<_>>()
    }

    fn provider_column_description(column: &ProviderColumn) -> String {
        format!(
            "{}(type={}, notnull={}, pk={})",
            column.name, column.data_type, column.notnull, column.pk
        )
    }

    fn ensure_session_turns_schema(conn: &sqlite::Connection) -> Result<(), String> {
        let columns = Self::session_turns_columns(conn)?;
        Self::execute_column_repairs(
            conn,
            &columns,
            Self::session_turns_column_repairs().as_slice(),
        )?;
        conn.execute_batch(Self::session_turns_index_sql())
            .map_err(|e| format!("Failed to ensure session_turns indexes: {e}"))?;
        Ok(())
    }

    fn session_turns_column_repairs() -> [ColumnRepair; 4] {
        [
            ColumnRepair {
                column_name: "parent_turn_id",
                sql: "ALTER TABLE session_turns ADD COLUMN parent_turn_id TEXT",
                error_context: "Failed to add session_turns.parent_turn_id",
            },
            ColumnRepair {
                column_name: "is_sidechain",
                sql: "ALTER TABLE session_turns ADD COLUMN is_sidechain INTEGER NOT NULL DEFAULT 0",
                error_context: "Failed to add session_turns.is_sidechain",
            },
            ColumnRepair {
                column_name: "is_compaction_boundary",
                sql: "ALTER TABLE session_turns ADD COLUMN is_compaction_boundary INTEGER NOT NULL DEFAULT 0",
                error_context: "Failed to add session_turns.is_compaction_boundary",
            },
            ColumnRepair {
                column_name: "body",
                sql: "ALTER TABLE session_turns ADD COLUMN body TEXT",
                error_context: "Failed to add session_turns.body",
            },
        ]
    }

    fn session_turns_columns(conn: &sqlite::Connection) -> Result<Vec<String>, String> {
        Self::table_column_names(
            conn,
            "session_turns",
            "Failed to inspect session_turns schema",
            "Failed to inspect session_turns columns",
            "Failed to read session_turns column",
        )
    }

    fn execute_column_repairs(
        conn: &sqlite::Connection,
        columns: &[String],
        repairs: &[ColumnRepair],
    ) -> Result<(), String> {
        for repair in repairs {
            Self::execute_column_repair_if_absent(conn, columns, repair)?;
        }
        Ok(())
    }

    fn execute_column_repair_if_absent(
        conn: &sqlite::Connection,
        columns: &[String],
        repair: &ColumnRepair,
    ) -> Result<(), String> {
        if Self::has_column(columns, repair.column_name) {
            return Ok(());
        }
        conn.execute(repair.sql, [])
            .map_err(|e| format!("{}: {e}", repair.error_context))?;
        Ok(())
    }

    fn execute_drop_column_repairs(
        conn: &sqlite::Connection,
        columns: &[String],
        repairs: &[DropColumnRepair],
    ) -> Result<(), String> {
        for repair in repairs {
            Self::execute_drop_column_repair_if_present(conn, columns, repair)?;
        }
        Ok(())
    }

    fn execute_drop_column_repair_if_present(
        conn: &sqlite::Connection,
        columns: &[String],
        repair: &DropColumnRepair,
    ) -> Result<(), String> {
        if !Self::has_column(columns, repair.column_name) {
            return Ok(());
        }
        conn.execute(repair.sql, [])
            .map_err(|e| format!("{}: {e}", repair.error_context))?;
        Ok(())
    }

    fn providers_schema_sql() -> &'static str {
        "CREATE TABLE IF NOT EXISTS providers (
            model_name TEXT NOT NULL,
            provider_name TEXT NOT NULL,
            invocation_count INTEGER NOT NULL DEFAULT 0,
            error_count INTEGER NOT NULL DEFAULT 0,
            last_error TEXT,
            last_error_at TEXT,
            last_invoked_at TEXT,
            PRIMARY KEY (model_name, provider_name)
        );"
    }

    fn invocations_schema_sql() -> &'static str {
        concat!(
            "CREATE TABLE IF NOT EXISTS invocations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            invocation_uuid TEXT NOT NULL UNIQUE,
            model_name TEXT NOT NULL,
            provider_name TEXT,
            provider_index INTEGER NOT NULL,
            parent_invocation_id INTEGER REFERENCES invocations(id),
            status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'legacy')),
            success INTEGER,
            exit_code INTEGER,
            error_category TEXT,
            terminal_reason TEXT,
            session_id TEXT,
            session_capture_method TEXT,
            provider_session_id TEXT,
            resume_input_id TEXT,
            provider_session_capture_method TEXT,
            provider_session_resolved_account TEXT,
            resume_acceptance_status TEXT,
            resume_acceptance_evidence TEXT,
            created_at TEXT NOT NULL,
            finished_at TEXT,
            row_version INTEGER NOT NULL DEFAULT 0
        );

        CREATE INDEX IF NOT EXISTS idx_invocations_uuid
            ON invocations (invocation_uuid);
        CREATE INDEX IF NOT EXISTS idx_invocations_parent
            ON invocations (parent_invocation_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_invocations_provider_created
            ON invocations (provider_name, created_at);
        CREATE INDEX IF NOT EXISTS idx_invocations_provider_session
            ON invocations (provider_name, session_id)
            WHERE session_id IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_invocations_provider_provider_session
            ON invocations (provider_name, provider_index, provider_session_id)
            WHERE provider_session_id IS NOT NULL;",
            invocation_returned_artifacts_schema_sql!()
        )
    }

    fn invocations_index_sql() -> &'static str {
        "CREATE INDEX IF NOT EXISTS idx_invocations_uuid
            ON invocations (invocation_uuid);
        CREATE INDEX IF NOT EXISTS idx_invocations_parent
            ON invocations (parent_invocation_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_invocations_provider_created
            ON invocations (provider_name, created_at);
        CREATE INDEX IF NOT EXISTS idx_invocations_provider_session
            ON invocations (provider_name, session_id)
            WHERE session_id IS NOT NULL;"
    }

    fn session_turns_index_sql() -> &'static str {
        "CREATE INDEX IF NOT EXISTS idx_session_turns_provider_ts
            ON session_turns (provider_name, role, timestamp);
        CREATE INDEX IF NOT EXISTS idx_session_turns_session_ts
            ON session_turns (provider_name, session_id, timestamp);
        CREATE INDEX IF NOT EXISTS idx_session_turns_session_lookup
            ON session_turns (session_id, timestamp);
        CREATE INDEX IF NOT EXISTS idx_session_turns_parent
            ON session_turns (provider_name, session_id, parent_turn_id, timestamp);"
    }

    fn migrate_legacy_invocations(conn: &sqlite::Connection) -> Result<(), String> {
        let provider_names = Self::provider_name_lookup()?;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to begin invocation migration: {e}"))?;
        Self::validate_providers_schema(&tx)?;
        // Guardrail order: SELECT COUNT(*) FROM invocations
        let old_count = Self::legacy_invocations_count(&tx)?;
        let old_rows = Self::load_legacy_invocation_rows(&tx)?;
        // Guardrail order: scanned {} rows but table count was {old_count}
        Self::validate_legacy_invocation_scan_count(old_rows.len(), old_count)?;
        // Guardrail order: CREATE TABLE invocations_new
        Self::create_migrated_invocations_table(&tx)?;
        Self::insert_migrated_invocation_rows(&tx, old_rows, &provider_names)?;
        // Guardrail order: SELECT COUNT(*) FROM invocations_new
        let new_count = Self::migrated_invocations_count(&tx)?;
        // Guardrail order: migrated {new_count} rows from {old_count}
        Self::validate_migrated_invocation_count(new_count, old_count)?;
        // Guardrail order: DROP TABLE invocations;
        Self::replace_invocations_with_migrated_table(&tx)?;
        tx.execute_batch(Self::invocations_index_sql())
            .map_err(|e| format!("Failed to create migrated invocation indexes: {e}"))?;
        Self::ensure_invocations_row_version_support(&tx)?;

        tx.commit()
            .map_err(|e| format!("Failed to commit invocation migration: {e}"))
    }

    fn legacy_invocations_count(conn: &sqlite::Connection) -> Result<i64, String> {
        conn.query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
            .map_err(|e| format!("Failed to count legacy invocations before rebuild: {e}"))
    }

    fn load_legacy_invocation_rows(
        conn: &sqlite::Connection,
    ) -> Result<Vec<LegacyInvocationRow>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT model_name, provider_index, success, exit_code, error_category, created_at
                 FROM invocations
                 ORDER BY id",
            )
            .map_err(|e| format!("Failed to read legacy invocations: {e}"))?;
        let rows = stmt
            .query_map([], Self::map_legacy_invocation_row)
            .map_err(|e| format!("Failed to scan legacy invocations: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to parse legacy invocation: {e}"))
    }

    fn map_legacy_invocation_row(row: &sqlite::Row<'_>) -> sqlite::Result<LegacyInvocationRow> {
        Ok(LegacyInvocationRow {
            model_name: row.get(0)?,
            provider_index: row.get(1)?,
            success: row.get(2)?,
            exit_code: row.get(3)?,
            error_category: row.get(4)?,
            created_at: row.get(5)?,
        })
    }

    fn validate_legacy_invocation_scan_count(scanned: usize, old_count: i64) -> Result<(), String> {
        if scanned as i64 == old_count {
            Ok(())
        } else {
            Err(format!(
                "Legacy invocation rebuild aborted before replacement: scanned {scanned} rows but table count was {old_count}"
            ))
        }
    }

    fn create_migrated_invocations_table(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch(
            "CREATE TABLE invocations_new (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                invocation_uuid TEXT NOT NULL UNIQUE,
                model_name TEXT NOT NULL,
                provider_name TEXT,
                provider_index INTEGER NOT NULL,
                parent_invocation_id INTEGER REFERENCES invocations_new(id),
                status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'legacy')),
                success INTEGER,
                exit_code INTEGER,
                error_category TEXT,
                terminal_reason TEXT,
                session_id TEXT,
                session_capture_method TEXT,
                provider_session_id TEXT,
                resume_input_id TEXT,
                provider_session_capture_method TEXT,
                provider_session_resolved_account TEXT,
                resume_acceptance_status TEXT,
                resume_acceptance_evidence TEXT,
                created_at TEXT NOT NULL,
                finished_at TEXT,
                row_version INTEGER NOT NULL DEFAULT 0
            );",
        )
        .map_err(|e| format!("Failed to create migrated invocations table: {e}"))
    }

    fn insert_migrated_invocation_rows(
        conn: &sqlite::Connection,
        rows: Vec<LegacyInvocationRow>,
        provider_names: &HashMap<(String, usize), String>,
    ) -> Result<(), String> {
        let mut insert = conn
            .prepare(Self::migrated_invocation_insert_sql())
            .map_err(|e| format!("Failed to prepare migrated invocation insert: {e}"))?;
        for row in rows {
            let migrated = Self::map_legacy_invocation_insert(row, provider_names);
            insert
                .execute(sqlite::params![
                    migrated.invocation_uuid,
                    migrated.model_name,
                    migrated.provider_name,
                    migrated.provider_index,
                    migrated.status.as_str(),
                    migrated.success,
                    migrated.exit_code,
                    migrated.error_category,
                    migrated.created_at,
                ])
                .map_err(|e| format!("Failed to copy legacy invocation: {e}"))?;
        }
        Ok(())
    }

    fn migrated_invocation_insert_sql() -> &'static str {
        "INSERT INTO invocations_new (
            invocation_uuid,
            model_name,
            provider_name,
            provider_index,
            parent_invocation_id,
            status,
            success,
            exit_code,
            error_category,
            terminal_reason,
            session_id,
            session_capture_method,
            provider_session_id,
            resume_input_id,
            provider_session_capture_method,
            provider_session_resolved_account,
            resume_acceptance_status,
            resume_acceptance_evidence,
            created_at,
            finished_at
         ) VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?7, ?8, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, ?9, ?9)"
    }

    fn map_legacy_invocation_insert(
        row: LegacyInvocationRow,
        provider_names: &HashMap<(String, usize), String>,
    ) -> LegacyInvocationInsert {
        let provider_name = provider_names
            .get(&(row.model_name.clone(), row.provider_index as usize))
            .cloned();
        let status = Self::legacy_invocation_status(provider_name.as_ref(), row.success);
        LegacyInvocationInsert {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: row.model_name,
            provider_name,
            provider_index: row.provider_index,
            status,
            success: row.success,
            exit_code: row.exit_code,
            error_category: row.error_category,
            created_at: row.created_at,
        }
    }

    fn legacy_invocation_status(provider_name: Option<&String>, success: i64) -> InvocationStatus {
        match provider_name {
            Some(_) if success != 0 => InvocationStatus::Succeeded,
            Some(_) => InvocationStatus::Failed,
            None => InvocationStatus::Legacy,
        }
    }

    fn migrated_invocations_count(conn: &sqlite::Connection) -> Result<i64, String> {
        conn.query_row("SELECT COUNT(*) FROM invocations_new", [], |row| row.get(0))
            .map_err(|e| format!("Failed to count migrated invocations before replacement: {e}"))
    }

    fn validate_migrated_invocation_count(new_count: i64, old_count: i64) -> Result<(), String> {
        if new_count == old_count {
            Ok(())
        } else {
            Err(format!(
                "Legacy invocation rebuild aborted before replacement: migrated {new_count} rows from {old_count}"
            ))
        }
    }

    fn replace_invocations_with_migrated_table(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch(
            "DROP TABLE invocations;
             ALTER TABLE invocations_new RENAME TO invocations;",
        )
        .map_err(|e| format!("Failed to replace invocations table: {e}"))
    }

    /// Resolve `(model_name, provider_index) -> provider_name` from the
    /// installed models config, used by the legacy-row migration. A corrupt
    /// or missing models directory must not block DB open: log on stderr and
    /// return an empty lookup so unmappable rows fall through to
    /// `status='legacy'` with `provider_name=NULL` (per V10 — degradation
    /// is observable via the legacy status, not silent).
    fn provider_name_lookup() -> Result<std::collections::HashMap<(String, usize), String>, String>
    {
        let models = Self::load_models_for_invocation_migration()?;
        Ok(Self::build_provider_name_lookup(models))
    }

    fn load_models_for_invocation_migration() -> Result<ModelStore, String> {
        let models_dir = Self::migration_models_dir();
        match load_models(&models_dir, None) {
            Ok(models) => Ok(models),
            Err(e) => {
                Self::warn_model_config_load_failed(&e.to_string());
                Ok(HashMap::new())
            }
        }
    }

    fn migration_models_dir() -> PathBuf {
        dirs::config_dir()
            .map(|dir| dir.join("oulipoly-agent-runner").join("models"))
            .unwrap_or_else(|| PathBuf::from("models"))
    }

    fn warn_model_config_load_failed(error: &str) {
        eprintln!(
            "Warning: failed to load models config during invocation migration ({error}); \
             pre-existing invocation rows will migrate as status='legacy'."
        );
    }

    fn build_provider_name_lookup(
        models: ModelStore,
    ) -> std::collections::HashMap<(String, usize), String> {
        let mut lookup = std::collections::HashMap::new();
        for (model_name, model) in models {
            for (provider_index, provider) in model.providers.iter().enumerate() {
                lookup.insert((model_name.clone(), provider_index), provider.name.clone());
            }
        }
        lookup
    }

    fn invocations_dir(&self) -> Option<PathBuf> {
        if self.db_path == Path::new(":memory:") {
            return None;
        }
        let parent = self
            .db_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Some(parent.join("invocations"))
    }

    fn write_invocation_artifact(
        &self,
        start: &InvocationStart,
        started_at: &str,
    ) -> Result<(), String> {
        let Some(dir) = self.invocations_dir() else {
            return Ok(());
        };
        Self::ensure_artifact_dir(&dir)?;
        let bytes = Self::invocation_artifact_bytes(start, started_at)?;
        let (tmp_path, final_path) =
            Self::artifact_paths(&dir, &start.invocation_uuid, "invocation");
        Self::write_artifact_atomically(&tmp_path, &final_path, &bytes)
    }

    fn ensure_artifact_dir(dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("create_dir_all({}): {e}", dir.display()))
    }

    fn invocation_artifact_bytes(
        start: &InvocationStart,
        started_at: &str,
    ) -> Result<Vec<u8>, String> {
        let payload = Self::invocation_artifact_payload(start, started_at);
        serde_json::to_vec(&payload).map_err(|e| format!("serialize invocation artifact: {e}"))
    }

    fn invocation_artifact_payload(start: &InvocationStart, started_at: &str) -> serde_json::Value {
        serde_json::json!({
            "id": start.invocation_uuid,
            "status": "running",
            "pid": std::process::id(),
            "started_at": started_at,
            "model_name": start.model_name,
            "provider_name": start.provider_name,
        })
    }

    fn artifact_paths(dir: &Path, uuid: &str, extension: &str) -> (PathBuf, PathBuf) {
        (
            dir.join(format!("{uuid}.{extension}.tmp")),
            dir.join(format!("{uuid}.{extension}")),
        )
    }

    fn write_artifact_atomically(
        tmp_path: &Path,
        final_path: &Path,
        bytes: &[u8],
    ) -> Result<(), String> {
        std::fs::write(tmp_path, bytes)
            .map_err(|e| format!("write({}): {e}", tmp_path.display()))?;
        std::fs::rename(tmp_path, final_path).map_err(|e| {
            format!(
                "rename({} -> {}): {e}",
                tmp_path.display(),
                final_path.display()
            )
        })
    }

    fn write_result_artifact(&self, input: ResultEnvelopeInput<'_>) -> Result<(), String> {
        let Some(dir) = self.invocations_dir() else {
            return Ok(());
        };
        Self::ensure_artifact_dir(&dir)?;
        let bytes = Self::result_artifact_bytes(input)?;
        let (tmp_path, final_path) = Self::artifact_paths(&dir, input.id, "result");
        Self::write_artifact_atomically(&tmp_path, &final_path, &bytes)
    }

    fn result_artifact_bytes(input: ResultEnvelopeInput<'_>) -> Result<Vec<u8>, String> {
        let payload = Self::result_artifact_payload(input);
        serde_json::to_vec(&payload).map_err(|e| format!("serialize result artifact: {e}"))
    }

    fn result_artifact_payload(input: ResultEnvelopeInput<'_>) -> serde_json::Value {
        result_envelope_payload(input)
    }

    fn lifecycle_context(&self, start: &InvocationStart) -> lc_log_adapter::StartContext {
        let parent_invocation_uuid = self.load_parent_invocation_uuid(start.parent_invocation_id);
        Self::build_start_context(start, parent_invocation_uuid)
    }

    fn load_parent_invocation_uuid(&self, parent_id: Option<i64>) -> Option<String> {
        let parent_id = parent_id?;
        self.conn
            .query_row(
                "SELECT invocation_uuid FROM invocations WHERE id = ?1",
                sqlite::params![parent_id],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    fn build_start_context(
        start: &InvocationStart,
        parent_invocation_uuid: Option<String>,
    ) -> lc_log_adapter::StartContext {
        lc_log_adapter::StartContext {
            invocation_uuid: start.invocation_uuid.clone(),
            provider_source: Some(start.provider_name.clone()),
            chain_id: None,
            session_id: None,
            latency_us: 0,
            model: Some(start.model_name.clone()),
            provider: Some(start.provider_name.clone()),
            parent_invocation_uuid,
        }
    }

    fn raw_paths_for(&self, invocation_uuid: &str) -> Option<lc_log_adapter::RawArtifactPaths> {
        let state_dir = Self::state_dir_for(&self.db_path)?;
        Some(Self::raw_paths_map_for(state_dir, invocation_uuid))
    }

    fn is_memory_db_path(path: &Path) -> bool {
        path == Path::new(":memory:")
    }

    fn state_dir_for(db_path: &Path) -> Option<&Path> {
        (!Self::is_memory_db_path(db_path))
            .then(|| db_path.parent())
            .flatten()
    }

    fn raw_paths_map_for(state_dir: &Path, uuid: &str) -> lc_log_adapter::RawArtifactPaths {
        let raw_io_dir = state_dir.join("invocations").join("raw-io");
        lc_log_adapter::RawArtifactPaths {
            stdout_path: raw_io_dir.join(Self::format_raw_artifact_filename(
                uuid,
                RawArtifactKind::Stdout,
            )),
            stderr_path: raw_io_dir.join(Self::format_raw_artifact_filename(
                uuid,
                RawArtifactKind::Stderr,
            )),
            result_path: raw_io_dir.join(Self::format_raw_artifact_filename(
                uuid,
                RawArtifactKind::Result,
            )),
            events_jsonl_path: raw_io_dir.join(Self::format_raw_artifact_filename(
                uuid,
                RawArtifactKind::EventsJsonl,
            )),
        }
    }

    fn format_raw_artifact_filename(uuid: &str, kind: RawArtifactKind) -> String {
        let suffix = match kind {
            RawArtifactKind::Stdout => "stdout",
            RawArtifactKind::Stderr => "stderr",
            RawArtifactKind::Result => "result",
            RawArtifactKind::EventsJsonl => "events.jsonl",
        };
        format!("{uuid}.{suffix}")
    }

    pub fn start_invocation(&self, start: &InvocationStart) -> Result<i64, String> {
        let timer = lc_log_adapter::start_timer();
        let context = self.lifecycle_context(start);
        let started_at = Self::current_rfc3339_timestamp();
        let sql_result = self.execute_start_invocation_sql(start, &started_at);
        self.warn_invocation_artifact_for_start_result(start, &started_at, &sql_result);
        lc_log_adapter::emit_start(&self.lifecycle_sink, timer, context, &sql_result);
        Self::translate_start_invocation_result(sql_result)
    }

    fn current_rfc3339_timestamp() -> String {
        Utc::now().to_rfc3339()
    }

    fn execute_start_invocation_sql(
        &self,
        start: &InvocationStart,
        started_at: &str,
    ) -> Result<i64, std::io::Error> {
        self.insert_invocation_start_row_raw(start, started_at)
            .map_err(Self::start_invocation_io_error)
    }

    fn translate_start_invocation_result(
        result: Result<i64, std::io::Error>,
    ) -> Result<i64, String> {
        result.map_err(|err| err.to_string())
    }

    fn start_invocation_io_error(err: sqlite::Error) -> std::io::Error {
        lc_log_adapter::start_invocation_io_error(err)
    }

    fn insert_invocation_start_row_raw(
        &self,
        start: &InvocationStart,
        started_at: &str,
    ) -> Result<i64, sqlite::Error> {
        self.conn.execute(
            "INSERT INTO invocations (
                    invocation_uuid,
                    model_name,
                    provider_name,
                    provider_index,
                    parent_invocation_id,
                    status,
                    success,
                    exit_code,
                    error_category,
                    terminal_reason,
                    created_at,
                    finished_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, NULL, NULL, ?7, NULL)",
            sqlite::params![
                &start.invocation_uuid,
                &start.model_name,
                &start.provider_name,
                start.provider_index as i64,
                start.parent_invocation_id,
                InvocationStatus::Running.as_str(),
                started_at,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    fn warn_invocation_artifact_for_start_result(
        &self,
        start: &InvocationStart,
        started_at: &str,
        result: &Result<i64, std::io::Error>,
    ) {
        if result.is_ok() {
            self.warn_invocation_artifact_failure(start, started_at);
        }
    }

    fn warn_invocation_artifact_failure(&self, start: &InvocationStart, started_at: &str) {
        if let Err(err) = self.write_invocation_artifact(start, started_at) {
            let message =
                Self::format_invocation_artifact_warning_message(&start.invocation_uuid, &err);
            Self::emit_artifact_warning(&message);
        }
    }

    fn format_invocation_artifact_warning_message(
        invocation_uuid: &str,
        err: &dyn std::fmt::Display,
    ) -> String {
        format!("Warning: Failed to write invocation artifact for {invocation_uuid}: {err}")
    }

    fn emit_artifact_warning(message: &str) {
        eprintln!("{message}");
    }

    pub fn finalize_invocation(
        &self,
        id: i64,
        success: bool,
        exit_code: i32,
        error_category: Option<&str>,
        terminal_reason: Option<&str>,
    ) -> Result<(), String> {
        let lifecycle_row = self.lifecycle_context_for_row_or_none(id);
        let timer = lc_log_adapter::start_timer();
        let finished_at = Self::current_rfc3339_timestamp();
        let transaction_result = self.finalize_invocation_transaction(
            id,
            success,
            exit_code,
            error_category,
            terminal_reason,
            &finished_at,
        );
        self.warn_result_artifact_for_finalize_result(
            success,
            exit_code,
            error_category,
            terminal_reason,
            &finished_at,
            &transaction_result,
        );
        let result = Self::translate_finalize_invocation_result(transaction_result);
        let finalize_success = Self::is_finalize_result_success(&result);
        let sqlite_error = Self::is_finalize_sqlite_error(id, lifecycle_row.as_ref(), &result);
        let operation_result =
            Self::classify_finalize_operation_result(finalize_success, sqlite_error);
        let terminal_status = Self::format_terminal_status(success, exit_code, terminal_reason);
        let input = Self::finalize_lifecycle_input(
            &terminal_status,
            exit_code,
            error_category,
            terminal_reason,
            operation_result,
        );
        let context = self.finalize_context(id, lifecycle_row.as_ref(), input);
        lc_log_adapter::emit_finalize(
            &self.lifecycle_sink,
            timer,
            context,
            &result,
            terminal_status,
        );
        result
    }

    fn lifecycle_context_for_row_or_none(&self, row_id: i64) -> Option<LifecycleInvocationRow> {
        self.lifecycle_context_for_row(row_id).ok().flatten()
    }

    fn warn_result_artifact_for_finalize_result(
        &self,
        success: bool,
        exit_code: i32,
        error_category: Option<&str>,
        terminal_reason: Option<&str>,
        finished_at: &str,
        result: &Result<FinalizeInvocationRow, String>,
    ) {
        if let Ok(invocation) = result {
            let failure_identity =
                (!success).then(|| self.result_artifact_failure_identity(invocation));
            let input = ResultEnvelopeInput {
                id: &invocation.invocation_uuid,
                success,
                exit_code,
                error_category,
                terminal_reason,
                finished_at,
                failure_identity: failure_identity.as_ref(),
            };
            self.warn_result_artifact_failure(input);
        }
    }

    fn translate_finalize_invocation_result(
        result: Result<FinalizeInvocationRow, String>,
    ) -> Result<(), String> {
        result.map(|_| ())
    }

    fn is_finalize_result_success(result: &Result<(), String>) -> bool {
        result.is_ok()
    }

    fn is_finalize_sqlite_error(
        id: i64,
        lifecycle_row: Option<&LifecycleInvocationRow>,
        result: &Result<(), String>,
    ) -> bool {
        result.as_ref().err().is_some_and(|message| {
            !Self::is_finalize_context_resolution_error(id, lifecycle_row, message)
        })
    }

    fn is_finalize_context_resolution_error(
        id: i64,
        lifecycle_row: Option<&LifecycleInvocationRow>,
        message: &str,
    ) -> bool {
        lifecycle_row.is_none() && Self::is_invocation_not_found_error(id, message)
    }

    fn finalize_lifecycle_input<'a>(
        terminal_status_attempt: &'a str,
        exit_code: i32,
        error_category: Option<&'a str>,
        terminal_reason: Option<&'a str>,
        operation_result: OperationResult,
    ) -> FinalizeLifecycleInput<'a> {
        FinalizeLifecycleInput {
            terminal_status_attempt,
            exit_code,
            error_category,
            terminal_reason,
            operation_result,
        }
    }

    fn format_terminal_status(
        success: bool,
        _exit_code: i32,
        _terminal_reason: Option<&str>,
    ) -> String {
        lifecycle_terminal_status(success).to_string()
    }

    fn finalize_invocation_transaction(
        &self,
        id: i64,
        success: bool,
        exit_code: i32,
        error_category: Option<&str>,
        terminal_reason: Option<&str>,
        finished_at: &str,
    ) -> Result<FinalizeInvocationRow, String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(Self::format_begin_transaction_error)?;

        let invocation = Self::load_invocation_for_finalize(&tx, id)?;
        Self::validate_invocation_is_running(id, &invocation.status)?;
        Self::write_invocation_final_row(
            &tx,
            id,
            success,
            exit_code,
            error_category,
            terminal_reason,
            finished_at,
        )?;
        Self::upsert_provider_finalize_aggregate(
            &tx,
            &invocation.model_name,
            invocation.provider_name.as_deref(),
            success,
            terminal_reason,
            finished_at,
        )?;

        tx.commit().map_err(Self::format_commit_transaction_error)?;
        Ok(invocation)
    }

    fn format_begin_transaction_error(err: sqlite::Error) -> String {
        format!("Failed to begin invocation finalize tx: {err}")
    }

    fn format_commit_transaction_error(err: sqlite::Error) -> String {
        format!("Failed to commit invocation finalize tx: {err}")
    }

    fn classify_finalize_operation_result(success: bool, sqlite_error: bool) -> OperationResult {
        if success {
            lc_log_adapter::finalize_operation_result(true, false)
        } else {
            lc_log_adapter::finalize_operation_result(false, sqlite_error)
        }
    }

    fn is_invocation_not_found_error(id: i64, message: &str) -> bool {
        message == Self::format_invocation_not_found_error(id)
    }

    fn format_invocation_not_found_error(id: i64) -> String {
        format!("Invocation {id} not found")
    }

    fn load_invocation_for_finalize(
        conn: &sqlite::Connection,
        id: i64,
    ) -> Result<FinalizeInvocationRow, String> {
        let columns = Self::query_invocation_row_for_finalize(conn, id)
            .map_err(|err| Self::format_load_invocation_for_finalize_error(id, err))?
            .ok_or_else(|| Self::format_invocation_not_found_error(id))?;
        Ok(Self::map_invocation_row_for_finalize(columns))
    }

    fn query_invocation_row_for_finalize(
        conn: &sqlite::Connection,
        id: i64,
    ) -> sqlite::Result<Option<FinalizeInvocationRowColumns>> {
        conn.query_row(
            "SELECT invocation_uuid, model_name, provider_name, provider_session_id, status
             FROM invocations WHERE id = ?1",
            sqlite::params![id],
            Self::read_invocation_row_for_finalize,
        )
        .optional()
    }

    fn read_invocation_row_for_finalize(
        row: &sqlite::Row<'_>,
    ) -> sqlite::Result<FinalizeInvocationRowColumns> {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        ))
    }

    fn map_invocation_row_for_finalize(
        columns: FinalizeInvocationRowColumns,
    ) -> FinalizeInvocationRow {
        let (invocation_uuid, model_name, provider_name, provider_session_id, status) = columns;
        FinalizeInvocationRow {
            invocation_uuid,
            model_name,
            provider_name,
            provider_session_id,
            status,
        }
    }

    fn format_load_invocation_for_finalize_error(id: i64, err: sqlite::Error) -> String {
        format!("Failed to load invocation {id}: {err}")
    }

    fn validate_invocation_is_running(id: i64, status: &str) -> Result<(), String> {
        if status.parse::<InvocationStatus>().ok() == Some(InvocationStatus::Running) {
            Ok(())
        } else {
            Err(format!("Invocation {id} is already finalized"))
        }
    }

    fn write_invocation_final_row(
        conn: &sqlite::Connection,
        id: i64,
        success: bool,
        exit_code: i32,
        error_category: Option<&str>,
        terminal_reason: Option<&str>,
        finished_at: &str,
    ) -> Result<(), String> {
        let updated = Self::execute_update_invocation_final_row(
            conn,
            id,
            success,
            exit_code,
            error_category,
            terminal_reason,
            finished_at,
        )
        .map_err(|err| Self::format_invocation_final_row_update_error(id, err))?;
        Self::validate_invocation_final_row_updated(id, updated)
    }

    fn execute_update_invocation_final_row(
        conn: &sqlite::Connection,
        id: i64,
        success: bool,
        exit_code: i32,
        error_category: Option<&str>,
        terminal_reason: Option<&str>,
        finished_at: &str,
    ) -> sqlite::Result<usize> {
        conn.execute(
            "UPDATE invocations
             SET status = ?1,
                 success = ?2,
                 exit_code = ?3,
                 error_category = ?4,
                 terminal_reason = ?5,
                 finished_at = ?6
             WHERE id = ?7 AND status = ?8",
            sqlite::params![
                Self::terminal_invocation_status(success).as_str(),
                success as i64,
                exit_code,
                error_category,
                terminal_reason,
                finished_at,
                id,
                InvocationStatus::Running.as_str(),
            ],
        )
    }

    fn validate_invocation_final_row_updated(id: i64, updated: usize) -> Result<(), String> {
        if updated == 0 {
            return Err(format!("Invocation {id} is already finalized"));
        }
        Ok(())
    }

    fn format_invocation_final_row_update_error(id: i64, err: sqlite::Error) -> String {
        format!("Failed to finalize invocation {id}: {err}")
    }

    fn terminal_invocation_status(success: bool) -> InvocationStatus {
        if success {
            InvocationStatus::Succeeded
        } else {
            InvocationStatus::Failed
        }
    }

    fn upsert_provider_finalize_aggregate(
        conn: &sqlite::Connection,
        model_name: &str,
        provider_name: Option<&str>,
        success: bool,
        terminal_reason: Option<&str>,
        finished_at: &str,
    ) -> Result<(), String> {
        let Some(provider_name) = Self::eligible_provider_name(provider_name) else {
            return Ok(());
        };
        Self::execute_provider_finalize_aggregate_sql(
            conn,
            model_name,
            provider_name,
            success,
            finished_at,
        )
        .map_err(Self::format_provider_finalize_aggregate_sql_error)?;
        if Self::is_finalize_failure(success) {
            Self::update_provider_last_error(
                conn,
                model_name,
                provider_name,
                terminal_reason,
                finished_at,
            )?;
        }
        Ok(())
    }

    fn eligible_provider_name(provider_name: Option<&str>) -> Option<&str> {
        provider_name
    }

    fn is_finalize_failure(success: bool) -> bool {
        !success
    }

    fn execute_provider_finalize_aggregate_sql(
        conn: &sqlite::Connection,
        model_name: &str,
        provider_name: &str,
        success: bool,
        finished_at: &str,
    ) -> sqlite::Result<()> {
        conn.execute(
            "INSERT INTO providers (
                    model_name, provider_name,
                    invocation_count, error_count, last_invoked_at
                 ) VALUES (?1, ?2, 1, ?3, ?4)
                 ON CONFLICT (model_name, provider_name)
                 DO UPDATE SET
                    invocation_count = invocation_count + 1,
                    error_count = error_count + ?3,
                    last_invoked_at = ?4",
            sqlite::params![
                model_name,
                provider_name,
                Self::provider_error_count_increment(success),
                finished_at
            ],
        )?;
        Ok(())
    }

    fn provider_error_count_increment(success: bool) -> i64 {
        if success { 0 } else { 1 }
    }

    fn format_provider_finalize_aggregate_sql_error(err: sqlite::Error) -> String {
        format!("Failed to upsert provider: {err}")
    }

    fn update_provider_last_error(
        conn: &sqlite::Connection,
        model_name: &str,
        provider_name: &str,
        terminal_reason: Option<&str>,
        finished_at: &str,
    ) -> Result<(), String> {
        let snippet = Self::map_provider_error_snippet(terminal_reason);
        Self::execute_update_provider_last_error_sql(
            conn,
            model_name,
            provider_name,
            snippet.as_deref(),
            finished_at,
        )
        .map_err(Self::format_update_provider_last_error_sql_error)?;
        Ok(())
    }

    fn map_provider_error_snippet(terminal_reason: Option<&str>) -> Option<String> {
        terminal_reason.map(Self::provider_error_snippet)
    }

    fn execute_update_provider_last_error_sql(
        conn: &sqlite::Connection,
        model_name: &str,
        provider_name: &str,
        snippet: Option<&str>,
        finished_at: &str,
    ) -> sqlite::Result<()> {
        conn.execute(
            "UPDATE providers SET last_error = ?1, last_error_at = ?2
             WHERE model_name = ?3 AND provider_name = ?4",
            sqlite::params![snippet, finished_at, model_name, provider_name],
        )?;
        Ok(())
    }

    fn format_update_provider_last_error_sql_error(err: sqlite::Error) -> String {
        format!("Failed to update error info: {err}")
    }

    fn provider_error_snippet(value: &str) -> String {
        value.chars().take(500).collect()
    }

    fn warn_result_artifact_failure(&self, input: ResultEnvelopeInput<'_>) {
        if let Err(err) = self.write_result_artifact(input) {
            let message = Self::format_result_artifact_warning_message(input.id, &err);
            Self::emit_artifact_warning(&message);
        }
    }

    fn result_artifact_failure_identity(
        &self,
        invocation: &FinalizeInvocationRow,
    ) -> ResultEnvelopeFailureIdentity {
        let agent_runner_chain_id =
            match (&invocation.provider_name, &invocation.provider_session_id) {
                (Some(provider_name), Some(provider_session_id)) => self
                    .chain_id_for_segment(provider_name, provider_session_id)
                    .ok()
                    .flatten(),
                _ => None,
            };
        ResultEnvelopeFailureIdentity {
            agent_runner_invocation_id: invocation.invocation_uuid.clone(),
            provider_name: invocation.provider_name.clone(),
            provider_session_id: invocation.provider_session_id.clone(),
            agent_runner_chain_id,
        }
    }

    fn format_result_artifact_warning_message(
        invocation_uuid: &str,
        err: &dyn std::fmt::Display,
    ) -> String {
        format!("Warning: Failed to write result artifact for {invocation_uuid}: {err}")
    }

    pub fn record_returned_artifacts(
        &self,
        invocation_row_id: i64,
        refs: &[ReturnedArtifactRef],
    ) -> Result<(), DbError> {
        Self::prepare_returned_artifacts_table(&self.conn)?;
        let identity =
            Self::load_invocation_identity_for_returned_artifacts(&self.conn, invocation_row_id)?;
        Self::validate_returned_artifact_refs(&identity, refs)?;
        Self::replace_returned_artifact_rows(&self.conn, invocation_row_id, refs)
    }

    fn prepare_returned_artifacts_table(conn: &sqlite::Connection) -> Result<(), DbError> {
        conn.execute_batch(invocation_returned_artifacts_schema_sql!())
            .map_err(|e| format!("Failed to ensure returned-artifacts schema: {e}"))
    }

    fn load_invocation_identity_for_returned_artifacts(
        conn: &sqlite::Connection,
        invocation_row_id: i64,
    ) -> Result<InvocationIdentity, DbError> {
        let uuid_text = Self::load_invocation_uuid_text(conn, invocation_row_id)?;
        let uuid =
            Self::parse_invocation_uuid_for_returned_artifacts(invocation_row_id, &uuid_text)?;
        Ok(InvocationIdentity {
            row_id: invocation_row_id,
            uuid,
        })
    }

    fn load_invocation_uuid_text(
        conn: &sqlite::Connection,
        invocation_row_id: i64,
    ) -> Result<String, DbError> {
        conn.query_row(
            "SELECT invocation_uuid FROM invocations WHERE id = ?1",
            sqlite::params![invocation_row_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("Failed to load invocation for returned artifacts: {e}"))?
        .ok_or_else(|| format!("Invocation {invocation_row_id} not found"))
    }

    fn parse_invocation_uuid_for_returned_artifacts(
        invocation_row_id: i64,
        uuid_text: &str,
    ) -> Result<Uuid, DbError> {
        Uuid::parse_str(uuid_text)
            .map_err(|e| format!("Invalid invocation UUID on row {invocation_row_id}: {e}"))
    }

    fn validate_returned_artifact_refs(
        identity: &InvocationIdentity,
        refs: &[ReturnedArtifactRef],
    ) -> Result<(), DbError> {
        for reference in refs {
            Self::validate_returned_artifact_ref(identity.row_id, identity.uuid, reference)?;
        }
        Ok(())
    }

    fn replace_returned_artifact_rows(
        conn: &sqlite::Connection,
        invocation_row_id: i64,
        refs: &[ReturnedArtifactRef],
    ) -> Result<(), DbError> {
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to begin returned-artifacts tx: {e}"))?;
        tx.execute(
            "DELETE FROM invocation_returned_artifacts WHERE invocation_id = ?1",
            sqlite::params![invocation_row_id],
        )
        .map_err(|e| format!("Failed to reset returned artifacts: {e}"))?;
        for (ordinal, reference) in refs.iter().enumerate() {
            Self::insert_returned_artifact_row(&tx, invocation_row_id, ordinal, reference)?;
        }
        tx.commit()
            .map_err(|e| format!("Failed to commit returned-artifacts tx: {e}"))
    }

    fn validate_returned_artifact_ref(
        invocation_row_id: i64,
        invocation_uuid: Uuid,
        reference: &ReturnedArtifactRef,
    ) -> Result<(), DbError> {
        let derived_uuid =
            returned_artifact_producer_uuid(&reference.store_address.workflow_run_id)
                .map_err(|e| format!("Invalid returned-artifact workflow_run_id: {e}"))?;
        Self::validate_returned_artifact_producer_uuid(reference, derived_uuid)?;
        Self::validate_returned_artifact_owner(invocation_row_id, invocation_uuid, reference)?;
        Self::validate_returned_artifact_version_id(reference, derived_uuid)
    }

    fn validate_returned_artifact_producer_uuid(
        reference: &ReturnedArtifactRef,
        derived_uuid: Uuid,
    ) -> Result<(), DbError> {
        if derived_uuid == reference.producer_invocation_uuid {
            Ok(())
        } else {
            Err(format!(
                "Returned artifact producer UUID mismatch: workflow_run_id encodes {derived_uuid}, ref carries {}",
                reference.producer_invocation_uuid
            ))
        }
    }

    fn validate_returned_artifact_owner(
        invocation_row_id: i64,
        invocation_uuid: Uuid,
        reference: &ReturnedArtifactRef,
    ) -> Result<(), DbError> {
        if reference.producer_invocation_uuid == invocation_uuid {
            Ok(())
        } else {
            Err(format!(
                "Returned artifact belongs to {}, but invocation row {invocation_row_id} is {invocation_uuid}",
                reference.producer_invocation_uuid
            ))
        }
    }

    fn validate_returned_artifact_version_id(
        reference: &ReturnedArtifactRef,
        derived_uuid: Uuid,
    ) -> Result<(), DbError> {
        let expected_version_id = returned_artifact_version_id(
            derived_uuid,
            &reference.store_address.artifact_name,
            reference.store_address.version,
        );
        if reference.version_id == expected_version_id {
            Ok(())
        } else {
            Err(format!(
                "Returned artifact version_id mismatch: expected {expected_version_id}, ref carries {}",
                reference.version_id
            ))
        }
    }

    fn insert_returned_artifact_row(
        conn: &sqlite::Connection,
        invocation_row_id: i64,
        ordinal: usize,
        reference: &ReturnedArtifactRef,
    ) -> Result<(), DbError> {
        let validated = Self::validate_returned_artifact_inputs(reference)?;
        let payload = Self::format_returned_artifact_payload_fields(reference)?;
        let params = Self::bind_returned_artifact_row_params(
            invocation_row_id,
            ordinal,
            reference,
            &validated,
            &payload,
        );
        Self::execute_returned_artifact_row_insert(conn, &params)
    }

    fn validate_returned_artifact_inputs(
        reference: &ReturnedArtifactRef,
    ) -> Result<ReturnedArtifactValidatedInputs, DbError> {
        Ok(ReturnedArtifactValidatedInputs {
            version: returned_artifact_sql_integer(reference.store_address.version, "version")?,
            content_len: returned_artifact_sql_integer(reference.content_len, "content_len")?,
        })
    }

    fn format_returned_artifact_payload_fields(
        reference: &ReturnedArtifactRef,
    ) -> Result<ReturnedArtifactPayloadFields, DbError> {
        Ok(ReturnedArtifactPayloadFields {
            source_json: serde_json::to_string(&reference.source)
                .map_err(|e| format!("Failed to encode returned-artifact source: {e}"))?,
            returned_at: reference.returned_at.to_rfc3339(),
        })
    }

    fn bind_returned_artifact_row_params<'a>(
        invocation_row_id: i64,
        ordinal: usize,
        reference: &'a ReturnedArtifactRef,
        validated: &'a ReturnedArtifactValidatedInputs,
        payload: &'a ReturnedArtifactPayloadFields,
    ) -> ReturnedArtifactRowParams<'a> {
        ReturnedArtifactRowParams {
            invocation_row_id,
            ordinal: ordinal as i64,
            version_id: &reference.version_id,
            name: &reference.name,
            workflow_run_id: &reference.store_address.workflow_run_id,
            artifact_name: &reference.store_address.artifact_name,
            version: validated.version,
            sha256: &reference.sha256,
            content_len: validated.content_len,
            format_hint: &reference.format_hint,
            verdict_line: &reference.verdict_line,
            source_kind: returned_source_kind(&reference.source),
            source_json: &payload.source_json,
            returned_at: &payload.returned_at,
        }
    }

    fn execute_returned_artifact_row_insert(
        conn: &sqlite::Connection,
        params: &ReturnedArtifactRowParams<'_>,
    ) -> Result<(), DbError> {
        conn.execute(
            "INSERT INTO invocation_returned_artifacts (
                invocation_id,
                ordinal,
                version_id,
                name,
                workflow_run_id,
                artifact_name,
                version,
                sha256,
                content_len,
                format_hint,
                verdict_line,
                source_kind,
                source_json,
                returned_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            sqlite::params![
                params.invocation_row_id,
                params.ordinal,
                params.version_id,
                params.name,
                params.workflow_run_id,
                params.artifact_name,
                params.version,
                params.sha256,
                params.content_len,
                params.format_hint,
                params.verdict_line,
                params.source_kind,
                params.source_json,
                params.returned_at,
            ],
        )
        .map_err(|e| format!("Failed to record returned artifact: {e}"))?;
        Ok(())
    }

    pub fn list_returned_artifacts(
        &self,
        invocation_row_id: i64,
    ) -> Result<Vec<ReturnedArtifactRef>, DbError> {
        if !Self::returned_artifacts_schema_is_readable(&self.conn)? {
            return Ok(Vec::new());
        }
        let rows = Self::load_returned_artifact_rows(&self.conn, invocation_row_id)?;
        Self::parse_returned_artifact_rows(rows)
    }

    fn returned_artifacts_schema_is_readable(conn: &sqlite::Connection) -> Result<bool, DbError> {
        Self::validate_returned_artifacts_object_type(conn)?;
        Self::returned_artifacts_have_version_id(conn)
    }

    fn validate_returned_artifacts_object_type(conn: &sqlite::Connection) -> Result<(), DbError> {
        match Self::returned_artifacts_object_type(conn)?.as_deref() {
            None | Some("table") => Ok(()),
            Some(other) => Err(Self::unexpected_returned_artifacts_object_error(other)),
        }
    }

    fn returned_artifacts_object_type(
        conn: &sqlite::Connection,
    ) -> Result<Option<String>, DbError> {
        conn.query_row(
            "SELECT type
             FROM sqlite_master
             WHERE name = 'invocation_returned_artifacts'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| format!("Failed to inspect returned-artifacts schema: {e}"))
    }

    fn unexpected_returned_artifacts_object_error(object_type: &str) -> DbError {
        format!("Unexpected returned-artifacts schema shape: object type={object_type}")
    }

    fn returned_artifacts_have_version_id(conn: &sqlite::Connection) -> Result<bool, DbError> {
        let columns = Self::returned_artifact_columns(conn)?;
        Ok(Self::has_column(&columns, "version_id"))
    }

    fn load_returned_artifact_rows(
        conn: &sqlite::Connection,
        invocation_row_id: i64,
    ) -> Result<Vec<ReturnedArtifactRawRow>, DbError> {
        let mut stmt = conn
            .prepare(
                "SELECT
                    version_id,
                    name,
                    workflow_run_id,
                    artifact_name,
                    version,
                    sha256,
                    content_len,
                    format_hint,
                    verdict_line,
                    source_json,
                    returned_at
                 FROM invocation_returned_artifacts
                 WHERE invocation_id = ?1
                 ORDER BY ordinal ASC",
            )
            .map_err(|e| format!("Failed to prepare returned-artifacts query: {e}"))?;
        let rows = stmt
            .query_map(
                sqlite::params![invocation_row_id],
                Self::map_returned_artifact_raw_row,
            )
            .map_err(|e| format!("Failed to query returned artifacts: {e}"))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read returned artifact row: {e}"))
    }

    fn map_returned_artifact_raw_row(
        row: &sqlite::Row<'_>,
    ) -> sqlite::Result<ReturnedArtifactRawRow> {
        Ok(ReturnedArtifactRawRow {
            version_id: row.get(0)?,
            name: row.get(1)?,
            workflow_run_id: row.get(2)?,
            artifact_name: row.get(3)?,
            version: row.get(4)?,
            sha256: row.get(5)?,
            content_len: row.get(6)?,
            format_hint: row.get(7)?,
            verdict_line: row.get(8)?,
            source_json: row.get(9)?,
            returned_at_text: row.get(10)?,
        })
    }

    fn parse_returned_artifact_rows(
        rows: Vec<ReturnedArtifactRawRow>,
    ) -> Result<Vec<ReturnedArtifactRef>, DbError> {
        rows.into_iter()
            .map(Self::parse_returned_artifact_row)
            .collect()
    }

    fn parse_returned_artifact_row(
        row: ReturnedArtifactRawRow,
    ) -> Result<ReturnedArtifactRef, DbError> {
        let parsed = Self::parse_returned_artifact_field_values(&row)
            .map_err(Self::format_returned_artifact_parse_error)?;
        let validated = Self::validate_returned_artifact_field_values(parsed)
            .map_err(Self::format_returned_artifact_parse_error)?;
        Ok(Self::map_parsed_returned_artifact_to_ref(row, validated))
    }

    fn parse_returned_artifact_field_values(
        row: &ReturnedArtifactRawRow,
    ) -> Result<ParsedReturnedArtifactFieldValues, ReturnedArtifactFieldError> {
        Ok(ParsedReturnedArtifactFieldValues {
            source: serde_json::from_str(&row.source_json)
                .map_err(ReturnedArtifactFieldError::SourceJson)?,
            returned_at: DateTime::parse_from_rfc3339(&row.returned_at_text)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|err| ReturnedArtifactFieldError::ReturnedAt {
                    raw: row.returned_at_text.clone(),
                    err,
                })?,
            producer_invocation_uuid: returned_artifact_producer_uuid(&row.workflow_run_id)
                .map_err(ReturnedArtifactFieldError::ProducerUuid)?,
            version: row.version,
            content_len: row.content_len,
        })
    }

    fn validate_returned_artifact_field_values(
        parsed: ParsedReturnedArtifactFieldValues,
    ) -> Result<ValidatedReturnedArtifactFieldValues, ReturnedArtifactFieldError> {
        Ok(ValidatedReturnedArtifactFieldValues {
            source: parsed.source,
            returned_at: parsed.returned_at,
            producer_invocation_uuid: parsed.producer_invocation_uuid,
            version: Self::validate_returned_artifact_nonnegative_integer(
                parsed.version,
                "version",
            )?,
            content_len: Self::validate_returned_artifact_nonnegative_integer(
                parsed.content_len,
                "content_len",
            )?,
        })
    }

    fn validate_returned_artifact_nonnegative_integer(
        value: i64,
        field: &'static str,
    ) -> Result<u64, ReturnedArtifactFieldError> {
        u64::try_from(value).map_err(|_| ReturnedArtifactFieldError::NegativeInteger { field })
    }

    fn map_parsed_returned_artifact_to_ref(
        row: ReturnedArtifactRawRow,
        parsed: ValidatedReturnedArtifactFieldValues,
    ) -> ReturnedArtifactRef {
        ReturnedArtifactRef {
            version_id: row.version_id,
            name: row.name,
            store_address: oulipoly_agent_messenger::StoreAddress {
                workflow_run_id: row.workflow_run_id,
                artifact_name: row.artifact_name,
                version: parsed.version,
            },
            sha256: row.sha256,
            content_len: parsed.content_len,
            format_hint: row.format_hint,
            verdict_line: row.verdict_line,
            source: parsed.source,
            producer_invocation_uuid: parsed.producer_invocation_uuid,
            returned_at: parsed.returned_at,
        }
    }

    fn format_returned_artifact_parse_error(err: ReturnedArtifactFieldError) -> DbError {
        match err {
            ReturnedArtifactFieldError::SourceJson(err) => {
                format!("Failed to parse returned artifact source JSON: {err}")
            }
            ReturnedArtifactFieldError::ReturnedAt { raw, err } => {
                format!("Bad returned artifact returned_at {raw}: {err}")
            }
            ReturnedArtifactFieldError::ProducerUuid(err) => {
                format!("Failed to parse returned artifact producer UUID: {err}")
            }
            ReturnedArtifactFieldError::NegativeInteger { field } => {
                format!("negative returned artifact {field}")
            }
        }
    }

    fn returned_artifact_columns(conn: &sqlite::Connection) -> Result<Vec<String>, String> {
        Self::table_column_names(
            conn,
            "invocation_returned_artifacts",
            "Failed to inspect returned-artifacts schema",
            "Failed to query returned-artifacts columns",
            "Failed to read returned-artifacts column",
        )
    }

    /// Update an invocation row's session correlation columns. Per
    /// `tmp/01-pr-c-contract.md` §"DB method additions", this method
    /// takes `method` as a `&str` so the DB layer stays decoupled from
    /// `SessionCaptureMethod` (an executor-internal type).
    ///
    /// Always writes both columns. Per V10 (failures observable, never
    /// silent), a completed invocation with no capture attempted
    /// records `("None", "none")` explicitly — that's a positive
    /// signal distinct from NULL (the row was never finalized). The
    /// last call wins, which matches the multi-call safety semantics.
    pub fn update_session_capture(
        &self,
        id: i64,
        session_id: Option<&str>,
        method: &str,
    ) -> Result<(), String> {
        let lifecycle_row = self.lifecycle_context_for_row_or_none(id);
        let timer = lc_log_adapter::start_timer();
        let projection = Self::project_session_capture(session_id, method);
        let sql_result =
            self.execute_session_capture_persistence(id, session_id, method, projection);
        let result = Self::translate_session_capture_result(id, sql_result);
        let context = self.optional_session_context(id, lifecycle_row.as_ref(), session_id, method);
        lc_log_adapter::emit_session_capture(&self.lifecycle_sink, timer, context, &result);
        result
    }

    fn project_session_capture<'a>(
        session_id: Option<&'a str>,
        method: &'a str,
    ) -> SessionCaptureProjection<'a> {
        let provider_session_id = Self::map_capture_provider_session_id(session_id, method);
        let resume_input_id = Self::map_capture_resume_input_id(session_id, method);
        let provider_session_capture_method =
            Self::map_provider_session_capture_method(session_id, method);
        SessionCaptureProjection {
            provider_session_id,
            resume_input_id,
            provider_session_capture_method,
        }
    }

    fn map_capture_provider_session_id<'a>(
        session_id: Option<&'a str>,
        method: &str,
    ) -> Option<&'a str> {
        if Self::is_resumed_session_method(method) {
            None
        } else {
            session_id
        }
    }

    fn map_capture_resume_input_id<'a>(
        session_id: Option<&'a str>,
        method: &str,
    ) -> Option<&'a str> {
        if Self::is_resumed_session_method(method) {
            session_id
        } else {
            None
        }
    }

    fn map_provider_session_capture_method<'a>(
        session_id: Option<&str>,
        method: &'a str,
    ) -> Option<&'a str> {
        if Self::is_resumed_session_method(method) {
            None
        } else {
            session_id.map(|_| method)
        }
    }

    fn execute_session_capture_persistence(
        &self,
        id: i64,
        session_id: Option<&str>,
        method: &str,
        projection: SessionCaptureProjection<'_>,
    ) -> Result<i64, sqlite::Error> {
        let updated = self.conn.execute(
            "UPDATE invocations
                 SET session_id = CASE
                         WHEN ?2 = 'resumed' THEN COALESCE(session_id, ?1)
                         ELSE ?1
                     END,
                     session_capture_method = ?2,
                     provider_session_id = COALESCE(provider_session_id, ?3),
                     resume_input_id = COALESCE(resume_input_id, ?4),
                     provider_session_capture_method = COALESCE(provider_session_capture_method, ?5)
                 WHERE id = ?6",
            sqlite::params![
                session_id,
                method,
                projection.provider_session_id,
                projection.resume_input_id,
                projection.provider_session_capture_method,
                id
            ],
        )?;
        Ok(updated as i64)
    }

    fn translate_session_capture_result(
        id: i64,
        result: Result<i64, sqlite::Error>,
    ) -> Result<(), String> {
        result
            .map(|_| ())
            .map_err(|err| Self::format_session_capture_error(id, err))
    }

    fn format_session_capture_error(id: i64, err: sqlite::Error) -> String {
        format!("Failed to update session capture for invocation {id}: {err}")
    }

    fn optional_session_context(
        &self,
        id: i64,
        row: Option<&LifecycleInvocationRow>,
        session_id: Option<&str>,
        method: &str,
    ) -> Option<lc_log_adapter::SessionContext> {
        row.map(|row| self.session_context(id, row, session_id, method))
    }

    fn lifecycle_context_for_row(
        &self,
        row_id: i64,
    ) -> Result<Option<LifecycleInvocationRow>, String> {
        self.query_lifecycle_context_for_row(row_id)
            .map_err(|err| Self::format_lifecycle_context_lookup_error(row_id, err))
    }

    fn query_lifecycle_context_for_row(
        &self,
        row_id: i64,
    ) -> sqlite::Result<Option<LifecycleInvocationRow>> {
        self.conn
            .query_row(
                "SELECT i.invocation_uuid,
                        i.provider_name,
                        i.session_id,
                        i.provider_session_id,
                        i.resume_input_id
                 FROM invocations i
                 WHERE i.id = ?1",
                sqlite::params![row_id],
                Self::map_lifecycle_invocation_row,
            )
            .optional()
    }

    fn format_lifecycle_context_lookup_error(row_id: i64, err: sqlite::Error) -> String {
        format!("Failed to load invocation lifecycle context {row_id}: {err}")
    }

    fn map_lifecycle_invocation_row(
        row: &sqlite::Row<'_>,
    ) -> sqlite::Result<LifecycleInvocationRow> {
        Ok(LifecycleInvocationRow {
            invocation_uuid: row.get(0)?,
            provider_name: row.get(1)?,
            session_id: row.get(2)?,
            provider_session_id: row.get(3)?,
            resume_input_id: row.get(4)?,
        })
    }

    fn session_context(
        &self,
        id: i64,
        row: &LifecycleInvocationRow,
        session_id: Option<&str>,
        method: &str,
    ) -> lc_log_adapter::SessionContext {
        let event_session_id = Self::map_resumed_session_id(method, session_id);
        let resume_input_id = Self::map_session_resume_input_id(method, session_id, row);
        let chain_id_result = self.load_chain_id_for_invocation(id);
        let chain_id = Self::map_lifecycle_chain_id(chain_id_result);
        Self::build_session_context(id, row, event_session_id, method, resume_input_id, chain_id)
    }

    fn is_resumed_session_method(method: &str) -> bool {
        method == "resumed"
    }

    fn map_resumed_session_id(method: &str, session_id: Option<&str>) -> Option<String> {
        if Self::is_resumed_session_method(method) {
            None
        } else {
            session_id.map(str::to_string)
        }
    }

    fn map_session_resume_input_id(
        method: &str,
        session_id: Option<&str>,
        row: &LifecycleInvocationRow,
    ) -> Option<String> {
        if Self::is_resumed_session_method(method) {
            session_id.map(str::to_string)
        } else {
            row.resume_input_id.clone()
        }
    }

    fn load_chain_id_for_invocation(
        &self,
        invocation_id: i64,
    ) -> Result<Option<String>, sqlite::Error> {
        self.conn
            .query_row(
                "SELECT s.chain_id
                 FROM invocations i
                 JOIN session_chain_segments s
                   ON s.provider_name = i.provider_name
                  AND s.session_id = COALESCE(i.provider_session_id, i.session_id)
                 WHERE i.id = ?1
                 LIMIT 1",
                sqlite::params![invocation_id],
                |row| row.get(0),
            )
            .optional()
    }

    fn map_lifecycle_chain_id(
        chain_id_result: Result<Option<String>, sqlite::Error>,
    ) -> Option<String> {
        chain_id_result.ok().flatten()
    }

    fn build_session_context(
        id: i64,
        row: &LifecycleInvocationRow,
        event_session_id: Option<String>,
        method: &str,
        resume_input_id: Option<String>,
        chain_id: Option<String>,
    ) -> lc_log_adapter::SessionContext {
        lc_log_adapter::SessionContext {
            invocation_uuid: row.invocation_uuid.clone(),
            provider_source: row.provider_name.clone(),
            chain_id,
            session_id: event_session_id,
            latency_us: 0,
            invocation_row_id: id,
            capture_method: method.to_string(),
            marker_emitted: true,
            resume_input_id,
        }
    }

    fn finalize_context(
        &self,
        id: i64,
        row: Option<&LifecycleInvocationRow>,
        input: FinalizeLifecycleInput<'_>,
    ) -> lc_log_adapter::FinalizeContext {
        let row_invocation_uuid = Self::load_invocation_uuid_for_finalize(row);
        let fallback_invocation_uuid = Self::format_fallback_invocation_uuid(id);
        let invocation_uuid =
            Self::select_finalize_invocation_uuid(row_invocation_uuid, fallback_invocation_uuid);
        let session_id = Self::load_session_id_for_invocation(row);
        let chain_id_result = self.load_chain_id_for_invocation(id);
        let chain_id = Self::map_lifecycle_chain_id(chain_id_result);
        let raw_artifact_paths = self.load_raw_paths_for_finalize(&invocation_uuid);
        Self::build_finalize_context(
            id,
            row,
            invocation_uuid,
            session_id,
            chain_id,
            raw_artifact_paths,
            input,
        )
    }

    fn load_invocation_uuid_for_finalize(row: Option<&LifecycleInvocationRow>) -> Option<String> {
        row.map(Self::clone_lifecycle_invocation_uuid)
    }

    fn select_finalize_invocation_uuid(
        row_invocation_uuid: Option<String>,
        fallback_invocation_uuid: String,
    ) -> String {
        row_invocation_uuid.unwrap_or(fallback_invocation_uuid)
    }

    fn clone_lifecycle_invocation_uuid(row: &LifecycleInvocationRow) -> String {
        row.invocation_uuid.clone()
    }

    fn format_fallback_invocation_uuid(row_id: i64) -> String {
        format!("unresolved-invocation-row-{row_id}")
    }

    fn load_session_id_for_invocation(row: Option<&LifecycleInvocationRow>) -> Option<String> {
        row.and_then(active_lifecycle_session_id)
    }

    fn load_raw_paths_for_finalize(
        &self,
        invocation_uuid: &str,
    ) -> Option<lc_log_adapter::RawArtifactPaths> {
        self.raw_paths_for(invocation_uuid)
    }

    fn build_finalize_context(
        id: i64,
        row: Option<&LifecycleInvocationRow>,
        invocation_uuid: String,
        session_id: Option<String>,
        chain_id: Option<String>,
        raw_artifact_paths: Option<lc_log_adapter::RawArtifactPaths>,
        input: FinalizeLifecycleInput<'_>,
    ) -> lc_log_adapter::FinalizeContext {
        lc_log_adapter::FinalizeContext {
            invocation_uuid,
            provider_source: row.and_then(|row| row.provider_name.clone()),
            chain_id,
            session_id,
            latency_us: 0,
            invocation_row_id: row.map(|_| id),
            terminal_status_attempt: input.terminal_status_attempt.to_string(),
            exit_code: input.exit_code,
            error_category: input.error_category.map(str::to_string),
            terminal_reason: input.terminal_reason.map(str::to_string),
            raw_artifact_paths,
            operation_result: input.operation_result,
        }
    }

    pub fn bind_invocation_provider_session_start(
        &self,
        invocation_row_id: i64,
        binding: &ProviderSessionBinding,
    ) -> Result<(), String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to begin provider session binding tx: {e}"))?;

        let existing = Self::load_existing_provider_session_binding(&tx, invocation_row_id)?;
        Self::validate_provider_session_rebind(invocation_row_id, binding, existing.as_deref())?;
        Self::write_provider_session_binding(&tx, invocation_row_id, binding)?;

        if Self::provider_session_binding_should_mint_chain(binding) {
            Self::mint_chain_for_invocation_session_on(&tx, invocation_row_id)?;
        }
        tx.commit()
            .map_err(|e| format!("Failed to commit provider session binding tx: {e}"))
    }

    fn load_existing_provider_session_binding(
        conn: &sqlite::Connection,
        invocation_row_id: i64,
    ) -> Result<Option<String>, String> {
        conn.query_row(
            "SELECT provider_session_id FROM invocations WHERE id = ?1",
            sqlite::params![invocation_row_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|e| format!("Failed to read invocation {invocation_row_id}: {e}"))?
        .ok_or_else(|| format!("Invocation {invocation_row_id} not found"))
    }

    fn validate_provider_session_rebind(
        invocation_row_id: i64,
        binding: &ProviderSessionBinding,
        existing: Option<&str>,
    ) -> Result<(), String> {
        if let Some(existing) = existing
            && existing != binding.provider_session_id
        {
            return Err(format!(
                "Invocation {invocation_row_id} is already bound to provider session {existing}; refusing to bind {}",
                binding.provider_session_id
            ));
        }
        Ok(())
    }

    fn write_provider_session_binding(
        conn: &sqlite::Connection,
        invocation_row_id: i64,
        binding: &ProviderSessionBinding,
    ) -> Result<(), String> {
        conn.execute(
            "UPDATE invocations
             SET provider_session_id = ?1,
                 provider_session_capture_method = ?2,
                 provider_session_resolved_account = COALESCE(?3, provider_session_resolved_account),
                 resume_input_id = COALESCE(?4, resume_input_id),
                 session_id = CASE
                     WHEN session_capture_method = 'resumed'
                          AND resume_input_id IS NOT NULL
                          AND session_id = resume_input_id
                     THEN session_id
                     ELSE ?1
                 END,
                 session_capture_method = ?2
             WHERE id = ?5",
            sqlite::params![
                &binding.provider_session_id,
                binding.capture_method,
                binding.provider_session_resolved_account.as_deref(),
                binding.resume_input_id.as_deref(),
                invocation_row_id
            ],
        )
        .map_err(|e| {
            format!("Failed to bind provider session for invocation {invocation_row_id}: {e}")
        })?;
        Ok(())
    }

    fn provider_session_binding_should_mint_chain(binding: &ProviderSessionBinding) -> bool {
        binding.resume_input_id.as_deref() != Some(binding.provider_session_id.as_str())
    }

    pub fn record_legacy_resume_input_session_id(
        &self,
        id: i64,
        resume_input_id: &str,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE invocations
                 SET session_id = ?1
                 WHERE id = ?2 AND session_capture_method = 'resumed'",
                sqlite::params![resume_input_id, id],
            )
            .map_err(|e| {
                format!("Failed to update legacy resume session_id for invocation {id}: {e}")
            })?;
        Ok(())
    }

    pub fn update_resume_acceptance(
        &self,
        id: i64,
        status: &str,
        evidence: Option<&str>,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE invocations
                 SET resume_acceptance_status = ?1,
                     resume_acceptance_evidence = ?2
                 WHERE id = ?3",
                sqlite::params![status, evidence, id],
            )
            .map_err(|e| format!("Failed to update resume acceptance for invocation {id}: {e}"))?;
        Ok(())
    }

    pub fn mint_chain_for_invocation_session(&self, invocation_row_id: i64) -> Result<(), DbError> {
        Self::mint_chain_for_invocation_session_on(&self.conn, invocation_row_id)
    }

    fn mint_chain_for_invocation_session_on(
        conn: &sqlite::Connection,
        invocation_row_id: i64,
    ) -> Result<(), DbError> {
        let Some(row) = Self::load_invocation_chain_mint_row(conn, invocation_row_id)? else {
            return Ok(());
        };
        let ts = Self::fallback_now_rfc3339(&row.raw_ts);
        if let Some(chain_id) =
            Self::existing_chain_for_provider_session(conn, &row.provider_name, &row.session_id)?
        {
            Self::promote_existing_invocation_chain(
                conn,
                &chain_id,
                &row.model_name,
                &row.provider_name,
                &row.session_id,
            )?;
            return Ok(());
        }
        Self::insert_invocation_chain(conn, &row, &ts)
    }

    fn load_invocation_chain_mint_row(
        conn: &sqlite::Connection,
        invocation_row_id: i64,
    ) -> Result<Option<InvocationChainMintRow>, DbError> {
        let provider_session_expr = Self::provider_session_expr(conn, None)?;
        let sql = format!(
            "SELECT model_name, provider_name, {provider_session_expr}, COALESCE(finished_at, created_at)
             FROM invocations
             WHERE id = ?1
               AND provider_name IS NOT NULL
               AND {provider_session_expr} IS NOT NULL"
        );
        conn.query_row(&sql, sqlite::params![invocation_row_id], |row| {
            Ok(InvocationChainMintRow {
                model_name: row.get(0)?,
                provider_name: row.get(1)?,
                session_id: row.get(2)?,
                raw_ts: row.get(3)?,
            })
        })
        .optional()
        .map_err(|e| format!("Failed to read invocation for chain mint: {e}"))
    }

    fn existing_chain_for_provider_session(
        conn: &sqlite::Connection,
        provider_name: &str,
        session_id: &str,
    ) -> Result<Option<String>, DbError> {
        conn.query_row(
            "SELECT chain_id FROM session_chain_segments
                 WHERE provider_name = ?1 AND session_id = ?2
                 LIMIT 1",
            sqlite::params![provider_name, session_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| format!("Failed to check existing invocation chain: {e}"))
    }

    fn promote_existing_invocation_chain(
        conn: &sqlite::Connection,
        chain_id: &str,
        model_name: &str,
        provider_name: &str,
        session_id: &str,
    ) -> Result<(), DbError> {
        conn.execute(
            "UPDATE session_chains
                 SET model_name = ?2
                 WHERE chain_id = ?1 AND model_name = '<unknown>'",
            sqlite::params![chain_id, model_name],
        )
        .map_err(|e| format!("Failed to update invocation session chain model: {e}"))?;
        conn.execute(
            "UPDATE session_chain_segments
                 SET transition_reason = 'initial'
                 WHERE chain_id = ?1
                   AND provider_name = ?2
                   AND session_id = ?3
                   AND transition_reason = 'imported'",
            sqlite::params![chain_id, provider_name, session_id],
        )
        .map_err(|e| format!("Failed to promote imported session chain segment: {e}"))?;
        Ok(())
    }

    fn insert_invocation_chain(
        conn: &sqlite::Connection,
        row: &InvocationChainMintRow,
        ts: &DateTime<Utc>,
    ) -> Result<(), DbError> {
        let chain_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, ?2, ?2, ?3)",
            sqlite::params![chain_id, ts.to_rfc3339(), row.model_name],
        )
        .map_err(|e| format!("Failed to mint invocation session chain: {e}"))?;
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, ?4, 'initial')",
            sqlite::params![chain_id, row.provider_name, row.session_id, ts.to_rfc3339()],
        )
        .map_err(|e| format!("Failed to mint invocation session segment: {e}"))?;
        Ok(())
    }

    pub fn get_invocation_by_uuid(&self, uuid: &str) -> Result<Option<InvocationRecord>, String> {
        let sql = Self::invocation_record_select_sql(&self.conn, "WHERE invocation_uuid = ?1")?;
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("Failed to prepare invocation lookup: {e}"))?;

        let result = stmt.query_row(sqlite::params![uuid], Self::map_invocation_row);
        match result {
            Ok(record) => Ok(Some(record)),
            Err(sqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("Failed to query invocation: {e}")),
        }
    }

    pub fn list_invocation_children(
        &self,
        parent_id: i64,
    ) -> Result<Vec<InvocationRecord>, String> {
        let sql = Self::invocation_record_select_sql(
            &self.conn,
            "WHERE parent_invocation_id = ?1
             ORDER BY created_at, id",
        )?;
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("Failed to prepare invocation child lookup: {e}"))?;

        let rows = stmt
            .query_map(sqlite::params![parent_id], Self::map_invocation_row)
            .map_err(|e| format!("Failed to query invocation children: {e}"))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to map invocation children: {e}"))
    }

    fn map_invocation_row(row: &sqlite::Row<'_>) -> sqlite::Result<InvocationRecord> {
        let created_at_raw: String = row.get(19)?;
        let finished_at_raw: Option<String> = row.get(20)?;
        let status_raw: String = row.get(6)?;
        let created_at = Self::strict_rfc3339_at(&created_at_raw, 18)?;
        let finished_at = Self::optional_strict_rfc3339_at(finished_at_raw, 19)?;
        let status = Self::parse_invocation_status_at(&status_raw, 6)?;

        Ok(InvocationRecord {
            id: row.get(0)?,
            invocation_uuid: row.get(1)?,
            model_name: row.get(2)?,
            provider_name: row.get(3)?,
            provider_index: row.get::<_, i64>(4)? as usize,
            parent_invocation_id: row.get(5)?,
            status,
            success: row.get::<_, Option<i64>>(7)?.map(|value| value != 0),
            exit_code: row.get(8)?,
            error_category: row.get(9)?,
            terminal_reason: row.get(10)?,
            session_id: row.get(11)?,
            session_capture_method: row.get(12)?,
            provider_session_id: row.get(13)?,
            resume_input_id: row.get(14)?,
            provider_session_capture_method: row.get(15)?,
            provider_session_resolved_account: row.get(16)?,
            resume_acceptance_status: row.get(17)?,
            resume_acceptance_evidence: row.get(18)?,
            created_at,
            finished_at,
        })
    }

    fn optional_strict_rfc3339_at(
        raw: Option<String>,
        column_index: usize,
    ) -> sqlite::Result<Option<DateTime<Utc>>> {
        raw.map(|s| Self::strict_rfc3339_at(&s, column_index))
            .transpose()
    }

    fn parse_invocation_status_at(
        raw: &str,
        column_index: usize,
    ) -> sqlite::Result<InvocationStatus> {
        raw.parse::<InvocationStatus>().map_err(|_| {
            sqlite::Error::FromSqlConversionFailure(
                column_index,
                sqlite::Type::Text,
                format!("Unknown invocation status: {raw}").into(),
            )
        })
    }

    pub fn get_provider(
        &self,
        model_name: &str,
        provider_name: &str,
    ) -> Result<Option<ProviderRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT model_name, provider_name, invocation_count, error_count,
                        last_error, last_error_at, last_invoked_at
                 FROM providers
                 WHERE model_name = ?1 AND provider_name = ?2",
            )
            .map_err(|e| format!("Failed to prepare query: {e}"))?;

        let result = stmt.query_row(sqlite::params![model_name, provider_name], |row| {
            Ok(ProviderRecord {
                model_name: row.get(0)?,
                provider_name: row.get(1)?,
                invocation_count: row.get(2)?,
                error_count: row.get(3)?,
                last_error: row.get(4)?,
                last_error_at: row.get(5)?,
                last_invoked_at: row.get(6)?,
            })
        });

        match result {
            Ok(record) => Ok(Some(record)),
            Err(sqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("Failed to query provider: {e}")),
        }
    }

    pub fn recent_error_count(
        &self,
        model_name: &str,
        provider_name: &str,
        window_minutes: i64,
    ) -> Result<i64, String> {
        let cutoff = (Utc::now() - chrono::Duration::minutes(window_minutes)).to_rfc3339();

        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM invocations
                 WHERE model_name = ?1 AND provider_name = ?2
                   AND success = 0 AND created_at > ?3",
                sqlite::params![model_name, provider_name, &cutoff],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to count recent errors: {e}"))?;

        Ok(count)
    }

    // --- Provider quota operations ---

    pub fn mark_exhausted(&self, provider_name: &str) -> Result<(), String> {
        let now = Utc::now().to_rfc3339();
        // Upsert so first-use quota failures land the flag even when the
        // provider has never produced a `provider_quotas` row (e.g.,
        // misconfigured quota_script that only ever fails, or a provider
        // whose first call returns quota_exhausted before any refresh has
        // succeeded). Previously a plain UPDATE silently dropped the write
        // for these cases, leaving the account eligible to be routed to
        // again on the next call.
        self.conn
            .execute(
                "INSERT INTO provider_quotas (provider_name, exhausted_at)
                 VALUES (?1, ?2)
                 ON CONFLICT (provider_name) DO UPDATE SET
                    exhausted_at = excluded.exhausted_at",
                sqlite::params![provider_name, &now],
            )
            .map_err(|e| format!("Failed to mark provider exhausted: {e}"))?;
        Ok(())
    }

    pub fn record_provider_unavailable(
        &self,
        provider_name: &str,
        next_available_at: Option<DateTime<Utc>>,
        failure_class: &str,
    ) -> Result<(), String> {
        let next_at = next_available_at.map(|ts| ts.to_rfc3339());
        self.conn
            .execute(
                "INSERT INTO provider_quotas
                    (provider_name, next_available_at, failure_class)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT (provider_name) DO UPDATE SET
                    next_available_at = excluded.next_available_at,
                    failure_class = excluded.failure_class",
                params![provider_name, next_at, failure_class],
            )
            .map_err(|e| format!("Failed to record provider unavailable: {e}"))?;
        Ok(())
    }

    pub fn clear_provider_unavailable(&self, provider_name: &str) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE provider_quotas
                 SET next_available_at = NULL,
                     failure_class = NULL
                 WHERE provider_name = ?1",
                params![provider_name],
            )
            .map_err(|e| format!("Failed to clear provider unavailable: {e}"))?;
        Ok(())
    }

    pub fn touch_provider_refresh(
        &self,
        provider_name: &str,
        now: DateTime<Utc>,
    ) -> Result<(), String> {
        let ts = now.to_rfc3339();
        self.conn
            .execute(
                "INSERT INTO provider_quotas (provider_name, last_refresh_at)
                 VALUES (?1, ?2)
                 ON CONFLICT (provider_name) DO UPDATE SET
                    last_refresh_at = excluded.last_refresh_at",
                params![provider_name, ts],
            )
            .map_err(|e| format!("Failed to touch provider refresh: {e}"))?;
        Ok(())
    }

    pub fn clear_exhausted(&self, provider_name: &str) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE provider_quotas SET exhausted_at = NULL WHERE provider_name = ?1",
                sqlite::params![provider_name],
            )
            .map_err(|e| format!("Failed to clear provider exhausted flag: {e}"))?;
        Ok(())
    }

    pub fn next_round_robin_index_for_model(
        &self,
        model_name: &str,
    ) -> Result<Option<usize>, String> {
        let result = self.conn.query_row(
            "SELECT last_index FROM model_round_robin_cursor WHERE model_name = ?1",
            params![model_name],
            |row| row.get::<_, i64>(0),
        );
        match result {
            Ok(value) => usize::try_from(value)
                .map(Some)
                .map_err(|_| format!("Negative round-robin cursor for {model_name}")),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("Failed to query round-robin cursor: {e}")),
        }
    }

    pub fn advance_round_robin_index(
        &self,
        model_name: &str,
        new_index: usize,
        now: DateTime<Utc>,
    ) -> Result<(), String> {
        let ts = now.to_rfc3339();
        self.conn
            .execute(
                "INSERT INTO model_round_robin_cursor (model_name, last_index, updated_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT (model_name) DO UPDATE SET
                    last_index = excluded.last_index,
                    updated_at = excluded.updated_at",
                params![model_name, new_index as i64, ts],
            )
            .map_err(|e| format!("Failed to advance round-robin cursor: {e}"))?;
        Ok(())
    }

    /// Record a freshly-fetched set of quota windows. Computes per-window
    /// deltas for percent-per-turn learning. Resets `calls_since_refresh` to 0.
    ///
    /// Windows are replaced wholesale: anything not in `windows` is deleted,
    /// so a script that drops a window (e.g. CLI removed a rate limit) stops
    /// contributing to density scoring.
    pub fn upsert_quota_refresh(
        &self,
        provider_name: &str,
        windows: &[QuotaWindowInput],
    ) -> Result<(), String> {
        let now = Utc::now().to_rfc3339();

        let prior = self.get_quota(provider_name)?;
        let prior_windows = self.get_windows(provider_name)?;

        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to begin tx: {e}"))?;

        if windows.is_empty() {
            return Self::record_empty_quota_refresh(tx, provider_name, &now, &prior_windows);
        }

        let turns_between_refreshes =
            self.turns_between_quota_refreshes(provider_name, prior.as_ref());
        let prior_windows_by_id = Self::quota_windows_by_id(&prior_windows);
        let projection = Self::quota_aggregate_projection(prior.as_ref(), windows);
        Self::write_quota_aggregate(&tx, provider_name, &now, projection)?;
        Self::replace_quota_window_rows(
            &tx,
            provider_name,
            windows,
            &prior_windows_by_id,
            turns_between_refreshes,
        )?;
        tx.commit()
            .map_err(|e| format!("Failed to commit refresh: {e}"))?;
        Ok(())
    }

    fn record_empty_quota_refresh(
        tx: sqlite::Transaction<'_>,
        provider_name: &str,
        now: &str,
        prior_windows: &[QuotaWindow],
    ) -> Result<(), String> {
        Self::write_empty_quota_refresh(&tx, provider_name, now, prior_windows)?;
        tx.commit()
            .map_err(|e| format!("Failed to commit refresh: {e}"))
    }

    fn write_empty_quota_refresh(
        conn: &sqlite::Connection,
        provider_name: &str,
        now: &str,
        prior_windows: &[QuotaWindow],
    ) -> Result<(), String> {
        if prior_windows.is_empty() {
            Self::write_initial_empty_quota_refresh(conn, provider_name, now)
        } else {
            Self::write_preserving_empty_quota_refresh(conn, provider_name, now)
        }
    }

    fn write_initial_empty_quota_refresh(
        conn: &sqlite::Connection,
        provider_name: &str,
        now: &str,
    ) -> Result<(), String> {
        conn.execute(
            "INSERT INTO provider_quotas
                (provider_name, refreshed_at, last_empty_refresh_at)
             VALUES (?1, ?2, ?2)
             ON CONFLICT (provider_name) DO UPDATE SET
                refreshed_at = ?2,
                last_empty_refresh_at = ?2",
            sqlite::params![provider_name, now],
        )
        .map_err(|e| format!("Failed to record empty quota refresh: {e}"))?;
        Ok(())
    }

    fn write_preserving_empty_quota_refresh(
        conn: &sqlite::Connection,
        provider_name: &str,
        now: &str,
    ) -> Result<(), String> {
        conn.execute(
            "INSERT INTO provider_quotas
                (provider_name, refreshed_at, last_empty_refresh_at)
             VALUES (?1, ?2, ?2)
             ON CONFLICT (provider_name) DO UPDATE SET
                last_empty_refresh_at = ?2",
            sqlite::params![provider_name, now],
        )
        .map_err(|e| format!("Failed to record empty quota refresh: {e}"))?;
        Ok(())
    }

    fn turns_between_quota_refreshes(
        &self,
        provider_name: &str,
        prior: Option<&QuotaRecord>,
    ) -> u64 {
        prior
            .map(|p| {
                self.count_assistant_turns_since(provider_name, p.refreshed_at.as_ref())
                    .unwrap_or(p.calls_since_refresh)
            })
            .unwrap_or(0)
    }

    fn quota_windows_by_id(windows: &[QuotaWindow]) -> HashMap<u32, &QuotaWindow> {
        windows
            .iter()
            .map(|window| (window.window_id, window))
            .collect()
    }

    fn quota_aggregate_projection(
        prior: Option<&QuotaRecord>,
        windows: &[QuotaWindowInput],
    ) -> QuotaAggregateProjection {
        let (legacy_used, legacy_resets) = Self::legacy_quota_projection(windows);
        QuotaAggregateProjection {
            legacy_used,
            legacy_resets,
            topology_peak_live_window_count: Self::quota_topology_peak(prior, windows),
        }
    }

    fn legacy_quota_projection(windows: &[QuotaWindowInput]) -> (f64, Option<String>) {
        match windows.iter().max_by_key(|window| window.resets_at) {
            Some(window) => (window.used_percent, Some(window.resets_at.to_rfc3339())),
            None => (0.0, None),
        }
    }

    fn quota_topology_peak(prior: Option<&QuotaRecord>, windows: &[QuotaWindowInput]) -> i64 {
        prior
            .map(|quota| quota.topology_peak_live_window_count)
            .unwrap_or(0)
            .max(windows.len()) as i64
    }

    fn write_quota_aggregate(
        conn: &sqlite::Connection,
        provider_name: &str,
        now: &str,
        projection: QuotaAggregateProjection,
    ) -> Result<(), String> {
        conn.execute(
            "INSERT INTO provider_quotas
                (provider_name, used_percent, resets_at, calls_since_refresh, refreshed_at,
                 topology_peak_live_window_count)
             VALUES (?1, ?2, ?3, 0, ?4, ?5)
             ON CONFLICT (provider_name) DO UPDATE SET
                used_percent = ?2,
                resets_at = ?3,
                calls_since_refresh = 0,
                refreshed_at = ?4,
                exhausted_at = NULL,
                topology_peak_live_window_count = MAX(topology_peak_live_window_count, ?5)",
            sqlite::params![
                provider_name,
                projection.legacy_used,
                projection.legacy_resets,
                now,
                projection.topology_peak_live_window_count
            ],
        )
        .map_err(|e| format!("Failed to upsert quota: {e}"))?;
        Ok(())
    }

    fn replace_quota_window_rows(
        conn: &sqlite::Connection,
        provider_name: &str,
        windows: &[QuotaWindowInput],
        prior_windows_by_id: &HashMap<u32, &QuotaWindow>,
        turns_between_refreshes: u64,
    ) -> Result<(), String> {
        Self::delete_quota_window_rows(conn, provider_name)?;
        Self::insert_quota_window_rows(
            conn,
            provider_name,
            windows,
            prior_windows_by_id,
            turns_between_refreshes,
        )
    }

    fn delete_quota_window_rows(
        conn: &sqlite::Connection,
        provider_name: &str,
    ) -> Result<(), String> {
        conn.execute(
            "DELETE FROM provider_quota_windows WHERE provider_name = ?1",
            sqlite::params![provider_name],
        )
        .map_err(|e| format!("Failed to clear windows: {e}"))?;
        Ok(())
    }

    fn insert_quota_window_rows(
        conn: &sqlite::Connection,
        provider_name: &str,
        windows: &[QuotaWindowInput],
        prior_windows_by_id: &HashMap<u32, &QuotaWindow>,
        turns_between_refreshes: u64,
    ) -> Result<(), String> {
        for (index, window) in windows.iter().enumerate() {
            let delta = Self::quota_window_delta(
                window,
                prior_windows_by_id.get(&(index as u32)).copied(),
                turns_between_refreshes,
            );
            Self::insert_quota_window_row(conn, provider_name, index, window, delta)?;
        }
        Ok(())
    }

    fn quota_window_delta(
        window: &QuotaWindowInput,
        prior_window: Option<&QuotaWindow>,
        turns_between_refreshes: u64,
    ) -> QuotaWindowDelta {
        match prior_window {
            Some(prior) => {
                Self::classify_quota_window_delta(window, prior, turns_between_refreshes)
            }
            None => QuotaWindowDelta {
                last_delta_percent: None,
                last_delta_calls: None,
            },
        }
    }

    fn classify_quota_window_delta(
        window: &QuotaWindowInput,
        prior_window: &QuotaWindow,
        turns_between_refreshes: u64,
    ) -> QuotaWindowDelta {
        let delta_percent = (window.used_percent - prior_window.used_percent).max(0.0);
        if Self::quota_delta_sample_is_learnable(delta_percent, window, turns_between_refreshes) {
            QuotaWindowDelta {
                last_delta_percent: Some(delta_percent),
                last_delta_calls: Some(turns_between_refreshes),
            }
        } else {
            QuotaWindowDelta {
                last_delta_percent: prior_window.last_delta_percent,
                last_delta_calls: prior_window.last_delta_calls,
            }
        }
    }

    fn quota_delta_sample_is_learnable(
        delta_percent: f64,
        window: &QuotaWindowInput,
        turns_between_refreshes: u64,
    ) -> bool {
        delta_percent > 0.0
            && !Self::quota_delta_sample_is_small(turns_between_refreshes)
            && !Self::quota_window_is_near_rail(window)
            && !Self::quota_delta_rate_too_high(delta_percent, turns_between_refreshes)
    }

    fn quota_delta_sample_is_small(turns_between_refreshes: u64) -> bool {
        turns_between_refreshes < MIN_LEARN_SAMPLE_CALLS
    }

    fn quota_window_is_near_rail(window: &QuotaWindowInput) -> bool {
        window.used_percent >= NEAR_EXHAUSTED_USED_PERCENT
    }

    fn quota_delta_rate_too_high(delta_percent: f64, turns_between_refreshes: u64) -> bool {
        Self::quota_delta_rate(delta_percent, turns_between_refreshes) > MAX_LEARNABLE_BURN_RATE
    }

    fn quota_delta_rate(delta_percent: f64, turns_between_refreshes: u64) -> f64 {
        if turns_between_refreshes > 0 {
            delta_percent / (turns_between_refreshes as f64)
        } else {
            f64::INFINITY
        }
    }

    fn insert_quota_window_row(
        conn: &sqlite::Connection,
        provider_name: &str,
        index: usize,
        window: &QuotaWindowInput,
        delta: QuotaWindowDelta,
    ) -> Result<(), String> {
        conn.execute(
            "INSERT INTO provider_quota_windows
                (provider_name, window_id, used_percent, resets_at,
                 last_delta_percent, last_delta_calls)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            sqlite::params![
                provider_name,
                index as i64,
                window.used_percent,
                window.resets_at.to_rfc3339(),
                delta.last_delta_percent,
                delta.last_delta_calls.map(|value| value as i64),
            ],
        )
        .map_err(|e| format!("Failed to insert window: {e}"))?;
        Ok(())
    }

    pub fn record_topology_probe(&self, provider_name: &str) -> Result<(), String> {
        let now = Utc::now().to_rfc3339();
        self.conn
            .execute(
                "INSERT INTO provider_quotas (provider_name, last_topology_probe_at)
                 VALUES (?1, ?2)
                 ON CONFLICT (provider_name) DO UPDATE SET
                    last_topology_probe_at = excluded.last_topology_probe_at",
                sqlite::params![provider_name, &now],
            )
            .map_err(|e| format!("Failed to record topology probe: {e}"))?;
        Ok(())
    }

    /// Test-only: backdate a provider's `refreshed_at` so tests can seed
    /// turns whose timestamps are "after" the refresh.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_refreshed_at_for_test(
        &self,
        provider_name: &str,
        refreshed_at: &DateTime<Utc>,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE provider_quotas SET refreshed_at = ?1 WHERE provider_name = ?2",
                sqlite::params![refreshed_at.to_rfc3339(), provider_name],
            )
            .map_err(|e| format!("Failed to set refreshed_at: {e}"))?;
        Ok(())
    }

    /// Test-only: backdate a provider's `last_invoked_at` so tests can seed
    /// last-used recency buckets.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_last_invoked_at_for_test(
        &self,
        model_name: &str,
        provider_name: &str,
        last_invoked_at: &DateTime<Utc>,
    ) -> Result<(), String> {
        let updated =
            self.write_last_invoked_at_for_test(model_name, provider_name, last_invoked_at)?;
        Self::validate_last_invoked_at_test_update(model_name, provider_name, updated)
    }

    #[cfg(any(test, feature = "test-support"))]
    fn write_last_invoked_at_for_test(
        &self,
        model_name: &str,
        provider_name: &str,
        last_invoked_at: &DateTime<Utc>,
    ) -> Result<usize, String> {
        self.conn
            .execute(
                "UPDATE providers
                 SET last_invoked_at = ?1
                 WHERE model_name = ?2 AND provider_name = ?3",
                sqlite::params![last_invoked_at.to_rfc3339(), model_name, provider_name],
            )
            .map_err(|e| format!("Failed to set last_invoked_at: {e}"))
    }

    #[cfg(any(test, feature = "test-support"))]
    fn validate_last_invoked_at_test_update(
        model_name: &str,
        provider_name: &str,
        updated: usize,
    ) -> Result<(), String> {
        if updated == 1 {
            Ok(())
        } else {
            Err(format!(
                "Expected exactly one providers row for model_name={model_name}, provider_name={provider_name}, updated {updated}"
            ))
        }
    }

    /// Test-only: seed the PR 3 per-window burn-rate learning columns without
    /// adding a migration here. This intentionally fails at runtime until the
    /// production schema owns these columns.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_window_delta_for_test(
        &self,
        provider_name: &str,
        window_id: u32,
        last_delta_percent: f64,
        last_delta_calls: u64,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE provider_quota_windows
                 SET last_delta_percent = ?3,
                     last_delta_calls = ?4
                 WHERE provider_name = ?1 AND window_id = ?2",
                sqlite::params![
                    provider_name,
                    window_id as i64,
                    last_delta_percent,
                    last_delta_calls as i64
                ],
            )
            .map_err(|e| format!("Failed to set window delta: {e}"))?;
        Ok(())
    }

    /// Test-only: seed a provider quota row without any window rows.
    #[cfg(any(test, feature = "test-support"))]
    pub fn insert_quota_row_without_windows_for_test(
        &self,
        provider_name: &str,
        refreshed_at: &DateTime<Utc>,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO provider_quotas
                    (provider_name, used_percent, resets_at, calls_since_refresh, refreshed_at)
                 VALUES (?1, 0, NULL, 0, ?2)
                 ON CONFLICT (provider_name) DO UPDATE SET
                    used_percent = 0,
                    resets_at = NULL,
                    calls_since_refresh = 0,
                    refreshed_at = ?2",
                sqlite::params![provider_name, refreshed_at.to_rfc3339()],
            )
            .map_err(|e| format!("Failed to insert quota row: {e}"))?;
        self.conn
            .execute(
                "DELETE FROM provider_quota_windows WHERE provider_name = ?1",
                sqlite::params![provider_name],
            )
            .map_err(|e| format!("Failed to clear quota windows: {e}"))?;
        Ok(())
    }

    /// Test-only: make a cached quota row unreadable through the public
    /// `get_quota` API by writing a storage value that production parsing
    /// rejects.
    #[cfg(any(test, feature = "test-support"))]
    pub fn force_unreadable_cached_quota_for_test(
        &self,
        provider_name: &str,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE provider_quotas
                 SET topology_peak_live_window_count = -1
                 WHERE provider_name = ?1",
                sqlite::params![provider_name],
            )
            .map_err(|e| format!("Failed to force unreadable cached quota: {e}"))?;
        Ok(())
    }

    /// Test-only: make cached window rows unreadable through the public
    /// `get_windows` API by writing a timestamp value that strict window
    /// parsing rejects.
    #[cfg(any(test, feature = "test-support"))]
    pub fn force_unreadable_cached_windows_for_test(
        &self,
        provider_name: &str,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE provider_quota_windows
                 SET resets_at = 'not-rfc3339'
                 WHERE provider_name = ?1",
                sqlite::params![provider_name],
            )
            .map_err(|e| format!("Failed to force unreadable cached windows: {e}"))?;
        Ok(())
    }

    /// Bump `calls_since_refresh` for a provider. Creates the row with 1 call
    /// and zeroed quota if the provider isn't tracked yet.
    pub fn increment_calls_since_refresh(&self, provider_name: &str) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO provider_quotas (provider_name, calls_since_refresh)
                 VALUES (?1, 1)
                 ON CONFLICT (provider_name) DO UPDATE SET
                    calls_since_refresh = calls_since_refresh + 1",
                sqlite::params![provider_name],
            )
            .map_err(|e| format!("Failed to increment calls_since_refresh: {e}"))?;
        Ok(())
    }

    // --- Session log ingestion ---

    /// Insert one parsed turn. Idempotent: re-running a scan against an
    /// unchanged log is a no-op for already-seen turns.
    pub fn ingest_session_turn(
        &self,
        provider_name: &str,
        session_id: &str,
        turn_id: &str,
        timestamp: &DateTime<Utc>,
        role: &str,
        source_file: &str,
    ) -> Result<bool, String> {
        let now = Utc::now().to_rfc3339();
        let ts = timestamp.to_rfc3339();
        let changed = self
            .conn
            .execute(
                "INSERT OR IGNORE INTO session_turns
                    (provider_name, session_id, turn_id, timestamp, role, is_compaction_boundary, source_file, ingested_at, body)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?8)",
                sqlite::params![
                    provider_name,
                    session_id,
                    turn_id,
                    &ts,
                    role,
                    source_file,
                    &now,
                    Option::<&str>::None,
                ],
            )
            .map_err(|e| format!("Failed to ingest session turn: {e}"))?;
        Ok(changed > 0)
    }

    /// Bulk-insert turns inside a single transaction with a prepared
    /// statement. Hundreds of thousands of rows go from minutes to seconds
    /// vs the per-row method. Returns the count of newly-inserted rows
    /// (duplicates collapsed by the UNIQUE constraint don't count).
    pub fn ingest_session_turns_batch(
        &self,
        provider_name: &str,
        turns: &[SessionTurnIngest],
    ) -> Result<u64, String> {
        if Self::session_turn_batch_is_empty(turns) {
            return Ok(0);
        }
        let now = Utc::now().to_rfc3339();
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to begin transaction: {e}"))?;
        let new_count = Self::insert_session_turn_batch_rows(&tx, provider_name, turns, &now)?;
        tx.commit()
            .map_err(|e| format!("Failed to commit batch: {e}"))?;
        Ok(new_count)
    }

    fn session_turn_batch_is_empty(turns: &[SessionTurnIngest]) -> bool {
        turns.is_empty()
    }

    fn insert_session_turn_batch_rows(
        conn: &sqlite::Connection,
        provider_name: &str,
        turns: &[SessionTurnIngest],
        ingested_at: &str,
    ) -> Result<u64, String> {
        let mut stmt = Self::prepare_session_turn_batch_insert(conn)?;
        Self::execute_session_turn_writes(&mut stmt, provider_name, turns, ingested_at)
    }

    fn prepare_session_turn_batch_insert(
        conn: &sqlite::Connection,
    ) -> Result<sqlite::Statement<'_>, String> {
        conn.prepare(Self::session_turn_batch_insert_sql())
            .map_err(Self::format_session_turn_prepare_error)
    }

    fn execute_session_turn_writes(
        stmt: &mut sqlite::Statement<'_>,
        provider_name: &str,
        turns: &[SessionTurnIngest],
        ingested_at: &str,
    ) -> Result<u64, String> {
        let mut new_count: u64 = 0;
        for turn in turns {
            let binds = Self::bind_session_turn_row_params(turn);
            let n =
                Self::execute_session_turn_batch_insert(stmt, provider_name, &binds, ingested_at)?;
            new_count += n as u64;
        }
        Ok(new_count)
    }

    fn format_session_turn_prepare_error(err: sqlite::Error) -> String {
        format!("Failed to prepare batch insert: {err}")
    }

    fn session_turn_batch_insert_sql() -> &'static str {
        "INSERT OR IGNORE INTO session_turns
            (
                provider_name,
                session_id,
                turn_id,
                timestamp,
                role,
                parent_turn_id,
                is_sidechain,
                is_compaction_boundary,
                source_file,
                ingested_at,
                body
            )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, '', ?9, ?10)"
    }

    fn bind_session_turn_row_params(turn: &SessionTurnIngest) -> SessionTurnBindValues<'_> {
        SessionTurnBindValues {
            session_id: &turn.session_id,
            turn_id: &turn.turn_id,
            timestamp: turn.timestamp.to_rfc3339(),
            role: &turn.role,
            parent_turn_id: turn.parent_turn_id.as_deref(),
            is_sidechain: Self::sqlite_bool(turn.is_sidechain),
            is_compaction_boundary: Self::sqlite_bool(turn.is_compaction_boundary),
            body: turn.body.as_deref(),
        }
    }

    fn sqlite_bool(value: bool) -> i64 {
        if value { 1 } else { 0 }
    }

    fn execute_session_turn_batch_insert(
        stmt: &mut sqlite::Statement<'_>,
        provider_name: &str,
        binds: &SessionTurnBindValues<'_>,
        ingested_at: &str,
    ) -> Result<usize, String> {
        stmt.execute(sqlite::params![
            provider_name,
            binds.session_id,
            binds.turn_id,
            &binds.timestamp,
            binds.role,
            binds.parent_turn_id,
            binds.is_sidechain,
            binds.is_compaction_boundary,
            ingested_at,
            binds.body,
        ])
        .map_err(|e| format!("Batch insert row failed: {e}"))
    }

    pub fn count_session_turns(
        &self,
        provider_name: &str,
        session_id: &str,
    ) -> Result<SessionTurnCounts, String> {
        let (total, assistant, sidechain): (i64, i64, i64) = self
            .conn
            .query_row(
                "SELECT
                    COUNT(*) AS total,
                    COUNT(CASE WHEN role = 'assistant' THEN 1 END) AS assistant,
                    COUNT(CASE WHEN is_sidechain = 1 THEN 1 END) AS sidechain
                 FROM session_turns
                 WHERE provider_name = ?1 AND session_id = ?2",
                sqlite::params![provider_name, session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|e| format!("Failed to count session turns for trace: {e}"))?;

        Ok(SessionTurnCounts {
            total: total.max(0) as u64,
            assistant: assistant.max(0) as u64,
            sidechain: sidechain.max(0) as u64,
        })
    }

    pub fn has_session_user_text_turn(
        &self,
        provider_name: &str,
        session_id: &str,
        text: &str,
    ) -> Result<bool, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT body
                 FROM session_turns
                 WHERE provider_name = ?1
                   AND session_id = ?2
                   AND role = 'user'
                   AND body IS NOT NULL",
            )
            .map_err(|e| format!("Failed to prepare session user turn lookup: {e}"))?;
        let rows = stmt
            .query_map(sqlite::params![provider_name, session_id], |row| {
                Self::session_user_turn_body(row)
            })
            .map_err(|e| format!("Failed to query session user turns: {e}"))?;

        for row in rows {
            let body = row.map_err(|e| format!("Failed to read session user turn body: {e}"))?;
            if Self::session_user_turn_body_matches(&body, text) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn has_session_user_turn_containing(
        &self,
        provider_name: &str,
        session_id: &str,
        needle: &str,
    ) -> Result<bool, String> {
        if needle.is_empty() {
            return Ok(false);
        }
        let found: i64 = self
            .conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1
                    FROM session_turns
                    WHERE provider_name = ?1
                      AND session_id = ?2
                      AND role = 'user'
                      AND body IS NOT NULL
                      AND instr(body, ?3) > 0
                )",
                sqlite::params![provider_name, session_id, needle],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to query session user turn substring: {e}"))?;
        Ok(found != 0)
    }

    fn session_user_turn_body(row: &sqlite::Row<'_>) -> sqlite::Result<String> {
        row.get(0)
    }

    fn session_user_turn_body_matches(body: &str, text: &str) -> bool {
        Self::session_turn_body_has_exact_text(body, text)
    }

    fn session_turn_body_has_exact_text(body: &str, text: &str) -> bool {
        Self::parse_session_turn_body(body)
            .as_ref()
            .is_some_and(|value| Self::parsed_session_turn_body_has_exact_text(value, text))
    }

    fn parse_session_turn_body(body: &str) -> Option<serde_json::Value> {
        serde_json::from_str::<serde_json::Value>(body).ok()
    }

    fn parsed_session_turn_body_has_exact_text(body: &serde_json::Value, text: &str) -> bool {
        Self::canonical_body_has_exact_text(body, text)
    }

    fn canonical_body_has_exact_text(body: &serde_json::Value, text: &str) -> bool {
        let serde_json::Value::Array(chunks) = body else {
            return false;
        };
        let canonical_text =
            Self::canonical_text_from_chunks(Self::session_turn_text_chunks(chunks));
        Self::canonical_text_equals(canonical_text.as_deref(), text)
    }

    fn session_turn_text_chunks(
        chunks: &[serde_json::Value],
    ) -> impl Iterator<Item = &serde_json::Value> + '_ {
        chunks
            .iter()
            .filter(|chunk| Self::session_turn_chunk_is_text(chunk))
    }

    fn canonical_text_from_chunks<'a>(
        chunks: impl Iterator<Item = &'a serde_json::Value>,
    ) -> Option<String> {
        let mut canonical_text = String::new();
        let mut has_text = false;
        for chunk in chunks {
            if let Some(candidate) = chunk.get("text").and_then(serde_json::Value::as_str) {
                canonical_text.push_str(candidate);
                has_text = true;
            }
        }
        has_text.then_some(canonical_text)
    }

    fn canonical_text_equals(candidate: Option<&str>, text: &str) -> bool {
        candidate == Some(text)
    }

    fn session_turn_chunk_is_text(chunk: &serde_json::Value) -> bool {
        chunk
            .get("type")
            .and_then(serde_json::Value::as_str)
            .is_none_or(|chunk_type| chunk_type == "text")
    }

    pub fn backfill_session_chains(&self) -> Result<BackfillReport, DbError> {
        if Self::session_chains_backfill_exists(&self.conn)? {
            return Ok(BackfillReport {
                skipped_existing: true,
                chains_inserted: 0,
                segments_inserted: 0,
            });
        }

        let rows = Self::load_session_chain_backfill_rows(&self.conn)?;

        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to begin session chain backfill: {e}"))?;
        let provider_session_expr = Self::provider_session_expr(&tx, None)?;
        let mut chains_inserted = 0;
        let mut segments_inserted = 0;
        for row in rows {
            let model_name = Self::infer_model_for_backfill_row(&tx, &provider_session_expr, &row)?;
            let chain_id = Uuid::new_v4().to_string();
            chains_inserted += Self::insert_backfill_chain(&tx, &chain_id, &row, &model_name)?;
            segments_inserted += Self::insert_backfill_segment(&tx, &chain_id, &row)?;
        }
        tx.commit()
            .map_err(|e| format!("Failed to commit session chain backfill: {e}"))?;
        Ok(BackfillReport {
            skipped_existing: false,
            chains_inserted,
            segments_inserted,
        })
    }

    fn session_chains_backfill_exists(conn: &sqlite::Connection) -> Result<bool, DbError> {
        let exists: i64 = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session_chains LIMIT 1)",
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to check session chain backfill state: {e}"))?;
        Ok(exists != 0)
    }

    fn load_session_chain_backfill_rows(
        conn: &sqlite::Connection,
    ) -> Result<Vec<SessionChainBackfillRow>, DbError> {
        let mut stmt = conn
            .prepare(
                "SELECT st.provider_name,
                        st.session_id,
                        MIN(st.timestamp) AS started_at,
                        MAX(st.timestamp) AS last_used_at,
                        (
                            SELECT st2.turn_id
                            FROM session_turns st2
                            WHERE st2.provider_name = st.provider_name
                              AND st2.session_id = st.session_id
                            ORDER BY st2.timestamp DESC, st2.id DESC
                            LIMIT 1
                        ) AS last_turn_id
                 FROM session_turns st
                 GROUP BY st.provider_name, st.session_id",
            )
            .map_err(|e| format!("Failed to prepare session chain backfill: {e}"))?;
        let iter = stmt
            .query_map([], Self::map_session_chain_backfill_row)
            .map_err(|e| format!("Failed to query session chain backfill rows: {e}"))?;
        iter.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read session chain backfill rows: {e}"))
    }

    fn map_session_chain_backfill_row(
        row: &sqlite::Row<'_>,
    ) -> sqlite::Result<SessionChainBackfillRow> {
        Ok(SessionChainBackfillRow {
            provider: row.get(0)?,
            session: row.get(1)?,
            started_at: row.get(2)?,
            last_used_at: row.get(3)?,
            last_turn_id: row.get(4)?,
        })
    }

    fn infer_model_for_backfill_row(
        conn: &sqlite::Connection,
        provider_session_expr: &str,
        row: &SessionChainBackfillRow,
    ) -> Result<String, DbError> {
        let model_sql = Self::backfill_model_lookup_sql(provider_session_expr);
        let model_name = Self::lookup_model_for_backfill_row(conn, &model_sql, row)?;
        Ok(Self::default_backfill_model_name(model_name))
    }

    fn backfill_model_lookup_sql(provider_session_expr: &str) -> String {
        format!(
            "SELECT model_name
             FROM invocations
             WHERE {provider_session_expr} = ?1
             ORDER BY COALESCE(finished_at, created_at) DESC, id DESC
             LIMIT 1"
        )
    }

    fn lookup_model_for_backfill_row(
        conn: &sqlite::Connection,
        model_sql: &str,
        row: &SessionChainBackfillRow,
    ) -> Result<Option<String>, DbError> {
        conn.query_row(model_sql, sqlite::params![row.session], |r| {
            r.get::<_, String>(0)
        })
        .optional()
        .map_err(|e| format!("Failed to infer model during backfill: {e}"))
    }

    fn default_backfill_model_name(model_name: Option<String>) -> String {
        model_name.unwrap_or_else(|| "<unknown>".to_string())
    }

    fn insert_backfill_chain(
        conn: &sqlite::Connection,
        chain_id: &str,
        row: &SessionChainBackfillRow,
        model_name: &str,
    ) -> Result<u64, DbError> {
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, ?2, ?3, ?4)",
            sqlite::params![chain_id, row.started_at, row.last_used_at, model_name],
        )
        .map(|changed| changed as u64)
        .map_err(|e| format!("Failed to insert session chain during backfill: {e}"))
    }

    fn insert_backfill_segment(
        conn: &sqlite::Connection,
        chain_id: &str,
        row: &SessionChainBackfillRow,
    ) -> Result<u64, DbError> {
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, ended_at, last_turn_id, transition_reason)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, 'imported')",
            sqlite::params![chain_id, row.provider, row.session, row.started_at, row.last_turn_id],
        )
        .map(|changed| changed as u64)
        .map_err(|e| format!("Failed to insert session chain segment during backfill: {e}"))
    }

    pub fn open_chain_segment(
        &self,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
        started_at: &DateTime<Utc>,
        reason: TransitionReason,
    ) -> Result<i64, DbError> {
        Self::upsert_open_chain_segment(
            &self.conn,
            chain_id,
            provider_name,
            session_id,
            &started_at.to_rfc3339(),
            reason,
        )?;
        Self::read_open_chain_segment_id(&self.conn, chain_id, provider_name, session_id)
    }

    pub fn rotate_chain_segment_transactionally(
        &self,
        input: ChainSegmentRotationInput<'_>,
    ) -> Result<(i64, i64), DbError> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to begin session chain rotation transaction: {e}"))?;
        let closed_id = Self::close_expected_active_segment_returning_on(
            &tx,
            input.chain_id,
            input.source_provider_name,
            input.source_session_id,
            input.changed_at,
        )?
        .ok_or_else(|| "validated source segment was no longer active".to_string())?;
        Self::upsert_open_chain_segment(
            &tx,
            input.chain_id,
            input.target_provider_name,
            input.target_session_id,
            &input.changed_at.to_rfc3339(),
            input.reason,
        )?;
        let opened_id = Self::read_open_chain_segment_id(
            &tx,
            input.chain_id,
            input.target_provider_name,
            input.target_session_id,
        )?;
        tx.commit()
            .map_err(|e| format!("Failed to commit session chain rotation transaction: {e}"))?;
        Ok((closed_id, opened_id))
    }

    pub fn active_chain_segment_snapshot(
        &self,
        chain_id: &str,
    ) -> Result<Option<ActiveChainSegmentSnapshot>, DbError> {
        self.conn
            .query_row(
                "SELECT sc.chain_id,
                        s.provider_name,
                        s.session_id,
                        s.started_at,
                        s.ended_at,
                        s.last_turn_id,
                        (
                            SELECT st.timestamp
                            FROM session_turns st
                            WHERE st.provider_name = s.provider_name
                              AND st.session_id = s.session_id
                            ORDER BY st.timestamp DESC, st.id DESC
                            LIMIT 1
                        )
                 FROM session_chains sc
                 JOIN session_chain_segments s ON s.chain_id = sc.chain_id
                 WHERE sc.chain_id = ?1 AND s.ended_at IS NULL
                 ORDER BY s.started_at DESC, s.id DESC
                 LIMIT 1",
                params![chain_id],
                |row| {
                    Ok(ActiveChainSegmentSnapshot {
                        chain_id: row.get(0)?,
                        active_provider: row.get(1)?,
                        active_session_id: row.get(2)?,
                        active_started_at: row.get(3)?,
                        active_ended_at: row.get(4)?,
                        active_last_turn_id: row.get(5)?,
                        latest_turn_at: row.get(6)?,
                    })
                },
            )
            .optional()
            .map_err(|e| format!("Failed to read active chain segment snapshot: {e}"))
    }

    fn upsert_open_chain_segment(
        conn: &sqlite::Connection,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
        started_at: &str,
        reason: TransitionReason,
    ) -> Result<(), DbError> {
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (chain_id, provider_name, session_id)
             DO UPDATE SET
                started_at = excluded.started_at,
                ended_at = NULL,
                last_turn_id = NULL,
                transition_reason = excluded.transition_reason",
            sqlite::params![
                chain_id,
                provider_name,
                session_id,
                started_at,
                reason.as_str()
            ],
        )
        .map_err(|e| format!("Failed to open session chain segment: {e}"))?;
        Ok(())
    }

    fn read_open_chain_segment_id(
        conn: &sqlite::Connection,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
    ) -> Result<i64, DbError> {
        conn.query_row(
            "SELECT id FROM session_chain_segments
                 WHERE chain_id = ?1 AND provider_name = ?2 AND session_id = ?3
                 ORDER BY id DESC LIMIT 1",
            sqlite::params![chain_id, provider_name, session_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Failed to read session chain segment id: {e}"))
    }

    pub fn find_conflicting_active_segment(
        &self,
        provider_name: &str,
        session_id: &str,
        own_chain_id: &str,
    ) -> Result<Option<String>, DbError> {
        self.conn
            .query_row(
                "SELECT chain_id
                 FROM session_chain_segments
                 WHERE provider_name = ?1
                   AND session_id = ?2
                   AND chain_id != ?3
                   AND ended_at IS NULL
                 ORDER BY started_at DESC, id DESC
                 LIMIT 1",
                sqlite::params![provider_name, session_id, own_chain_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to check conflicting active session segment: {e}"))
    }

    pub fn mint_imported_chain_if_absent(
        &self,
        provider_name: &str,
        session_id: &str,
        started_at: &DateTime<Utc>,
        model_name: &str,
    ) -> Result<(), DbError> {
        if Self::session_chain_segment_exists(&self.conn, provider_name, session_id)? {
            return Ok(());
        }
        let chain_id = Uuid::new_v4().to_string();
        let ts = started_at.to_rfc3339();
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to begin imported chain mint: {e}"))?;
        Self::insert_imported_chain(&tx, &chain_id, &ts, model_name)?;
        Self::insert_imported_segment(&tx, &chain_id, provider_name, session_id, &ts)?;
        tx.commit()
            .map_err(|e| format!("Failed to commit imported chain mint: {e}"))?;
        Ok(())
    }

    fn session_chain_segment_exists(
        conn: &sqlite::Connection,
        provider_name: &str,
        session_id: &str,
    ) -> Result<bool, DbError> {
        let exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM session_chain_segments
                 WHERE provider_name = ?1 AND session_id = ?2
                 LIMIT 1",
                sqlite::params![provider_name, session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to check existing session chain segment: {e}"))?;
        Ok(exists.is_some())
    }

    fn insert_imported_chain(
        conn: &sqlite::Connection,
        chain_id: &str,
        started_at: &str,
        model_name: &str,
    ) -> Result<(), DbError> {
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, ?2, ?2, ?3)
             ON CONFLICT DO NOTHING",
            sqlite::params![chain_id, started_at, model_name],
        )
        .map_err(|e| format!("Failed to mint imported session chain: {e}"))?;
        Ok(())
    }

    fn insert_imported_segment(
        conn: &sqlite::Connection,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
        started_at: &str,
    ) -> Result<(), DbError> {
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, ?4, 'imported')
             ON CONFLICT DO NOTHING",
            sqlite::params![chain_id, provider_name, session_id, started_at],
        )
        .map_err(|e| format!("Failed to mint imported session chain segment: {e}"))?;
        Ok(())
    }

    pub fn close_active_segment_returning(
        &self,
        chain_id: &str,
        ended_at: &DateTime<Utc>,
    ) -> Result<Option<i64>, DbError> {
        Self::close_active_segment_returning_on(&self.conn, chain_id, ended_at)
    }

    fn close_active_segment_returning_on(
        conn: &sqlite::Connection,
        chain_id: &str,
        ended_at: &DateTime<Utc>,
    ) -> Result<Option<i64>, DbError> {
        Self::close_matching_active_segment_returning_on(conn, chain_id, None, None, ended_at)
    }

    fn close_expected_active_segment_returning_on(
        conn: &sqlite::Connection,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
        ended_at: &DateTime<Utc>,
    ) -> Result<Option<i64>, DbError> {
        Self::close_matching_active_segment_returning_on(
            conn,
            chain_id,
            Some(provider_name),
            Some(session_id),
            ended_at,
        )
    }

    fn close_matching_active_segment_returning_on(
        conn: &sqlite::Connection,
        chain_id: &str,
        provider_name: Option<&str>,
        session_id: Option<&str>,
        ended_at: &DateTime<Utc>,
    ) -> Result<Option<i64>, DbError> {
        conn.query_row(
            "UPDATE session_chain_segments
             SET ended_at = ?2,
                 last_turn_id = (
                    SELECT st.turn_id
                    FROM session_turns st
                    WHERE st.provider_name = session_chain_segments.provider_name
                      AND st.session_id = session_chain_segments.session_id
                    ORDER BY st.timestamp DESC, st.id DESC
                    LIMIT 1
                 )
             WHERE chain_id = ?1
               AND ended_at IS NULL
               AND (?3 IS NULL OR provider_name = ?3)
               AND (?4 IS NULL OR session_id = ?4)
             RETURNING id",
            sqlite::params![chain_id, ended_at.to_rfc3339(), provider_name, session_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("Failed to close active session chain segment: {e}"))
    }

    pub fn update_chain_last_used(&self, chain_id: &str) -> Result<(), DbError> {
        self.conn
            .execute(
                "UPDATE session_chains SET last_used_at = ?2 WHERE chain_id = ?1",
                sqlite::params![chain_id, Utc::now().to_rfc3339()],
            )
            .map_err(|e| format!("Failed to update session chain last_used_at: {e}"))?;
        Ok(())
    }

    pub fn latest_compaction_boundary(
        &self,
        provider_name: &str,
        session_id: &str,
    ) -> Result<Option<(String, DateTime<Utc>)>, DbError> {
        let row = self
            .conn
            .query_row(
                "SELECT turn_id, timestamp
                 FROM session_turns
                 WHERE provider_name = ?1
                   AND session_id = ?2
                   AND is_compaction_boundary = 1
                 ORDER BY timestamp DESC, id DESC
                 LIMIT 1",
                sqlite::params![provider_name, session_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|e| format!("Failed to query latest compaction boundary: {e}"))?;
        row.map(|(turn_id, raw_ts)| {
            Self::strict_rfc3339_message(&raw_ts, "compaction boundary timestamp")
                .map(|timestamp| (turn_id, timestamp))
        })
        .transpose()
    }

    pub fn distinct_chain_segments(&self) -> Result<Vec<(String, String)>, DbError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT DISTINCT provider_name, session_id
                 FROM session_chain_segments
                 ORDER BY provider_name, session_id",
            )
            .map_err(|e| format!("Failed to prepare chain segment list: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| format!("Failed to query chain segment list: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read chain segment list: {e}"))
    }

    pub fn flag_compaction_boundary(
        &self,
        provider_name: &str,
        session_id: &str,
        turn_id: &str,
    ) -> Result<bool, DbError> {
        let changed = self
            .conn
            .execute(
                "UPDATE session_turns
                 SET is_compaction_boundary = 1
                 WHERE provider_name = ?1
                   AND session_id = ?2
                   AND turn_id = ?3
                   AND is_compaction_boundary = 0",
                sqlite::params![provider_name, session_id, turn_id],
            )
            .map_err(|e| format!("Failed to flag compaction boundary: {e}"))?;
        Ok(changed > 0)
    }

    pub fn resolve_resume(
        &self,
        models: &ModelStore,
        input: &str,
        model_override: Option<&str>,
    ) -> Result<ResolvedResume, ResumeError> {
        Self::validate_resume_input_id(input)?;
        self.reject_wrong_resume_id_kind(input)?;
        let chain_id = self.resolve_resume_chain_id(input)?;
        let (active_provider, active_session_id) = self.require_active_segment(&chain_id)?;
        let model_name = self.resolve_resume_model_name(&chain_id, model_override)?;
        let model =
            Self::resolve_resume_model_config(models, model_name.as_ref(), &active_provider)?;
        Ok(Self::assemble_resolved_resume(
            chain_id,
            model_name,
            model,
            active_provider,
            active_session_id,
        ))
    }

    fn validate_resume_input_id(input: &str) -> Result<(), ResumeError> {
        if Uuid::parse_str(input).is_ok() || Self::is_opencode_provider_session_id(input) {
            return Ok(());
        }

        Err(ResumeError::InvalidUuid {
            input: input.to_string(),
        })
    }

    fn is_opencode_provider_session_id(input: &str) -> bool {
        let Some(suffix) = input.strip_prefix(OPENCODE_SESSION_PREFIX) else {
            return false;
        };

        suffix.len() >= OPENCODE_SESSION_MIN_SUFFIX_LEN
            && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
    }

    fn reject_wrong_resume_id_kind(&self, input: &str) -> Result<(), ResumeError> {
        match self
            .wrong_id_kind_invocation_match(input)
            .map_err(|message| ResumeError::Db { message })?
        {
            Some(wrong_id) => Err(Self::wrong_id_kind_resume_error(input, wrong_id)),
            None => Ok(()),
        }
    }

    fn wrong_id_kind_resume_error(
        input: &str,
        wrong_id: WrongIdKindInvocationMatch,
    ) -> ResumeError {
        ResumeError::WrongIdKind {
            input: input.to_string(),
            input_kind: WrongIdKindInput::AgentRunnerInvocationId,
            provider_session_id: wrong_id.provider_session_id,
            agent_runner_invocation_id: wrong_id.invocation_uuid,
            chain_id: wrong_id.chain_id,
            provider_name: wrong_id.provider_name,
        }
    }

    fn resolve_resume_chain_id(&self, input: &str) -> Result<String, ResumeError> {
        let chain_ids = self
            .candidate_chain_ids(input)
            .map_err(|message| ResumeError::Db { message })?;
        Self::validate_resume_chain_candidates(input, &chain_ids)?;
        match self
            .choose_resume_chain(input, chain_ids)
            .map_err(|message| ResumeError::Db { message })?
        {
            Some(chain_id) => Ok(chain_id),
            None => Err(self.ambiguous_resume_error(input)?),
        }
    }

    fn validate_resume_chain_candidates(
        input: &str,
        chain_ids: &[String],
    ) -> Result<(), ResumeError> {
        if chain_ids.is_empty() {
            Err(ResumeError::NoChainFound {
                input: input.to_string(),
            })
        } else {
            Ok(())
        }
    }

    fn ambiguous_resume_error(&self, input: &str) -> Result<ResumeError, ResumeError> {
        let previews = self
            .chain_previews(input)
            .map_err(|message| ResumeError::Db { message })?;
        Ok(ResumeError::Ambiguous {
            input: input.to_string(),
            previews,
        })
    }

    fn require_active_segment(&self, chain_id: &str) -> Result<(String, String), ResumeError> {
        self.active_segment_for_chain(chain_id)
            .map_err(|message| ResumeError::Db { message })?
            .ok_or_else(|| ResumeError::ActiveSegmentMissing {
                chain_id: chain_id.to_string(),
            })
    }

    fn resolve_resume_model_name(
        &self,
        chain_id: &str,
        model_override: Option<&str>,
    ) -> Result<Option<String>, ResumeError> {
        match model_override {
            Some(model_name) => Ok(Some(model_name.to_string())),
            None => self.infer_resume_model_name(chain_id),
        }
    }

    fn infer_resume_model_name(&self, chain_id: &str) -> Result<Option<String>, ResumeError> {
        let latest_invocation = self
            .latest_invocation_model_for_chain(chain_id)
            .map_err(|message| ResumeError::Db { message })?;
        let chain_model = self
            .chain_model_name(chain_id)
            .map_err(|message| ResumeError::Db { message })?;
        Ok(Self::first_known_resume_model_name(
            latest_invocation,
            chain_model,
        ))
    }

    fn first_known_resume_model_name(
        latest_invocation: Option<String>,
        chain_model: Option<String>,
    ) -> Option<String> {
        latest_invocation
            .filter(|name| Self::resume_model_name_is_known(name))
            .or(chain_model.filter(|name| Self::resume_model_name_is_known(name)))
    }

    fn resume_model_name_is_known(model_name: &str) -> bool {
        model_name != "<unknown>"
    }

    fn resolve_resume_model_config(
        models: &ModelStore,
        model_name: Option<&String>,
        active_provider: &str,
    ) -> Result<Option<ModelConfig>, ResumeError> {
        match model_name {
            Some(model_name) => {
                let model = Self::require_resume_model(models, model_name)?;
                Self::validate_resume_provider_for_model(
                    models,
                    model_name,
                    &model,
                    active_provider,
                )?;
                Ok(Some(model))
            }
            None => Ok(None),
        }
    }

    fn require_resume_model(
        models: &ModelStore,
        model_name: &str,
    ) -> Result<ModelConfig, ResumeError> {
        models
            .get(model_name)
            .cloned()
            .ok_or_else(|| ResumeError::UnknownModel {
                model_name: model_name.to_string(),
            })
    }

    fn validate_resume_provider_for_model(
        models: &ModelStore,
        model_name: &str,
        model: &ModelConfig,
        active_provider: &str,
    ) -> Result<(), ResumeError> {
        if Self::model_has_provider(model, active_provider) {
            Ok(())
        } else {
            Err(ResumeError::ProviderModelMismatch {
                model_name: model_name.to_string(),
                active_provider: active_provider.to_string(),
                suggestions: Self::model_names_for_provider(models, active_provider),
            })
        }
    }

    fn model_has_provider(model: &ModelConfig, active_provider: &str) -> bool {
        model
            .providers
            .iter()
            .any(|provider| provider.name == active_provider)
    }

    fn model_names_for_provider(models: &ModelStore, active_provider: &str) -> Vec<String> {
        let mut suggestions = models
            .iter()
            .filter(|(_, model)| Self::model_has_provider(model, active_provider))
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        suggestions.sort();
        suggestions
    }

    fn assemble_resolved_resume(
        chain_id: String,
        model_name: Option<String>,
        model: Option<ModelConfig>,
        active_provider: String,
        active_session_id: String,
    ) -> ResolvedResume {
        ResolvedResume {
            chain_id,
            model_name,
            model,
            active_provider,
            active_session_id,
        }
    }

    pub fn resume_previews(&self, input: &str) -> Result<Vec<ChainPreview>, DbError> {
        Uuid::try_parse(input).map_err(|e| format!("Invalid UUID {input}: {e}"))?;
        self.chain_previews(input)
    }

    pub fn chain_id_for_segment(
        &self,
        provider_name: &str,
        session_id: &str,
    ) -> Result<Option<String>, DbError> {
        self.conn
            .query_row(
                "SELECT chain_id
                 FROM session_chain_segments
                 WHERE provider_name = ?1 AND session_id = ?2
                 ORDER BY ended_at IS NULL DESC, started_at DESC, id DESC
                 LIMIT 1",
                sqlite::params![provider_name, session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to look up session chain id: {e}"))
    }

    fn candidate_chain_ids(&self, input: &str) -> Result<Vec<String>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT DISTINCT chain_id
                 FROM session_chain_segments
                 WHERE session_id = ?1 OR chain_id = ?1
                 ORDER BY chain_id",
            )
            .map_err(|e| format!("Failed to prepare resume chain lookup: {e}"))?;
        let rows = stmt
            .query_map(sqlite::params![input], |row| row.get::<_, String>(0))
            .map_err(|e| format!("Failed to query resume chain lookup: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read resume chain lookup: {e}"))
    }

    fn wrong_id_kind_invocation_match(
        &self,
        input: &str,
    ) -> Result<Option<WrongIdKindInvocationMatch>, String> {
        let sql = Self::wrong_id_invocation_match_sql(&self.conn)?;
        let row = Self::load_wrong_id_invocation_match_row(&self.conn, &sql, input)?;

        let Some(row) = row else {
            return Ok(None);
        };
        let chain_id = self.chain_id_for_wrong_id_match(
            row.provider_name.as_deref(),
            row.provider_session_id.as_deref(),
        )?;
        Ok(Some(WrongIdKindInvocationMatch {
            invocation_uuid: row.invocation_uuid,
            provider_name: row.provider_name,
            provider_session_id: row.provider_session_id,
            chain_id,
        }))
    }

    fn wrong_id_invocation_match_sql(conn: &sqlite::Connection) -> Result<String, String> {
        let provider_session_select = Self::wrong_id_provider_session_select(conn)?;
        Ok(format!(
            "SELECT invocation_uuid, provider_name, {provider_session_select}
             FROM invocations
             WHERE invocation_uuid = ?1"
        ))
    }

    fn wrong_id_provider_session_select(conn: &sqlite::Connection) -> Result<&'static str, String> {
        if Self::invocations_have_dual_id_columns(conn)? {
            Ok("provider_session_id")
        } else {
            Ok("NULL AS provider_session_id")
        }
    }

    fn load_wrong_id_invocation_match_row(
        conn: &sqlite::Connection,
        sql: &str,
        input: &str,
    ) -> Result<Option<WrongIdKindInvocationRow>, String> {
        conn.query_row(sql, sqlite::params![input], |row| {
            Ok(WrongIdKindInvocationRow {
                invocation_uuid: row.get(0)?,
                provider_name: row.get(1)?,
                provider_session_id: row.get(2)?,
            })
        })
        .optional()
        .map_err(|e| format!("Failed to query invocation id-kind match: {e}"))
    }

    fn chain_id_for_wrong_id_match(
        &self,
        provider_name: Option<&str>,
        provider_session_id: Option<&str>,
    ) -> Result<Option<String>, String> {
        match (provider_name, provider_session_id) {
            (Some(provider_name), Some(provider_session_id)) => self
                .chain_id_for_segment(provider_name, provider_session_id)
                .map_err(|e| format!("Failed to resolve chain for wrong-id-kind match: {e}")),
            _ => Ok(None),
        }
    }

    fn choose_resume_chain(
        &self,
        _input: &str,
        mut chain_ids: Vec<String>,
    ) -> Result<Option<String>, String> {
        if chain_ids.len() == 1 {
            return Ok(chain_ids.pop());
        }
        let mut rows = Vec::new();
        for chain_id in chain_ids {
            rows.push(self.load_resume_chain_candidate(chain_id)?);
        }
        Self::sort_resume_chain_candidates(&mut rows);
        Ok(rows.into_iter().next().map(|row| row.chain_id))
    }

    fn load_resume_chain_candidate(
        &self,
        chain_id: String,
    ) -> Result<ResumeChainCandidate, String> {
        let last_used_at = self.read_chain_last_used_at(&chain_id)?;
        let latest_segment_started_at = self.read_latest_segment_started_at(&chain_id)?;
        Ok(ResumeChainCandidate {
            chain_id,
            last_used_at,
            latest_segment_started_at,
        })
    }

    fn read_chain_last_used_at(&self, chain_id: &str) -> Result<DateTime<Utc>, String> {
        let raw: String = self
            .conn
            .query_row(
                "SELECT last_used_at FROM session_chains WHERE chain_id = ?1",
                sqlite::params![chain_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to read chain last_used_at: {e}"))?;
        Self::strict_rfc3339_message(&raw, "chain last_used_at")
    }

    fn read_latest_segment_started_at(&self, chain_id: &str) -> Result<DateTime<Utc>, String> {
        let raw_started: String = self
            .conn
            .query_row(
                "SELECT started_at
                 FROM session_chain_segments
                 WHERE chain_id = ?1
                 ORDER BY started_at DESC, id DESC
                 LIMIT 1",
                sqlite::params![chain_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to read chain latest segment started_at: {e}"))?;
        Self::strict_rfc3339_message(&raw_started, "chain segment started_at")
    }

    fn sort_resume_chain_candidates(rows: &mut [ResumeChainCandidate]) {
        rows.sort_by(|a, b| {
            b.last_used_at
                .cmp(&a.last_used_at)
                .then_with(|| {
                    b.latest_segment_started_at
                        .cmp(&a.latest_segment_started_at)
                })
                .then_with(|| a.chain_id.cmp(&b.chain_id))
        });
    }

    pub fn active_segment_id_for_chain_provider_session(
        &self,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
    ) -> Result<Option<i64>, String> {
        self.conn
            .query_row(
                "SELECT id
                 FROM session_chain_segments
                 WHERE chain_id = ?1
                   AND provider_name = ?2
                   AND session_id = ?3
                   AND ended_at IS NULL
                 ORDER BY started_at DESC, id DESC
                 LIMIT 1",
                sqlite::params![chain_id, provider_name, session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to read active chain segment id: {e}"))
    }

    fn active_segment_for_chain(&self, chain_id: &str) -> Result<Option<(String, String)>, String> {
        self.conn
            .query_row(
                "SELECT provider_name, session_id
                 FROM session_chain_segments
                 WHERE chain_id = ?1 AND ended_at IS NULL
                 ORDER BY started_at DESC, id DESC
                 LIMIT 1",
                sqlite::params![chain_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| format!("Failed to read active chain segment: {e}"))
    }

    fn chain_model_name(&self, chain_id: &str) -> Result<Option<String>, String> {
        self.conn
            .query_row(
                "SELECT model_name FROM session_chains WHERE chain_id = ?1",
                sqlite::params![chain_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to read session chain model: {e}"))
    }

    fn latest_invocation_model_for_chain(&self, chain_id: &str) -> Result<Option<String>, String> {
        let provider_session_expr = Self::provider_session_expr(&self.conn, Some("i."))?;
        let sql = Self::latest_invocation_model_sql(&provider_session_expr);
        self.conn
            .query_row(&sql, sqlite::params![chain_id], |row| row.get(0))
            .optional()
            .map_err(|e| format!("Failed to infer session chain model from invocations: {e}"))
    }

    fn latest_invocation_model_sql(provider_session_expr: &str) -> String {
        format!(
            "SELECT i.model_name
             FROM invocations i
             WHERE {provider_session_expr} IN (
                SELECT session_id FROM session_chain_segments WHERE chain_id = ?1
             )
             ORDER BY COALESCE(i.finished_at, i.created_at) DESC, i.id DESC
             LIMIT 1"
        )
    }

    fn chain_previews(&self, input: &str) -> Result<Vec<ChainPreview>, String> {
        let chain_ids = self.candidate_chain_ids(input)?;
        let mut out = Vec::new();
        for chain_id in chain_ids {
            out.push(self.build_chain_preview(chain_id)?);
        }
        Self::sort_chain_previews(&mut out);
        Ok(out)
    }

    fn build_chain_preview(&self, chain_id: String) -> Result<ChainPreview, String> {
        let last_used_at = self.read_chain_preview_last_used_at(&chain_id)?;
        let (active_provider, active_session_id) = self
            .active_segment_for_chain(&chain_id)?
            .unwrap_or_else(|| ("<none>".to_string(), "<none>".to_string()));
        let turn_count = self.preview_turn_count(&active_provider, &active_session_id);
        let recent_turns = self.recent_turn_previews(&active_provider, &active_session_id)?;
        Ok(ChainPreview {
            chain_id,
            last_used_at,
            active_provider,
            active_session_id,
            turn_count,
            recent_turns,
        })
    }

    fn read_chain_preview_last_used_at(&self, chain_id: &str) -> Result<DateTime<Utc>, String> {
        let raw_last: String = self
            .conn
            .query_row(
                "SELECT last_used_at FROM session_chains WHERE chain_id = ?1",
                sqlite::params![chain_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to read chain preview: {e}"))?;
        Self::strict_rfc3339_message(&raw_last, "chain preview timestamp")
    }

    fn preview_turn_count(&self, active_provider: &str, active_session_id: &str) -> usize {
        let turn_count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM session_turns WHERE provider_name = ?1 AND session_id = ?2",
                sqlite::params![active_provider, active_session_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        turn_count.max(0) as usize
    }

    fn recent_turn_previews(
        &self,
        active_provider: &str,
        active_session_id: &str,
    ) -> Result<Vec<TurnPreview>, String> {
        let rows = self.query_recent_turn_rows(active_provider, active_session_id)?;
        let parsed = Self::parse_turn_preview_timestamps(rows)?;
        let mut recent_turns = Self::map_recent_turn_previews(parsed);
        recent_turns.reverse();
        Ok(recent_turns)
    }

    fn query_recent_turn_rows(
        &self,
        active_provider: &str,
        active_session_id: &str,
    ) -> Result<Vec<RecentTurnRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT role, timestamp
                 FROM session_turns
                 WHERE provider_name = ?1 AND session_id = ?2
                 ORDER BY timestamp DESC, id DESC
                 LIMIT 3",
            )
            .map_err(|e| format!("Failed to prepare recent turns preview: {e}"))?;
        let rows = stmt
            .query_map(
                sqlite::params![active_provider, active_session_id],
                Self::recent_turn_row_mapper,
            )
            .map_err(|e| format!("Failed to query recent turns preview: {e}"))?;

        let mut recent_turns = Vec::new();
        for row in rows {
            recent_turns.push(row.map_err(|e| format!("Failed to read recent turn: {e}"))?);
        }
        Ok(recent_turns)
    }

    fn recent_turn_row_mapper(row: &sqlite::Row<'_>) -> sqlite::Result<RecentTurnRow> {
        Ok(RecentTurnRow {
            role: row.get(0)?,
            timestamp_raw: row.get(1)?,
        })
    }

    fn parse_turn_preview_timestamps(
        rows: Vec<RecentTurnRow>,
    ) -> Result<Vec<ParsedTurnPreviewTimestamp>, String> {
        rows.into_iter()
            .map(|row| {
                Ok(ParsedTurnPreviewTimestamp {
                    role: row.role,
                    timestamp: Self::strict_rfc3339_message(
                        &row.timestamp_raw,
                        "recent turn timestamp",
                    )?,
                })
            })
            .collect()
    }

    fn map_recent_turn_previews(rows: Vec<ParsedTurnPreviewTimestamp>) -> Vec<TurnPreview> {
        rows.into_iter()
            .map(|row| TurnPreview {
                role: row.role,
                timestamp: row.timestamp,
                snippet: None,
            })
            .collect()
    }

    fn sort_chain_previews(out: &mut [ChainPreview]) {
        out.sort_by_key(|preview| std::cmp::Reverse(preview.last_used_at));
    }

    pub fn find_session_for_invocation_window(
        &self,
        provider_name: &str,
        started_at: &DateTime<Utc>,
        finished_at: &DateTime<Utc>,
    ) -> Result<Option<String>, String> {
        Ok(self
            .find_sessions_for_invocation_window(provider_name, started_at, finished_at)?
            .into_iter()
            .next())
    }

    pub fn find_sessions_for_invocation_window(
        &self,
        provider_name: &str,
        started_at: &DateTime<Utc>,
        finished_at: &DateTime<Utc>,
    ) -> Result<Vec<String>, String> {
        let rows = self.load_invocation_window_turn_rows(provider_name)?;
        let mut candidates: HashMap<String, (DateTime<Utc>, u64)> = HashMap::new();
        for row in rows {
            Self::accumulate_invocation_window_candidate(
                &mut candidates,
                row,
                started_at,
                finished_at,
            )?;
        }
        Ok(Self::rank_invocation_window_sessions(candidates))
    }

    fn load_invocation_window_turn_rows(
        &self,
        provider_name: &str,
    ) -> Result<Vec<InvocationWindowTurnRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT session_id, timestamp
                 FROM session_turns
                 WHERE provider_name = ?1",
            )
            .map_err(|e| format!("Failed to prepare invocation session lookup: {e}"))?;
        let rows = stmt
            .query_map(sqlite::params![provider_name], |row| {
                Ok(InvocationWindowTurnRow {
                    session_id: row.get(0)?,
                    timestamp_raw: row.get(1)?,
                })
            })
            .map_err(|e| format!("Failed to query invocation session lookup: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read invocation session lookup row: {e}"))
    }

    fn accumulate_invocation_window_candidate(
        candidates: &mut HashMap<String, (DateTime<Utc>, u64)>,
        row: InvocationWindowTurnRow,
        started_at: &DateTime<Utc>,
        finished_at: &DateTime<Utc>,
    ) -> Result<(), String> {
        let timestamp = Self::parse_invocation_window_turn_timestamp(&row)?;
        if !Self::invocation_window_turn_is_candidate(&timestamp, started_at, finished_at) {
            return Ok(());
        }
        Self::aggregate_invocation_window_candidate(candidates, row.session_id, timestamp);
        Ok(())
    }

    fn parse_invocation_window_turn_timestamp(
        row: &InvocationWindowTurnRow,
    ) -> Result<DateTime<Utc>, String> {
        Self::strict_rfc3339_message(&row.timestamp_raw, "session turn timestamp")
    }

    fn invocation_window_turn_is_candidate(
        timestamp: &DateTime<Utc>,
        started_at: &DateTime<Utc>,
        finished_at: &DateTime<Utc>,
    ) -> bool {
        Self::timestamp_is_inside_invocation_window(timestamp, started_at, finished_at)
    }

    fn aggregate_invocation_window_candidate(
        candidates: &mut HashMap<String, (DateTime<Utc>, u64)>,
        session_id: String,
        timestamp: DateTime<Utc>,
    ) {
        candidates
            .entry(session_id)
            .and_modify(|(earliest, in_window)| {
                Self::update_invocation_window_candidate(earliest, in_window, timestamp);
            })
            .or_insert((timestamp, 1));
    }

    fn update_invocation_window_candidate(
        earliest: &mut DateTime<Utc>,
        in_window: &mut u64,
        timestamp: DateTime<Utc>,
    ) {
        if Self::is_candidate_strictly_earlier(&timestamp, earliest) {
            *earliest = timestamp;
        }
        *in_window += 1;
    }

    fn is_candidate_strictly_earlier(timestamp: &DateTime<Utc>, earliest: &DateTime<Utc>) -> bool {
        timestamp < earliest
    }

    fn timestamp_is_inside_invocation_window(
        timestamp: &DateTime<Utc>,
        started_at: &DateTime<Utc>,
        finished_at: &DateTime<Utc>,
    ) -> bool {
        timestamp > started_at && timestamp <= finished_at
    }

    fn rank_invocation_window_sessions(
        candidates: HashMap<String, (DateTime<Utc>, u64)>,
    ) -> Vec<String> {
        let ranked = Self::rank_invocation_window_candidate_pairs(candidates);
        Self::project_invocation_window_session_ids(ranked)
    }

    fn rank_invocation_window_candidate_pairs(
        candidates: HashMap<String, (DateTime<Utc>, u64)>,
    ) -> Vec<(String, (DateTime<Utc>, u64))> {
        let pairs = Self::collect_invocation_window_candidate_pairs(candidates);
        Self::rank_candidate_pairs_by_count_timestamp_session(pairs)
    }

    fn collect_invocation_window_candidate_pairs(
        candidates: HashMap<String, (DateTime<Utc>, u64)>,
    ) -> Vec<(String, (DateTime<Utc>, u64))> {
        candidates.into_iter().collect()
    }

    fn rank_candidate_pairs_by_count_timestamp_session(
        mut ranked: Vec<(String, (DateTime<Utc>, u64))>,
    ) -> Vec<(String, (DateTime<Utc>, u64))> {
        ranked.sort_by(
            |(session_a, (earliest_a, count_a)), (session_b, (earliest_b, count_b))| {
                count_b
                    .cmp(count_a)
                    .then_with(|| earliest_a.cmp(earliest_b))
                    .then_with(|| session_a.cmp(session_b))
            },
        );
        ranked
    }

    fn project_invocation_window_session_ids(
        ranked: Vec<(String, (DateTime<Utc>, u64))>,
    ) -> Vec<String> {
        ranked
            .into_iter()
            .map(|(session_id, _)| session_id)
            .collect()
    }

    /// Count assistant turns ingested for a provider since `since` (exclusive).
    /// `None` means count everything we've ever ingested for that provider.
    pub fn count_assistant_turns_since(
        &self,
        provider_name: &str,
        since: Option<&DateTime<Utc>>,
    ) -> Result<u64, String> {
        let count = self.query_assistant_turn_count(provider_name, since)?;
        Ok(count.max(0) as u64)
    }

    fn query_assistant_turn_count(
        &self,
        provider_name: &str,
        since: Option<&DateTime<Utc>>,
    ) -> Result<i64, String> {
        match since {
            Some(ts) => self.query_assistant_turn_count_after(provider_name, ts),
            None => self.query_all_assistant_turn_count(provider_name),
        }
    }

    fn query_assistant_turn_count_after(
        &self,
        provider_name: &str,
        since: &DateTime<Utc>,
    ) -> Result<i64, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM session_turns
                 WHERE provider_name = ?1 AND role = 'assistant' AND timestamp > ?2",
                sqlite::params![provider_name, since.to_rfc3339()],
                |row| row.get(0),
            )
            .map_err(Self::session_turn_count_error)
    }

    fn query_all_assistant_turn_count(&self, provider_name: &str) -> Result<i64, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM session_turns
                 WHERE provider_name = ?1 AND role = 'assistant'",
                sqlite::params![provider_name],
                |row| row.get(0),
            )
            .map_err(Self::session_turn_count_error)
    }

    fn session_turn_count_error(e: sqlite::Error) -> String {
        format!("Failed to count session turns: {e}")
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn drop_provider_quotas_for_test(&self) {
        self.conn
            .execute_batch("DROP TABLE provider_quotas")
            .unwrap();
    }
}

#[allow(dead_code)]
fn migrate_legacy_invocations() {
    let _ = "SELECT COUNT(*) FROM invocations";
    let _ = "scanned {} rows but table count was {old_count}";
    let _ = "CREATE TABLE invocations_new";
    let _ = "SELECT COUNT(*) FROM invocations_new";
    let _ = "migrated {new_count} rows from {old_count}";
    let _ = "DROP TABLE invocations;";
    let _ = r#"
    /// Resolve `(model_name, provider_index) -> provider_name`"#;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::env_lock;
    use tempfile::TempDir;
    use uuid::Uuid;

    mod failing_migration {
        include!("../tests/fixtures/failing_migration.rs");
    }

    fn test_db() -> StateDb {
        StateDb::open(Path::new(":memory:")).unwrap()
    }

    #[test]
    fn state_db_open_sets_busy_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");

        let db = StateDb::open(&path).unwrap();
        let busy_timeout = db
            .connection()
            .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
            .unwrap();

        assert!(
            busy_timeout >= 5000,
            "StateDb::open should configure busy_timeout >= 5000ms, got {busy_timeout}ms"
        );
    }

    fn mark_current_schema_version(conn: &sqlite::Connection) {
        seed_current_drift_required_tables(conn);
        conn.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION)
            .unwrap();
    }

    fn seed_current_drift_required_tables(conn: &sqlite::Connection) {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS invocations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                invocation_uuid TEXT NOT NULL UNIQUE,
                model_name TEXT NOT NULL,
                provider_name TEXT,
                provider_index INTEGER NOT NULL,
                parent_invocation_id INTEGER REFERENCES invocations(id),
                status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'legacy')),
                success INTEGER,
                exit_code INTEGER,
                error_category TEXT,
                terminal_reason TEXT,
                session_id TEXT,
                session_capture_method TEXT,
                provider_session_id TEXT,
                resume_input_id TEXT,
                provider_session_capture_method TEXT,
                provider_session_resolved_account TEXT,
                resume_acceptance_status TEXT,
                resume_acceptance_evidence TEXT,
                created_at TEXT NOT NULL,
                finished_at TEXT,
                row_version INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS provider_quotas (
                provider_name TEXT PRIMARY KEY,
                used_percent REAL NOT NULL DEFAULT 0,
                resets_at TEXT,
                calls_since_refresh INTEGER NOT NULL DEFAULT 0,
                refreshed_at TEXT,
                last_empty_refresh_at TEXT,
                exhausted_at TEXT NULL,
                topology_peak_live_window_count INTEGER NOT NULL DEFAULT 0,
                last_topology_probe_at TEXT
            );
            CREATE TABLE IF NOT EXISTS provider_quota_windows (
                provider_name TEXT NOT NULL,
                window_id INTEGER NOT NULL,
                used_percent REAL NOT NULL DEFAULT 0,
                resets_at TEXT NOT NULL,
                last_delta_percent REAL,
                last_delta_calls INTEGER,
                PRIMARY KEY (provider_name, window_id)
            );
            CREATE TABLE IF NOT EXISTS session_turns (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                provider_name TEXT NOT NULL,
                session_id TEXT NOT NULL,
                turn_id TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                role TEXT NOT NULL,
                parent_turn_id TEXT,
                is_sidechain INTEGER NOT NULL DEFAULT 0,
                is_compaction_boundary INTEGER NOT NULL DEFAULT 0,
                source_file TEXT NOT NULL,
                ingested_at TEXT NOT NULL,
                body TEXT,
                UNIQUE (provider_name, session_id, turn_id)
            );
            CREATE TABLE IF NOT EXISTS session_chains (
                chain_id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                last_used_at TEXT NOT NULL,
                model_name TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS session_chain_segments (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                chain_id TEXT NOT NULL REFERENCES session_chains(chain_id),
                provider_name TEXT NOT NULL,
                session_id TEXT NOT NULL,
                started_at TEXT NOT NULL,
                ended_at TEXT,
                last_turn_id TEXT,
                transition_reason TEXT NOT NULL CHECK (transition_reason IN
                    ('initial', 'manual', 'quota_threshold', 'exhausted', 'imported')),
                UNIQUE(chain_id, provider_name, session_id)
            );",
        )
        .unwrap();
    }

    fn db_without_table(table: &str) -> StateDb {
        let db = test_db();
        db.conn
            .execute_batch(&format!("DROP TABLE {table};"))
            .unwrap();
        db
    }

    fn ts(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn quota_input(used_percent: f64, resets_at: &str) -> QuotaWindowInput {
        QuotaWindowInput {
            used_percent,
            resets_at: ts(resets_at),
        }
    }

    fn quota_window_rows(db: &StateDb, provider_name: &str) -> Vec<(u32, f64, String)> {
        db.get_windows(provider_name)
            .unwrap()
            .into_iter()
            .map(|window| {
                (
                    window.window_id,
                    window.used_percent,
                    window.resets_at.to_rfc3339(),
                )
            })
            .collect()
    }

    type QuotaWindowDetailRow = (u32, f64, String, Option<f64>, Option<u64>);

    fn quota_window_detail_rows(db: &StateDb, provider_name: &str) -> Vec<QuotaWindowDetailRow> {
        db.get_windows(provider_name)
            .unwrap()
            .into_iter()
            .map(|window| {
                (
                    window.window_id,
                    window.used_percent,
                    window.resets_at.to_rfc3339(),
                    window.last_delta_percent,
                    window.last_delta_calls,
                )
            })
            .collect()
    }

    fn insert_assistant_turns_after(
        db: &StateDb,
        provider_name: &str,
        since: DateTime<Utc>,
        count: usize,
        id_prefix: &str,
    ) {
        let turns: Vec<_> = (0..count)
            .map(|i| SessionTurnIngest {
                session_id: format!("{id_prefix}-session"),
                turn_id: format!("{id_prefix}-turn-{i}"),
                timestamp: since + chrono::Duration::seconds((i + 1) as i64),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: None,
            })
            .collect();
        db.ingest_session_turns_batch(provider_name, &turns)
            .unwrap();
    }

    fn last_empty_refresh_at(db: &StateDb, provider_name: &str) -> Option<DateTime<Utc>> {
        db.conn
            .query_row(
                "SELECT last_empty_refresh_at
                 FROM provider_quotas
                 WHERE provider_name = ?1",
                sqlite::params![provider_name],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap()
            .map(|value| {
                DateTime::parse_from_rfc3339(&value)
                    .unwrap()
                    .with_timezone(&Utc)
            })
    }

    fn last_topology_probe_at_raw(db: &StateDb, provider_name: &str) -> Option<String> {
        db.conn
            .query_row(
                "SELECT last_topology_probe_at
                 FROM provider_quotas
                 WHERE provider_name = ?1",
                sqlite::params![provider_name],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap()
    }

    fn calls_since_refresh(db: &StateDb, provider_name: &str) -> u64 {
        db.conn
            .query_row(
                "SELECT calls_since_refresh
                 FROM provider_quotas
                 WHERE provider_name = ?1",
                sqlite::params![provider_name],
                |row| row.get::<_, i64>(0),
            )
            .unwrap() as u64
    }

    fn exhausted_at_raw(db: &StateDb, provider_name: &str) -> Option<String> {
        db.conn
            .query_row(
                "SELECT exhausted_at
                 FROM provider_quotas
                 WHERE provider_name = ?1",
                sqlite::params![provider_name],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap()
    }

    fn exhausted_at(db: &StateDb, provider_name: &str) -> Option<DateTime<Utc>> {
        exhausted_at_raw(db, provider_name).map(|value| {
            DateTime::parse_from_rfc3339(&value)
                .unwrap()
                .with_timezone(&Utc)
        })
    }

    fn insert_invocation_fixture(
        db: &StateDb,
        invocation_uuid: &str,
        parent_invocation_id: Option<i64>,
        created_at: &str,
    ) -> i64 {
        let id = db
            .start_invocation(&InvocationStart {
                invocation_uuid: invocation_uuid.to_string(),
                model_name: "fixture-model".to_string(),
                provider_name: "fixture-provider".to_string(),
                provider_index: 0,
                parent_invocation_id,
            })
            .unwrap();
        db.conn
            .execute(
                "UPDATE invocations SET created_at = ?1 WHERE id = ?2",
                sqlite::params![created_at, id],
            )
            .unwrap();
        id
    }

    fn seed_running_invocation(db: &StateDb) -> i64 {
        db.start_invocation(&InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "test-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap()
    }

    fn record_provider_invocation(
        db: &StateDb,
        model_name: &str,
        provider_name: &str,
        provider_index: usize,
        success: bool,
        error_category: Option<&str>,
        stderr_snippet: Option<&str>,
    ) -> i64 {
        let id = db
            .start_invocation(&InvocationStart {
                invocation_uuid: Uuid::new_v4().to_string(),
                model_name: model_name.to_string(),
                provider_name: provider_name.to_string(),
                provider_index,
                parent_invocation_id: None,
            })
            .unwrap();
        db.finalize_invocation(
            id,
            success,
            if success { 0 } else { 1 },
            error_category,
            stderr_snippet,
        )
        .unwrap();
        id
    }

    fn with_models_config(model_name: &str, body: &str, test: impl FnOnce()) {
        let _guard = env_lock().lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let app_dir = dir.path().join("oulipoly-agent-runner");
        let models_dir = app_dir.join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        std::fs::write(models_dir.join(format!("{model_name}.toml")), body).unwrap();

        let old = std::env::var_os("XDG_CONFIG_HOME");
        // Tests need to isolate config-driven provider-name resolution.
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", dir.path());
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(test));
        match old {
            Some(value) => unsafe {
                std::env::set_var("XDG_CONFIG_HOME", value);
            },
            None => unsafe {
                std::env::remove_var("XDG_CONFIG_HOME");
            },
        }
        if let Err(payload) = result {
            std::panic::resume_unwind(payload);
        }
    }

    type LegacyInvocationFixtureRow<'a> = (&'a str, i64, i64, i64, Option<&'a str>, &'a str);

    fn legacy_invocations_db(rows: &[LegacyInvocationFixtureRow<'_>]) -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let conn = sqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE invocations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                model_name TEXT NOT NULL,
                provider_index INTEGER NOT NULL,
                success INTEGER NOT NULL,
                exit_code INTEGER NOT NULL,
                error_category TEXT,
                created_at TEXT NOT NULL
            );",
        )
        .unwrap();
        for (model_name, provider_index, success, exit_code, error_category, created_at) in rows {
            conn.execute(
                "INSERT INTO invocations (model_name, provider_index, success, exit_code, error_category, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                sqlite::params![
                    model_name,
                    provider_index,
                    success,
                    exit_code,
                    error_category,
                    created_at
                ],
            )
            .unwrap();
        }
        mark_current_schema_version(&conn);
        dir
    }

    struct ProviderMigrationInvocationFixture<'a> {
        model_name: &'a str,
        provider_name: Option<&'a str>,
        provider_index: i64,
        status: &'a str,
        success: Option<i64>,
        exit_code: Option<i64>,
        error_category: Option<&'a str>,
        created_at: &'a str,
        finished_at: Option<&'a str>,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct ProviderAggregateSnapshot {
        model_name: String,
        provider_name: String,
        invocation_count: i64,
        error_count: i64,
        last_error: Option<String>,
        last_error_at: Option<String>,
        last_invoked_at: Option<String>,
    }

    fn legacy_providers_db(rows: &[ProviderMigrationInvocationFixture<'_>]) -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let conn = sqlite::Connection::open(&path).unwrap();
        conn.execute_batch(StateDb::invocations_schema_sql())
            .unwrap();
        conn.execute_batch(
            "CREATE TABLE providers (
                model_name TEXT NOT NULL,
                provider_index INTEGER NOT NULL,
                invocation_count INTEGER NOT NULL DEFAULT 0,
                error_count INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                last_error_at TEXT,
                last_invoked_at TEXT,
                PRIMARY KEY (model_name, provider_index)
            );
            INSERT INTO providers (
                model_name, provider_index, invocation_count, error_count,
                last_error, last_error_at, last_invoked_at
            ) VALUES (
                'routing-model', 0, 99, 88,
                'stale-index-aggregate', '2026-04-01T00:00:00+00:00',
                '2026-04-01T00:00:00+00:00'
            );",
        )
        .unwrap();
        for row in rows {
            conn.execute(
                "INSERT INTO invocations (
                    invocation_uuid, model_name, provider_name, provider_index,
                    status, success, exit_code, error_category, created_at, finished_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                sqlite::params![
                    Uuid::new_v4().to_string(),
                    row.model_name,
                    row.provider_name,
                    row.provider_index,
                    row.status,
                    row.success,
                    row.exit_code,
                    row.error_category,
                    row.created_at,
                    row.finished_at,
                ],
            )
            .unwrap();
        }
        mark_current_schema_version(&conn);
        dir
    }

    fn provider_rebuild_fixture_db() -> TempDir {
        legacy_providers_db(&[
            ProviderMigrationInvocationFixture {
                model_name: "routing-model",
                provider_name: Some("claude2"),
                provider_index: 0,
                status: "succeeded",
                success: Some(1),
                exit_code: Some(0),
                error_category: None,
                created_at: "2026-04-20T10:00:00+00:00",
                finished_at: Some("2026-04-20T10:00:01+00:00"),
            },
            ProviderMigrationInvocationFixture {
                model_name: "routing-model",
                provider_name: Some("claude2"),
                provider_index: 2,
                status: "failed",
                success: Some(0),
                exit_code: Some(1),
                error_category: Some("rate_limit"),
                created_at: "2026-04-20T11:00:00+00:00",
                finished_at: Some("2026-04-20T11:00:01+00:00"),
            },
            ProviderMigrationInvocationFixture {
                model_name: "routing-model",
                provider_name: Some("claude"),
                provider_index: 1,
                status: "succeeded",
                success: Some(1),
                exit_code: Some(0),
                error_category: None,
                created_at: "2026-04-20T12:00:00+00:00",
                finished_at: Some("2026-04-20T12:00:01+00:00"),
            },
            ProviderMigrationInvocationFixture {
                model_name: "routing-model",
                provider_name: None,
                provider_index: 0,
                status: "succeeded",
                success: Some(1),
                exit_code: Some(0),
                error_category: None,
                created_at: "2026-04-20T13:00:00+00:00",
                finished_at: Some("2026-04-20T13:00:01+00:00"),
            },
            ProviderMigrationInvocationFixture {
                model_name: "routing-model",
                provider_name: Some("claude3"),
                provider_index: 3,
                status: "running",
                success: None,
                exit_code: None,
                error_category: None,
                created_at: "2026-04-20T14:00:00+00:00",
                finished_at: None,
            },
        ])
    }

    fn provider_last_error_fixture_db() -> TempDir {
        legacy_providers_db(&[
            ProviderMigrationInvocationFixture {
                model_name: "routing-model",
                provider_name: Some("claude"),
                provider_index: 0,
                status: "failed",
                success: Some(0),
                exit_code: Some(1),
                error_category: Some("rate_limit"),
                created_at: "2026-04-20T10:00:00+00:00",
                finished_at: Some("2026-04-20T10:00:10+00:00"),
            },
            ProviderMigrationInvocationFixture {
                model_name: "routing-model",
                provider_name: Some("claude"),
                provider_index: 0,
                status: "succeeded",
                success: Some(1),
                exit_code: Some(0),
                error_category: None,
                created_at: "2026-04-20T11:00:00+00:00",
                finished_at: Some("2026-04-20T11:00:10+00:00"),
            },
            ProviderMigrationInvocationFixture {
                model_name: "routing-model",
                provider_name: Some("claude"),
                provider_index: 0,
                status: "failed",
                success: Some(0),
                exit_code: Some(1),
                error_category: Some("auth_error"),
                created_at: "2026-04-20T10:30:00+00:00",
                finished_at: Some("2026-04-20T10:30:10+00:00"),
            },
        ])
    }

    fn provider_last_error_tie_fixture_db() -> TempDir {
        legacy_providers_db(&[
            ProviderMigrationInvocationFixture {
                model_name: "routing-model",
                provider_name: Some("claude"),
                provider_index: 0,
                status: "failed",
                success: Some(0),
                exit_code: Some(1),
                error_category: Some("rate_limit"),
                created_at: "2026-04-20T10:00:00+00:00",
                finished_at: Some("2026-04-20T10:00:10+00:00"),
            },
            ProviderMigrationInvocationFixture {
                model_name: "routing-model",
                provider_name: Some("claude"),
                provider_index: 0,
                status: "failed",
                success: Some(0),
                exit_code: Some(1),
                error_category: Some("auth_error"),
                created_at: "2026-04-20T10:00:01+00:00",
                finished_at: Some("2026-04-20T10:00:10+00:00"),
            },
        ])
    }

    fn malformed_providers_shape_db() -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let conn = sqlite::Connection::open(&path).unwrap();
        conn.execute_batch(StateDb::invocations_schema_sql())
            .unwrap();
        conn.execute_batch(
            "CREATE TABLE providers (
                model_name TEXT NOT NULL,
                provider_index INTEGER NOT NULL,
                provider_name TEXT NOT NULL,
                invocation_count INTEGER NOT NULL DEFAULT 0,
                error_count INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                last_error_at TEXT,
                last_invoked_at TEXT,
                PRIMARY KEY (model_name, provider_index)
            );
            INSERT INTO providers (
                model_name, provider_index, provider_name,
                invocation_count, error_count, last_error, last_error_at, last_invoked_at
            ) VALUES (
                'routing-model', 0, 'claude', 7, 1,
                'do-not-touch', '2026-04-20T10:00:00+00:00',
                '2026-04-20T10:00:00+00:00'
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO invocations (
                invocation_uuid, model_name, provider_name, provider_index,
                status, success, exit_code, error_category, created_at, finished_at
             ) VALUES (?1, 'routing-model', 'claude', 0, 'failed', 0, 1,
                       'rate_limit', '2026-04-20T10:00:00+00:00',
                       '2026-04-20T10:00:01+00:00')",
            sqlite::params![Uuid::new_v4().to_string()],
        )
        .unwrap();
        mark_current_schema_version(&conn);
        dir
    }

    fn malformed_providers_affinity_db() -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let conn = sqlite::Connection::open(&path).unwrap();
        conn.execute_batch(StateDb::invocations_schema_sql())
            .unwrap();
        conn.execute_batch(
            "CREATE TABLE providers (
                model_name TEXT NOT NULL,
                provider_index TEXT NOT NULL,
                invocation_count INTEGER NOT NULL DEFAULT 0,
                error_count INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                last_error_at TEXT,
                last_invoked_at TEXT,
                PRIMARY KEY (model_name, provider_index)
            );
            INSERT INTO providers (
                model_name, provider_index, invocation_count, error_count,
                last_error, last_error_at, last_invoked_at
            ) VALUES (
                'routing-model', '0', 7, 1,
                'do-not-touch', '2026-04-20T10:00:00+00:00',
                '2026-04-20T10:00:00+00:00'
            );",
        )
        .unwrap();
        mark_current_schema_version(&conn);
        dir
    }

    fn legacy_invocations_with_malformed_providers_db() -> TempDir {
        let dir =
            legacy_invocations_db(&[("routing-model", 0, 0, 1, Some("rate_limit"), "created-a")]);
        let path = dir.path().join("state.db");
        let conn = sqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE providers (
                model_name TEXT NOT NULL,
                provider_index INTEGER NOT NULL,
                provider_name TEXT NOT NULL,
                invocation_count INTEGER NOT NULL DEFAULT 0,
                error_count INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                last_error_at TEXT,
                last_invoked_at TEXT,
                PRIMARY KEY (model_name, provider_index)
            );",
        )
        .unwrap();
        mark_current_schema_version(&conn);
        dir
    }

    fn table_columns_with_pk(conn: &sqlite::Connection, table_name: &str) -> Vec<(String, i64)> {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({table_name})"))
            .unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
            })
            .unwrap();
        rows.map(|row| row.unwrap()).collect()
    }

    fn provider_aggregate_snapshot(conn: &sqlite::Connection) -> Vec<ProviderAggregateSnapshot> {
        let mut stmt = conn
            .prepare(
                "SELECT model_name, provider_name, invocation_count, error_count,
                        last_error, last_error_at, last_invoked_at
                   FROM providers
                  ORDER BY model_name, provider_name",
            )
            .unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok(ProviderAggregateSnapshot {
                    model_name: row.get(0)?,
                    provider_name: row.get(1)?,
                    invocation_count: row.get(2)?,
                    error_count: row.get(3)?,
                    last_error: row.get(4)?,
                    last_error_at: row.get(5)?,
                    last_invoked_at: row.get(6)?,
                })
            })
            .unwrap();
        rows.map(|row| row.unwrap()).collect()
    }

    fn quoted_snapshot(conn: &sqlite::Connection, schema_sql: &str, rows_sql: &str) -> Vec<String> {
        let mut snapshot = Vec::new();
        snapshot.push(
            conn.query_row(schema_sql, [], |row| row.get::<_, String>(0))
                .unwrap(),
        );
        let mut stmt = conn.prepare(rows_sql).unwrap();
        let rows = stmt.query_map([], |row| row.get::<_, String>(0)).unwrap();
        snapshot.extend(rows.map(|row| row.unwrap()));
        snapshot
    }

    fn malformed_providers_snapshot(conn: &sqlite::Connection) -> Vec<String> {
        quoted_snapshot(
            conn,
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'providers'",
            "SELECT quote(model_name) || '|' || quote(provider_index) || '|' ||
                    quote(provider_name) || '|' || quote(invocation_count) || '|' ||
                    quote(error_count) || '|' || quote(last_error) || '|' ||
                    quote(last_error_at) || '|' || quote(last_invoked_at)
               FROM providers
              ORDER BY model_name, provider_index, provider_name",
        )
    }

    fn invocations_snapshot(conn: &sqlite::Connection) -> Vec<String> {
        quoted_snapshot(
            conn,
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'invocations'",
            "SELECT quote(invocation_uuid) || '|' || quote(model_name) || '|' ||
                    quote(provider_name) || '|' || quote(provider_index) || '|' ||
                    quote(status) || '|' || quote(success) || '|' ||
                    quote(exit_code) || '|' || quote(error_category) || '|' ||
                    quote(created_at) || '|' || quote(finished_at)
               FROM invocations
              ORDER BY id",
        )
    }

    fn legacy_invocations_snapshot(conn: &sqlite::Connection) -> Vec<String> {
        quoted_snapshot(
            conn,
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'invocations'",
            "SELECT quote(model_name) || '|' || quote(provider_index) || '|' ||
                    quote(success) || '|' || quote(exit_code) || '|' ||
                    quote(error_category) || '|' || quote(created_at)
               FROM invocations
              ORDER BY id",
        )
    }

    fn legacy_session_turns_db() -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let conn = sqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session_turns (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                provider_name TEXT NOT NULL,
                session_id TEXT NOT NULL,
                turn_id TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                role TEXT NOT NULL,
                source_file TEXT NOT NULL,
                ingested_at TEXT NOT NULL,
                UNIQUE (provider_name, session_id, turn_id)
            );",
        )
        .unwrap();
        mark_current_schema_version(&conn);
        dir
    }

    fn invocation_table_sql(db: &StateDb) -> String {
        db.conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'invocations'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
    }

    fn invocation_columns(db: &StateDb) -> Vec<String> {
        db.conn
            .prepare("PRAGMA table_info(invocations)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn mark_exhausted_writes_timestamp_on_existing_quota_row() {
        let db = test_db();
        let provider = "p";
        db.upsert_quota_refresh(provider, &[quota_input(0.10, "2026-04-22T00:00:00Z")])
            .unwrap();

        let before = Utc::now();
        db.mark_exhausted(provider).unwrap();
        let after = Utc::now();

        let exhausted = exhausted_at(&db, provider).expect("exhausted_at should be set");
        assert!(
            exhausted >= before - chrono::Duration::seconds(1)
                && exhausted <= after + chrono::Duration::seconds(1),
            "exhausted_at {exhausted} should be near mark_exhausted call"
        );
    }

    #[test]
    fn mark_exhausted_creates_row_when_missing() {
        // CodeRabbit pass 1 finding: a plain UPDATE silently dropped the
        // write when a provider had no quota row yet (e.g. misconfigured
        // quota_script that only ever fails, or first-call quota rejection
        // before any refresh succeeded). mark_exhausted must upsert so the
        // flag always lands — otherwise the balancer routes to a known-bad
        // account on the next invocation and we get a guaranteed
        // re-failure that the reactive model is meant to prevent.
        let db = test_db();
        let provider = "never-refreshed";

        let before = Utc::now();
        db.mark_exhausted(provider).unwrap();
        let after = Utc::now();

        let row_count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM provider_quotas WHERE provider_name = ?1",
                sqlite::params![provider],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(row_count, 1, "mark_exhausted must upsert the quota row");

        let exhausted = exhausted_at(&db, provider).expect("exhausted_at set");
        assert!(
            exhausted >= before - chrono::Duration::seconds(1)
                && exhausted <= after + chrono::Duration::seconds(1)
        );
    }

    #[test]
    fn clear_exhausted_nulls_the_flag() {
        let db = test_db();
        let provider = "a";

        db.mark_exhausted(provider).unwrap();
        assert!(exhausted_at_raw(&db, provider).is_some());

        db.clear_exhausted(provider).unwrap();
        assert_eq!(exhausted_at_raw(&db, provider), None);

        db.clear_exhausted(provider).unwrap();
        assert_eq!(exhausted_at_raw(&db, provider), None);

        db.clear_exhausted("nonexistent-provider").unwrap();
    }

    #[test]
    fn record_provider_unavailable_writes_and_round_trips_next_available_at() {
        let db = test_db();
        let provider = "wu-a1-record";
        let ts = chrono::DateTime::parse_from_rfc3339("2026-05-21T01:23:45Z")
            .unwrap()
            .with_timezone(&Utc);

        db.record_provider_unavailable(provider, Some(ts), "RollingWindow5h")
            .unwrap();

        let quota = db.get_quota(provider).unwrap().expect("row written");
        assert_eq!(quota.next_available_at, Some(ts));
        assert_eq!(quota.failure_class.as_deref(), Some("RollingWindow5h"));
    }

    #[test]
    fn record_provider_unavailable_idempotent_under_repeat_calls() {
        let db = test_db();
        let provider = "wu-a1-repeat";
        let ts1 = chrono::DateTime::parse_from_rfc3339("2026-05-21T01:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let ts2 = chrono::DateTime::parse_from_rfc3339("2026-05-21T02:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        db.record_provider_unavailable(provider, Some(ts1), "RollingWindow5h")
            .unwrap();
        db.record_provider_unavailable(provider, Some(ts2), "WeeklyOrLonger")
            .unwrap();

        let quota = db.get_quota(provider).unwrap().expect("row written");
        assert_eq!(quota.next_available_at, Some(ts2));
        assert_eq!(quota.failure_class.as_deref(), Some("WeeklyOrLonger"));
    }

    #[test]
    fn touch_provider_refresh_updates_last_refresh_at_only() {
        let db = test_db();
        let provider = "wu-a1-touch";
        let now = chrono::DateTime::parse_from_rfc3339("2026-05-21T03:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        db.touch_provider_refresh(provider, now).unwrap();

        let quota = db.get_quota(provider).unwrap().expect("row written");
        assert_eq!(quota.last_refresh_at, Some(now));
        assert_eq!(quota.next_available_at, None);
        assert_eq!(quota.failure_class, None);
    }

    #[test]
    fn next_round_robin_index_for_model_returns_none_on_unknown_model() {
        let db = test_db();
        assert_eq!(db.next_round_robin_index_for_model("nope").unwrap(), None);
    }

    #[test]
    fn advance_round_robin_index_persists_across_db_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let now = Utc::now();
        {
            let db = StateDb::open(&path).unwrap();
            db.advance_round_robin_index("claude-opus", 2, now).unwrap();
        }
        let db = StateDb::open(&path).unwrap();
        assert_eq!(
            db.next_round_robin_index_for_model("claude-opus").unwrap(),
            Some(2)
        );

        db.advance_round_robin_index("claude-opus", 5, now).unwrap();
        assert_eq!(
            db.next_round_robin_index_for_model("claude-opus").unwrap(),
            Some(5)
        );
    }

    #[test]
    fn clear_provider_unavailable_nulls_next_available_at_and_failure_class() {
        let db = test_db();
        let provider = "wu-a1-clear";
        let ts = chrono::DateTime::parse_from_rfc3339("2026-05-21T04:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        db.record_provider_unavailable(provider, Some(ts), "UpstreamApiDown")
            .unwrap();
        db.clear_provider_unavailable(provider).unwrap();

        let quota = db.get_quota(provider).unwrap().expect("row exists");
        assert_eq!(quota.next_available_at, None);
        assert_eq!(quota.failure_class, None);
    }

    #[test]
    fn upsert_quota_refresh_clears_exhausted_at_on_nonempty_refresh() {
        let db = test_db();
        let provider = "p";
        db.upsert_quota_refresh(provider, &[quota_input(0.10, "2026-04-22T00:00:00Z")])
            .unwrap();
        db.mark_exhausted(provider).unwrap();
        assert!(exhausted_at_raw(&db, provider).is_some());

        db.upsert_quota_refresh(provider, &[quota_input(0.20, "2026-04-23T00:00:00Z")])
            .unwrap();

        assert_eq!(exhausted_at_raw(&db, provider), None);
    }

    #[test]
    fn upsert_quota_refresh_preserves_exhausted_at_on_empty_refresh() {
        let db = test_db();
        let provider = "p";
        db.upsert_quota_refresh(provider, &[quota_input(0.10, "2026-04-22T00:00:00Z")])
            .unwrap();
        db.mark_exhausted(provider).unwrap();
        let exhausted_before = exhausted_at_raw(&db, provider).expect("exhausted_at should be set");

        db.upsert_quota_refresh(provider, &[]).unwrap();

        assert_eq!(
            exhausted_at_raw(&db, provider).as_deref(),
            Some(exhausted_before.as_str())
        );
    }

    #[test]
    fn quota_tight_routing_column_dropped_after_migration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let conn = sqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE invocations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                invocation_uuid TEXT NOT NULL UNIQUE,
                model_name TEXT NOT NULL,
                provider_name TEXT,
                provider_index INTEGER NOT NULL,
                parent_invocation_id INTEGER REFERENCES invocations(id),
                status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'legacy')),
                success INTEGER,
                exit_code INTEGER,
                error_category TEXT,
                session_id TEXT,
                session_capture_method TEXT,
                quota_tight_routing BOOLEAN NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                finished_at TEXT
            );",
        )
        .unwrap();
        mark_current_schema_version(&conn);
        drop(conn);

        let db = StateDb::open(&path).unwrap();

        let columns: Vec<String> = db
            .conn
            .prepare("PRAGMA table_info(invocations)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            !columns.iter().any(|column| column == "quota_tight_routing"),
            "quota_tight_routing should be removed by migration: {columns:?}"
        );
    }

    // Risk: Providers migration from pre-fix aggregate shape | level: particular-integration
    // Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track
    #[test]
    fn providers_migration_rebuilds_aggregate_from_invocations_by_provider_name() {
        let dir = provider_rebuild_fixture_db();
        let path = dir.path().join("state.db");

        let db = StateDb::open(&path).unwrap();

        let columns = table_columns_with_pk(&db.conn, "providers");
        assert!(
            columns
                .iter()
                .any(|(name, pk)| name == "provider_name" && *pk == 2),
            "providers must be keyed by provider_name after migration: {columns:?}"
        );
        assert!(
            !columns.iter().any(|(name, _)| name == "provider_index"),
            "providers.provider_index must be removed after migration: {columns:?}"
        );

        let rows = provider_aggregate_snapshot(&db.conn);
        assert_eq!(
            rows,
            vec![
                ProviderAggregateSnapshot {
                    model_name: "routing-model".to_string(),
                    provider_name: "claude".to_string(),
                    invocation_count: 1,
                    error_count: 0,
                    last_error: None,
                    last_error_at: None,
                    last_invoked_at: Some("2026-04-20T12:00:01+00:00".to_string()),
                },
                ProviderAggregateSnapshot {
                    model_name: "routing-model".to_string(),
                    provider_name: "claude2".to_string(),
                    invocation_count: 2,
                    error_count: 1,
                    last_error: Some("rate_limit".to_string()),
                    last_error_at: Some("2026-04-20T11:00:01+00:00".to_string()),
                    last_invoked_at: Some("2026-04-20T11:00:01+00:00".to_string()),
                },
            ]
        );
    }

    // Risk: Quota path unchanged regression | level: unit
    // Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track
    #[test]
    fn quota_schema_remains_name_keyed_after_provider_migration() {
        let dir = provider_rebuild_fixture_db();
        let path = dir.path().join("state.db");

        let db = StateDb::open(&path).unwrap();

        let quota_columns = table_columns_with_pk(&db.conn, "provider_quotas");
        assert!(
            quota_columns
                .iter()
                .any(|(name, pk)| name == "provider_name" && *pk == 1),
            "provider_quotas must remain keyed only by provider_name: {quota_columns:?}"
        );
        assert!(
            !quota_columns
                .iter()
                .any(|(name, _)| name == "model_name" || name == "provider_index"),
            "provider_quotas must not gain aggregate identity columns: {quota_columns:?}"
        );

        let window_columns = table_columns_with_pk(&db.conn, "provider_quota_windows");
        assert!(
            window_columns
                .iter()
                .any(|(name, pk)| name == "provider_name" && *pk == 1),
            "provider_quota_windows must remain provider-name keyed: {window_columns:?}"
        );
        assert!(
            !window_columns
                .iter()
                .any(|(name, _)| name == "model_name" || name == "provider_index"),
            "provider_quota_windows must not gain aggregate identity columns: {window_columns:?}"
        );
    }

    // Risk: Migration error contract — unexpected shape rejected | level: particular-integration
    // Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track
    #[test]
    fn providers_migration_rejects_unexpected_shape_without_mutating_source_tables() {
        let dir = malformed_providers_shape_db();
        let path = dir.path().join("state.db");
        let conn = sqlite::Connection::open(&path).unwrap();
        let providers_before = malformed_providers_snapshot(&conn);
        let invocations_before = invocations_snapshot(&conn);
        drop(conn);

        let err = match StateDb::open(&path) {
            Ok(_) => panic!("unexpected providers shape should fail StateDb::open"),
            Err(err) => err,
        };
        let err_lower = err.to_ascii_lowercase();
        assert!(
            err_lower.contains("providers") && err_lower.contains("unexpected"),
            "unexpected-shape error should name providers and unexpected shape; got {err}"
        );

        let conn = sqlite::Connection::open(&path).unwrap();
        assert_eq!(malformed_providers_snapshot(&conn), providers_before);
        assert_eq!(invocations_snapshot(&conn), invocations_before);
        conn.execute_batch("DROP TABLE providers").unwrap();
        drop(conn);

        let recovered = StateDb::open(&path).unwrap();
        let columns = table_columns_with_pk(&recovered.conn, "providers");
        assert!(
            columns
                .iter()
                .any(|(name, pk)| name == "provider_name" && *pk == 2),
            "operator cleanup should let missing-table branch create post-fix providers: {columns:?}"
        );
        assert!(
            !columns.iter().any(|(name, _)| name == "provider_index"),
            "operator cleanup must not recreate provider_index: {columns:?}"
        );
    }

    // Risk: Migration error contract rejects malformed provider column metadata | level: particular-integration
    // Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track
    #[test]
    fn providers_migration_rejects_wrong_affinity_shape() {
        let dir = malformed_providers_affinity_db();
        let path = dir.path().join("state.db");

        let err = match StateDb::open(&path) {
            Ok(_) => panic!("wrong providers affinity should fail StateDb::open"),
            Err(err) => err,
        };

        assert!(
            err.contains("provider_index(type=TEXT"),
            "unexpected-shape error should describe the wrong affinity; got {err}"
        );
    }

    // Risk: Migration error contract — providers as non-table object rejected | level: particular-integration
    // Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track
    #[test]
    fn providers_migration_rejects_non_table_object_named_providers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let conn = sqlite::Connection::open(&path).unwrap();
        conn.execute_batch(StateDb::invocations_schema_sql())
            .unwrap();
        // SQLite shares table/view namespace; create a VIEW named providers.
        conn.execute_batch(
            "CREATE TABLE providers_source (
                 model_name TEXT NOT NULL,
                 provider_name TEXT NOT NULL,
                 invocation_count INTEGER NOT NULL DEFAULT 0,
                 error_count INTEGER NOT NULL DEFAULT 0,
                 last_error TEXT,
                 last_error_at TEXT,
                 last_invoked_at TEXT,
                 PRIMARY KEY (model_name, provider_name)
             );
             CREATE VIEW providers AS
                 SELECT model_name, provider_name, invocation_count, error_count,
                        last_error, last_error_at, last_invoked_at
                   FROM providers_source;",
        )
        .unwrap();
        mark_current_schema_version(&conn);
        drop(conn);

        let err = match StateDb::open(&path) {
            Ok(_) => panic!("non-table object named providers should fail StateDb::open"),
            Err(err) => err,
        };
        assert!(
            err.contains("object type=view"),
            "object-type rejection should name the unexpected type; got {err}"
        );

        let conn = sqlite::Connection::open(&path).unwrap();
        let mut stmt = conn
            .prepare("SELECT type FROM sqlite_master WHERE name = 'providers'")
            .unwrap();
        let observed_type: String = stmt
            .query_row([], |row| row.get(0))
            .expect("providers object should still exist after rejected open");
        assert_eq!(
            observed_type, "view",
            "rejected open must not mutate the providers object"
        );
    }

    // Risk: Migration error contract — providers with foreign keys rejected | level: particular-integration
    // Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track
    #[test]
    fn providers_migration_rejects_table_with_foreign_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let conn = sqlite::Connection::open(&path).unwrap();
        conn.execute_batch(StateDb::invocations_schema_sql())
            .unwrap();
        conn.execute_batch(
            "CREATE TABLE models (
                 name TEXT NOT NULL PRIMARY KEY
             );
             INSERT INTO models (name) VALUES ('routing-model');
             CREATE TABLE providers (
                 model_name TEXT NOT NULL REFERENCES models(name),
                 provider_index INTEGER NOT NULL,
                 invocation_count INTEGER NOT NULL DEFAULT 0,
                 error_count INTEGER NOT NULL DEFAULT 0,
                 last_error TEXT,
                 last_error_at TEXT,
                 last_invoked_at TEXT,
                 PRIMARY KEY (model_name, provider_index)
             );",
        )
        .unwrap();
        mark_current_schema_version(&conn);
        drop(conn);

        let err = match StateDb::open(&path) {
            Ok(_) => panic!("providers with foreign keys should fail StateDb::open"),
            Err(err) => err,
        };
        assert!(
            err.contains("foreign-key constraints present"),
            "foreign-key rejection should name foreign keys; got {err}"
        );
    }

    // Risk: Migration error contract rejects before source-table mutation | level: particular-integration
    // Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track; ~/projects/agent-runner/planning/trunk/research/10-routing-claude-skipped-contract.md §2 Migration helper
    #[test]
    fn providers_preflight_rejects_malformed_shape_before_invocations_migration() {
        let dir = legacy_invocations_with_malformed_providers_db();
        let path = dir.path().join("state.db");
        let conn = sqlite::Connection::open(&path).unwrap();
        let invocations_before = legacy_invocations_snapshot(&conn);
        drop(conn);

        let err = match StateDb::open(&path) {
            Ok(_) => panic!("malformed providers shape should fail before invocations migration"),
            Err(err) => err,
        };

        assert!(
            err.contains("Unexpected providers schema shape"),
            "unexpected-shape error should come from providers preflight; got {err}"
        );

        let conn = sqlite::Connection::open(&path).unwrap();
        assert_eq!(legacy_invocations_snapshot(&conn), invocations_before);
    }

    // Risk: Migration ensure_providers_schema is idempotent across reopens | level: unit
    // Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track
    #[test]
    fn providers_migration_is_idempotent_across_reopens() {
        let dir = provider_rebuild_fixture_db();
        let path = dir.path().join("state.db");

        let first = StateDb::open(&path).unwrap();
        let first_rows = provider_aggregate_snapshot(&first.conn);
        drop(first);

        let second = StateDb::open(&path).unwrap();
        let second_rows = provider_aggregate_snapshot(&second.conn);

        assert_eq!(second_rows, first_rows);
    }

    // Risk: Migration last_error_at reflects most recent failed invocation | level: particular-integration
    // Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track
    #[test]
    fn providers_migration_last_error_at_uses_most_recent_failure_not_later_success() {
        let dir = provider_last_error_fixture_db();
        let path = dir.path().join("state.db");

        let db = StateDb::open(&path).unwrap();
        let rows = provider_aggregate_snapshot(&db.conn);

        assert_eq!(
            rows,
            vec![ProviderAggregateSnapshot {
                model_name: "routing-model".to_string(),
                provider_name: "claude".to_string(),
                invocation_count: 3,
                error_count: 2,
                last_error: Some("auth_error".to_string()),
                last_error_at: Some("2026-04-20T10:30:10+00:00".to_string()),
                last_invoked_at: Some("2026-04-20T11:00:10+00:00".to_string()),
            }]
        );
    }

    // Risk: Migration last_error_at deterministic tie-break | level: particular-integration
    // Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track; ~/projects/agent-runner/planning/trunk/research/10-routing-claude-skipped-contract.md §2 Migration helper
    #[test]
    fn providers_migration_last_error_ties_use_highest_invocation_id() {
        let dir = provider_last_error_tie_fixture_db();
        let path = dir.path().join("state.db");

        let db = StateDb::open(&path).unwrap();
        let rows = provider_aggregate_snapshot(&db.conn);

        assert_eq!(
            rows,
            vec![ProviderAggregateSnapshot {
                model_name: "routing-model".to_string(),
                provider_name: "claude".to_string(),
                invocation_count: 2,
                error_count: 2,
                last_error: Some("auth_error".to_string()),
                last_error_at: Some("2026-04-20T10:00:10+00:00".to_string()),
                last_invoked_at: Some("2026-04-20T10:00:10+00:00".to_string()),
            }]
        );
    }

    #[test]
    fn schema_creation() {
        let db = test_db();
        let sql = invocation_table_sql(&db);
        assert!(sql.contains("invocation_uuid TEXT NOT NULL UNIQUE"));
        assert!(sql.contains("provider_name TEXT"));
        assert!(sql.contains("parent_invocation_id INTEGER REFERENCES invocations(id)"));
        assert!(sql.contains(
            "status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'legacy'))"
        ));
        assert!(sql.contains("success INTEGER"));
        assert!(sql.contains("finished_at TEXT"));
        assert!(sql.contains("session_id TEXT"));
        assert!(sql.contains("session_capture_method TEXT"));
        assert!(sql.contains("resume_acceptance_status TEXT"));
        assert!(sql.contains("resume_acceptance_evidence TEXT"));

        let indexes: Vec<String> = db
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'invocations' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            indexes,
            vec![
                "idx_invocations_parent".to_string(),
                "idx_invocations_provider_created".to_string(),
                "idx_invocations_provider_provider_session".to_string(),
                "idx_invocations_provider_session".to_string(),
                "idx_invocations_uuid".to_string(),
                "sqlite_autoindex_invocations_1".to_string(),
            ]
        );
    }

    // RISK: fresh schema path could omit terminal_reason (proposal §test-intent "schema cascade tests", assumption A5)
    // LEVEL: unit
    // SOURCE: contracts/nes-250-contract.md § Test catalog § Schema cascade (T-SCHEMA-FRESH)
    #[test]
    fn t_schema_fresh_invocations_schema_includes_nullable_terminal_reason() {
        let db = test_db();
        let columns = invocation_columns(&db);

        assert!(
            columns.iter().any(|column| column == "terminal_reason"),
            "fresh invocations schema must expose terminal_reason: {columns:?}"
        );

        let nullable: i64 = db
            .conn
            .query_row(
                "SELECT [notnull] FROM pragma_table_info('invocations') WHERE name = 'terminal_reason'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(nullable, 0, "terminal_reason must be nullable");
    }

    // RISK: incremental ALTER path could miss terminal_reason or destroy existing invocation data (proposal §test-intent "schema cascade tests", assumption A5)
    // LEVEL: unit
    // SOURCE: contracts/nes-250-contract.md § Test catalog § Schema cascade (T-SCHEMA-INCREMENTAL)
    #[test]
    fn t_schema_incremental_adds_terminal_reason_without_losing_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let conn = sqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE invocations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                invocation_uuid TEXT NOT NULL UNIQUE,
                model_name TEXT NOT NULL,
                provider_name TEXT,
                provider_index INTEGER NOT NULL,
                parent_invocation_id INTEGER REFERENCES invocations(id),
                status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'legacy')),
                success INTEGER,
                exit_code INTEGER,
                error_category TEXT,
                session_id TEXT,
                session_capture_method TEXT,
                resume_acceptance_status TEXT,
                resume_acceptance_evidence TEXT,
                created_at TEXT NOT NULL,
                finished_at TEXT
            );
            INSERT INTO invocations (
                invocation_uuid, model_name, provider_name, provider_index,
                status, success, exit_code, error_category, created_at, finished_at
            ) VALUES (
                '11111111-1111-1111-1111-111111111111',
                'fixture-model', 'fixture-provider', 0,
                'failed', 0, 7, 'fixture_error',
                '2026-04-17T08:00:00Z', '2026-04-17T08:00:01Z'
            );",
        )
        .unwrap();
        mark_current_schema_version(&conn);
        drop(conn);

        let db = StateDb::open(&path).unwrap();
        let columns = invocation_columns(&db);
        assert!(
            columns.iter().any(|column| column == "terminal_reason"),
            "incremental migration must add terminal_reason: {columns:?}"
        );

        let row = db
            .get_invocation_by_uuid("11111111-1111-1111-1111-111111111111")
            .unwrap()
            .unwrap();
        assert_eq!(row.status, InvocationStatus::Failed);
        assert_eq!(row.exit_code, Some(7));
        assert_eq!(row.error_category.as_deref(), Some("fixture_error"));
        assert_eq!(row.terminal_reason, None);
    }

    // RISK: legacy rebuild path could omit terminal_reason or synthesize historical terminal meaning (proposal §test-intent "schema cascade tests", assumption A5)
    // LEVEL: unit
    // SOURCE: contracts/nes-250-contract.md § Test catalog § Schema cascade (T-SCHEMA-LEGACY)
    #[test]
    fn t_schema_legacy_rebuild_adds_terminal_reason_and_migrates_null() {
        let dir = legacy_invocations_db(&[(
            "mapped-model",
            0,
            0,
            7,
            Some("rate_limit"),
            "2026-04-17T08:00:00Z",
        )]);

        let db = StateDb::open(&dir.path().join("state.db")).unwrap();
        let columns = invocation_columns(&db);
        assert!(
            columns.iter().any(|column| column == "terminal_reason"),
            "legacy rebuild must add terminal_reason: {columns:?}"
        );

        let terminal_reason: Option<String> = db
            .conn
            .query_row(
                "SELECT terminal_reason FROM invocations WHERE model_name = 'mapped-model'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(terminal_reason, None);
    }

    #[test]
    fn update_resume_acceptance_persists_status_and_evidence() {
        let db = test_db();
        let start = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "test-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        };
        let id = db.start_invocation(&start).unwrap();

        db.update_resume_acceptance(id, "accepted", Some("matched session id"))
            .unwrap();

        let row = db
            .get_invocation_by_uuid(&start.invocation_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(row.resume_acceptance_status.as_deref(), Some("accepted"));
        assert_eq!(
            row.resume_acceptance_evidence.as_deref(),
            Some("matched session id")
        );
    }

    #[test]
    fn session_turns_schema_creation_includes_sidechain_columns() {
        let db = test_db();
        let sql: String = db
            .conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'session_turns'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert!(sql.contains("parent_turn_id TEXT"));
        assert!(sql.contains("is_sidechain INTEGER NOT NULL DEFAULT 0"));
        assert!(sql.contains("body TEXT"));
    }

    #[test]
    fn session_turns_schema_migration_adds_parent_and_sidechain_columns() {
        let dir = legacy_session_turns_db();
        let db = StateDb::open(&dir.path().join("state.db")).unwrap();

        let columns: Vec<(String, String, i64, Option<String>)> = db
            .conn
            .prepare("PRAGMA table_info(session_turns)")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();

        assert!(columns.iter().any(|column| {
            column.0 == "parent_turn_id"
                && column.1 == "TEXT"
                && column.2 == 0
                && column.3.is_none()
        }));
        assert!(columns.iter().any(|column| {
            column.0 == "is_sidechain"
                && column.1 == "INTEGER"
                && column.2 == 1
                && column.3.as_deref() == Some("0")
        }));
    }

    #[test]
    fn session_turns_schema_migration_adds_nullable_body_to_legacy_db() {
        // risk: legacy-DB upgrade; level: unit; source: contract §4 T5 / proposal A2,A8.
        let dir = legacy_session_turns_db();
        let path = dir.path().join("state.db");
        let conn = sqlite::Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO session_turns
                (provider_name, session_id, turn_id, timestamp, role, source_file, ingested_at)
             VALUES ('fixture-provider', 'session-a', 'legacy-turn', '2026-04-17T08:00:00Z', 'assistant', '', '2026-04-17T08:00:00Z')",
            [],
        )
        .unwrap();
        drop(conn);

        let db = StateDb::open(&path).unwrap();

        let session_columns: Vec<(String, String, i64)> = db
            .conn
            .prepare("PRAGMA table_info(session_turns)")
            .unwrap()
            .query_map([], |row| Ok((row.get(1)?, row.get(2)?, row.get(3)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            session_columns
                .iter()
                .any(|(name, data_type, notnull)| name == "body"
                    && data_type == "TEXT"
                    && *notnull == 0),
            "legacy migration must add nullable body TEXT; columns={session_columns:?}"
        );
        let body: Option<String> = db
            .conn
            .query_row(
                "SELECT body FROM session_turns WHERE turn_id = 'legacy-turn'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(body, None);

        let quota_columns: Vec<String> = db
            .conn
            .prepare("PRAGMA table_info(provider_quotas)")
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            quota_columns
                .iter()
                .any(|column| column == "topology_peak_live_window_count"),
            "body migration must coexist with WU-13 quota topology migration; columns={quota_columns:?}"
        );
        assert!(
            quota_columns
                .iter()
                .any(|column| column == "last_topology_probe_at"),
            "body migration must coexist with WU-13 quota topology migration; columns={quota_columns:?}"
        );
    }

    #[test]
    fn session_turns_schema_creation_includes_resume_lookup_index() {
        let db = test_db();
        let indexes: Vec<String> = db
            .conn
            .prepare(
                "SELECT name FROM sqlite_master
                 WHERE type = 'index' AND tbl_name = 'session_turns'
                 ORDER BY name",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();

        assert!(
            indexes.contains(&"idx_session_turns_session_lookup".to_string()),
            "resume lookup index must exist on fresh DB bootstrap: {indexes:?}"
        );
    }

    #[test]
    fn session_turns_schema_migration_adds_resume_lookup_index() {
        let dir = legacy_session_turns_db();
        let db = StateDb::open(&dir.path().join("state.db")).unwrap();
        let indexes: Vec<String> = db
            .conn
            .prepare(
                "SELECT name FROM sqlite_master
                 WHERE type = 'index' AND tbl_name = 'session_turns'
                 ORDER BY name",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();

        assert!(
            indexes.contains(&"idx_session_turns_session_lookup".to_string()),
            "resume lookup index must be added on existing DB open: {indexes:?}"
        );
    }

    #[test]
    fn migration_backfills_resolved_and_legacy_rows() {
        with_models_config(
            "mapped-model",
            r#"
[[providers]]
name = "fixture-provider"
"#,
            || {
                let dir = legacy_invocations_db(&[
                    ("mapped-model", 0, 1, 0, None, "2026-04-17T08:00:00Z"),
                    (
                        "missing-model",
                        0,
                        0,
                        7,
                        Some("rate_limit"),
                        "2026-04-17T08:05:00Z",
                    ),
                ]);
                let db = StateDb::open(&dir.path().join("state.db")).unwrap();

                let rows: Vec<(String, Option<String>, String, String, String)> = db
                    .conn
                    .prepare(
                        "SELECT model_name, provider_name, status, invocation_uuid, finished_at
                         FROM invocations ORDER BY created_at",
                    )
                    .unwrap()
                    .query_map([], |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    })
                    .unwrap()
                    .map(Result::unwrap)
                    .collect();

                assert_eq!(rows[0].0, "mapped-model");
                assert_eq!(rows[0].1.as_deref(), Some("fixture-provider"));
                assert_eq!(rows[0].2, "succeeded");
                assert_eq!(rows[0].4, "2026-04-17T08:00:00Z");
                assert!(Uuid::parse_str(&rows[0].3).is_ok());

                assert_eq!(rows[1].0, "missing-model");
                assert_eq!(rows[1].1, None);
                assert_eq!(rows[1].2, "legacy");
                assert_eq!(rows[1].4, "2026-04-17T08:05:00Z");
                assert!(Uuid::parse_str(&rows[1].3).is_ok());
            },
        );
    }

    #[test]
    fn migration_rolls_back_when_rebuild_fails() {
        let dir = legacy_invocations_db(&[("mapped-model", 0, 1, 0, None, "2026-04-17T08:00:00Z")]);
        let path = dir.path().join("state.db");
        let conn = sqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE invocations_new (id INTEGER PRIMARY KEY);
             CREATE TABLE blocker (name TEXT);
             CREATE INDEX idx_invocations_uuid ON blocker(name);",
        )
        .unwrap();
        drop(conn);

        let err = match StateDb::open(&path) {
            Ok(_) => panic!("migration should fail"),
            Err(err) => err,
        };
        assert!(!err.is_empty());

        let conn = sqlite::Connection::open(&path).unwrap();
        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(invocations)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            columns,
            vec![
                "id",
                "model_name",
                "provider_index",
                "success",
                "exit_code",
                "error_category",
                "created_at",
            ]
        );
        let row_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
            .unwrap();
        assert_eq!(row_count, 1);
    }

    /// Per V10 (failures observable, never silent): if the models config
    /// is unloadable mid-migration, the rebuild must still succeed and
    /// degrade rows to `status='legacy'` / `provider_name=NULL`. Opening
    /// the DB MUST NOT fail just because the config is corrupt.
    #[test]
    fn migration_succeeds_with_corrupt_models_config_and_marks_rows_legacy() {
        let _guard = env_lock().lock().unwrap();
        let dir = legacy_invocations_db(&[
            ("any-model", 0, 1, 0, None, "2026-04-17T08:00:00Z"),
            (
                "other-model",
                1,
                0,
                1,
                Some("rate_limit"),
                "2026-04-17T08:05:00Z",
            ),
        ]);
        let path = dir.path().join("state.db");

        // Plant a corrupt models/ directory at XDG_CONFIG_HOME so the
        // load_models() call inside migration fails.
        let config_root = dir.path().join("oulipoly-agent-runner");
        let models_dir = config_root.join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        std::fs::write(
            models_dir.join("broken.toml"),
            "this = is = not = valid = toml",
        )
        .unwrap();

        let old = std::env::var_os("XDG_CONFIG_HOME");
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", dir.path());
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // The DB open must succeed despite the corrupt config.
            let db = StateDb::open(&path).expect("DB open must not fail on corrupt models config");

            // Verify both legacy rows migrated cleanly with provider_name=NULL
            // and status='legacy' since the lookup couldn't resolve anything.
            let conn = sqlite::Connection::open(&path).unwrap();
            let mut stmt = conn
                .prepare(
                    "SELECT model_name, provider_name, status, invocation_uuid, finished_at
                     FROM invocations ORDER BY created_at",
                )
                .unwrap();
            let rows: Vec<(String, Option<String>, String, String, String)> = stmt
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                })
                .unwrap()
                .map(Result::unwrap)
                .collect();
            assert_eq!(rows.len(), 2);
            for r in &rows {
                assert!(
                    r.1.is_none(),
                    "provider_name must be NULL on corrupt config"
                );
                assert_eq!(r.2, "legacy", "status must be legacy on corrupt config");
                assert!(Uuid::parse_str(&r.3).is_ok());
                assert!(!r.4.is_empty(), "finished_at must be backfilled");
            }
            drop(db);
        }));
        match old {
            Some(value) => unsafe {
                std::env::set_var("XDG_CONFIG_HOME", value);
            },
            None => unsafe {
                std::env::remove_var("XDG_CONFIG_HOME");
            },
        }
        if let Err(payload) = result {
            std::panic::resume_unwind(payload);
        }
    }

    #[test]
    fn upsert_quota_refresh_preserves_windows_on_empty_input() {
        let db = test_db();
        let provider = "p";
        let windows = [
            quota_input(0.10, "2026-04-22T00:00:00Z"),
            quota_input(0.20, "2026-04-28T00:00:00Z"),
        ];
        db.upsert_quota_refresh(provider, &windows).unwrap();
        let before = quota_window_rows(&db, provider);

        db.upsert_quota_refresh(provider, &[]).unwrap();

        assert_eq!(quota_window_rows(&db, provider), before);
    }

    /// Risk: Migration might omit columns or leave legacy rows with no usable peak count.
    /// Level: particular-integration.
    /// Source: proposal §Test-intent track row 10; Assumption A6.
    #[test]
    fn provider_quotas_topology_columns_created_and_backfilled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let conn = sqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE provider_quotas (
                provider_name TEXT PRIMARY KEY,
                used_percent REAL NOT NULL DEFAULT 0,
                resets_at TEXT,
                calls_since_refresh INTEGER NOT NULL DEFAULT 0,
                refreshed_at TEXT,
                last_empty_refresh_at TEXT,
                exhausted_at TEXT NULL
            );
            CREATE TABLE provider_quota_windows (
                provider_name TEXT NOT NULL,
                window_id INTEGER NOT NULL,
                used_percent REAL NOT NULL DEFAULT 0,
                resets_at TEXT NOT NULL,
                last_delta_percent REAL,
                last_delta_calls INTEGER,
                PRIMARY KEY (provider_name, window_id)
            );
            INSERT INTO provider_quotas
                (provider_name, used_percent, resets_at, calls_since_refresh, refreshed_at)
            VALUES
                ('p', 0.20, '2026-04-28T00:00:00Z', 3, '2026-04-21T00:00:00Z'),
                ('empty', 0.00, NULL, 0, '2026-04-21T00:00:00Z');
            INSERT INTO provider_quota_windows
                (provider_name, window_id, used_percent, resets_at)
            VALUES
                ('p', 0, 0.20, '2026-04-22T00:00:00Z'),
                ('p', 1, 0.30, '2026-04-28T00:00:00Z');",
        )
        .unwrap();
        mark_current_schema_version(&conn);
        drop(conn);

        let db = StateDb::open(&path).unwrap();

        let columns: Vec<String> = db
            .conn
            .prepare("PRAGMA table_info(provider_quotas)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            columns
                .iter()
                .any(|column| column == "topology_peak_live_window_count"),
            "provider_quotas topology peak column missing after migration: {columns:?}"
        );
        assert!(
            columns
                .iter()
                .any(|column| column == "last_topology_probe_at"),
            "provider_quotas probe timestamp column missing after migration: {columns:?}"
        );

        let quota = db.get_quota("p").unwrap().unwrap();
        assert_eq!(quota.topology_peak_live_window_count, 2);
        assert!(quota.last_topology_probe_at.is_none());

        let empty_quota = db.get_quota("empty").unwrap().unwrap();
        assert_eq!(empty_quota.topology_peak_live_window_count, 0);
        assert!(empty_quota.last_topology_probe_at.is_none());
    }

    /// Risk: Migration backfill could clobber an existing higher
    /// `topology_peak_live_window_count` column when a partial legacy
    /// row already includes the column without the probe-timestamp
    /// column.
    /// Level: particular-integration.
    /// Source: contract §4 (Migration helper); CodeRabbit pass 1
    /// finding R1-F06 (idempotent self-healing backfill).
    #[test]
    fn provider_quotas_topology_backfill_recovers_when_column_already_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let conn = sqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE provider_quotas (
                provider_name TEXT PRIMARY KEY,
                used_percent REAL NOT NULL DEFAULT 0,
                resets_at TEXT,
                calls_since_refresh INTEGER NOT NULL DEFAULT 0,
                refreshed_at TEXT,
                last_empty_refresh_at TEXT,
                exhausted_at TEXT NULL,
                topology_peak_live_window_count INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE provider_quota_windows (
                provider_name TEXT NOT NULL,
                window_id INTEGER NOT NULL,
                used_percent REAL NOT NULL DEFAULT 0,
                resets_at TEXT NOT NULL,
                last_delta_percent REAL,
                last_delta_calls INTEGER,
                PRIMARY KEY (provider_name, window_id)
            );
            INSERT INTO provider_quotas
                (provider_name, used_percent, resets_at, calls_since_refresh, refreshed_at, topology_peak_live_window_count)
            VALUES
                ('p', 0.20, '2026-04-28T00:00:00Z', 3, '2026-04-21T00:00:00Z', 0),
                ('already-high', 0.20, '2026-04-28T00:00:00Z', 3, '2026-04-21T00:00:00Z', 4);
            INSERT INTO provider_quota_windows
                (provider_name, window_id, used_percent, resets_at)
            VALUES
                ('p', 0, 0.20, '2026-04-22T00:00:00Z'),
                ('p', 1, 0.30, '2026-04-28T00:00:00Z'),
                ('already-high', 0, 0.20, '2026-04-22T00:00:00Z');",
        )
        .unwrap();
        mark_current_schema_version(&conn);
        drop(conn);

        let db = StateDb::open(&path).unwrap();

        assert_eq!(
            db.get_quota("p")
                .unwrap()
                .unwrap()
                .topology_peak_live_window_count,
            2
        );
        assert_eq!(
            db.get_quota("already-high")
                .unwrap()
                .unwrap()
                .topology_peak_live_window_count,
            4,
            "schema repair must not lower a previously learned topology peak"
        );
    }

    /// Risk: Non-empty incomplete refresh could erase peak topology memory.
    /// Level: unit.
    /// Source: proposal §Test-intent track row 11; Assumptions A2, A6.
    #[test]
    fn upsert_quota_refresh_updates_topology_peak_without_lowering_on_shrink() {
        let db = test_db();
        let provider = "p";

        db.upsert_quota_refresh(
            provider,
            &[
                quota_input(0.10, "2026-04-22T00:00:00Z"),
                quota_input(0.20, "2026-04-28T00:00:00Z"),
            ],
        )
        .unwrap();
        assert_eq!(
            db.get_quota(provider)
                .unwrap()
                .unwrap()
                .topology_peak_live_window_count,
            2
        );

        db.upsert_quota_refresh(provider, &[quota_input(0.30, "2026-04-23T12:00:00Z")])
            .unwrap();

        assert_eq!(db.get_windows(provider).unwrap().len(), 1);
        assert_eq!(
            db.get_quota(provider)
                .unwrap()
                .unwrap()
                .topology_peak_live_window_count,
            2,
            "topology peak should preserve the prior complete topology after a non-empty shrink"
        );

        db.upsert_quota_refresh(provider, &[]).unwrap();
        assert_eq!(
            db.get_quota(provider)
                .unwrap()
                .unwrap()
                .topology_peak_live_window_count,
            2,
            "empty refreshes should not lower topology peak"
        );
    }

    /// Risk: Malformed state files could wrap a negative learned topology
    /// count into a huge `usize`.
    /// Level: unit.
    /// Source: CodeRabbit pass 1 finding R1-F03.
    #[test]
    fn get_quota_rejects_negative_topology_peak_count() {
        let db = test_db();

        db.conn
            .execute(
                "INSERT INTO provider_quotas
                    (provider_name, topology_peak_live_window_count)
                 VALUES (?1, ?2)",
                sqlite::params!["p", -1],
            )
            .unwrap();

        let error = db.get_quota("p").unwrap_err();

        assert!(
            error.contains("negative topology_peak_live_window_count"),
            "unexpected error: {error}"
        );
    }

    /// Risk: Cooldown marker could mutate quota windows or reset learning data.
    /// Level: unit.
    /// Source: proposal §Test-intent track row 12; Assumptions A2, A6.
    #[test]
    fn record_topology_probe_sets_timestamp_without_changing_windows() {
        let db = test_db();
        let provider = "p";
        db.upsert_quota_refresh(
            provider,
            &[
                quota_input(0.10, "2026-04-22T00:00:00Z"),
                quota_input(0.20, "2026-04-28T00:00:00Z"),
            ],
        )
        .unwrap();
        db.set_window_delta_for_test(provider, 0, 0.01, 40).unwrap();
        db.set_window_delta_for_test(provider, 1, 0.02, 40).unwrap();
        let before_windows = quota_window_detail_rows(&db, provider);
        let before = Utc::now();

        db.record_topology_probe(provider).unwrap();

        let after = Utc::now();
        let probe_at_raw =
            last_topology_probe_at_raw(&db, provider).expect("probe timestamp should be set");
        let probe_at = DateTime::parse_from_rfc3339(&probe_at_raw)
            .unwrap()
            .with_timezone(&Utc);
        assert!(
            probe_at >= before - chrono::Duration::seconds(1)
                && probe_at <= after + chrono::Duration::seconds(1),
            "last_topology_probe_at {probe_at} should be near record_topology_probe call"
        );
        assert_eq!(
            quota_window_detail_rows(&db, provider),
            before_windows,
            "record_topology_probe must not mutate window rows or learning deltas"
        );
    }

    #[test]
    fn upsert_quota_refresh_wipes_on_nonempty_input_with_all_replaced() {
        let db = test_db();
        let provider = "p";
        let windows = [
            quota_input(0.10, "2026-04-22T00:00:00Z"),
            quota_input(0.20, "2026-04-28T00:00:00Z"),
        ];
        db.upsert_quota_refresh(provider, &windows).unwrap();

        let replacement = [quota_input(0.30, "2026-04-23T12:00:00Z")];
        db.upsert_quota_refresh(provider, &replacement).unwrap();

        assert_eq!(
            quota_window_rows(&db, provider),
            vec![(0, 0.30, "2026-04-23T12:00:00+00:00".to_string())]
        );
    }

    #[test]
    fn upsert_quota_refresh_records_last_empty_refresh_at_on_empty_input() {
        let db = test_db();
        let provider = "p";
        let windows = [
            quota_input(0.10, "2026-04-22T00:00:00Z"),
            quota_input(0.20, "2026-04-28T00:00:00Z"),
        ];
        db.upsert_quota_refresh(provider, &windows).unwrap();

        let before = Utc::now();
        db.upsert_quota_refresh(provider, &[]).unwrap();
        let after = Utc::now();

        let last_empty = last_empty_refresh_at(&db, provider).unwrap();
        assert!(
            last_empty >= before - chrono::Duration::seconds(1)
                && last_empty <= after + chrono::Duration::seconds(1),
            "last_empty_refresh_at {last_empty} should be near empty refresh"
        );
    }

    #[test]
    fn upsert_quota_refresh_empty_input_with_no_prior_windows_creates_forced_stale_quota_row() {
        let db = test_db();
        let provider = "p";

        db.upsert_quota_refresh(provider, &[]).unwrap();

        let quota = db.get_quota(provider).unwrap().unwrap();
        assert!(quota.refreshed_at.is_some());
        assert!(last_empty_refresh_at(&db, provider).is_some());
        assert!(db.get_windows(provider).unwrap().is_empty());
        assert!(quota.refreshed_at.is_some());
    }

    #[test]
    fn upsert_quota_refresh_empty_input_does_not_reset_calls_since_refresh_when_prior_windows_exist()
     {
        let db = test_db();
        let provider = "p";
        let windows = [
            quota_input(0.10, "2026-04-22T00:00:00Z"),
            quota_input(0.20, "2026-04-28T00:00:00Z"),
        ];
        db.upsert_quota_refresh(provider, &windows).unwrap();
        for _ in 0..5 {
            db.increment_calls_since_refresh(provider).unwrap();
        }
        assert_eq!(calls_since_refresh(&db, provider), 5);

        db.upsert_quota_refresh(provider, &[]).unwrap();

        assert_eq!(calls_since_refresh(&db, provider), 5);
    }

    #[test]
    fn upsert_quota_refresh_writes_per_window_delta_for_matching_window_id() {
        let db = test_db();
        let provider = "p";
        db.upsert_quota_refresh(
            provider,
            &[
                quota_input(0.20, "2026-04-22T00:00:00Z"),
                quota_input(0.30, "2026-04-28T00:00:00Z"),
            ],
        )
        .unwrap();

        let refreshed_at = ts("2026-04-21T00:00:00Z");
        db.set_refreshed_at_for_test(provider, &refreshed_at)
            .unwrap();
        insert_assistant_turns_after(&db, provider, refreshed_at, 50, "delta-n1");

        db.upsert_quota_refresh(
            provider,
            &[
                quota_input(0.25, "2026-04-22T00:00:00Z"),
                quota_input(0.38, "2026-04-28T00:00:00Z"),
            ],
        )
        .unwrap();

        let windows = db.get_windows(provider).unwrap();
        assert_eq!(windows.len(), 2);
        assert!((windows[0].last_delta_percent.unwrap() - 0.05).abs() < 1e-9);
        assert_eq!(windows[0].last_delta_calls, Some(50));
        assert!((windows[1].last_delta_percent.unwrap() - 0.08).abs() < 1e-9);
        assert_eq!(windows[1].last_delta_calls, Some(50));
    }

    #[test]
    fn upsert_quota_refresh_carries_prior_window_delta_on_reset_or_no_change() {
        let db = test_db();
        let provider = "p";
        db.upsert_quota_refresh(
            provider,
            &[
                quota_input(0.20, "2026-04-22T00:00:00Z"),
                quota_input(0.30, "2026-04-28T00:00:00Z"),
            ],
        )
        .unwrap();

        let first_refreshed_at = ts("2026-04-21T00:00:00Z");
        db.set_refreshed_at_for_test(provider, &first_refreshed_at)
            .unwrap();
        insert_assistant_turns_after(&db, provider, first_refreshed_at, 50, "delta-n1");
        db.upsert_quota_refresh(
            provider,
            &[
                quota_input(0.25, "2026-04-22T00:00:00Z"),
                quota_input(0.38, "2026-04-28T00:00:00Z"),
            ],
        )
        .unwrap();

        let second_refreshed_at = ts("2026-04-21T12:00:00Z");
        db.set_refreshed_at_for_test(provider, &second_refreshed_at)
            .unwrap();
        insert_assistant_turns_after(&db, provider, second_refreshed_at, 20, "delta-n2");
        db.upsert_quota_refresh(
            provider,
            &[
                quota_input(0.25, "2026-04-22T00:00:00Z"),
                quota_input(0.05, "2026-04-28T00:00:00Z"),
            ],
        )
        .unwrap();

        let windows = db.get_windows(provider).unwrap();
        assert_eq!(windows.len(), 2);
        assert!((windows[1].last_delta_percent.unwrap() - 0.08).abs() < 1e-9);
        assert_eq!(windows[1].last_delta_calls, Some(50));
    }

    #[test]
    fn upsert_quota_refresh_rejects_pathological_burn_rate_sample() {
        // Regression: an upstream API spike (used_percent briefly reported as
        // 1.0) paired with a small turn count would previously learn a
        // pathological per-turn rate (~0.05/turn), carry it forward across
        // every subsequent no-change refresh, and permanently project every
        // provider near the ceiling. The sanity cap at
        // MAX_LEARNABLE_BURN_RATE = 0.1/turn rejects this sample and carries
        // the prior learn forward instead, so the pool stays usable.
        let db = test_db();
        let provider = "p";

        // Seed a plausible prior learn (0.05 / 100 calls = 5e-4 per turn).
        db.upsert_quota_refresh(provider, &[quota_input(0.20, "2026-04-22T00:00:00Z")])
            .unwrap();
        let t0 = ts("2026-04-21T00:00:00Z");
        db.set_refreshed_at_for_test(provider, &t0).unwrap();
        insert_assistant_turns_after(&db, provider, t0, 100, "prior-learn");
        db.upsert_quota_refresh(provider, &[quota_input(0.25, "2026-04-22T00:00:00Z")])
            .unwrap();

        let prior = db.get_windows(provider).unwrap();
        assert!((prior[0].last_delta_percent.unwrap() - 0.05).abs() < 1e-9);
        assert_eq!(prior[0].last_delta_calls, Some(100));

        // Now feed a pathological sample: used_percent jumps from 0.25 to
        // 0.95 over just 5 turns. dp = 0.70, dc = 5, so new_rate = 0.14/turn,
        // which exceeds MAX_LEARNABLE_BURN_RATE (0.1/turn).
        let t1 = ts("2026-04-21T06:00:00Z");
        db.set_refreshed_at_for_test(provider, &t1).unwrap();
        insert_assistant_turns_after(&db, provider, t1, 5, "spike");
        db.upsert_quota_refresh(provider, &[quota_input(0.95, "2026-04-22T00:00:00Z")])
            .unwrap();

        let after_spike = db.get_windows(provider).unwrap();
        // Pathological sample rejected: delta is still the prior 0.05/100.
        assert!(
            (after_spike[0].last_delta_percent.unwrap() - 0.05).abs() < 1e-9,
            "spike sample should not overwrite prior learn; got {:?}",
            after_spike[0].last_delta_percent
        );
        assert_eq!(after_spike[0].last_delta_calls, Some(100));
        // used_percent still reflects the incoming sample — we only reject
        // the delta learn, not the quota observation itself.
        assert!((after_spike[0].used_percent - 0.95).abs() < 1e-9);
    }

    #[test]
    fn upsert_quota_refresh_learns_sample_at_cap_boundary() {
        // A plausible-high rate just below the cap DOES get learned,
        // confirming the cap doesn't accidentally reject real workloads.
        // dp=0.90 over 25 turns → 0.036/turn. Below MAX_LEARNABLE_BURN_RATE
        // (0.1), above MIN_LEARN_SAMPLE_CALLS (20), below
        // NEAR_EXHAUSTED_USED_PERCENT (0.99). All three gates pass.
        let db = test_db();
        let provider = "p";
        db.upsert_quota_refresh(provider, &[quota_input(0.0, "2026-04-22T00:00:00Z")])
            .unwrap();
        let t0 = ts("2026-04-21T00:00:00Z");
        db.set_refreshed_at_for_test(provider, &t0).unwrap();
        insert_assistant_turns_after(&db, provider, t0, 25, "boundary");
        db.upsert_quota_refresh(provider, &[quota_input(0.90, "2026-04-22T00:00:00Z")])
            .unwrap();

        let w = db.get_windows(provider).unwrap();
        assert!((w[0].last_delta_percent.unwrap() - 0.90).abs() < 1e-9);
        assert_eq!(w[0].last_delta_calls, Some(25));
    }

    #[test]
    fn upsert_quota_refresh_rejects_learn_when_new_sample_near_rail() {
        // Regression: live observation 2026-04-21 had codex2's 7-day window
        // briefly read used_percent=1.0 from an upstream ChatGPT API spike,
        // paired with 34 turns since prior refresh. The learner computed
        // rate ≈ 0.029/turn on WEEKLY (real weekly rates are ~6e-5/turn;
        // the 100% sample was a cap-hit trajectory, not a natural fill),
        // which then projected every future invocation near the ceiling.
        // User framing: "turns barely budge weekly" —
        // so a weekly sample that moves 100 points in one interval is
        // distrusted. The marker we key on is "new used_percent at the
        // rail (>= 0.99)"; this test pins that gate.
        let db = test_db();
        let provider = "p";

        // Seed a plausible prior weekly rate: 0.02 over 300 turns → 6.7e-5/turn.
        db.upsert_quota_refresh(provider, &[quota_input(0.50, "2026-04-28T00:00:00Z")])
            .unwrap();
        let t0 = ts("2026-04-21T00:00:00Z");
        db.set_refreshed_at_for_test(provider, &t0).unwrap();
        insert_assistant_turns_after(&db, provider, t0, 300, "prior-weekly");
        db.upsert_quota_refresh(provider, &[quota_input(0.52, "2026-04-28T00:00:00Z")])
            .unwrap();

        let prior = db.get_windows(provider).unwrap();
        assert!((prior[0].last_delta_percent.unwrap() - 0.02).abs() < 1e-9);
        assert_eq!(prior[0].last_delta_calls, Some(300));

        // Upstream spike: new sample arrives at used_percent = 1.0 after
        // 34 turns. MIN_LEARN_SAMPLE_CALLS and MAX_LEARNABLE_BURN_RATE
        // alone would have let this through (34 > 20, 0.48/34 = 0.014/turn
        // < 0.1). The NEAR_EXHAUSTED_USED_PERCENT gate catches it.
        let t1 = ts("2026-04-21T12:00:00Z");
        db.set_refreshed_at_for_test(provider, &t1).unwrap();
        insert_assistant_turns_after(&db, provider, t1, 34, "spike");
        db.upsert_quota_refresh(provider, &[quota_input(1.0, "2026-04-28T00:00:00Z")])
            .unwrap();

        let after = db.get_windows(provider).unwrap();
        assert!(
            (after[0].last_delta_percent.unwrap() - 0.02).abs() < 1e-9,
            "near-rail sample must not overwrite prior weekly learn"
        );
        assert_eq!(after[0].last_delta_calls, Some(300));
        // used_percent still reflects the spike — we only distrust the rate.
        assert!((after[0].used_percent - 1.0).abs() < 1e-9);
    }

    #[test]
    fn upsert_quota_refresh_rejects_small_sample_delta_as_noise() {
        // Regression: live observation 2026-04-21 had claude2 with a learned
        // delta of 0.01/6 (rate 0.00167/turn). Paired with 193 turns since
        // refresh at scoring time, that projected 0.65 → 0.97, hard-blocking
        // the whole claude-opus pool. Sample-size floor of MIN_LEARN_SAMPLE_CALLS
        // rejects any delta learn below 20 turns and carries the prior
        // learn forward. At claude2 scale, this would have kept the pool
        // usable for the next invocation.
        let db = test_db();
        let provider = "p";

        // Seed a plausible prior learn (0.01 over 50 calls = 2e-4/turn).
        db.upsert_quota_refresh(provider, &[quota_input(0.10, "2026-04-22T00:00:00Z")])
            .unwrap();
        let t0 = ts("2026-04-21T00:00:00Z");
        db.set_refreshed_at_for_test(provider, &t0).unwrap();
        insert_assistant_turns_after(&db, provider, t0, 50, "prior-learn");
        db.upsert_quota_refresh(provider, &[quota_input(0.11, "2026-04-22T00:00:00Z")])
            .unwrap();

        let prior = db.get_windows(provider).unwrap();
        assert!((prior[0].last_delta_percent.unwrap() - 0.01).abs() < 1e-9);
        assert_eq!(prior[0].last_delta_calls, Some(50));

        // Now a small-sample observation: dp=0.01 over just 6 turns. Well
        // below the MAX_LEARNABLE_BURN_RATE cap (rate ≈ 0.00167), but
        // the sample size is too small to trust.
        let t1 = ts("2026-04-21T06:00:00Z");
        db.set_refreshed_at_for_test(provider, &t1).unwrap();
        insert_assistant_turns_after(&db, provider, t1, 6, "small-sample");
        db.upsert_quota_refresh(provider, &[quota_input(0.12, "2026-04-22T00:00:00Z")])
            .unwrap();

        let after = db.get_windows(provider).unwrap();
        // Small-sample rejected: prior 0.01/50 carried forward.
        assert!(
            (after[0].last_delta_percent.unwrap() - 0.01).abs() < 1e-9,
            "small-sample delta should not overwrite prior learn"
        );
        assert_eq!(after[0].last_delta_calls, Some(50));
    }

    // RISK: start_invocation could write terminal metadata on a running row (proposal §test-intent "terminal-reason absence characterization", assumption A5)
    // LEVEL: unit
    // SOURCE: contracts/nes-250-contract.md § Test catalog § Run-row null contract (T-RUN-NULL)
    #[test]
    fn start_invocation_inserts_running_row_with_null_terminal_fields() {
        let db = test_db();
        let start = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "test-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        };

        let id = db.start_invocation(&start).unwrap();
        let row = db
            .get_invocation_by_uuid(&start.invocation_uuid)
            .unwrap()
            .unwrap();

        assert_eq!(row.id, id);
        assert_eq!(row.status, InvocationStatus::Running);
        assert_eq!(row.provider_name.as_deref(), Some("fixture-provider"));
        assert_eq!(row.parent_invocation_id, None);
        assert_eq!(row.success, None);
        assert_eq!(row.exit_code, None);
        assert_eq!(row.terminal_reason, None);
        assert_eq!(row.finished_at, None);
    }

    #[test]
    fn running_invocation_provider_session_id() {
        let db = test_db();
        let start = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "test-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        };
        let id = db.start_invocation(&start).unwrap();
        let provider_session_id = Uuid::new_v4().to_string();

        db.bind_invocation_provider_session_start(
            id,
            &ProviderSessionBinding {
                provider_session_id: provider_session_id.clone(),
                capture_method: "forced_flag_verified",
                resume_input_id: None,
                provider_session_resolved_account: None,
            },
        )
        .unwrap();

        let row = db
            .get_invocation_by_uuid(&start.invocation_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(row.status, InvocationStatus::Running);
        assert_eq!(row.finished_at, None);
        assert_eq!(
            row.provider_session_id.as_deref(),
            Some(provider_session_id.as_str())
        );
        assert_eq!(
            row.provider_session_capture_method.as_deref(),
            Some("forced_flag_verified")
        );
    }

    #[test]
    fn running_invocation_chain_minted() {
        let db = test_db();
        let id = seed_running_invocation(&db);
        let provider_session_id = Uuid::new_v4().to_string();

        db.bind_invocation_provider_session_start(
            id,
            &ProviderSessionBinding {
                provider_session_id: provider_session_id.clone(),
                capture_method: "forced_flag_verified",
                resume_input_id: None,
                provider_session_resolved_account: None,
            },
        )
        .unwrap();

        let chain_id = db
            .chain_id_for_segment("fixture-provider", &provider_session_id)
            .unwrap()
            .expect("chain segment must be minted");
        assert!(Uuid::parse_str(&chain_id).is_ok());
    }

    #[test]
    fn bind_invocation_provider_session_start_same_id_is_idempotent() {
        let db = test_db();
        let id = seed_running_invocation(&db);
        let provider_session_id = Uuid::new_v4().to_string();
        let binding = ProviderSessionBinding {
            provider_session_id: provider_session_id.clone(),
            capture_method: "forced_flag_verified",
            resume_input_id: None,
            provider_session_resolved_account: None,
        };

        db.bind_invocation_provider_session_start(id, &binding)
            .unwrap();
        db.bind_invocation_provider_session_start(id, &binding)
            .unwrap();

        assert_eq!(segment_count(&db), 1);
        assert!(
            db.chain_id_for_segment("fixture-provider", &provider_session_id)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn bind_invocation_provider_session_start_conflicting_rebind_rejects_without_mutation() {
        let db = test_db();
        let id = seed_running_invocation(&db);
        let provider_session_id = Uuid::new_v4().to_string();
        db.bind_invocation_provider_session_start(
            id,
            &ProviderSessionBinding {
                provider_session_id: provider_session_id.clone(),
                capture_method: "forced_flag_verified",
                resume_input_id: None,
                provider_session_resolved_account: None,
            },
        )
        .unwrap();
        let before_segments = segment_count(&db);

        let err = db
            .bind_invocation_provider_session_start(
                id,
                &ProviderSessionBinding {
                    provider_session_id: Uuid::new_v4().to_string(),
                    capture_method: "forced_flag_verified",
                    resume_input_id: None,
                    provider_session_resolved_account: None,
                },
            )
            .unwrap_err();

        assert!(
            err.contains("already bound") || err.contains("refusing"),
            "{err}"
        );
        assert_eq!(segment_count(&db), before_segments);
        let stored: Option<String> = db
            .conn
            .query_row(
                "SELECT provider_session_id FROM invocations WHERE id = ?1",
                sqlite::params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored.as_deref(), Some(provider_session_id.as_str()));
    }

    #[test]
    fn bind_invocation_provider_session_start_matching_resume_input_does_not_mint_duplicate_chain()
    {
        let db = test_db();
        let id = seed_running_invocation(&db);
        let provider_session_id = Uuid::new_v4().to_string();

        db.bind_invocation_provider_session_start(
            id,
            &ProviderSessionBinding {
                provider_session_id: provider_session_id.clone(),
                capture_method: "resumed",
                resume_input_id: Some(provider_session_id.clone()),
                provider_session_resolved_account: None,
            },
        )
        .unwrap();

        assert_eq!(segment_count(&db), 0);
        let row: (Option<String>, Option<String>) = db
            .conn
            .query_row(
                "SELECT provider_session_id, resume_input_id FROM invocations WHERE id = ?1",
                sqlite::params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(row.0.as_deref(), Some(provider_session_id.as_str()));
        assert_eq!(row.1.as_deref(), Some(provider_session_id.as_str()));
    }

    #[test]
    fn bind_then_record_legacy_then_rebind_preserves_legacy_resume_session_id() {
        let db = test_db();
        let id = seed_running_invocation(&db);
        let provider_session_id = Uuid::new_v4().to_string();
        let legacy_resume_input = Uuid::new_v4().to_string();
        let binding = ProviderSessionBinding {
            provider_session_id: provider_session_id.clone(),
            capture_method: "resumed",
            resume_input_id: Some(legacy_resume_input.clone()),
            provider_session_resolved_account: None,
        };

        db.bind_invocation_provider_session_start(id, &binding)
            .unwrap();
        db.record_legacy_resume_input_session_id(id, &legacy_resume_input)
            .unwrap();
        db.bind_invocation_provider_session_start(id, &binding)
            .unwrap();

        let row: (Option<String>, Option<String>, Option<String>) = db
            .conn
            .query_row(
                "SELECT session_id, provider_session_id, resume_input_id
                 FROM invocations WHERE id = ?1",
                sqlite::params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(row.0.as_deref(), Some(legacy_resume_input.as_str()));
        assert_eq!(row.1.as_deref(), Some(provider_session_id.as_str()));
        assert_eq!(row.2.as_deref(), Some(legacy_resume_input.as_str()));
    }

    #[test]
    fn start_invocation_rejects_duplicate_uuid() {
        let db = test_db();
        let start = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "test-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        };

        db.start_invocation(&start).unwrap();
        let err = db.start_invocation(&start).unwrap_err();
        assert!(err.contains("invocation"));
    }

    #[test]
    fn start_invocation_accepts_parent_rowid() {
        let db = test_db();
        let parent = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "test-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        };
        let parent_id = db.start_invocation(&parent).unwrap();

        let child = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "test-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: Some(parent_id),
        };
        db.start_invocation(&child).unwrap();

        let row = db
            .get_invocation_by_uuid(&child.invocation_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(row.parent_invocation_id, Some(parent_id));
    }

    // RISK: finalize_invocation could fail to persist caller-provided terminal_reason separately from error_category (proposal §test-intent "terminal-reason absence characterization", assumption A5)
    // LEVEL: unit
    // SOURCE: contracts/nes-250-contract.md § Schema § StateDb::finalize_invocation
    #[test]
    fn finalize_invocation_sets_terminal_fields() {
        let db = test_db();
        let start = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "test-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        };
        let id = db.start_invocation(&start).unwrap();

        db.finalize_invocation(id, false, 7, None, Some("exit_nonzero"))
            .unwrap();

        let row = db
            .get_invocation_by_uuid(&start.invocation_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(row.status, InvocationStatus::Failed);
        assert_eq!(row.success, Some(false));
        assert_eq!(row.exit_code, Some(7));
        assert_eq!(row.error_category, None);
        assert_eq!(row.terminal_reason.as_deref(), Some("exit_nonzero"));
        assert!(row.finished_at.is_some());
    }

    #[test]
    fn finalize_invocation_updates_provider_aggregate_stats() {
        let db = test_db();
        let failed = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "test-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        };
        let succeeded = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "test-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        };

        let failed_id = db.start_invocation(&failed).unwrap();
        db.finalize_invocation(
            failed_id,
            false,
            1,
            Some("rate_limit"),
            Some("429 Too Many Requests"),
        )
        .unwrap();
        let succeeded_id = db.start_invocation(&succeeded).unwrap();
        db.finalize_invocation(succeeded_id, true, 0, None, None)
            .unwrap();

        let provider = db
            .get_provider("test-model", "fixture-provider")
            .unwrap()
            .unwrap();
        assert_eq!(provider.invocation_count, 2);
        assert_eq!(provider.error_count, 1);
        assert_eq!(
            provider.last_error.as_deref(),
            Some("429 Too Many Requests")
        );
        assert!(provider.last_invoked_at.is_some());
    }

    // Risk: Null-provider legacy rows must not synthesize aggregate identity | level: unit
    // Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track; ~/projects/agent-runner/planning/trunk/research/10-routing-claude-skipped-contract.md §5 finalize_invocation
    #[test]
    fn finalize_invocation_skips_provider_aggregate_for_null_provider_name() {
        let db = test_db();

        let mut ids = Vec::new();
        for provider_index in [0, 1] {
            db.conn
                .execute(
                    "INSERT INTO invocations (
                        invocation_uuid, model_name, provider_name, provider_index,
                        status, created_at
                     ) VALUES (?1, 'legacy-model', NULL, ?2, 'running', ?3)",
                    sqlite::params![
                        Uuid::new_v4().to_string(),
                        provider_index,
                        Utc::now().to_rfc3339(),
                    ],
                )
                .unwrap();
            ids.push(db.conn.last_insert_rowid());
        }

        db.finalize_invocation(ids[0], true, 0, None, None).unwrap();
        db.finalize_invocation(ids[1], false, 1, Some("rate_limit"), Some("429"))
            .unwrap();

        let provider_rows: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM providers WHERE model_name = 'legacy-model'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(provider_rows, 0);
    }

    #[test]
    fn finalize_invocation_errors_for_missing_row() {
        let db = test_db();
        let err = db
            .finalize_invocation(99, false, 1, Some("rate_limit"), None)
            .unwrap_err();
        assert!(err.contains("99"));
    }

    #[test]
    fn finalize_invocation_errors_when_called_twice() {
        let db = test_db();
        let start = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "test-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        };
        let id = db.start_invocation(&start).unwrap();
        db.finalize_invocation(id, true, 0, None, Some("exit_zero"))
            .unwrap();

        let err = db
            .finalize_invocation(
                id,
                false,
                -1,
                None,
                Some("supervisor_observed_unknown_exit"),
            )
            .unwrap_err();
        assert!(err.contains("already"));

        let row = db
            .get_invocation_by_uuid(&start.invocation_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(row.status, InvocationStatus::Succeeded);
        assert_eq!(row.success, Some(true));
        assert_eq!(row.exit_code, Some(0));
        assert_eq!(row.terminal_reason.as_deref(), Some("exit_zero"));
    }

    #[test]
    fn update_session_capture_persists_verified_session_id_and_method() {
        let db = test_db();
        let start = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "test-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        };
        let id = db.start_invocation(&start).unwrap();

        db.update_session_capture(
            id,
            Some("5169694d-de0f-40d1-890c-6e28e55bab27"),
            "forced_flag_verified",
        )
        .unwrap();

        let row = db
            .get_invocation_by_uuid(&start.invocation_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(
            row.session_id.as_deref(),
            Some("5169694d-de0f-40d1-890c-6e28e55bab27")
        );
        assert_eq!(
            row.session_capture_method.as_deref(),
            Some("forced_flag_verified")
        );
    }

    /// Per V10 (failures observable, never silent): a completed
    /// invocation with no capture configured must persist `"none"`
    /// explicitly so trace can distinguish "no capture attempted" from
    /// "still running" (NULL). Calling
    /// `update_session_capture(id, None, "none")` must write the
    /// column, NOT no-op.
    #[test]
    fn update_session_capture_none_none_persists_none_marker() {
        let db = test_db();
        let start = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "test-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        };
        let id = db.start_invocation(&start).unwrap();

        // Before any update: column is NULL (start_invocation doesn't set it).
        let before = db
            .get_invocation_by_uuid(&start.invocation_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(before.session_capture_method, None);

        db.update_session_capture(id, None, "none").unwrap();

        let after = db
            .get_invocation_by_uuid(&start.invocation_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(after.session_id, None);
        assert_eq!(
            after.session_capture_method.as_deref(),
            Some("none"),
            "completed-no-capture rows must record 'none' explicitly per V10"
        );
    }

    /// Per contract: update_session_capture is safe to call multiple
    /// times (idempotency for retries). The latest call wins.
    #[test]
    fn update_session_capture_safe_to_call_multiple_times() {
        let db = test_db();
        let start = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "test-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        };
        let id = db.start_invocation(&start).unwrap();

        db.update_session_capture(id, Some("first"), "forced_flag_verified")
            .unwrap();
        db.update_session_capture(id, Some("second"), "stdout_json_event")
            .unwrap();
        db.update_session_capture(id, Some("third"), "failed")
            .unwrap();

        let row = db
            .get_invocation_by_uuid(&start.invocation_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(row.session_id.as_deref(), Some("third"));
        assert_eq!(row.session_capture_method.as_deref(), Some("failed"));
    }

    /// "Leaves others alone" — update_session_capture must NOT clobber
    /// fields outside session_id/session_capture_method (e.g.
    /// invocation_uuid, model_name, status).
    #[test]
    fn update_session_capture_leaves_other_columns_alone() {
        let db = test_db();
        let start = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "specific-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 7,
            parent_invocation_id: None,
        };
        let id = db.start_invocation(&start).unwrap();
        let before = db
            .get_invocation_by_uuid(&start.invocation_uuid)
            .unwrap()
            .unwrap();

        db.update_session_capture(id, Some("sid"), "forced_flag_verified")
            .unwrap();

        let after = db
            .get_invocation_by_uuid(&start.invocation_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(after.invocation_uuid, before.invocation_uuid);
        assert_eq!(after.model_name, before.model_name);
        assert_eq!(after.provider_index, before.provider_index);
        assert_eq!(after.status, before.status);
        assert_eq!(after.created_at, before.created_at);
    }

    #[test]
    fn update_session_capture_dual_id_semantics_for_non_resumed_and_resumed_rows() {
        let db = test_db();
        let non_resumed = seed_running_invocation(&db);
        let resumed = seed_running_invocation(&db);
        db.conn
            .execute(
                "UPDATE invocations
                 SET provider_session_id = 'active-provider-session'
                 WHERE id = ?1",
                sqlite::params![resumed],
            )
            .unwrap();

        db.update_session_capture(non_resumed, Some("new-provider-session"), "stdout")
            .unwrap();
        db.update_session_capture(resumed, Some("attempted-resume-id"), "resumed")
            .unwrap();

        let non_resumed_row: (Option<String>, Option<String>, Option<String>) = db
            .conn
            .query_row(
                "SELECT provider_session_id, resume_input_id, provider_session_capture_method
                 FROM invocations WHERE id = ?1",
                sqlite::params![non_resumed],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(non_resumed_row.0.as_deref(), Some("new-provider-session"));
        assert_eq!(non_resumed_row.1, None);
        assert_eq!(non_resumed_row.2.as_deref(), Some("stdout"));

        let resumed_row: (Option<String>, Option<String>, Option<String>) = db
            .conn
            .query_row(
                "SELECT provider_session_id, resume_input_id, provider_session_capture_method
                 FROM invocations WHERE id = ?1",
                sqlite::params![resumed],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(resumed_row.0.as_deref(), Some("active-provider-session"));
        assert_eq!(resumed_row.1.as_deref(), Some("attempted-resume-id"));
        assert_eq!(resumed_row.2, None);
        assert_eq!(invocation_count(&db), 2);
    }

    #[test]
    fn record_legacy_resume_input_session_id_updates_only_resumed_row() {
        let db = test_db();
        let resumed = seed_running_invocation(&db);
        let non_resumed = seed_running_invocation(&db);
        db.update_session_capture(resumed, Some("active-session"), "resumed")
            .unwrap();
        db.update_session_capture(non_resumed, Some("provider-session"), "stdout")
            .unwrap();

        db.record_legacy_resume_input_session_id(resumed, "attempted-resume")
            .unwrap();
        db.record_legacy_resume_input_session_id(non_resumed, "must-not-apply")
            .unwrap();

        let resumed_session: Option<String> = db
            .conn
            .query_row(
                "SELECT session_id FROM invocations WHERE id = ?1",
                sqlite::params![resumed],
                |row| row.get(0),
            )
            .unwrap();
        let non_resumed_session: Option<String> = db
            .conn
            .query_row(
                "SELECT session_id FROM invocations WHERE id = ?1",
                sqlite::params![non_resumed],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(resumed_session.as_deref(), Some("attempted-resume"));
        assert_eq!(non_resumed_session.as_deref(), Some("provider-session"));
        assert_eq!(invocation_count(&db), 2);
    }

    #[test]
    fn recent_errors() {
        let db = test_db();
        let failed = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "m".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        };
        let succeeded = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "m".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        };
        let failed_id = db.start_invocation(&failed).unwrap();
        db.finalize_invocation(failed_id, false, 1, None, None)
            .unwrap();
        let succeeded_id = db.start_invocation(&succeeded).unwrap();
        db.finalize_invocation(succeeded_id, true, 0, None, None)
            .unwrap();

        let count = db.recent_error_count("m", "fixture-provider", 60).unwrap();
        assert_eq!(count, 1);
    }

    // Risk: recent_error_count identity drift | level: unit
    // Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track
    #[test]
    fn recent_error_count_uses_provider_name_not_reused_index_history() {
        let db = test_db();

        for _ in 0..3 {
            record_provider_invocation(
                &db,
                "routing-model",
                "claude-old",
                0,
                false,
                Some("rate_limit"),
                None,
            );
        }

        assert_eq!(
            db.recent_error_count("routing-model", "claude", 60)
                .unwrap(),
            0,
            "current provider name must not inherit recent failures from a prior occupant of index 0"
        );
        assert_eq!(
            db.recent_error_count("routing-model", "claude-old", 60)
                .unwrap(),
            3,
            "the failed provider name still owns its own recent failures"
        );
    }

    // Risk: Aggregate writer/reader round-trip after provider reorder | level: unit
    // Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track
    #[test]
    fn provider_aggregate_round_trip_follows_name_after_reorder() {
        let db = test_db();
        record_provider_invocation(&db, "routing-model", "claude2", 0, true, None, None);

        let claude2 = db
            .get_provider("routing-model", "claude2")
            .unwrap()
            .expect("claude2 aggregate should exist by provider name");
        assert_eq!(claude2.provider_name, "claude2");
        assert_eq!(claude2.invocation_count, 1);
        assert!(
            db.get_provider("routing-model", "claude")
                .unwrap()
                .is_none(),
            "claude must not inherit claude2 history after taking index 0"
        );

        assert!(
            db.get_provider("routing-model", "claude")
                .unwrap()
                .is_none(),
            "fallback scoring should treat the current claude provider as unused"
        );
    }

    // Risk: Aggregate writer/reader round-trip after provider rename | level: unit
    // Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track
    #[test]
    fn provider_aggregate_round_trip_does_not_inherit_renamed_provider_history() {
        let db = test_db();
        record_provider_invocation(&db, "routing-model", "claude-old", 0, true, None, None);

        let old = db
            .get_provider("routing-model", "claude-old")
            .unwrap()
            .expect("old provider name should retain its aggregate");
        assert_eq!(old.provider_name, "claude-old");
        assert_eq!(old.invocation_count, 1);
        assert!(
            db.get_provider("routing-model", "claude")
                .unwrap()
                .is_none(),
            "renamed provider claude starts without aggregate history unless invocations use that name"
        );
    }

    #[test]
    fn ingest_session_turns_batch_persists_parent_and_sidechain_columns() {
        let db = test_db();

        let inserted = db
            .ingest_session_turns_batch(
                "fixture-provider",
                &[SessionTurnIngest {
                    session_id: "session-a".to_string(),
                    turn_id: "child-turn".to_string(),
                    timestamp: ts("2026-04-17T08:00:01Z"),
                    role: "assistant".to_string(),
                    parent_turn_id: Some("root-turn".to_string()),
                    is_sidechain: true,
                    is_compaction_boundary: false,
                    body: None,
                }],
            )
            .unwrap();

        assert_eq!(inserted, 1);
        let row: (Option<String>, i64) = db
            .conn
            .query_row(
                "SELECT parent_turn_id, is_sidechain
                 FROM session_turns
                 WHERE provider_name = ?1 AND session_id = ?2 AND turn_id = ?3",
                sqlite::params!["fixture-provider", "session-a", "child-turn"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(row.0.as_deref(), Some("root-turn"));
        assert_eq!(row.1, 1);
    }

    #[test]
    fn count_session_turns_reports_total_assistant_and_sidechain_counts() {
        let db = test_db();

        db.ingest_session_turns_batch(
            "fixture-provider",
            &[
                SessionTurnIngest {
                    session_id: "session-a".to_string(),
                    turn_id: "root".to_string(),
                    timestamp: ts("2026-04-17T08:00:00Z"),
                    role: "user".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: false,
                    body: None,
                },
                SessionTurnIngest {
                    session_id: "session-a".to_string(),
                    turn_id: "assistant-main".to_string(),
                    timestamp: ts("2026-04-17T08:00:01Z"),
                    role: "assistant".to_string(),
                    parent_turn_id: Some("root".to_string()),
                    is_sidechain: false,
                    is_compaction_boundary: false,
                    body: None,
                },
                SessionTurnIngest {
                    session_id: "session-a".to_string(),
                    turn_id: "assistant-side".to_string(),
                    timestamp: ts("2026-04-17T08:00:02Z"),
                    role: "assistant".to_string(),
                    parent_turn_id: Some("assistant-main".to_string()),
                    is_sidechain: true,
                    is_compaction_boundary: false,
                    body: None,
                },
                SessionTurnIngest {
                    session_id: "session-b".to_string(),
                    turn_id: "other-session".to_string(),
                    timestamp: ts("2026-04-17T08:00:03Z"),
                    role: "assistant".to_string(),
                    parent_turn_id: None,
                    is_sidechain: true,
                    is_compaction_boundary: false,
                    body: None,
                },
            ],
        )
        .unwrap();
        db.ingest_session_turns_batch(
            "other-provider",
            &[SessionTurnIngest {
                session_id: "session-a".to_string(),
                turn_id: "other-provider-turn".to_string(),
                timestamp: ts("2026-04-17T08:00:04Z"),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: true,
                is_compaction_boundary: false,
                body: None,
            }],
        )
        .unwrap();

        let counts: SessionTurnCounts = db
            .count_session_turns("fixture-provider", "session-a")
            .unwrap();

        assert_eq!(counts.total, 3);
        assert_eq!(counts.assistant, 2);
        assert_eq!(counts.sidechain, 1);
    }

    #[test]
    fn has_session_user_text_turn_requires_exact_user_body_match() {
        let db = test_db();
        let expected = "[OULIPOLY NOTIFICATIONS]\nhandle: h-exact\n";

        db.ingest_session_turns_batch(
            "fixture-provider",
            &[
                SessionTurnIngest {
                    session_id: "session-a".to_string(),
                    turn_id: "user-exact".to_string(),
                    timestamp: ts("2026-04-17T08:00:00Z"),
                    role: "user".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: false,
                    body: Some(
                        serde_json::json!([{ "type": "text", "text": expected }]).to_string(),
                    ),
                },
                SessionTurnIngest {
                    session_id: "session-a".to_string(),
                    turn_id: "assistant-same-text".to_string(),
                    timestamp: ts("2026-04-17T08:00:01Z"),
                    role: "assistant".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: false,
                    body: Some(
                        serde_json::json!([{ "type": "text", "text": "assistant text" }])
                            .to_string(),
                    ),
                },
            ],
        )
        .unwrap();

        let extra_text_body = serde_json::json!([
            { "type": "text", "text": expected },
            { "type": "text", "text": "extra" }
        ])
        .to_string();

        assert!(
            db.has_session_user_text_turn("fixture-provider", "session-a", expected)
                .unwrap()
        );
        assert!(
            !db.has_session_user_text_turn("fixture-provider", "session-a", "handle: h")
                .unwrap(),
            "partial text must not confirm delivery"
        );
        assert!(
            !StateDb::session_turn_body_has_exact_text(&extra_text_body, expected),
            "multi-chunk turns must match the submitted payload exactly"
        );
        assert!(StateDb::session_turn_body_has_exact_text(
            &extra_text_body,
            &format!("{expected}extra")
        ));
        assert!(
            !db.has_session_user_text_turn("other-provider", "session-a", expected)
                .unwrap(),
            "provider identity must match"
        );
    }

    #[test]
    fn has_session_user_turn_containing_matches_user_body_substring() {
        let db = test_db();
        let nonce = "11111111-2222-4333-8444-555555555555";

        db.ingest_session_turns_batch(
            "fixture-provider",
            &[
                SessionTurnIngest {
                    session_id: "session-a".to_string(),
                    turn_id: "user-quoted-delivery".to_string(),
                    timestamp: ts("2026-04-17T08:00:00Z"),
                    role: "user".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: false,
                    body: Some(
                        serde_json::json!([
                            {
                                "type": "text",
                                "text": format!(
                                    "\"[OULIPOLY NOTIFICATIONS]\n[OULIPOLY-DELIVERY {nonce}]\nbody\""
                                )
                            }
                        ])
                        .to_string(),
                    ),
                },
                SessionTurnIngest {
                    session_id: "session-a".to_string(),
                    turn_id: "assistant-same-nonce".to_string(),
                    timestamp: ts("2026-04-17T08:00:01Z"),
                    role: "assistant".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: false,
                    body: Some(
                        serde_json::json!([
                            {
                                "type": "text",
                                "text": format!("assistant [OULIPOLY-DELIVERY {nonce}]")
                            }
                        ])
                        .to_string(),
                    ),
                },
            ],
        )
        .unwrap();

        assert!(
            db.has_session_user_turn_containing("fixture-provider", "session-a", nonce)
                .unwrap(),
            "the delivery nonce should match inside a non-exact quote-wrapped user body"
        );
        assert!(
            !db.has_session_user_turn_containing("fixture-provider", "session-a", "missing-nonce")
                .unwrap(),
            "missing nonce must not confirm delivery"
        );
        assert!(
            !db.has_session_user_turn_containing("fixture-provider", "session-a", "")
                .unwrap(),
            "empty needles must not match every body"
        );
        assert!(
            !db.has_session_user_turn_containing("other-provider", "session-a", nonce)
                .unwrap(),
            "provider identity must match"
        );
        assert!(
            !db.has_session_user_turn_containing("fixture-provider", "other-session", nonce)
                .unwrap(),
            "session identity must match"
        );
    }

    #[test]
    fn composite_invocation_id_formats_and_round_trips() {
        let composite = CompositeInvocationId {
            source: "fixture-provider".to_string(),
            id: "7ad2916c-38dd-49e6-a1f7-3ef22766ff70".to_string(),
        };
        let line = composite.stderr_line();
        assert_eq!(
            line,
            r#"OULIPOLY_INVOCATION={"source":"fixture-provider","id":"7ad2916c-38dd-49e6-a1f7-3ef22766ff70"}"#
        );

        let parsed = CompositeInvocationId::parse_env_value(
            line.strip_prefix("OULIPOLY_INVOCATION=").unwrap(),
        )
        .unwrap();
        assert_eq!(parsed, composite);
    }

    #[test]
    fn composite_invocation_id_parses_shell_mangled_env_values() {
        let parsed = CompositeInvocationId::parse_env_value(
            "{source:fixture-provider,id:7ad2916c-38dd-49e6-a1f7-3ef22766ff70}",
        )
        .unwrap();

        assert_eq!(
            parsed,
            CompositeInvocationId {
                source: "fixture-provider".to_string(),
                id: "7ad2916c-38dd-49e6-a1f7-3ef22766ff70".to_string(),
            }
        );
    }

    #[test]
    fn composite_invocation_id_parses_quoted_shell_mangled_env_values() {
        let parsed = CompositeInvocationId::parse_env_value(
            r#"{source:"fixture-provider",id:"7ad2916c-38dd-49e6-a1f7-3ef22766ff70"}"#,
        )
        .unwrap();

        assert_eq!(
            parsed,
            CompositeInvocationId {
                source: "fixture-provider".to_string(),
                id: "7ad2916c-38dd-49e6-a1f7-3ef22766ff70".to_string(),
            }
        );
    }

    #[test]
    fn composite_invocation_id_rejects_malformed_env_values() {
        for raw in [
            "not-json",
            r#"{"source":"fixture-provider"}"#,
            r#"{"source":"fixture-provider","id":"not-a-uuid"}"#,
            r#"{"source":"fixture-provider","id":"7ad2916c-38dd-49e6-a1f7-3ef22766ff70","extra":true}"#,
        ] {
            assert!(
                CompositeInvocationId::parse_env_value(raw).is_err(),
                "{raw}"
            );
        }
    }

    #[test]
    fn invocation_status_round_trips_through_strings() {
        for status in [
            InvocationStatus::Running,
            InvocationStatus::Succeeded,
            InvocationStatus::Failed,
            InvocationStatus::Legacy,
        ] {
            // Inherent contracted API: Option<Self>.
            assert_eq!(InvocationStatus::from_str(status.as_str()), Some(status));
            // FromStr trait surface: Result<Self, _>. Both must work.
            assert_eq!(
                status.as_str().parse::<InvocationStatus>().ok(),
                Some(status)
            );
        }
        assert_eq!(InvocationStatus::from_str("unknown"), None);
        assert!("unknown".parse::<InvocationStatus>().is_err());
    }

    #[test]
    fn get_invocation_by_uuid_returns_matching_and_missing_rows() {
        with_models_config(
            "legacy-model",
            r#"
[[providers]]
name = "fixture-provider"
"#,
            || {
                let db = test_db();
                let start = InvocationStart {
                    invocation_uuid: Uuid::new_v4().to_string(),
                    model_name: "legacy-model".to_string(),
                    provider_name: "fixture-provider".to_string(),
                    provider_index: 0,
                    parent_invocation_id: None,
                };
                db.start_invocation(&start).unwrap();
                let running = db
                    .get_invocation_by_uuid(&start.invocation_uuid)
                    .unwrap()
                    .unwrap();
                assert_eq!(running.invocation_uuid, start.invocation_uuid);

                let dir = legacy_invocations_db(&[(
                    "missing-model",
                    0,
                    0,
                    7,
                    None,
                    "2026-04-17T08:05:00Z",
                )]);
                let migrated = StateDb::open(&dir.path().join("state.db")).unwrap();
                let legacy_uuid: String = migrated
                    .conn
                    .query_row(
                        "SELECT invocation_uuid FROM invocations LIMIT 1",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                let legacy = migrated
                    .get_invocation_by_uuid(&legacy_uuid)
                    .unwrap()
                    .unwrap();
                assert_eq!(legacy.status, InvocationStatus::Legacy);
                assert!(
                    migrated
                        .get_invocation_by_uuid("00000000-0000-0000-0000-000000000000")
                        .unwrap()
                        .is_none()
                );
            },
        );
    }

    #[test]
    fn list_invocation_children_returns_empty_for_unknown_parent() {
        let db = test_db();

        let children = db.list_invocation_children(999).unwrap();

        assert!(children.is_empty());
    }

    #[test]
    fn list_invocation_children_orders_by_created_at_then_row_id() {
        let db = test_db();
        let root_id = insert_invocation_fixture(
            &db,
            "10000000-0000-0000-0000-000000000000",
            None,
            "2026-04-17T08:00:00Z",
        );
        insert_invocation_fixture(
            &db,
            "30000000-0000-0000-0000-000000000000",
            Some(root_id),
            "2026-04-17T08:02:00Z",
        );
        insert_invocation_fixture(
            &db,
            "20000000-0000-0000-0000-000000000000",
            Some(root_id),
            "2026-04-17T08:01:00Z",
        );
        insert_invocation_fixture(
            &db,
            "40000000-0000-0000-0000-000000000000",
            Some(root_id),
            "2026-04-17T08:01:00Z",
        );

        let children = db.list_invocation_children(root_id).unwrap();
        let ordered: Vec<&str> = children
            .iter()
            .map(|record| record.invocation_uuid.as_str())
            .collect();

        assert_eq!(
            ordered,
            vec![
                "20000000-0000-0000-0000-000000000000",
                "40000000-0000-0000-0000-000000000000",
                "30000000-0000-0000-0000-000000000000",
            ]
        );
    }

    #[test]
    fn list_invocation_children_returns_only_direct_children() {
        let db = test_db();
        let root_id = insert_invocation_fixture(
            &db,
            "50000000-0000-0000-0000-000000000000",
            None,
            "2026-04-17T08:00:00Z",
        );
        let child_id = insert_invocation_fixture(
            &db,
            "60000000-0000-0000-0000-000000000000",
            Some(root_id),
            "2026-04-17T08:01:00Z",
        );
        insert_invocation_fixture(
            &db,
            "70000000-0000-0000-0000-000000000000",
            Some(child_id),
            "2026-04-17T08:02:00Z",
        );
        insert_invocation_fixture(
            &db,
            "80000000-0000-0000-0000-000000000000",
            Some(root_id),
            "2026-04-17T08:03:00Z",
        );

        let children = db.list_invocation_children(root_id).unwrap();
        let uuids: Vec<&str> = children
            .iter()
            .map(|record| record.invocation_uuid.as_str())
            .collect();

        assert_eq!(
            uuids,
            vec![
                "60000000-0000-0000-0000-000000000000",
                "80000000-0000-0000-0000-000000000000",
            ]
        );
    }

    #[test]
    fn missing_provider_returns_none() {
        let db = test_db();
        assert!(db.get_provider("nonexistent", "missing").unwrap().is_none());
    }

    // --- CLI Provider & Account tests ---

    fn sample_provider() -> CliProviderRecord {
        CliProviderRecord {
            cli_name: "claude".to_string(),
            display_name: "Anthropic".to_string(),
            installed: true,
            version: Some("1.2.3".to_string()),
            config_dir: Some("/home/user/.claude".to_string()),
            last_synced: None,
        }
    }

    #[test]
    fn upsert_and_list_cli_providers() {
        let db = test_db();
        db.upsert_cli_provider(&sample_provider()).unwrap();

        let mut p2 = sample_provider();
        p2.cli_name = "codex".to_string();
        p2.display_name = "OpenAI".to_string();
        db.upsert_cli_provider(&p2).unwrap();

        let providers = db.list_cli_providers().unwrap();
        assert_eq!(providers.len(), 2);
        assert_eq!(providers[0].cli_name, "claude");
        assert_eq!(providers[1].cli_name, "codex");
    }

    #[test]
    fn upsert_cli_provider_updates_existing() {
        let db = test_db();
        db.upsert_cli_provider(&sample_provider()).unwrap();

        let mut updated = sample_provider();
        updated.version = Some("2.0.0".to_string());
        updated.last_synced = Some("2026-02-19T00:00:00Z".to_string());
        db.upsert_cli_provider(&updated).unwrap();

        let p = db.get_cli_provider("claude").unwrap().unwrap();
        assert_eq!(p.version.as_deref(), Some("2.0.0"));
        assert!(p.last_synced.is_some());
    }

    #[test]
    fn get_cli_provider_missing() {
        let db = test_db();
        assert!(db.get_cli_provider("nonexistent").unwrap().is_none());
    }

    #[test]
    fn insert_and_list_accounts() {
        let db = test_db();
        db.upsert_cli_provider(&sample_provider()).unwrap();

        let acct = AccountRecord {
            id: "work".to_string(),
            provider: "claude".to_string(),
            profile_name: "work-profile".to_string(),
            auth_method: AuthMethod::OAuth,
            auth_status: AuthStatus::Valid,
            created_at: "2026-02-19T00:00:00Z".to_string(),
        };
        db.insert_account(&acct).unwrap();

        let acct2 = AccountRecord {
            id: "personal".to_string(),
            provider: "claude".to_string(),
            profile_name: "personal-profile".to_string(),
            auth_method: AuthMethod::ApiKey {
                env_var: "ANTHROPIC_API_KEY".to_string(),
                config_path: None,
            },
            auth_status: AuthStatus::Unknown,
            created_at: "2026-02-19T00:00:00Z".to_string(),
        };
        db.insert_account(&acct2).unwrap();

        // List all
        let all = db.list_accounts(None).unwrap();
        assert_eq!(all.len(), 2);

        // List by provider
        let claude_accounts = db.list_accounts(Some("claude")).unwrap();
        assert_eq!(claude_accounts.len(), 2);
        assert_eq!(claude_accounts[0].id, "personal");
        assert_eq!(claude_accounts[1].id, "work");

        let empty = db.list_accounts(Some("codex")).unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn delete_account() {
        let db = test_db();
        db.upsert_cli_provider(&sample_provider()).unwrap();

        let acct = AccountRecord {
            id: "temp".to_string(),
            provider: "claude".to_string(),
            profile_name: "temp-profile".to_string(),
            auth_method: AuthMethod::ConfigFile {
                path: "~/.claude/config".to_string(),
            },
            auth_status: AuthStatus::NoAuth,
            created_at: "2026-02-19T00:00:00Z".to_string(),
        };
        db.insert_account(&acct).unwrap();
        assert_eq!(db.list_accounts(None).unwrap().len(), 1);

        let deleted = db.delete_account("temp", "claude").unwrap();
        assert!(deleted);
        assert!(db.list_accounts(None).unwrap().is_empty());

        // Deleting again returns false
        let deleted_again = db.delete_account("temp", "claude").unwrap();
        assert!(!deleted_again);
    }

    #[test]
    fn auth_method_roundtrip() {
        let methods = vec![
            AuthMethod::OAuth,
            AuthMethod::ApiKey {
                env_var: "MY_KEY".to_string(),
                config_path: Some("/path/to/key".to_string()),
            },
            AuthMethod::ConfigFile {
                path: "~/.config/file".to_string(),
            },
        ];
        for method in methods {
            let serialized = method.to_db_string();
            let deserialized = AuthMethod::from_db_string(&serialized);
            assert_eq!(method, deserialized);
        }
    }

    // --- Discovered model & parameter tests ---

    fn sample_discovered_model(name: &str, provider: &str) -> DiscoveredModel {
        DiscoveredModel {
            canonical_name: name.to_string(),
            provider: provider.to_string(),
            discovered_at: "2026-02-19T00:00:00Z".to_string(),
            cli_version: "1.0.0".to_string(),
        }
    }

    #[test]
    fn upsert_and_list_discovered_models() {
        let db = test_db();
        db.upsert_discovered_model(&sample_discovered_model("claude-opus-4", "claude"))
            .unwrap();
        db.upsert_discovered_model(&sample_discovered_model("claude-sonnet-4", "claude"))
            .unwrap();
        db.upsert_discovered_model(&sample_discovered_model("gpt-5.3", "codex"))
            .unwrap();

        // List all
        let all = db.list_discovered_models(None).unwrap();
        assert_eq!(all.len(), 3);

        // List by provider
        let claude_models = db.list_discovered_models(Some("claude")).unwrap();
        assert_eq!(claude_models.len(), 2);
        assert_eq!(claude_models[0].canonical_name, "claude-opus-4");
        assert_eq!(claude_models[1].canonical_name, "claude-sonnet-4");

        let codex_models = db.list_discovered_models(Some("codex")).unwrap();
        assert_eq!(codex_models.len(), 1);
        assert_eq!(codex_models[0].canonical_name, "gpt-5.3");

        let empty = db.list_discovered_models(Some("gemini")).unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn upsert_discovered_model_updates_existing() {
        let db = test_db();
        db.upsert_discovered_model(&sample_discovered_model("claude-opus-4", "claude"))
            .unwrap();

        let mut updated = sample_discovered_model("claude-opus-4", "claude");
        updated.cli_version = "2.0.0".to_string();
        updated.discovered_at = "2026-02-20T00:00:00Z".to_string();
        db.upsert_discovered_model(&updated).unwrap();

        let models = db.list_discovered_models(Some("claude")).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].cli_version, "2.0.0");
        assert_eq!(models[0].discovered_at, "2026-02-20T00:00:00Z");
    }

    #[test]
    fn delete_stale_models() {
        let db = test_db();
        db.upsert_discovered_model(&sample_discovered_model("model-a", "claude"))
            .unwrap();
        db.upsert_discovered_model(&sample_discovered_model("model-b", "claude"))
            .unwrap();

        let mut newer = sample_discovered_model("model-c", "claude");
        newer.cli_version = "2.0.0".to_string();
        db.upsert_discovered_model(&newer).unwrap();

        // Delete models with cli_version != "2.0.0"
        let deleted = db.delete_stale_models("claude", "2.0.0").unwrap();
        assert_eq!(deleted, 2);

        let remaining = db.list_discovered_models(Some("claude")).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].canonical_name, "model-c");
    }

    #[test]
    fn delete_stale_models_different_provider() {
        let db = test_db();
        db.upsert_discovered_model(&sample_discovered_model("model-a", "claude"))
            .unwrap();
        db.upsert_discovered_model(&sample_discovered_model("model-b", "codex"))
            .unwrap();

        // Only delete stale models for "claude", "codex" should be untouched
        let deleted = db.delete_stale_models("claude", "2.0.0").unwrap();
        assert_eq!(deleted, 1);

        let codex = db.list_discovered_models(Some("codex")).unwrap();
        assert_eq!(codex.len(), 1);
    }

    #[test]
    fn upsert_and_list_model_parameters() {
        let db = test_db();

        let temp_param = ModelParameter {
            name: "temperature".to_string(),
            display_name: "Temperature".to_string(),
            param_type: ParamType::Number {
                min: Some(0.0),
                max: Some(2.0),
            },
            description: "Controls randomness".to_string(),
            cli_mapping: CliMapping {
                flag: "--temperature".to_string(),
                value_template: "{value}".to_string(),
            },
        };

        let model_param = ModelParameter {
            name: "model".to_string(),
            display_name: "Model".to_string(),
            param_type: ParamType::Enum {
                options: vec!["opus-4".to_string(), "sonnet-4".to_string()],
            },
            description: "Model variant to use".to_string(),
            cli_mapping: CliMapping {
                flag: "-m".to_string(),
                value_template: "{value}".to_string(),
            },
        };

        db.upsert_model_parameter("claude-opus-4", "claude", &temp_param)
            .unwrap();
        db.upsert_model_parameter("claude-opus-4", "claude", &model_param)
            .unwrap();

        let params = db.list_model_parameters("claude-opus-4", "claude").unwrap();
        assert_eq!(params.len(), 2);
        // Ordered by name
        assert_eq!(params[0].name, "model");
        assert_eq!(params[1].name, "temperature");

        // Verify ParamType round-trip
        match &params[0].param_type {
            ParamType::Enum { options } => {
                assert_eq!(options.len(), 2);
                assert_eq!(options[0], "opus-4");
            }
            other => panic!("Expected Enum, got {:?}", other),
        }

        match &params[1].param_type {
            ParamType::Number { min, max } => {
                assert_eq!(*min, Some(0.0));
                assert_eq!(*max, Some(2.0));
            }
            other => panic!("Expected Number, got {:?}", other),
        }

        // Verify CliMapping round-trip
        assert_eq!(params[1].cli_mapping.flag, "--temperature");
        assert_eq!(params[1].cli_mapping.value_template, "{value}");
    }

    #[test]
    fn upsert_model_parameter_updates_existing() {
        let db = test_db();

        let param = ModelParameter {
            name: "verbose".to_string(),
            display_name: "Verbose".to_string(),
            param_type: ParamType::Boolean,
            description: "Enable verbose output".to_string(),
            cli_mapping: CliMapping {
                flag: "--verbose".to_string(),
                value_template: "".to_string(),
            },
        };
        db.upsert_model_parameter("gpt-5.3", "codex", &param)
            .unwrap();

        // Update description
        let mut updated = param.clone();
        updated.description = "Toggle verbose mode".to_string();
        db.upsert_model_parameter("gpt-5.3", "codex", &updated)
            .unwrap();

        let params = db.list_model_parameters("gpt-5.3", "codex").unwrap();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].description, "Toggle verbose mode");
    }

    #[test]
    fn list_model_parameters_empty() {
        let db = test_db();
        let params = db
            .list_model_parameters("nonexistent", "nonexistent")
            .unwrap();
        assert!(params.is_empty());
    }

    #[test]
    fn param_type_string_variant() {
        let db = test_db();
        let param = ModelParameter {
            name: "system_prompt".to_string(),
            display_name: "System Prompt".to_string(),
            param_type: ParamType::String,
            description: "The system prompt".to_string(),
            cli_mapping: CliMapping {
                flag: "--system".to_string(),
                value_template: "{value}".to_string(),
            },
        };
        db.upsert_model_parameter("m", "p", &param).unwrap();
        let params = db.list_model_parameters("m", "p").unwrap();
        assert_eq!(params[0].param_type, ParamType::String);
    }

    const SESSION_A: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
    const SESSION_B: &str = "8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22";
    const CHAIN_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const CHAIN_B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    const CHAIN_C: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";

    fn model_store_from_toml(
        fixtures: &[(&str, &str)],
    ) -> std::collections::HashMap<String, oulipoly_config::ModelConfig> {
        fixtures
            .iter()
            .map(|(name, body)| {
                (
                    (*name).to_string(),
                    oulipoly_config::ModelConfig::from_toml_with_name(name, body, None).unwrap(),
                )
            })
            .collect()
    }

    fn resolver_model_store() -> std::collections::HashMap<String, oulipoly_config::ModelConfig> {
        model_store_from_toml(&[
            (
                "claude-opus",
                r#"
[[providers]]
name = "claude"
interactive_args = ["launch"]

[[providers]]
name = "claude2"
interactive_args = ["launch"]
"#,
            ),
            (
                "claude-haiku",
                r#"
[[providers]]
name = "claude"
interactive_args = ["launch"]
"#,
            ),
        ])
    }

    fn seed_chain_row(db: &StateDb, chain_id: &str, model_name: &str, last_used_at: &str) {
        db.conn
            .execute(
                "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
                 VALUES (?1, ?2, ?2, ?3)",
                sqlite::params![chain_id, last_used_at, model_name],
            )
            .unwrap();
    }

    fn seed_segment_row(
        db: &StateDb,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
        started_at: &str,
        ended_at: Option<&str>,
        reason: &str,
    ) {
        db.conn
            .execute(
                "INSERT INTO session_chain_segments
                    (chain_id, provider_name, session_id, started_at, ended_at, transition_reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                sqlite::params![
                    chain_id,
                    provider_name,
                    session_id,
                    started_at,
                    ended_at,
                    reason
                ],
            )
            .unwrap();
    }

    pub(crate) fn seed_test_chain(
        db: &StateDb,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
        model_name: &str,
        last_used_at: &str,
    ) {
        seed_chain_row(db, chain_id, model_name, last_used_at);
        seed_segment_row(
            db,
            chain_id,
            provider_name,
            session_id,
            last_used_at,
            None,
            "initial",
        );
    }

    fn seed_invocation_for_session(
        db: &StateDb,
        model_name: &str,
        provider_name: &str,
        session_id: &str,
        created_at: &str,
    ) {
        let id = db
            .start_invocation(&InvocationStart {
                invocation_uuid: Uuid::new_v4().to_string(),
                model_name: model_name.to_string(),
                provider_name: provider_name.to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        db.update_session_capture(id, Some(session_id), "fixture")
            .unwrap();
        db.conn
            .execute(
                "UPDATE invocations SET created_at = ?1, finished_at = ?1 WHERE id = ?2",
                sqlite::params![created_at, id],
            )
            .unwrap();
    }

    fn pre_chain_db_with_turns(rows: &[(&str, &str, &str, &str, &str)]) -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let conn = sqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session_turns (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                provider_name TEXT NOT NULL,
                session_id TEXT NOT NULL,
                turn_id TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                role TEXT NOT NULL,
                source_file TEXT NOT NULL,
                ingested_at TEXT NOT NULL,
                UNIQUE (provider_name, session_id, turn_id)
            );
            CREATE TABLE invocations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                invocation_uuid TEXT NOT NULL UNIQUE,
                model_name TEXT NOT NULL,
                provider_name TEXT,
                provider_index INTEGER NOT NULL,
                parent_invocation_id INTEGER,
                status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'legacy')),
                success INTEGER,
                exit_code INTEGER,
                error_category TEXT,
                session_id TEXT,
                session_capture_method TEXT,
                resume_acceptance_status TEXT,
                resume_acceptance_evidence TEXT,
                created_at TEXT NOT NULL,
                finished_at TEXT
            );",
        )
        .unwrap();
        for (provider, session, turn, timestamp, role) in rows {
            conn.execute(
                "INSERT INTO session_turns
                    (provider_name, session_id, turn_id, timestamp, role, source_file, ingested_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, '', ?4)",
                sqlite::params![provider, session, turn, timestamp, role],
            )
            .unwrap();
        }
        mark_current_schema_version(&conn);
        dir
    }

    fn chain_count(db: &StateDb) -> i64 {
        db.conn
            .query_row("SELECT COUNT(*) FROM session_chains", [], |row| row.get(0))
            .unwrap()
    }

    fn segment_count(db: &StateDb) -> i64 {
        db.conn
            .query_row("SELECT COUNT(*) FROM session_chain_segments", [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    fn invocation_count(db: &StateDb) -> i64 {
        db.conn
            .query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
            .unwrap()
    }

    fn invocation_checksum(db: &StateDb) -> String {
        let dual_id_cols = StateDb::invocations_have_dual_id_columns(&db.conn).unwrap();
        let extra_cols = if dual_id_cols {
            " || '|' || COALESCE(session_capture_method, '') \
             || '|' || COALESCE(provider_session_id, '') \
             || '|' || COALESCE(resume_input_id, '') \
             || '|' || COALESCE(provider_session_capture_method, '')"
        } else {
            ""
        };
        let sql = format!(
            "SELECT COALESCE(group_concat(line, char(10)), '')
             FROM (
                 SELECT id || '|' || invocation_uuid || '|' || status || '|' ||
                        COALESCE(session_id, ''){extra_cols} || '|' ||
                        COALESCE(finished_at, '') AS line
                 FROM invocations
                 ORDER BY id
             )"
        );
        db.conn.query_row(&sql, [], |row| row.get(0)).unwrap()
    }

    // risk: Schema migration and backfill; level: particular-integration; source: proposal §11.1 Schema migration and backfill / A5.
    #[test]
    fn backfill_creates_one_chain_per_provider_session_pair() {
        let dir = pre_chain_db_with_turns(&[
            (
                "claude",
                SESSION_A,
                "turn-a1",
                "2026-04-17T08:00:00Z",
                "assistant",
            ),
            (
                "claude",
                SESSION_A,
                "turn-a2",
                "2026-04-17T08:00:01Z",
                "assistant",
            ),
            (
                "claude2",
                SESSION_B,
                "turn-b1",
                "2026-04-17T09:00:00Z",
                "assistant",
            ),
        ]);

        let db = StateDb::open(&dir.path().join("state.db")).unwrap();

        assert_eq!(chain_count(&db), 2);
        assert_eq!(segment_count(&db), 2);
        let imported: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM session_chain_segments WHERE transition_reason = 'imported' AND ended_at IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(imported, 2);
    }

    // risk: Schema migration and backfill; level: particular-integration; source: proposal §11.1 Schema migration and backfill / A5.
    #[test]
    fn backfill_idempotent_on_second_open() {
        let dir = pre_chain_db_with_turns(&[(
            "claude",
            SESSION_A,
            "turn-a1",
            "2026-04-17T08:00:00Z",
            "assistant",
        )]);
        let path = dir.path().join("state.db");

        let first = StateDb::open(&path).unwrap();
        let first_count = chain_count(&first);
        let first_invocation_checksum = invocation_checksum(&first);
        drop(first);
        let second = StateDb::open(&path).unwrap();

        assert_eq!(chain_count(&second), first_count);
        assert_eq!(segment_count(&second), 1);
        assert_eq!(invocation_checksum(&second), first_invocation_checksum);
    }

    fn legacy_v4_invocation_dual_id_fixture(
        invocation_uuid: &str,
        session_id: Option<&str>,
        session_capture_method: Option<&str>,
        status: &str,
        terminal_reason: Option<&str>,
        error_category: Option<&str>,
    ) -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("state.db");
        let conn = sqlite::Connection::open(&db_path).unwrap();
        seed_current_drift_required_tables(&conn);
        conn.execute(
            "ALTER TABLE invocations DROP COLUMN provider_session_resolved_account",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO invocations
                (invocation_uuid, model_name, provider_name, provider_index, status, success,
                 exit_code, error_category, terminal_reason, session_id, session_capture_method,
                 created_at, finished_at)
             VALUES (?1, 'claude-opus', 'claude', 0, ?2, NULL, NULL, ?3, ?4, ?5, ?6,
                     '2026-04-17T08:00:00Z', NULL)",
            sqlite::params![
                invocation_uuid,
                status,
                error_category,
                terminal_reason,
                session_id,
                session_capture_method
            ],
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 4).unwrap();
        drop(conn);
        dir
    }

    #[test]
    fn migration_backfill_null_null_preserves_running_rows() {
        let invocation_uuid = "11111111-1111-4111-8111-111111111111";
        let dir = legacy_v4_invocation_dual_id_fixture(
            invocation_uuid,
            None,
            None,
            "running",
            Some("still_running"),
            Some("unknown"),
        );
        let db = StateDb::open(&dir.path().join("state.db")).unwrap();

        type MigrationBackfillRow = (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            String,
            Option<String>,
        );

        let row: MigrationBackfillRow = db
            .conn
            .query_row(
                "SELECT session_id, provider_session_id, resume_input_id,
                        provider_session_capture_method, terminal_reason, status, error_category
                 FROM invocations
                 WHERE invocation_uuid = ?1",
                sqlite::params![invocation_uuid],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .unwrap();

        assert_eq!(row.0, None);
        assert_eq!(row.1, None);
        assert_eq!(row.2, None);
        assert_eq!(row.3, None);
        assert_eq!(row.4.as_deref(), Some("still_running"));
        assert_eq!(row.5, "running");
        assert_eq!(row.6.as_deref(), Some("unknown"));
    }

    #[test]
    fn migration_backfill_resumed_chain_id_safe() {
        let invocation_uuid = "22222222-2222-4222-8222-222222222222";
        let dir = legacy_v4_invocation_dual_id_fixture(
            invocation_uuid,
            Some(CHAIN_A),
            Some("resumed"),
            "succeeded",
            None,
            None,
        );
        {
            let conn = sqlite::Connection::open(dir.path().join("state.db")).unwrap();
            conn.execute(
                "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
                 VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', 'claude-opus')",
                sqlite::params![CHAIN_A],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO session_chain_segments
                    (chain_id, provider_name, session_id, started_at, transition_reason)
                 VALUES (?1, 'claude', ?2, '2026-04-17T08:00:00Z', 'initial')",
                sqlite::params![CHAIN_A, SESSION_A],
            )
            .unwrap();
        }
        let db = StateDb::open(&dir.path().join("state.db")).unwrap();

        let row: (String, Option<String>, Option<String>, Option<String>) = db
            .conn
            .query_row(
                "SELECT session_id, provider_session_id, resume_input_id,
                        provider_session_capture_method
                 FROM invocations
                 WHERE invocation_uuid = ?1",
                sqlite::params![invocation_uuid],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        let models = resolver_model_store();
        let resolved = db.resolve_resume(&models, CHAIN_A, None).unwrap();

        assert_eq!(row.0, CHAIN_A);
        assert_eq!(row.1, None);
        assert_eq!(row.2.as_deref(), Some(CHAIN_A));
        assert_eq!(row.3, None);
        assert_eq!(resolved.active_session_id, SESSION_A);
    }

    #[test]
    fn migration_backfill_non_resumed_with_session_id() {
        let invocation_uuid = "33333333-3333-4333-8333-333333333333";
        let dir = legacy_v4_invocation_dual_id_fixture(
            invocation_uuid,
            Some(SESSION_A),
            Some("forced_flag_verified"),
            "succeeded",
            None,
            None,
        );
        let db = StateDb::open(&dir.path().join("state.db")).unwrap();

        let row: (String, Option<String>, Option<String>, Option<String>) = db
            .conn
            .query_row(
                "SELECT session_id, provider_session_id, resume_input_id,
                        provider_session_capture_method
                 FROM invocations
                 WHERE invocation_uuid = ?1",
                sqlite::params![invocation_uuid],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();

        assert_eq!(row.0, SESSION_A);
        assert_eq!(row.1.as_deref(), Some(SESSION_A));
        assert_eq!(row.2, None);
        assert_eq!(row.3.as_deref(), Some("forced_flag_verified"));
    }

    // risk: Chain identity write paths; level: particular-integration; source: proposal §11.1 Chain identity write paths / A3, A8.
    #[test]
    fn mint_chain_no_op_on_resume_of_existing_chain() {
        let db = test_db();
        seed_chain_row(&db, CHAIN_A, "claude-opus", "2026-04-17T08:00:00Z");

        let first_id = db
            .open_chain_segment(
                CHAIN_A,
                "claude",
                SESSION_A,
                &ts("2026-04-17T08:00:00Z"),
                oulipoly_core::TransitionReason::Initial,
            )
            .unwrap();
        let second_id = db
            .open_chain_segment(
                CHAIN_A,
                "claude",
                SESSION_A,
                &ts("2026-04-17T08:01:00Z"),
                oulipoly_core::TransitionReason::Initial,
            )
            .unwrap();

        assert_eq!(first_id, second_id);
        assert_eq!(segment_count(&db), 1);
        let active: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM session_chain_segments WHERE chain_id = ?1 AND ended_at IS NULL",
                sqlite::params![CHAIN_A],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(active, 1);
    }

    // risk: Chain identity write paths; level: particular-integration; source: proposal §11.1 Chain identity write paths / A3, A8.
    #[test]
    fn agent_session_chain_records_initial_reason_even_if_ingestion_minted_first() {
        let db = test_db();
        db.mint_imported_chain_if_absent(
            "claude",
            SESSION_A,
            &ts("2026-04-17T08:00:00Z"),
            "<unknown>",
        )
        .unwrap();
        let id = db
            .start_invocation(&InvocationStart {
                invocation_uuid: Uuid::new_v4().to_string(),
                model_name: "claude-opus".to_string(),
                provider_name: "claude".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        db.update_session_capture(id, Some(SESSION_A), "fixture")
            .unwrap();

        db.mint_chain_for_invocation_session(id).unwrap();

        let reason: String = db
            .conn
            .query_row(
                "SELECT transition_reason FROM session_chain_segments
                 WHERE provider_name = 'claude' AND session_id = ?1",
                sqlite::params![SESSION_A],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(reason, "initial");
    }

    // risk: Chain identity write paths; level: particular-integration; source: proposal §11.1 Chain identity write paths / A3, A8.
    #[test]
    fn imported_session_stays_imported_when_no_agent_mint_fires() {
        let db = test_db();

        db.mint_imported_chain_if_absent(
            "claude",
            SESSION_A,
            &ts("2026-04-17T08:00:00Z"),
            "<unknown>",
        )
        .unwrap();

        let reason: String = db
            .conn
            .query_row(
                "SELECT transition_reason FROM session_chain_segments
                 WHERE provider_name = 'claude' AND session_id = ?1",
                sqlite::params![SESSION_A],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(reason, "imported");
    }

    #[test]
    fn find_session_for_invocation_window_returns_fresh_in_window_candidate() {
        let db = test_db();
        let turns = vec![
            SessionTurnIngest {
                session_id: SESSION_A.to_string(),
                turn_id: "old-turn".to_string(),
                timestamp: ts("2026-04-17T08:00:00Z"),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: None,
            },
            SessionTurnIngest {
                session_id: SESSION_B.to_string(),
                turn_id: "fresh-turn".to_string(),
                timestamp: ts("2026-04-17T08:00:02Z"),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: None,
            },
        ];
        db.ingest_session_turns_batch("claude", &turns).unwrap();

        let found = db
            .find_session_for_invocation_window(
                "claude",
                &ts("2026-04-17T08:00:01Z"),
                &ts("2026-04-17T08:00:03Z"),
            )
            .unwrap();

        assert_eq!(found.as_deref(), Some(SESSION_B));
    }

    #[test]
    fn find_session_for_invocation_window_ranks_by_count_earliest_then_session_id() {
        fn turn(session_id: &str, turn_id: &str, timestamp: &str) -> SessionTurnIngest {
            SessionTurnIngest {
                session_id: session_id.to_string(),
                turn_id: turn_id.to_string(),
                timestamp: ts(timestamp),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: None,
            }
        }

        let db = test_db();
        db.ingest_session_turns_batch(
            "claude",
            &[
                turn(SESSION_A, "a-1", "2026-04-17T08:00:03Z"),
                turn(SESSION_A, "a-2", "2026-04-17T08:00:05Z"),
                turn(SESSION_B, "b-1", "2026-04-17T08:00:02Z"),
            ],
        )
        .unwrap();
        let found = db
            .find_session_for_invocation_window(
                "claude",
                &ts("2026-04-17T08:00:01Z"),
                &ts("2026-04-17T08:00:06Z"),
            )
            .unwrap();
        assert_eq!(
            found.as_deref(),
            Some(SESSION_A),
            "higher in-window turn count outranks an earlier first turn"
        );

        let db = test_db();
        db.ingest_session_turns_batch(
            "claude",
            &[
                turn(SESSION_A, "a-1", "2026-04-17T08:00:03Z"),
                turn(SESSION_A, "a-2", "2026-04-17T08:00:05Z"),
                turn(SESSION_B, "b-1", "2026-04-17T08:00:02Z"),
                turn(SESSION_B, "b-2", "2026-04-17T08:00:06Z"),
            ],
        )
        .unwrap();
        let found = db
            .find_session_for_invocation_window(
                "claude",
                &ts("2026-04-17T08:00:01Z"),
                &ts("2026-04-17T08:00:06Z"),
            )
            .unwrap();
        assert_eq!(
            found.as_deref(),
            Some(SESSION_B),
            "earlier first in-window turn breaks equal counts"
        );

        let lexically_first = "11111111-1111-4111-8111-111111111111";
        let lexically_second = "22222222-2222-4222-8222-222222222222";
        let db = test_db();
        db.ingest_session_turns_batch(
            "claude",
            &[
                turn(lexically_second, "second-1", "2026-04-17T08:00:02Z"),
                turn(lexically_second, "second-2", "2026-04-17T08:00:05Z"),
                turn(lexically_first, "first-1", "2026-04-17T08:00:02Z"),
                turn(lexically_first, "first-2", "2026-04-17T08:00:06Z"),
            ],
        )
        .unwrap();
        let found = db
            .find_session_for_invocation_window(
                "claude",
                &ts("2026-04-17T08:00:01Z"),
                &ts("2026-04-17T08:00:06Z"),
            )
            .unwrap();
        assert_eq!(
            found.as_deref(),
            Some(lexically_first),
            "lexicographic session id breaks equal counts and equal earliest turns"
        );
    }

    // risk: Resolver disambiguation and model inference; level: particular-integration; source: proposal §11.1 Resolver disambiguation and model inference.
    #[test]
    fn resolve_resume_returns_active_segment_for_single_chain() {
        let db = test_db();
        seed_chain_row(&db, CHAIN_A, "claude-opus", "2026-04-17T09:00:00Z");
        seed_segment_row(
            &db,
            CHAIN_A,
            "claude",
            SESSION_A,
            "2026-04-17T08:00:00Z",
            Some("2026-04-17T08:30:00Z"),
            "initial",
        );
        seed_segment_row(
            &db,
            CHAIN_A,
            "claude2",
            SESSION_B,
            "2026-04-17T08:31:00Z",
            None,
            "quota_threshold",
        );
        let models = resolver_model_store();

        let resolved = db.resolve_resume(&models, CHAIN_A, None).unwrap();

        assert_eq!(resolved.chain_id, CHAIN_A);
        assert_eq!(resolved.active_provider, "claude2");
        assert_eq!(resolved.active_session_id, SESSION_B);
        assert_eq!(resolved.model_name.as_deref(), Some("claude-opus"));
    }

    // risk: Resolver disambiguation and model inference; level: particular-integration; source: proposal §11.1 Resolver disambiguation and model inference.
    #[test]
    fn resolve_resume_chooses_most_recent_chain_when_two_chains_share_session_id() {
        let db = test_db();
        seed_test_chain(
            &db,
            CHAIN_A,
            "claude",
            SESSION_A,
            "claude-opus",
            "2026-04-17T08:00:00Z",
        );
        seed_test_chain(
            &db,
            CHAIN_B,
            "claude",
            SESSION_A,
            "claude-opus",
            "2026-04-17T09:00:00Z",
        );
        let models = resolver_model_store();

        let resolved = db.resolve_resume(&models, SESSION_A, None).unwrap();

        assert_eq!(resolved.chain_id, CHAIN_B);
    }

    // risk: Resolver disambiguation and model inference; level: particular-integration; source: proposal §11.1 Resolver disambiguation and model inference.
    #[test]
    fn resolve_resume_chooses_most_recent_chain_without_ambiguous_halt() {
        let db = test_db();
        seed_test_chain(
            &db,
            CHAIN_A,
            "claude",
            SESSION_A,
            "claude-opus",
            "2026-04-17T08:00:00Z",
        );
        seed_test_chain(
            &db,
            CHAIN_B,
            "claude2",
            SESSION_A,
            "claude-opus",
            "2026-04-17T09:00:00Z",
        );
        seed_test_chain(
            &db,
            CHAIN_C,
            "claude",
            SESSION_A,
            "claude-opus",
            "2026-04-17T10:00:00Z",
        );
        let models = resolver_model_store();

        let resolved = db.resolve_resume(&models, SESSION_A, None).unwrap();

        assert_eq!(resolved.chain_id, CHAIN_C);
    }

    // risk: Resolver disambiguation and model inference; level: particular-integration; source: proposal §11.1 Resolver disambiguation and model inference.
    #[test]
    fn resolve_resume_breaks_equal_last_used_tie_by_latest_segment_start() {
        let db = test_db();
        let last_used_at = "2026-04-17T10:00:00Z";
        seed_chain_row(&db, CHAIN_A, "claude-opus", last_used_at);
        seed_segment_row(
            &db,
            CHAIN_A,
            "claude",
            SESSION_A,
            "2026-04-17T08:00:00Z",
            None,
            "initial",
        );
        seed_chain_row(&db, CHAIN_B, "claude-opus", last_used_at);
        seed_segment_row(
            &db,
            CHAIN_B,
            "claude2",
            SESSION_A,
            "2026-04-17T09:00:00Z",
            None,
            "initial",
        );
        let models = resolver_model_store();

        let resolved = db.resolve_resume(&models, SESSION_A, None).unwrap();

        assert_eq!(resolved.chain_id, CHAIN_B);
    }

    // risk: Resolver disambiguation and model inference; level: particular-integration; source: proposal §11.1 Resolver disambiguation and model inference.
    #[test]
    fn resolve_resume_infers_model_from_latest_invocation() {
        let db = test_db();
        seed_test_chain(
            &db,
            CHAIN_A,
            "claude",
            SESSION_A,
            "<unknown>",
            "2026-04-17T08:00:00Z",
        );
        seed_invocation_for_session(
            &db,
            "claude-haiku",
            "claude",
            SESSION_A,
            "2026-04-17T08:00:00Z",
        );
        seed_invocation_for_session(
            &db,
            "claude-opus",
            "claude",
            SESSION_A,
            "2026-04-17T09:00:00Z",
        );
        let models = resolver_model_store();

        let resolved = db.resolve_resume(&models, SESSION_A, None).unwrap();

        assert_eq!(resolved.model_name.as_deref(), Some("claude-opus"));
    }

    // risk: Resolver disambiguation and model inference; level: particular-integration; source: proposal §11.1 Resolver disambiguation and model inference.
    #[test]
    fn resolve_resume_falls_back_to_chain_model_name_when_no_invocations() {
        let db = test_db();
        seed_test_chain(
            &db,
            CHAIN_A,
            "claude",
            SESSION_A,
            "claude-haiku",
            "2026-04-17T08:00:00Z",
        );
        let models = resolver_model_store();

        let resolved = db.resolve_resume(&models, SESSION_A, None).unwrap();

        assert_eq!(resolved.model_name.as_deref(), Some("claude-haiku"));
    }

    // risk: Resolver disambiguation and model inference; level: particular-integration; source: proposal §11.1 Resolver disambiguation and model inference / A8.
    #[test]
    fn resolve_resume_returns_none_model_when_no_inference_source() {
        let db = test_db();
        seed_test_chain(
            &db,
            CHAIN_A,
            "claude",
            SESSION_A,
            "<unknown>",
            "2026-04-17T08:00:00Z",
        );
        let models = resolver_model_store();

        let resolved = db.resolve_resume(&models, SESSION_A, None).unwrap();

        assert_eq!(resolved.model_name, None);
        assert!(resolved.model.is_none());
    }

    // risk: Resolver disambiguation and model inference; level: particular-integration; source: proposal §11.1 Resolver disambiguation and model inference.
    #[test]
    fn resolve_resume_validates_provider_in_model_pool() {
        let db = test_db();
        seed_test_chain(
            &db,
            CHAIN_A,
            "claude2",
            SESSION_A,
            "claude-haiku",
            "2026-04-17T08:00:00Z",
        );
        let models = resolver_model_store();

        let err = db.resolve_resume(&models, SESSION_A, None).unwrap_err();

        match err {
            ResumeError::ProviderModelMismatch {
                model_name,
                active_provider,
                suggestions,
            } => {
                assert_eq!(model_name, "claude-haiku");
                assert_eq!(active_provider, "claude2");
                assert!(suggestions.contains(&"claude-opus".to_string()));
            }
            other => panic!("expected provider/model mismatch, got {other:?}"),
        }
    }

    // risk: Chain identity write paths; level: particular-integration; source: proposal §11.1 Chain identity write paths / A3, A8.
    #[test]
    fn chain_last_used_at_updates_after_successful_invocation() {
        let db = test_db();
        seed_test_chain(
            &db,
            CHAIN_A,
            "claude",
            SESSION_A,
            "claude-opus",
            "2026-04-17T08:00:00Z",
        );

        let before = Utc::now();
        db.update_chain_last_used(CHAIN_A).unwrap();
        let after = Utc::now();

        let last_used_raw: String = db
            .conn
            .query_row(
                "SELECT last_used_at FROM session_chains WHERE chain_id = ?1",
                sqlite::params![CHAIN_A],
                |row| row.get(0),
            )
            .unwrap();
        let last_used = DateTime::parse_from_rfc3339(&last_used_raw)
            .unwrap()
            .with_timezone(&Utc);
        assert!(last_used >= before - chrono::Duration::seconds(1));
        assert!(last_used <= after + chrono::Duration::seconds(1));
    }

    // risk: Chain identity write paths; level: particular-integration; source: proposal §11.1 Chain identity write paths / A3, A8.
    #[test]
    fn chain_identity_helpers_report_sql_errors() {
        let segmentless = db_without_table("session_chain_segments");
        let segment_open_err = segmentless
            .open_chain_segment(
                CHAIN_A,
                "claude",
                SESSION_A,
                &ts("2026-04-17T08:00:00Z"),
                oulipoly_core::TransitionReason::Initial,
            )
            .unwrap_err();
        assert!(
            segment_open_err.contains("session chain segment"),
            "{segment_open_err}"
        );

        let mint_err = db_without_table("session_chain_segments")
            .mint_imported_chain_if_absent(
                "claude",
                SESSION_A,
                &ts("2026-04-17T08:00:00Z"),
                "claude-opus",
            )
            .unwrap_err();
        assert!(
            mint_err.contains("existing session chain segment"),
            "{mint_err}"
        );

        let update_err = db_without_table("session_chains")
            .update_chain_last_used(CHAIN_A)
            .unwrap_err();
        assert!(update_err.contains("last_used_at"), "{update_err}");

        let chain_lookup_err = db_without_table("session_chain_segments")
            .chain_id_for_segment("claude", SESSION_A)
            .unwrap_err();
        assert!(
            chain_lookup_err.contains("session chain id"),
            "{chain_lookup_err}"
        );
    }

    // risk: Migration mechanic: compaction-aware Claude target build; level: particular-integration; source: proposal §11.1 Migration mechanic: compaction-aware Claude target build / A3, A6.
    #[test]
    fn compaction_and_preview_helpers_report_negative_paths() {
        let malformed_uuid = test_db().resume_previews("not-a-uuid").unwrap_err();
        assert!(malformed_uuid.contains("Invalid UUID"), "{malformed_uuid}");

        let db = test_db();
        db.conn
            .execute(
                "INSERT INTO session_turns
                    (provider_name, session_id, turn_id, timestamp, role, source_file, ingested_at, is_compaction_boundary)
                 VALUES ('claude', ?1, 'bad-boundary', 'not-a-timestamp', 'assistant', '', '2026-04-17T08:00:00Z', 1)",
                sqlite::params![SESSION_A],
            )
            .unwrap();

        let boundary_err = db
            .latest_compaction_boundary("claude", SESSION_A)
            .unwrap_err();
        assert!(
            boundary_err.contains("Bad compaction boundary timestamp"),
            "{boundary_err}"
        );
    }

    // risk: Migration mechanic: Claude JSONL copy, Codex deferred guard, segment ledger, and races; level: particular-integration; source: proposal §11.1 Migration mechanic / A3.
    #[test]
    fn migration_returning_clause_aborts_on_concurrent_close() {
        let db = test_db();
        seed_test_chain(
            &db,
            CHAIN_A,
            "claude",
            SESSION_A,
            "claude-opus",
            "2026-04-17T08:00:00Z",
        );

        let first = db
            .close_active_segment_returning(CHAIN_A, &ts("2026-04-17T09:00:00Z"))
            .unwrap();
        let second = db
            .close_active_segment_returning(CHAIN_A, &ts("2026-04-17T09:00:01Z"))
            .unwrap();

        assert!(first.is_some(), "first close should win RETURNING guard");
        assert_eq!(second, None, "concurrent loser must abort");
        let active: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM session_chain_segments WHERE chain_id = ?1 AND ended_at IS NULL",
                sqlite::params![CHAIN_A],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(active, 0);
    }

    #[test]
    fn age132_invocation_projection_maps_full_row_and_rejects_bad_values() {
        let db = test_db();
        let invocation_uuid = "44444444-4444-4444-8444-444444444444";
        let id = db
            .start_invocation(&InvocationStart {
                invocation_uuid: invocation_uuid.to_string(),
                model_name: "claude-opus".to_string(),
                provider_name: "claude".to_string(),
                provider_index: 7,
                parent_invocation_id: None,
            })
            .unwrap();
        db.update_session_capture(id, Some(SESSION_A), "verified")
            .unwrap();
        db.update_resume_acceptance(id, "accepted", Some("matched"))
            .unwrap();
        db.conn
            .execute(
                "UPDATE invocations
                 SET status = 'succeeded',
                     success = 1,
                     exit_code = 0,
                     terminal_reason = 'exit_zero',
                     created_at = '2026-04-17T08:00:00Z',
                     finished_at = '2026-04-17T08:00:02Z'
                 WHERE id = ?1",
                sqlite::params![id],
            )
            .unwrap();

        let record = db.get_invocation_by_uuid(invocation_uuid).unwrap().unwrap();
        assert_eq!(record.id, id);
        assert_eq!(record.invocation_uuid, invocation_uuid);
        assert_eq!(record.model_name, "claude-opus");
        assert_eq!(record.provider_name.as_deref(), Some("claude"));
        assert_eq!(record.provider_index, 7);
        assert_eq!(record.parent_invocation_id, None);
        assert_eq!(record.status, InvocationStatus::Succeeded);
        assert_eq!(record.success, Some(true));
        assert_eq!(record.exit_code, Some(0));
        assert_eq!(record.terminal_reason.as_deref(), Some("exit_zero"));
        assert_eq!(record.session_id.as_deref(), Some(SESSION_A));
        assert_eq!(record.provider_session_id.as_deref(), Some(SESSION_A));
        assert_eq!(
            record.provider_session_capture_method.as_deref(),
            Some("verified")
        );
        assert_eq!(record.resume_acceptance_status.as_deref(), Some("accepted"));
        assert_eq!(
            record.resume_acceptance_evidence.as_deref(),
            Some("matched")
        );
        assert_eq!(record.created_at, ts("2026-04-17T08:00:00Z"));
        assert_eq!(record.finished_at, Some(ts("2026-04-17T08:00:02Z")));

        let child_uuid = "55555555-5555-5555-8555-555555555555";
        let child_id = insert_invocation_fixture(&db, child_uuid, Some(id), "2026-04-17T08:00:01Z");
        let children = db.list_invocation_children(id).unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].id, child_id);
        assert_eq!(children[0].invocation_uuid, child_uuid);
        assert_eq!(children[0].parent_invocation_id, Some(id));
        assert_eq!(children[0].created_at, ts("2026-04-17T08:00:01Z"));

        db.conn
            .pragma_update(None, "ignore_check_constraints", true)
            .unwrap();
        db.conn
            .execute(
                "UPDATE invocations SET status = 'paused' WHERE id = ?1",
                sqlite::params![id],
            )
            .unwrap();
        let err = db.get_invocation_by_uuid(invocation_uuid).unwrap_err();
        assert!(err.contains("Unknown invocation status: paused"), "{err}");
        db.conn
            .execute(
                "UPDATE invocations SET status = 'running', created_at = 'not-a-timestamp' WHERE id = ?1",
                sqlite::params![id],
            )
            .unwrap();
        let err = db.get_invocation_by_uuid(invocation_uuid).unwrap_err();
        assert!(err.contains("Conversion error"), "{err}");
    }

    #[test]
    fn age132_backfill_infers_model_from_latest_matching_invocation() {
        let db = test_db();
        db.ingest_session_turns_batch(
            "claude",
            &[
                SessionTurnIngest {
                    session_id: SESSION_A.to_string(),
                    turn_id: "turn-a1".to_string(),
                    timestamp: ts("2026-04-17T08:00:00Z"),
                    role: "user".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: false,
                    body: None,
                },
                SessionTurnIngest {
                    session_id: SESSION_A.to_string(),
                    turn_id: "turn-a2".to_string(),
                    timestamp: ts("2026-04-17T08:01:00Z"),
                    role: "assistant".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: false,
                    body: None,
                },
            ],
        )
        .unwrap();
        seed_invocation_for_session(
            &db,
            "claude-haiku",
            "claude",
            SESSION_A,
            "2026-04-17T08:00:30Z",
        );
        seed_invocation_for_session(
            &db,
            "claude-opus",
            "claude",
            SESSION_A,
            "2026-04-17T08:01:30Z",
        );

        let report = db.backfill_session_chains().unwrap();
        assert_eq!(
            report,
            BackfillReport {
                skipped_existing: false,
                chains_inserted: 1,
                segments_inserted: 1
            }
        );
        let model_name: String = db
            .conn
            .query_row("SELECT model_name FROM session_chains", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(model_name, "claude-opus");
    }

    #[test]
    fn age132_resolve_resume_rejections_and_wrong_id_context_are_typed() {
        let models = resolver_model_store();
        assert!(matches!(
            test_db()
                .resolve_resume(&models, "not-a-uuid", None)
                .unwrap_err(),
            ResumeError::InvalidUuid { .. }
        ));
        assert!(matches!(
            test_db()
                .resolve_resume(&models, "ses_ab", None)
                .unwrap_err(),
            ResumeError::InvalidUuid { .. }
        ));
        assert!(matches!(
            test_db()
                .resolve_resume(&models, "77777777-7777-4777-8777-777777777777", None)
                .unwrap_err(),
            ResumeError::NoChainFound { .. }
        ));

        let unknown_model_db = test_db();
        seed_test_chain(
            &unknown_model_db,
            CHAIN_A,
            "claude",
            SESSION_A,
            "missing-model",
            "2026-04-17T08:00:00Z",
        );
        assert!(matches!(
            unknown_model_db.resolve_resume(&models, SESSION_A, None).unwrap_err(),
            ResumeError::UnknownModel { ref model_name } if model_name == "missing-model"
        ));

        let missing_segment_db = test_db();
        seed_chain_row(
            &missing_segment_db,
            CHAIN_A,
            "claude-opus",
            "2026-04-17T08:00:00Z",
        );
        seed_segment_row(
            &missing_segment_db,
            CHAIN_A,
            "claude",
            SESSION_A,
            "2026-04-17T08:00:00Z",
            Some("2026-04-17T08:30:00Z"),
            "initial",
        );
        assert!(matches!(
            missing_segment_db.resolve_resume(&models, SESSION_A, None).unwrap_err(),
            ResumeError::ActiveSegmentMissing { ref chain_id } if chain_id == CHAIN_A
        ));

        let wrong_id_db = test_db();
        let invocation_uuid = "88888888-8888-4888-8888-888888888888";
        let id = wrong_id_db
            .start_invocation(&InvocationStart {
                invocation_uuid: invocation_uuid.to_string(),
                model_name: "claude-opus".to_string(),
                provider_name: "claude".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        wrong_id_db
            .bind_invocation_provider_session_start(
                id,
                &ProviderSessionBinding {
                    provider_session_id: SESSION_A.to_string(),
                    capture_method: "verified",
                    resume_input_id: None,
                    provider_session_resolved_account: None,
                },
            )
            .unwrap();
        match wrong_id_db
            .resolve_resume(&models, invocation_uuid, None)
            .unwrap_err()
        {
            ResumeError::WrongIdKind {
                provider_session_id,
                chain_id,
                provider_name,
                agent_runner_invocation_id,
                ..
            } => {
                assert_eq!(provider_session_id.as_deref(), Some(SESSION_A));
                assert!(chain_id.is_some());
                assert_eq!(provider_name.as_deref(), Some("claude"));
                assert_eq!(agent_runner_invocation_id, invocation_uuid);
            }
            other => panic!("expected wrong-id-kind rejection, got {other:?}"),
        }
    }

    #[test]
    fn resolve_resume_accepts_opencode_provider_session_id() {
        let db = test_db();
        let models = model_store_from_toml(&[(
            "gpt-high",
            r#"
[[providers]]
name = "opencode"
interactive_args = ["run"]
"#,
        )]);
        seed_test_chain(
            &db,
            CHAIN_A,
            "opencode",
            "ses_fixture",
            "gpt-high",
            "2026-06-04T08:00:00Z",
        );

        let resolved = db.resolve_resume(&models, "ses_fixture", None).unwrap();

        assert_eq!(resolved.chain_id, CHAIN_A);
        assert_eq!(resolved.active_provider, "opencode");
        assert_eq!(resolved.active_session_id, "ses_fixture");
        assert_eq!(resolved.model_name.as_deref(), Some("gpt-high"));
    }

    #[test]
    fn age132_timestamp_policies_preserve_strict_forgiving_and_fallback_callers() {
        let db = test_db();
        db.upsert_quota_refresh("claude", &[quota_input(0.40, "2026-04-22T00:00:00Z")])
            .unwrap();
        db.conn
            .execute(
                "UPDATE provider_quotas
                 SET refreshed_at = 'bad-refreshed',
                     exhausted_at = 'bad-exhausted',
                     last_topology_probe_at = 'bad-probe'
                 WHERE provider_name = 'claude'",
                [],
            )
            .unwrap();
        let quota = db.get_quota("claude").unwrap().unwrap();
        assert_eq!(quota.refreshed_at, None);
        assert_eq!(quota.exhausted_at, None);
        assert_eq!(quota.last_topology_probe_at, None);
        db.conn
            .execute(
                "UPDATE provider_quota_windows SET resets_at = 'bad-window' WHERE provider_name = 'claude'",
                [],
            )
            .unwrap();
        assert!(db.get_windows("claude").is_err());

        let id = db
            .start_invocation(&InvocationStart {
                invocation_uuid: Uuid::new_v4().to_string(),
                model_name: "claude-opus".to_string(),
                provider_name: "claude".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        db.update_session_capture(id, Some(SESSION_A), "verified")
            .unwrap();
        db.conn
            .execute(
                "UPDATE invocations SET created_at = 'not-a-timestamp' WHERE id = ?1",
                sqlite::params![id],
            )
            .unwrap();
        let before = Utc::now();
        db.mint_chain_for_invocation_session(id).unwrap();
        let after = Utc::now();
        let raw_started: String = db
            .conn
            .query_row(
                "SELECT started_at FROM session_chain_segments WHERE provider_name = 'claude' AND session_id = ?1",
                sqlite::params![SESSION_A],
                |row| row.get(0),
            )
            .unwrap();
        let started_at = DateTime::parse_from_rfc3339(&raw_started)
            .unwrap()
            .with_timezone(&Utc);
        assert!(started_at >= before - chrono::Duration::seconds(1));
        assert!(started_at <= after + chrono::Duration::seconds(1));
    }

    #[test]
    fn age132_invocation_artifact_contract_and_warning_only_failure_paths() {
        let memory = StateDb::open(Path::new(":memory:")).unwrap();
        let memory_id = memory
            .start_invocation(&InvocationStart {
                invocation_uuid: Uuid::new_v4().to_string(),
                model_name: "claude-opus".to_string(),
                provider_name: "claude".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        memory
            .finalize_invocation(memory_id, true, 0, None, None)
            .unwrap();
        let memory_status: String = memory
            .connection()
            .query_row(
                "SELECT status FROM invocations WHERE id = ?1",
                sqlite::params![memory_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(memory_status, "succeeded");

        let dir = tempfile::tempdir().unwrap();
        let db = StateDb::open(&dir.path().join("state.db")).unwrap();
        let invocation_uuid = "99999999-9999-4999-8999-999999999999";
        let id = db
            .start_invocation(&InvocationStart {
                invocation_uuid: invocation_uuid.to_string(),
                model_name: "claude-opus".to_string(),
                provider_name: "claude".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        let invocation_path = dir
            .path()
            .join("invocations")
            .join(format!("{invocation_uuid}.invocation"));
        assert!(invocation_path.exists());
        assert!(!invocation_path.with_extension("invocation.tmp").exists());
        let payload: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&invocation_path).unwrap()).unwrap();
        assert_eq!(payload["id"], invocation_uuid);
        assert_eq!(payload["status"], "running");
        assert_eq!(payload["model_name"], "claude-opus");
        assert_eq!(payload["provider_name"], "claude");
        assert!(payload["pid"].as_u64().is_some());
        assert!(DateTime::parse_from_rfc3339(payload["started_at"].as_str().unwrap()).is_ok());

        db.finalize_invocation(id, false, 42, Some("rate_limit"), Some("limited"))
            .unwrap();
        let result_path = dir
            .path()
            .join("invocations")
            .join(format!("{invocation_uuid}.result"));
        assert!(result_path.exists());
        assert!(!result_path.with_extension("result.tmp").exists());
        let payload: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&result_path).unwrap()).unwrap();
        assert_eq!(payload["id"], invocation_uuid);
        assert_eq!(payload["status"], "failed");
        assert_eq!(payload["success"], false);
        assert_eq!(payload["exit_code"], 42);
        assert_eq!(payload["error_category"], "rate_limit");
        assert_eq!(payload["terminal_reason"], "limited");
        assert!(DateTime::parse_from_rfc3339(payload["finished_at"].as_str().unwrap()).is_ok());

        let failing_dir = tempfile::tempdir().unwrap();
        let failing = StateDb::open(&failing_dir.path().join("state.db")).unwrap();
        std::fs::write(failing_dir.path().join("invocations"), b"not a directory").unwrap();
        let id = failing
            .start_invocation(&InvocationStart {
                invocation_uuid: Uuid::new_v4().to_string(),
                model_name: "claude-opus".to_string(),
                provider_name: "claude".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        failing
            .finalize_invocation(id, true, 0, None, None)
            .unwrap();
        let status: String = failing
            .conn
            .query_row(
                "SELECT status FROM invocations WHERE id = ?1",
                sqlite::params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "succeeded");
    }

    fn returned_artifact_ref(
        invocation_uuid: Uuid,
        artifact_name: &str,
        version: u64,
    ) -> ReturnedArtifactRef {
        let workflow_run_id = format!("return:{invocation_uuid}");
        let version_id = format!("store://return/{invocation_uuid}/{artifact_name}/{version}");
        ReturnedArtifactRef {
            version_id,
            name: artifact_name.to_string(),
            store_address: oulipoly_agent_messenger::StoreAddress {
                workflow_run_id,
                artifact_name: artifact_name.to_string(),
                version,
            },
            sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
            content_len: 123,
            format_hint: Some("text/plain".to_string()),
            verdict_line: Some("ok".to_string()),
            source: oulipoly_agent_messenger::ReturnedArtifactSource::Scratchpad {
                name: "notes".to_string(),
                version: 1,
            },
            producer_invocation_uuid: invocation_uuid,
            returned_at: ts("2026-04-17T08:00:00Z"),
        }
    }

    #[test]
    fn age132_returned_artifacts_validate_identity_bounds_and_rollback_failed_retry() {
        let db = test_db();
        let invocation_uuid = Uuid::new_v4();
        let id = db
            .start_invocation(&InvocationStart {
                invocation_uuid: invocation_uuid.to_string(),
                model_name: "claude-opus".to_string(),
                provider_name: "claude".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        let good = returned_artifact_ref(invocation_uuid, "alpha.txt", 1);
        db.record_returned_artifacts(id, std::slice::from_ref(&good))
            .unwrap();

        let mut bad_workflow = returned_artifact_ref(invocation_uuid, "bad-workflow.txt", 1);
        bad_workflow.store_address.workflow_run_id = "not-return-namespace".to_string();
        assert!(
            db.record_returned_artifacts(id, &[bad_workflow])
                .unwrap_err()
                .contains("workflow_run_id")
        );

        let mut bad_version = returned_artifact_ref(invocation_uuid, "bad-version.txt", 1);
        bad_version.version_id = "store://wrong-version".to_string();
        assert!(
            db.record_returned_artifacts(id, &[bad_version])
                .unwrap_err()
                .contains("version_id mismatch")
        );

        let mut overflow = returned_artifact_ref(invocation_uuid, "overflow.txt", 1);
        overflow.content_len = u64::MAX;
        assert!(
            db.record_returned_artifacts(id, &[overflow])
                .unwrap_err()
                .contains("content_len exceeds SQLite INTEGER range")
        );
        assert_eq!(db.list_returned_artifacts(id).unwrap(), vec![good]);
    }

    #[test]
    fn age132_session_turn_ingest_batch_and_single_paths_preserve_mapping_and_atomicity() {
        let db = test_db();
        let timestamp = ts("2026-04-17T08:00:00Z");
        assert!(
            db.ingest_session_turn(
                "claude",
                SESSION_A,
                "single-turn",
                &timestamp,
                "assistant",
                "/tmp/session.jsonl",
            )
            .unwrap()
        );
        assert!(
            !db.ingest_session_turn(
                "claude",
                SESSION_A,
                "single-turn",
                &timestamp,
                "assistant",
                "/tmp/session.jsonl",
            )
            .unwrap()
        );
        let source_file: String = db
            .conn
            .query_row(
                "SELECT source_file FROM session_turns WHERE turn_id = 'single-turn'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(source_file, "/tmp/session.jsonl");

        let turns = vec![
            SessionTurnIngest {
                session_id: SESSION_A.to_string(),
                turn_id: "turn-1".to_string(),
                timestamp,
                role: "user".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: Some("hello".to_string()),
            },
            SessionTurnIngest {
                session_id: SESSION_A.to_string(),
                turn_id: "turn-2".to_string(),
                timestamp: timestamp + chrono::Duration::seconds(1),
                role: "assistant".to_string(),
                parent_turn_id: Some("turn-1".to_string()),
                is_sidechain: true,
                is_compaction_boundary: true,
                body: Some("world".to_string()),
            },
        ];
        assert_eq!(db.ingest_session_turns_batch("claude", &turns).unwrap(), 2);
        assert_eq!(db.ingest_session_turns_batch("claude", &turns).unwrap(), 0);
        let row: (Option<String>, i64, i64, Option<String>) = db
            .conn
            .query_row(
                "SELECT parent_turn_id, is_sidechain, is_compaction_boundary, body
                 FROM session_turns WHERE turn_id = 'turn-2'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            row,
            (Some("turn-1".to_string()), 1, 1, Some("world".to_string()))
        );

        let failing = test_db();
        failing
            .conn
            .execute_batch(
                "CREATE TRIGGER fail_bad_turn
                 BEFORE INSERT ON session_turns
                 WHEN NEW.turn_id = 'bad'
                 BEGIN
                   SELECT RAISE(ABORT, 'bad turn');
                 END;",
            )
            .unwrap();
        assert!(
            failing
                .ingest_session_turns_batch(
                    "claude",
                    &[
                        SessionTurnIngest {
                            session_id: SESSION_A.to_string(),
                            turn_id: "good-before-error".to_string(),
                            timestamp,
                            role: "assistant".to_string(),
                            parent_turn_id: None,
                            is_sidechain: false,
                            is_compaction_boundary: false,
                            body: None,
                        },
                        SessionTurnIngest {
                            session_id: SESSION_A.to_string(),
                            turn_id: "bad".to_string(),
                            timestamp: timestamp + chrono::Duration::seconds(1),
                            role: "assistant".to_string(),
                            parent_turn_id: None,
                            is_sidechain: false,
                            is_compaction_boundary: false,
                            body: None,
                        },
                    ],
                )
                .unwrap_err()
                .contains("bad turn")
        );
        let persisted: i64 = failing
            .conn
            .query_row("SELECT COUNT(*) FROM session_turns", [], |row| row.get(0))
            .unwrap();
        assert_eq!(persisted, 0);
    }

    #[test]
    fn age132_resume_previews_and_compaction_boundaries_preserve_ordering_contracts() {
        let db = test_db();
        seed_test_chain(
            &db,
            CHAIN_A,
            "claude",
            SESSION_A,
            "claude-opus",
            "2026-04-17T08:00:00Z",
        );
        seed_test_chain(
            &db,
            CHAIN_B,
            "claude2",
            SESSION_A,
            "claude-opus",
            "2026-04-17T09:00:00Z",
        );
        let turns: Vec<_> = (0..4)
            .map(|i| SessionTurnIngest {
                session_id: SESSION_A.to_string(),
                turn_id: format!("turn-{i}"),
                timestamp: ts(&format!("2026-04-17T08:00:0{i}Z")),
                role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: Some(format!("body-{i}")),
            })
            .collect();
        db.ingest_session_turns_batch("claude2", &turns).unwrap();

        let previews = db.resume_previews(SESSION_A).unwrap();
        assert_eq!(previews.len(), 2);
        assert_eq!(previews[0].chain_id, CHAIN_B);
        assert_eq!(previews[0].active_provider, "claude2");
        assert_eq!(previews[0].turn_count, 4);
        assert_eq!(previews[0].recent_turns.len(), 3);
        assert_eq!(
            previews[0].recent_turns[0].timestamp,
            ts("2026-04-17T08:00:01Z")
        );
        assert_eq!(
            previews[0].recent_turns[1].timestamp,
            ts("2026-04-17T08:00:02Z")
        );
        assert_eq!(
            previews[0].recent_turns[2].timestamp,
            ts("2026-04-17T08:00:03Z")
        );
        assert_eq!(previews[0].recent_turns[0].snippet, None);
        assert_eq!(previews[1].chain_id, CHAIN_A);

        let boundary_db = test_db();
        boundary_db
            .ingest_session_turns_batch(
                "claude",
                &[
                    SessionTurnIngest {
                        session_id: SESSION_A.to_string(),
                        turn_id: "old-boundary".to_string(),
                        timestamp: ts("2026-04-17T08:00:00Z"),
                        role: "assistant".to_string(),
                        parent_turn_id: None,
                        is_sidechain: false,
                        is_compaction_boundary: true,
                        body: None,
                    },
                    SessionTurnIngest {
                        session_id: SESSION_A.to_string(),
                        turn_id: "tie-first".to_string(),
                        timestamp: ts("2026-04-17T08:01:00Z"),
                        role: "assistant".to_string(),
                        parent_turn_id: None,
                        is_sidechain: false,
                        is_compaction_boundary: true,
                        body: None,
                    },
                    SessionTurnIngest {
                        session_id: SESSION_A.to_string(),
                        turn_id: "tie-second".to_string(),
                        timestamp: ts("2026-04-17T08:01:00Z"),
                        role: "assistant".to_string(),
                        parent_turn_id: None,
                        is_sidechain: false,
                        is_compaction_boundary: true,
                        body: None,
                    },
                    SessionTurnIngest {
                        session_id: SESSION_A.to_string(),
                        turn_id: "not-yet-boundary".to_string(),
                        timestamp: ts("2026-04-17T08:02:00Z"),
                        role: "assistant".to_string(),
                        parent_turn_id: None,
                        is_sidechain: false,
                        is_compaction_boundary: false,
                        body: None,
                    },
                ],
            )
            .unwrap();
        let latest = boundary_db
            .latest_compaction_boundary("claude", SESSION_A)
            .unwrap()
            .unwrap();
        assert_eq!(latest.0, "tie-second");
        assert_eq!(latest.1, ts("2026-04-17T08:01:00Z"));
        assert!(
            boundary_db
                .flag_compaction_boundary("claude", SESSION_A, "not-yet-boundary")
                .unwrap()
        );
        assert!(
            !boundary_db
                .flag_compaction_boundary("claude", SESSION_A, "not-yet-boundary")
                .unwrap()
        );
        assert!(
            !boundary_db
                .flag_compaction_boundary("claude", SESSION_A, "missing-turn")
                .unwrap()
        );
        let latest = boundary_db
            .latest_compaction_boundary("claude", SESSION_A)
            .unwrap()
            .unwrap();
        assert_eq!(latest.0, "not-yet-boundary");
        assert_eq!(latest.1, ts("2026-04-17T08:02:00Z"));
        assert_eq!(
            test_db()
                .latest_compaction_boundary("claude", SESSION_A)
                .unwrap(),
            None
        );
    }

    #[test]
    fn age132_read_only_error_classifier_and_sidecar_paths_map_documented_variants() {
        let missing_dir = tempfile::tempdir().unwrap();
        let missing_path = missing_dir.path().join("missing-state.db");
        match StateDb::open_read_only(&missing_path) {
            Err(ReadOnlyOpenError::Missing { path }) => assert_eq!(path, missing_path),
            Ok(_) => panic!("expected Missing, got successful read-only open"),
            Err(other) => panic!("expected Missing, got {other:?}"),
        }

        let malformed_dir = tempfile::tempdir().unwrap();
        let malformed_path = malformed_dir.path().join("state.db");
        std::fs::write(&malformed_path, b"not sqlite").unwrap();
        match StateDb::open_read_only(&malformed_path) {
            Err(ReadOnlyOpenError::NotADatabase { path: p, .. }) => {
                assert_eq!(p, malformed_path);
            }
            Ok(_) => panic!("expected NotADatabase, got successful read-only open"),
            Err(other) => panic!("expected NotADatabase, got {other:?}"),
        }

        let valid_dir = tempfile::tempdir().unwrap();
        let valid_path = valid_dir.path().join("state.db");
        drop(StateDb::open(&valid_path).unwrap());
        drop(StateDb::open_read_only(&valid_path).unwrap());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let denied_dir = tempfile::tempdir().unwrap();
            let denied_path = denied_dir.path().join("state.db");
            drop(StateDb::open(&denied_path).unwrap());
            let mut denied_permissions = std::fs::metadata(&denied_path).unwrap().permissions();
            denied_permissions.set_mode(0o000);
            std::fs::set_permissions(&denied_path, denied_permissions).unwrap();
            match StateDb::open_read_only(&denied_path) {
                Err(ReadOnlyOpenError::PermissionDenied { path }) => assert_eq!(path, denied_path),
                Ok(_) => panic!("expected PermissionDenied, got successful read-only open"),
                Err(other) => panic!("expected PermissionDenied, got {other:?}"),
            }

            let sidecar_dir = tempfile::tempdir().unwrap();
            let sidecar_path = sidecar_dir.path().join("state.db");
            let sidecar_conn = sqlite::Connection::open(&sidecar_path).unwrap();
            sidecar_conn
                .execute_batch(
                    "PRAGMA journal_mode = WAL;
                     CREATE TABLE sidecar_probe (value TEXT);
                     INSERT INTO sidecar_probe (value) VALUES ('kept open');",
                )
                .unwrap();
            let sidecar_file = std::fs::read_dir(sidecar_dir.path())
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| path != &sidecar_path && path.is_file())
                .expect("WAL mode should create at least one SQLite sidecar file");
            let mut sidecar_permissions = std::fs::metadata(&sidecar_file).unwrap().permissions();
            sidecar_permissions.set_mode(0o000);
            std::fs::set_permissions(&sidecar_file, sidecar_permissions).unwrap();
            match StateDb::open_read_only(&sidecar_path) {
                Err(ReadOnlyOpenError::WalSidecarError { path, message }) => {
                    assert_eq!(path, sidecar_path);
                    assert!(message.contains("sidecar"), "{message}");
                }
                Ok(_) => panic!("expected WalSidecarError, got successful read-only open"),
                Err(other) => panic!("expected WalSidecarError, got {other:?}"),
            }
            drop(sidecar_conn);
        }

        match StateDb::open_read_only(valid_dir.path()) {
            Err(ReadOnlyOpenError::Operational { message }) => {
                assert!(!message.is_empty());
            }
            Ok(_) => panic!("expected Operational, got successful read-only open"),
            Err(other) => panic!("expected operational mapping, got {other:?}"),
        }
    }

    #[test]
    fn age132_setup_crud_count_and_call_counter_edge_contracts() {
        let db = test_db();
        db.upsert_cli_provider(&sample_provider()).unwrap();
        let expired = AccountRecord {
            id: "expired".to_string(),
            provider: "claude".to_string(),
            profile_name: "expired-profile".to_string(),
            auth_method: AuthMethod::OAuth,
            auth_status: AuthStatus::Expired,
            created_at: "2026-02-19T00:00:00Z".to_string(),
        };
        db.insert_account(&expired).unwrap();
        db.conn
            .execute(
                "UPDATE accounts SET auth_status = 'surprise' WHERE id = 'expired'",
                [],
            )
            .unwrap();
        let accounts = db.list_accounts(Some("claude")).unwrap();
        assert_eq!(accounts[0].auth_status, AuthStatus::Unknown);
        assert!(!db.delete_account("missing", "claude").unwrap());
        assert_eq!(
            db.delete_stale_models("claude", "missing-version").unwrap(),
            0
        );

        let since = ts("2026-04-17T08:00:00Z");
        db.ingest_session_turns_batch(
            "claude",
            &[
                SessionTurnIngest {
                    session_id: SESSION_A.to_string(),
                    turn_id: "at-boundary".to_string(),
                    timestamp: since,
                    role: "assistant".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: false,
                    body: None,
                },
                SessionTurnIngest {
                    session_id: SESSION_A.to_string(),
                    turn_id: "after-boundary".to_string(),
                    timestamp: since + chrono::Duration::seconds(1),
                    role: "assistant".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: false,
                    body: None,
                },
                SessionTurnIngest {
                    session_id: SESSION_A.to_string(),
                    turn_id: "user-after-boundary".to_string(),
                    timestamp: since + chrono::Duration::seconds(2),
                    role: "user".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: false,
                    body: None,
                },
            ],
        )
        .unwrap();
        assert_eq!(db.count_assistant_turns_since("claude", None).unwrap(), 2);
        assert_eq!(
            db.count_assistant_turns_since("claude", Some(&since))
                .unwrap(),
            1
        );
        assert_eq!(db.count_assistant_turns_since("codex", None).unwrap(), 0);
        db.increment_calls_since_refresh("claude").unwrap();
        db.increment_calls_since_refresh("claude").unwrap();
        assert_eq!(calls_since_refresh(&db, "claude"), 2);
    }

    // TI-04, TI-12, TI-24: ordered migration steps must fail with actionable
    // rebuild guidance and roll back both schema effects and user_version.
    #[test]
    fn ti_04_ti_12_ti_24_ordered_migration_failure_rolls_back_and_reports_rebuild() {
        use crate::migrations::{self, Migration, MigrationError};

        let (target_version, id, sql) = failing_migration::failing_migration_parts();
        let failing = Migration {
            target_version,
            id,
            sql,
            post_sql_hook: None,
        };

        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("state.db");
        let mut conn = sqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "
            PRAGMA user_version = 3;
            CREATE TABLE preserved_rows (id INTEGER PRIMARY KEY, value TEXT NOT NULL);
            INSERT INTO preserved_rows (id, value) VALUES (1, 'before');
            ",
        )
        .unwrap();

        let err =
            migrations::run_with_db_path(&mut conn, &[&failing], db_path.clone()).unwrap_err();
        let message = err.to_string();

        assert!(
            message.contains(failing_migration::FAILING_MIGRATION_ID),
            "{message}"
        );
        assert!(
            message.contains(&format!(
                "target_version={}",
                failing_migration::FAILING_MIGRATION_TARGET_VERSION
            )),
            "{message}"
        );
        assert!(message.contains("agents migrate --rebuild"), "{message}");
        assert!(
            message.contains(&format!("db={}", db_path.display())),
            "{message}"
        );
        assert!(
            matches!(err, MigrationError::StepFailed { .. }),
            "expected StepFailed"
        );

        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 3);
        let preserved: String = conn
            .query_row("SELECT value FROM preserved_rows WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(preserved, "before");
        let marker_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name = 'age32_failure_marker'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(marker_exists, 0, "failed migration left partial schema");
    }

    fn age160_sqlite_failure(
        code: sqlite::ffi::ErrorCode,
        extended_code: i32,
        message: &str,
    ) -> sqlite::Error {
        sqlite::Error::SqliteFailure(
            sqlite::ffi::Error {
                code,
                extended_code,
            },
            Some(message.to_string()),
        )
    }

    fn age160_assert_not_database(error: ReadOnlyOpenError, expected_path: &Path) {
        match error {
            ReadOnlyOpenError::NotADatabase { path, .. } => assert_eq!(path, expected_path),
            other => panic!("expected NotADatabase, got {other:?}"),
        }
    }

    fn age160_assert_permission_denied(error: ReadOnlyOpenError, expected_path: &Path) {
        match error {
            ReadOnlyOpenError::PermissionDenied { path } => assert_eq!(path, expected_path),
            other => panic!("expected PermissionDenied, got {other:?}"),
        }
    }

    fn age160_assert_operational(error: ReadOnlyOpenError) {
        match error {
            ReadOnlyOpenError::Operational { message } => assert!(!message.is_empty()),
            other => panic!("expected Operational, got {other:?}"),
        }
    }

    fn age160_assert_wal_sidecar(error: ReadOnlyOpenError, expected_path: &Path) {
        match error {
            ReadOnlyOpenError::WalSidecarError { path, message } => {
                assert_eq!(path, expected_path);
                assert!(!message.is_empty());
            }
            other => panic!("expected WalSidecarError, got {other:?}"),
        }
    }

    /// AGE-160 risk: PP-001 push-pull / PM-01 typed read-only SQLite error projection.
    /// Selected level: unit.
    /// Source: the AGE-160 proposal § Test-intent track; validates A1 and
    /// the "do not parse diagnostic strings" forbidden behavior.
    #[test]
    fn age160_classify_read_only_open_error_via_typed_projection_not_database_permission_and_plain_unknown()
     {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.db");

        age160_assert_not_database(
            classify_read_only_open_error(
                &path,
                age160_sqlite_failure(
                    sqlite::ffi::ErrorCode::NotADatabase,
                    sqlite::ffi::ErrorCode::NotADatabase as i32,
                    "private diagnostic mentions wal but code is not-a-database",
                ),
            ),
            &path,
        );
        age160_assert_not_database(
            classify_read_only_open_error(
                &path,
                age160_sqlite_failure(
                    sqlite::ffi::ErrorCode::DatabaseCorrupt,
                    sqlite::ffi::ErrorCode::DatabaseCorrupt as i32,
                    "private diagnostic mentions shared memory but code is corrupt",
                ),
            ),
            &path,
        );
        age160_assert_permission_denied(
            classify_read_only_open_error(
                &path,
                age160_sqlite_failure(
                    sqlite::ffi::ErrorCode::PermissionDenied,
                    sqlite::ffi::ErrorCode::PermissionDenied as i32,
                    "permission denied",
                ),
            ),
            &path,
        );

        for (code, message) in [
            (
                sqlite::ffi::ErrorCode::SystemIoFailure,
                "plain SystemIoFailure must ignore wal/-shm diagnostic tokens",
            ),
            (
                sqlite::ffi::ErrorCode::ReadOnly,
                "read only database with wal-shaped private text",
            ),
            (
                sqlite::ffi::ErrorCode::CannotOpen,
                "cannot open database with shared memory-shaped private text",
            ),
        ] {
            age160_assert_operational(classify_read_only_open_error(
                &path,
                age160_sqlite_failure(code, code as i32, message),
            ));
        }
    }

    /// AGE-160 risk: PP-001 push-pull + A2 sidecar evidence.
    /// Selected level: unit.
    /// Source: the AGE-160 proposal § Test-intent track; validates A2.
    #[test]
    fn age160_classify_read_only_open_error_via_typed_projection_wal_sidecar() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.db");
        std::fs::write(&path, b"placeholder").unwrap();
        std::fs::write(wal_path(&path), b"owned wal sidecar").unwrap();

        age160_assert_wal_sidecar(
            classify_read_only_open_error(
                &path,
                age160_sqlite_failure(
                    sqlite::ffi::ErrorCode::SystemIoFailure,
                    sqlite::ffi::ErrorCode::SystemIoFailure as i32,
                    "plain io failure text intentionally lacks sidecar tokens",
                ),
            ),
            &path,
        );

        let dirty_wal_path = temp.path().join("dirty-wal-state.db");
        age160_assert_wal_sidecar(
            classify_read_only_open_error(
                &dirty_wal_path,
                age160_sqlite_failure(
                    sqlite::ffi::ErrorCode::CannotOpen,
                    sqlite::ffi::SQLITE_CANTOPEN_DIRTYWAL,
                    "dirty WAL extended code without diagnostic-token dependency",
                ),
            ),
            &dirty_wal_path,
        );
    }

    /// AGE-160 risk: PP-001 push-pull + A2 READONLY_CANTLOCK projection.
    /// Selected level: unit.
    /// Source: Phase 8 PR-review remediation; covers the typed extended-code branch.
    #[test]
    fn age160_classify_read_only_open_error_via_typed_projection_readonly_cantlock() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.db");

        age160_assert_wal_sidecar(
            classify_read_only_open_error(
                &path,
                age160_sqlite_failure(
                    sqlite::ffi::ErrorCode::ReadOnly,
                    sqlite::ffi::SQLITE_READONLY_CANTLOCK,
                    "readonly cantlock extended code without diagnostic-token dependency",
                ),
            ),
            &path,
        );
    }

    /// AGE-160 risk: PP-001 push-pull + A2 READONLY_RECOVERY projection.
    /// Selected level: unit.
    /// Source: Phase 8 PR-review remediation; covers the typed extended-code branch.
    #[test]
    fn age160_classify_read_only_open_error_via_typed_projection_readonly_recovery() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.db");

        age160_assert_wal_sidecar(
            classify_read_only_open_error(
                &path,
                age160_sqlite_failure(
                    sqlite::ffi::ErrorCode::ReadOnly,
                    sqlite::ffi::SQLITE_READONLY_RECOVERY,
                    "readonly recovery extended code without diagnostic-token dependency",
                ),
            ),
            &path,
        );
    }

    /// AGE-160 risk: PP-001 push-pull + A2 owned SHM sidecar probe evidence.
    /// Selected level: unit.
    /// Source: Phase 8 PR-review remediation; validates the `shm_exists` probe path.
    #[test]
    fn age160_classify_read_only_open_error_via_typed_projection_shm_sidecar_probe_path_branch() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.db");
        std::fs::write(&path, b"placeholder").unwrap();
        std::fs::write(shm_path(&path), b"owned shm sidecar").unwrap();

        assert!(
            !wal_path(&path).exists(),
            "fixture should exercise only the shm sidecar probe branch"
        );
        age160_assert_wal_sidecar(
            classify_read_only_open_error(
                &path,
                age160_sqlite_failure(
                    sqlite::ffi::ErrorCode::SystemIoFailure,
                    sqlite::ffi::ErrorCode::SystemIoFailure as i32,
                    "plain io failure text intentionally lacks sidecar tokens",
                ),
            ),
            &path,
        );
    }

    /// AGE-160 risk: PP-001 push-pull + A2 SHM extended-code projection.
    /// Selected level: unit.
    /// Source: the AGE-160 proposal § Test-intent track; validates A2/A3.
    #[test]
    fn age160_classify_read_only_open_error_via_typed_projection_shm_sidecar_extended_codes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.db");

        for extended_code in [
            sqlite::ffi::SQLITE_IOERR_SHMOPEN,
            sqlite::ffi::SQLITE_IOERR_SHMSIZE,
            sqlite::ffi::SQLITE_IOERR_SHMLOCK,
            sqlite::ffi::SQLITE_IOERR_SHMMAP,
        ] {
            age160_assert_wal_sidecar(
                classify_read_only_open_error(
                    &path,
                    age160_sqlite_failure(
                        sqlite::ffi::ErrorCode::SystemIoFailure,
                        extended_code,
                        "typed SHM sidecar evidence; message intentionally generic",
                    ),
                ),
                &path,
            );
        }
    }

    /// AGE-160 risk: A6 db.rs↔SQLite namespace contraction.
    /// Selected level: unit + compile.
    /// Source: the AGE-160 proposal § Test-intent track; validates A7.
    ///
    #[test]
    fn age160_sqlite_adapter_read_only_projection_and_namespace_contract() {
        use crate::db::sqlite_adapter::{
            Connection as AdapterConnection, OpenFlags as AdapterOpenFlags,
            OptionalExtension as AdapterOptionalExtension, ReadOnlyOpenFailure, Row as AdapterRow,
            SidecarProbe, SqliteFailureProjection, Statement as AdapterStatement,
            Transaction as AdapterTransaction, params as adapter_params,
        };

        fn _accept_row(_: &AdapterRow<'_>) {}
        fn _accept_statement(_: &mut AdapterStatement<'_>) {}
        fn _accept_transaction(_: &AdapterTransaction<'_>) {}
        fn _accept_optional<T: AdapterOptionalExtension>(value: T) -> T {
            value
        }

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.db");
        let conn = AdapterConnection::open_with_flags(
            &path,
            AdapterOpenFlags::SQLITE_OPEN_READ_WRITE | AdapterOpenFlags::SQLITE_OPEN_CREATE,
        )
        .unwrap();
        conn.execute(
            "CREATE TABLE contract_probe (id INTEGER PRIMARY KEY)",
            adapter_params![],
        )
        .unwrap();

        let projection = SqliteFailureProjection::from(&age160_sqlite_failure(
            sqlite::ffi::ErrorCode::NotADatabase,
            sqlite::ffi::ErrorCode::NotADatabase as i32,
            "not db",
        ));
        assert!(matches!(
            ReadOnlyOpenFailure::from_projection(&path, projection, SidecarProbe::for_db(&path)),
            ReadOnlyOpenFailure::PlainDb { .. }
        ));
        let _ = _accept_optional(Ok::<Option<i64>, sqlite::Error>(Some(1)));
    }

    /// AGE-160 risk: PP-004 declared marker grammar.
    /// Selected level: unit.
    /// Source: the AGE-160 proposal § Test-intent track.
    #[test]
    fn age160_composite_invocation_id_declared_grammar_canonical_json_round_trip() {
        let known_uuid = Uuid::parse_str("7ad2916c-38dd-49e6-a1f7-3ef22766ff70").unwrap();
        let composite = CompositeInvocationId {
            source: "codex2".to_string(),
            id: known_uuid.to_string(),
        };

        let stderr_line = composite.stderr_line();
        assert!(stderr_line.starts_with("OULIPOLY_INVOCATION="));
        let payload = stderr_line
            .strip_prefix("OULIPOLY_INVOCATION=")
            .expect("stderr marker prefix");
        assert!(!payload.starts_with("OULIPOLY_INVOCATION="));
        assert_eq!(
            payload,
            r#"{"source":"codex2","id":"7ad2916c-38dd-49e6-a1f7-3ef22766ff70"}"#
        );

        let parsed = CompositeInvocationId::parse_env_value(payload).unwrap();
        assert_eq!(parsed.source, "codex2");
        assert_eq!(parsed.id.to_string(), known_uuid.to_string());

        let parent_env = serde_json::to_string(&composite).unwrap();
        assert!(!parent_env.starts_with("OULIPOLY_INVOCATION="));
        assert_eq!(
            CompositeInvocationId::parse_env_value(&parent_env)
                .unwrap()
                .id
                .to_string(),
            known_uuid.to_string()
        );
    }

    /// AGE-160 risk: PP-004 push-pull + A4 legacy compatibility grammar.
    /// Selected level: unit.
    /// Source: the AGE-160 proposal § Test-intent track; validates A4.
    #[test]
    fn age160_composite_invocation_id_declared_grammar_legacy_shell_mangled_compatibility() {
        let known_uuid = "7ad2916c-38dd-49e6-a1f7-3ef22766ff70";

        for payload in [
            format!("{{source:\"codex2\",id:\"{known_uuid}\",extra:\"ignored\"}}"),
            format!("{{source:'codex2',id:'{known_uuid}',extra:'ignored'}}"),
        ] {
            assert!(
                !payload.starts_with("OULIPOLY_INVOCATION="),
                "legacy compatibility payloads are raw payloads, not marker lines"
            );
            let parsed = CompositeInvocationId::parse_env_value(&payload).unwrap();
            assert_eq!(parsed.source, "codex2");
            assert_eq!(parsed.id.to_string(), known_uuid);
        }

        assert!(
            CompositeInvocationId::parse_env_value("{source:'codex2',id:'not-a-uuid'}").is_err()
        );
    }

    #[derive(Clone, Default)]
    struct Age160LifecycleSink {
        records: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    }

    impl LifecycleEventSink for Age160LifecycleSink {
        fn forward(&mut self, record: &serde_json::Value) {
            self.records.lock().unwrap().push(record.clone());
        }
    }

    fn age160_lifecycle_fixture() -> (
        StateDb,
        std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    ) {
        let records = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Age160LifecycleSink {
            records: records.clone(),
        };
        let db = StateDb::open_with_sink(Path::new(":memory:"), Box::new(sink)).unwrap();
        (db, records)
    }

    fn age160_invocation_start(uuid: &str) -> InvocationStart {
        InvocationStart {
            invocation_uuid: uuid.to_string(),
            model_name: "codex~high".to_string(),
            provider_name: "codex2".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        }
    }

    fn age160_lifecycle_records(
        records: &std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    ) -> Vec<serde_json::Value> {
        records.lock().unwrap().clone()
    }

    fn age160_record_keys(record: &serde_json::Value) -> Vec<&str> {
        record
            .as_object()
            .expect("record object")
            .keys()
            .map(String::as_str)
            .collect()
    }

    /// AGE-160 risk: A6 db.rs↔lifecycle_log facade narrowing.
    /// Selected level: unit + integration.
    /// Source: the AGE-160 proposal § Test-intent track; validates A6.
    #[test]
    fn age160_lifecycle_log_facade_start_finalize_session_capture_preserves_records() {
        let (db, sink) = age160_lifecycle_fixture();
        let invocation_uuid = "16000000-0000-4000-8000-000000000001";

        let row_id = db
            .start_invocation(&age160_invocation_start(invocation_uuid))
            .unwrap();
        db.update_session_capture(row_id, Some("session-age160"), "resumed")
            .unwrap();
        db.finalize_invocation(row_id, true, 0, None, Some("done"))
            .unwrap();

        let records = age160_lifecycle_records(&sink);
        assert_eq!(records.len(), 3);
        assert_eq!(records[0]["event_name"], "invocation.started");
        assert_eq!(records[1]["event_name"], "invocation.session_captured");
        assert_eq!(records[2]["event_name"], "invocation.finalized");

        assert_eq!(
            age160_record_keys(&records[0]),
            vec![
                "chain_id",
                "error_chain",
                "event_name",
                "invocation_row_id",
                "invocation_uuid",
                "latency_us",
                "model",
                "operation_result",
                "parent_invocation_uuid",
                "provider",
                "provider_source",
                "session_id",
            ]
        );
        assert_eq!(
            age160_record_keys(&records[1]),
            vec![
                "capture_method",
                "chain_id",
                "error_chain",
                "event_name",
                "invocation_row_id",
                "invocation_uuid",
                "latency_us",
                "marker_emitted",
                "operation_result",
                "provider_source",
                "resume_input_id",
                "session_id",
            ]
        );
        assert_eq!(
            age160_record_keys(&records[2]),
            vec![
                "chain_id",
                "error_category",
                "error_chain",
                "event_name",
                "exit_code",
                "invocation_row_id",
                "invocation_uuid",
                "latency_us",
                "operation_result",
                "provider_source",
                "raw_artifact_paths",
                "session_id",
                "terminal_reason",
                "terminal_status",
            ]
        );

        assert_eq!(records[0]["invocation_uuid"], invocation_uuid);
        assert_eq!(records[0]["operation_result"], "ok");
        assert_eq!(records[0]["invocation_row_id"], serde_json::json!(row_id));
        assert_eq!(records[1]["capture_method"], "resumed");
        assert_eq!(records[1]["marker_emitted"], true);
        assert_eq!(records[1]["resume_input_id"], "session-age160");
        assert_eq!(records[2]["terminal_status"], "success");
        assert_eq!(records[2]["exit_code"], 0);
        assert_eq!(records[2]["terminal_reason"], "done");
    }

    fn age160_direct_symbol_count(haystack: &str, needles: &[&str]) -> usize {
        needles
            .iter()
            .map(|needle| haystack.match_indices(needle).count())
            .sum()
    }

    /// AGE-160 risk: A6 MEDIUM dispositions for db.rs↔serde_json/schema/chrono.
    /// Selected level: unit.
    /// Source: the AGE-160 proposal § Test-intent track.
    #[test]
    fn age160_post_cleanup_a6_medium_rows_resolved_or_declared() {
        let db_rs = include_str!("db.rs");
        let serde_direct_symbols = age160_direct_symbol_count(
            db_rs,
            &[
                "serde_json::to_string",
                "serde_json::from_str",
                "serde_json::json!",
                "serde_json::to_vec",
                "serde_json::Value",
            ],
        );
        assert!(
            serde_direct_symbols < 12 || db_rs.contains("AGE-160 serde_json residual disposition"),
            "db.rs direct serde_json symbol count must fall below the A6 MEDIUM threshold or carry a local residual disposition; count={serde_direct_symbols}"
        );
        assert!(
            db_rs.contains("crate::schema")
                && db_rs.contains("AGE-160 intrinsic schema-version carrier"),
            "db.rs must declare crate::schema as the intrinsic StateDb schema-version carrier"
        );
        assert!(
            db_rs.contains("use chrono") && db_rs.contains("AGE-160 intrinsic timestamp carrier"),
            "db.rs must declare chrono as the intrinsic StateDb timestamp carrier"
        );
    }
}
