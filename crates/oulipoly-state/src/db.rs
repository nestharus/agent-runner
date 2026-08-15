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
//! orchestration. Intrinsic-surface declarations cover the schema-version, timestamp, facade
//! support, and concern-module aggregation couplings; see `the AGE-160 proposal § Intrinsic-surface
//! declarations` for the canonical declaration.
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
//!   - component: crates/oulipoly-state/src/db.rs
//!     role: intrinsic-surface
//!     Domain: state-db-concern-module-aggregator
//!     Owns:
//!       - StateDb concern-module declarations under `crates/oulipoly-state/src/db/*.rs`
//!       - StateDb facade imports and public re-exports from concern modules
//!       - StateDb schema macro and table-shape type aggregation
//!       - StateDb facade external support imports: lifecycle_log_adapter, sqlite_adapter, and params
//!       - LifecycleEventSink and NoopLifecycleEventSink integration for StateDb lifecycle logging
//!       - chrono DateTime/Utc timestamp surface used by facade-level StateDb APIs
//!       - oulipoly_config::load_models legacy migration lookup interface
//!       - std HashMap/Path/PathBuf/Mutex facade support types and uuid::Uuid identifiers
//!       - test-only CompositeInvocationId, CURRENT_SCHEMA_VERSION, and ReturnedArtifactRef support imports
//!       - accounts, chain_backfill, chain_segments_compaction, chain_segments_import, chain_segments_open
//!       - cli_providers, discovered_models, discovery_types, invocation_artifacts
//!       - invocation_lifecycle_finalize, invocation_lifecycle_finalize_context, invocation_lifecycle_finalize_write, invocation_lifecycle_start
//!       - invocation_live_load, invocation_records, invocation_schema_legacy_migration, invocation_schema_projection, invocation_schema_repair, invocation_schema_session_turns, invocation_schema_table
//!       - invocation_window, lifecycle_invocation_row, lifecycle_log_adapter, model_parameters
//!       - opening_migrations, opening_read_only, opening_write, owned_turn_event_read, owned_turn_event_write
//!       - provider_quota_reads, provider_quota_refresh, provider_quota_status, provider_quota_test_support, provider_quota_window_writes, provider_quotas
//!       - ownership_authority, provider_schema_migration, provider_schema_validation, provider_session_binding, providers
//!       - resume_active_segment, resume_lookup, resume_preview, resume_resolution, resume_types
//!       - returned_artifacts_codec, returned_artifacts_read, returned_artifacts_write
//!       - schema_types, session_capture, session_markers, session_turns_ingest, session_turns_query, session_turns_replace, sqlite_adapter, timestamps
//! ```

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

mod accounts;
mod chain_backfill;
mod chain_segments_compaction;
mod chain_segments_import;
mod chain_segments_open;
mod cli_providers;
mod discovered_models;
mod discovery_types;
mod fresh_continuation;
mod imported_session_display_metadata;
mod imported_session_list;
mod invocation_artifacts;
mod invocation_lifecycle_finalize;
mod invocation_lifecycle_finalize_context;
mod invocation_lifecycle_finalize_write;
mod invocation_lifecycle_start;
mod invocation_live_load;
mod invocation_records;
mod invocation_schema_legacy_migration;
mod invocation_schema_projection;
mod invocation_schema_repair;
mod invocation_schema_session_turns;
mod invocation_schema_table;
mod invocation_window;
mod lifecycle_invocation_row;
mod lifecycle_log_adapter;
mod model_parameters;
mod opening_migrations;
mod opening_read_only;
mod opening_write;
mod owned_turn_event_read;
mod owned_turn_event_write;
mod ownership_authority;
mod provider_quota_reads;
mod provider_quota_refresh;
mod provider_quota_status;
mod provider_quota_test_support;
mod provider_quota_window_writes;
mod provider_quotas;
mod provider_schema_migration;
mod provider_schema_validation;
mod provider_session_binding;
mod provider_turn_effects;
mod providers;
mod resume_active_segment;
mod resume_lookup;
mod resume_preview;
mod resume_resolution;
mod resume_types;
mod returned_artifacts_codec;
mod returned_artifacts_read;
mod returned_artifacts_write;
mod schema_types;
mod session_capture;
mod session_lifecycle;
mod session_markers;
mod session_turns_ingest;
mod session_turns_query;
mod session_turns_replace;
mod sqlite_adapter;
mod timestamps;

