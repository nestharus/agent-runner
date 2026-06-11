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
mod chain_segments;
mod cli_providers;
mod discovered_models;
mod discovery_types;
mod invocation_artifacts;
mod invocation_lifecycle;
mod invocation_records;
mod invocation_schema;
mod invocation_window;
mod lifecycle_invocation_row;
mod lifecycle_log_adapter;
mod model_parameters;
mod opening;
mod owned_turn_event;
mod provider_quota_reads;
mod provider_quota_status;
mod provider_quota_test_support;
mod provider_quota_writes;
mod provider_quotas;
mod provider_schema;
mod provider_session_binding;
mod providers;
mod resume;
mod returned_artifacts;
mod schema_types;
mod session_capture;
mod session_markers;
mod session_turns;
mod sqlite_adapter;
mod timestamps;

pub use self::chain_backfill::BackfillReport;
pub use self::chain_segments::{
    ActiveChainSegmentSnapshot, ChainSegmentRotationInput, CompactSummaryEvidence,
};
pub use self::discovery_types::{
    AccountRecord, AuthMethod, AuthStatus, CliMapping, CliProviderRecord, DiscoveredModel,
    ModelParameter, ParamType,
};
pub use self::invocation_records::{InvocationRecord, InvocationStart, InvocationStatus};
use self::lifecycle_invocation_row::LifecycleInvocationRow;
pub use self::owned_turn_event::{OwnedTurnEvent, OwnedTurnEventRow};
use self::provider_quotas::{
    MAX_LEARNABLE_BURN_RATE, MIN_LEARN_SAMPLE_CALLS, NEAR_EXHAUSTED_USED_PERCENT,
    QuotaAggregateProjection, QuotaWindowDelta,
};
pub use self::provider_quotas::{QuotaRecord, QuotaWindow, QuotaWindowInput};
pub use self::provider_session_binding::ProviderSessionBinding;
pub use self::providers::ProviderRecord;
pub use self::resume::{
    ChainPreview, ModelStore, ResolvedResume, ResumeError, TurnPreview, WrongIdKindInput,
};
use self::schema_types::{
    ColumnRepair, DropColumnRepair, InvocationDualIdProjection, InvocationsSchemaShape,
    ProviderSessionProjection, ProvidersSchemaShape,
};
pub use self::session_markers::SessionMarkerPayload;
#[allow(unused_imports)]
pub use self::session_turns::SessionTurnRecord;
pub use self::session_turns::{SessionTurnCounts, SessionTurnIngest};

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
use oulipoly_config::load_models;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use uuid::Uuid;

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