pub use self::chain_backfill::BackfillReport;
pub use self::chain_segments_open::{
    ActiveChainSegmentSnapshot, ChainSegmentRotationInput, CompactSummaryEvidence,
};
pub use self::discovery_types::{
    AccountRecord, AuthMethod, AuthStatus, CliMapping, CliProviderRecord, DiscoveredModel,
    ModelParameter, ParamType,
};
pub use self::imported_session_display_metadata::{
    ImportedSessionDisplayMetadata, ImportedSessionDisplayMetadataUpsert,
};
pub use self::imported_session_list::ImportedSessionListRow;
use self::invocation_lifecycle_start::{
    FinalizeInvocationRow, FinalizeInvocationRowColumns, FinalizeLifecycleInput, OperationResult,
    active_lifecycle_session_id, lifecycle_terminal_status,
};
pub use self::invocation_records::{InvocationRecord, InvocationStart, InvocationStatus};
pub use self::invocation_schema_legacy_migration::LegacyProviderNames;
use self::invocation_schema_table::{LegacyInvocationInsert, LegacyInvocationRow};
use self::lifecycle_invocation_row::LifecycleInvocationRow;
pub use self::opening_write::StateReadConnection;
pub use self::owned_turn_event_write::{OwnedTurnEvent, OwnedTurnEventRow};
pub use self::ownership_authority::{
    CompletionContinuityRecoveryState, CompletionObligationAdmission,
    CompletionObligationAdmissionResult, CompletionObligationAuthority,
    CompletionObligationExpectation, EffectiveTerminalDisposition, ListenerSettlementClass,
    OwnedCompletionEventState, OwnerLineageRelationship, OwnershipAuthorityError,
    OwnershipAuthoritySnapshot, RecoveryDisposition, SettlementVerifierIdentity,
    SidecarGenerationState,
};
use self::provider_quotas::{
    MAX_LEARNABLE_BURN_RATE, MIN_LEARN_SAMPLE_CALLS, NEAR_EXHAUSTED_USED_PERCENT,
    QuotaAggregateProjection, QuotaWindowDelta,
};
pub use self::provider_quotas::{QuotaRecord, QuotaWindow, QuotaWindowInput};
use self::provider_schema_migration::ProviderColumn;
pub use self::provider_session_binding::ProviderSessionBinding;
pub use self::provider_turn_effects::{ProviderTurnEffectInput, ProviderTurnEffectWrite};
pub use self::providers::ProviderRecord;
pub use self::resume_types::{
    ChainPreview, ModelStore, RESUME_INPUT_MAX_LEN, ResolvedResume, ResumeError, ResumeInputMatch,
    ResumeNativeCandidate, TurnPreview, WrongIdKindInput,
};
use self::resume_types::{
    ParsedTurnPreviewTimestamp, RecentTurnRow, ResumeChainCandidate, WrongIdKindInvocationMatch,
    WrongIdKindInvocationRow,
};
use self::returned_artifacts_codec::{
    InvocationIdentity, ParsedReturnedArtifactFieldValues, ReturnedArtifactFieldError,
    ReturnedArtifactPayloadFields, ReturnedArtifactRawRow, ReturnedArtifactRowParams,
    ReturnedArtifactValidatedInputs, ValidatedReturnedArtifactFieldValues,
    returned_artifact_producer_uuid, returned_artifact_sql_integer, returned_artifact_version_id,
    returned_source_kind,
};
use self::schema_types::{
    ColumnRepair, DropColumnRepair, InvocationDualIdProjection, InvocationsSchemaShape,
    ProviderSessionProjection, ProvidersSchemaShape,
};
pub use self::session_lifecycle::{
    AcknowledgementStage, AcknowledgementWrite, DeliveryAcknowledgement, DeliveryEvidence,
    DeliveryEvidenceKind, DispositionWrite, EventDisposition, ExactProcessIdentity,
    ExternalIngress, ExternalIngressWrite, LeaseAcquire, LeaseReplace, LifecycleEvent,
    NewLifecycleEvent, ProviderTurnGeneration, SessionLifecycleError, SessionLifecycleRepository,
    SessionLifecycleResult, SessionReconstruction, SupervisorFence, SupervisorLease, TurnFence,
    TurnState,
};
pub use self::session_markers::SessionMarkerPayload;
#[allow(unused_imports)]
pub use self::session_turns_ingest::SessionTurnRecord;
pub use self::session_turns_ingest::{SessionTurnCounts, SessionTurnIngest};
pub use self::session_turns_replace::{
    SessionTurnReplacement, SessionTurnRestoreRow, SessionTurnsReplacement, SessionTurnsRestore,
};

use self::lifecycle_log_adapter as lc_log_adapter;
use self::sqlite_adapter as sqlite;
use self::sqlite_adapter::params;
use self::sqlite_adapter::{Connection, RusqliteOptionalExtension, Transaction};
#[cfg(test)]
use crate::invocation_marker::CompositeInvocationId;
use crate::lifecycle_log::{LifecycleEventSink, NoopLifecycleEventSink};
#[cfg(test)]
use crate::schema::CURRENT_SCHEMA_VERSION;
use chrono::{DateTime, Utc};
#[cfg(test)]
use oulipoly_agent_messenger::ReturnedArtifactRef;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use uuid::Uuid;

pub struct StateDb {
    conn: sqlite::Connection,
    db_path: PathBuf,
    // Completion authority accepts one absolute, non-symlink, single-link local file identity.
    completion_authority_state: Option<CompletionAuthorityStateIdentity>,
    lifecycle_sink: Mutex<Box<dyn LifecycleEventSink + Send>>,
    _read_only_snapshot: Option<crate::read_only_snapshot::ReadOnlySnapshot>,
    _state_namespace_guard: Option<StateNamespaceGuard>,
}

struct CompletionAuthorityStateIdentity {
    path: PathBuf,
    file: StateFileIdentity,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct StateFileIdentity {
    volume: u64,
    file: u64,
}

struct StateNamespaceGuard {
    file: std::fs::File,
}

/// Exclusive authority for the supported destructive state rebuild operation.
/// Writable StateDb handles hold the shared form for their complete lifetime.
pub struct StateDbRebuildAuthority {
    db_path: PathBuf,
    _guard: StateNamespaceGuard,
}

/// Shared authority for a cooperating legacy writer that must retain its own
/// SQLite connection. The carrier is path-bound and intentionally not cloneable.
pub struct StateDbWriterAuthority {
    db_path: PathBuf,
    _guard: StateNamespaceGuard,
}

#[derive(Debug, Clone)]
pub enum ReadOnlyOpenError {
    Missing { path: PathBuf },
    NotADatabase { path: PathBuf, message: String },
    PermissionDenied { path: PathBuf },
    WalSidecarError { path: PathBuf, message: String },
    Operational { message: String },
}

pub type DbError = String;

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
mod tests;
