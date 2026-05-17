use crate::migrations;
use crate::schema::{
    CURRENT_SCHEMA_VERSION, MINIMUM_SUPPORTED_SCHEMA_VERSION, SchemaCompatibility,
};
use chrono::{DateTime, Utc};
use oulipoly_agent_messenger::ReturnedArtifactRef;
use oulipoly_config::{ModelConfig, load_models};
use oulipoly_core::TransitionReason;
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Ceiling on the per-turn burn rate that `upsert_quota_refresh` is willing
/// to learn from a single refresh-to-refresh sample. A transient upstream
/// spike (observed on the ChatGPT usage endpoint: `used_percent` briefly
/// reported as 1.0 before the window reset) paired with a small turn count
/// produced a learned rate of ~0.05/turn that then got carried forward across
/// subsequent no-change refreshes, projecting every provider near the ceiling
/// and making the whole pool look unusable. The highest plausible real
/// rate observed in live data is ~5e-4/turn on a 5h Claude window; 0.1/turn
/// is a 200× safety margin that still filters the spike case.
const MAX_LEARNABLE_BURN_RATE: f64 = 0.1;

/// Minimum assistant-turn sample size before a refresh-to-refresh delta is
/// accepted as a burn-rate learn. Below this, a 1%-on-6-turns observation
/// extrapolates to rates that are dominated by sample noise — when the
/// rate is then multiplied by `turns_since_refresh` at scoring time, a
/// 65%-used window can project to 97% on nothing but measurement error,
/// making the provider look nearly exhausted. Live-caught 2026-04-21 on `claude2` with
/// `last_delta_percent=0.01 / last_delta_calls=6` → projected
/// 0.65 + 193×0.00167 = 0.972, blocking the whole claude-opus pool. 20
/// turns is the empirical floor where per-turn rates stabilize to within
/// ~2× of the long-run mean across observed Claude/Codex samples.
const MIN_LEARN_SAMPLE_CALLS: u64 = 20;

/// Refuse to learn a burn rate from a sample where the window's
/// `used_percent` is already near its ceiling. A 100%-reading window did
/// not fill at a natural rate during the prior inter-refresh interval — it
/// hit a wall at some unknown point during that window and stayed pinned.
/// The dp/dc ratio from that interval is an artifact of the cap, not a
/// physical rate. Live-caught 2026-04-21 on `codex2` after a transient
/// ChatGPT upstream spike reported `used_percent=1.0` on the 7-day
/// window: learned rate became 1.0/34 ≈ 0.029/turn on WEEKLY (where real
/// rates live near 6e-5/turn), projecting every subsequent invocation
/// near the ceiling on nothing but a bad sample. User intuition:
/// "turns barely budge weekly" — so any single sample imputing a weekly
/// move > 1 point is suspect, and the cleanest marker of "suspect" is
/// "the sample is at the rail." Matching ceiling from score_by_density.
const NEAR_EXHAUSTED_USED_PERCENT: f64 = 0.99;

pub struct StateDb {
    conn: Connection,
    db_path: PathBuf,
}

#[derive(Debug, Clone)]
pub enum ReadOnlyOpenError {
    Missing { path: PathBuf },
    NotADatabase { path: PathBuf, message: String },
    PermissionDenied { path: PathBuf },
    WalSidecarError { path: PathBuf, message: String },
    Operational { message: String },
}

fn classify_read_only_open_error(path: &Path, err: rusqlite::Error) -> ReadOnlyOpenError {
    let message = err.to_string();
    match &err {
        rusqlite::Error::SqliteFailure(error, _) => match error.code {
            ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt => {
                ReadOnlyOpenError::NotADatabase {
                    path: path.to_path_buf(),
                    message,
                }
            }
            ErrorCode::PermissionDenied => ReadOnlyOpenError::PermissionDenied {
                path: path.to_path_buf(),
            },
            ErrorCode::SystemIoFailure
                if message.contains("-wal")
                    || message.contains("-shm")
                    || message.to_ascii_lowercase().contains("wal")
                    || message.to_ascii_lowercase().contains("shared memory") =>
            {
                ReadOnlyOpenError::WalSidecarError {
                    path: path.to_path_buf(),
                    message,
                }
            }
            _ => ReadOnlyOpenError::Operational { message },
        },
        _ => ReadOnlyOpenError::Operational { message },
    }
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

/// Per-provider (account) metadata. Keyed on provider name (e.g. `claude`,
/// `claude2`), which spans every model routed through that account.
/// The actual quota numbers live in `provider_quota_windows` — one row per
/// rolling window the CLI exposes (e.g. 5-hour + 7-day).
#[derive(Debug, Clone)]
pub struct QuotaRecord {
    pub provider_name: String,
    /// Calls recorded against this provider since the last refresh.
    pub calls_since_refresh: u64,
    pub refreshed_at: Option<DateTime<Utc>>,
    pub exhausted_at: Option<DateTime<Utc>>,
    pub topology_peak_live_window_count: usize,
    pub last_topology_probe_at: Option<DateTime<Utc>>,
}

/// One rolling-quota window reported by a provider's quota script.
/// `window_id` is a stable per-provider position index (window 0, 1, …)
/// so the same window survives across refreshes for delta-learning.
#[derive(Debug, Clone)]
pub struct QuotaWindow {
    pub provider_name: String,
    pub window_id: u32,
    /// 0..1 ratio. 0.23 = 23% of this window's budget consumed.
    pub used_percent: f64,
    pub resets_at: DateTime<Utc>,
    pub last_delta_percent: Option<f64>,
    pub last_delta_calls: Option<u64>,
}

/// Input to `upsert_quota_refresh` — one window's freshly-fetched values.
#[derive(Debug, Clone)]
pub struct QuotaWindowInput {
    pub used_percent: f64,
    pub resets_at: DateTime<Utc>,
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

fn returned_artifact_producer_uuid(workflow_run_id: &str) -> rusqlite::Result<Uuid> {
    let uuid_text = workflow_run_id.strip_prefix("return:").ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "returned artifact workflow_run_id is not in return namespace",
            )),
        )
    })?;
    Uuid::parse_str(uuid_text).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(err))
    })
}

fn returned_artifact_version_id(
    invocation_uuid: Uuid,
    artifact_name: &str,
    version: u64,
) -> String {
    let mut encoded_name = String::new();
    for byte in artifact_name.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded_name.push(byte as char);
        } else {
            encoded_name.push_str(&format!("%{byte:02X}"));
        }
    }
    format!("store://return/{invocation_uuid}/{encoded_name}/{version}")
}

fn returned_artifact_sql_integer(value: u64, field: &str) -> Result<i64, DbError> {
    i64::try_from(value)
        .map_err(|_| format!("Returned artifact {field} exceeds SQLite INTEGER range: {value}"))
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CompositeInvocationId {
    pub source: String,
    pub id: String,
}

impl CompositeInvocationId {
    /// Format as `OULIPOLY_INVOCATION=...` without a trailing newline.
    pub fn stderr_line(&self) -> String {
        format!(
            "OULIPOLY_INVOCATION={}",
            serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
        )
    }

    /// Parse from a raw env-var value and validate the UUID payload.
    pub fn parse_env_value(s: &str) -> Result<Self, String> {
        let parsed: CompositeInvocationId = serde_json::from_str(s)
            .or_else(|json_err| Self::parse_shell_mangled_env_value(s).ok_or(json_err))
            .map_err(|e| format!("Invalid invocation JSON: {e}"))?;
        Uuid::parse_str(&parsed.id).map_err(|e| format!("Invalid invocation UUID: {e}"))?;
        Ok(parsed)
    }

    fn parse_shell_mangled_env_value(s: &str) -> Option<Self> {
        let inner = s.trim().strip_prefix('{')?.strip_suffix('}')?;
        let mut source = None;
        let mut id = None;
        for part in inner.split(',') {
            let (key, value) = part.split_once(':')?;
            let value = value
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
            match key.trim() {
                "source" => source = Some(value),
                "id" => id = Some(value),
                _ => {}
            }
        }
        Some(Self {
            source: source?,
            id: id?,
        })
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
}

struct WrongIdKindInvocationMatch {
    invocation_uuid: String,
    provider_name: Option<String>,
    provider_session_id: Option<String>,
    chain_id: Option<String>,
}

// --- Model discovery entities ---

/// The type of a model parameter, stored as JSON in SQLite.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParamType {
    /// A parameter that accepts one of a fixed set of values.
    Enum { options: Vec<String> },
    /// A free-form string parameter.
    String,
    /// A numeric parameter with optional bounds.
    Number { min: Option<f64>, max: Option<f64> },
    /// A boolean flag parameter.
    Boolean,
}

/// How a parameter maps to CLI flags when invoking the model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CliMapping {
    /// The CLI flag, e.g. "--temperature" or "-m".
    pub flag: String,
    /// A template for the value, e.g. "{value}" or "model:{value}".
    pub value_template: String,
}

/// A model discovered from a CLI provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredModel {
    pub canonical_name: String,
    pub provider: String,
    pub discovered_at: String,
    pub cli_version: String,
}

/// A parameter for a discovered model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelParameter {
    pub name: String,
    pub display_name: String,
    pub param_type: ParamType,
    pub description: String,
    pub cli_mapping: CliMapping,
}

// --- Provider & Account entities (provider-accounts redesign) ---

/// How an account authenticates with its provider CLI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthMethod {
    /// CLI handles the OAuth flow (browser redirect, token exchange).
    OAuth,
    /// Authentication via an API key, stored in an env var or config file.
    ApiKey {
        env_var: String,
        config_path: Option<String>,
    },
    /// Authentication via a CLI-specific config file.
    ConfigFile { path: String },
}

impl AuthMethod {
    /// Serialize to a JSON string for SQLite storage.
    fn to_db_string(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| r#"{"type":"oauth"}"#.to_string())
    }

    /// Deserialize from a JSON string stored in SQLite.
    fn from_db_string(s: &str) -> Self {
        serde_json::from_str(s).unwrap_or(AuthMethod::OAuth)
    }
}

/// Whether the account's authentication credentials are currently valid.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthStatus {
    Valid,
    Expired,
    Unknown,
    NoAuth,
}

impl AuthStatus {
    fn as_str(&self) -> &'static str {
        match self {
            AuthStatus::Valid => "valid",
            AuthStatus::Expired => "expired",
            AuthStatus::Unknown => "unknown",
            AuthStatus::NoAuth => "no_auth",
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "valid" => AuthStatus::Valid,
            "expired" => AuthStatus::Expired,
            "no_auth" => AuthStatus::NoAuth,
            _ => AuthStatus::Unknown,
        }
    }
}

/// A CLI tool that can execute AI model requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliProviderRecord {
    pub cli_name: String,
    pub display_name: String,
    pub installed: bool,
    pub version: Option<String>,
    pub config_dir: Option<String>,
    pub last_synced: Option<String>,
}

/// An authenticated profile within a provider CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountRecord {
    pub id: String,
    pub provider: String,
    pub profile_name: String,
    pub auth_method: AuthMethod,
    pub auth_status: AuthStatus,
    pub created_at: String,
}

impl StateDb {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create state directory: {e}"))?;
        }

        let mut conn =
            Connection::open(path).map_err(|e| format!("Failed to open state DB: {e}"))?;

        let compatibility = migrations::classify(&conn)?;
        let ran_open_migrations = matches!(
            compatibility,
            SchemaCompatibility::Fresh
                | SchemaCompatibility::Migratable { .. }
                | SchemaCompatibility::LegacyVersionless
        );
        match compatibility {
            SchemaCompatibility::Fresh => {
                Self::set_wal_mode(&conn)?;
                let plan = migrations::current_plan_from(0).map_err(|e| e.to_string())?;
                migrations::run_with_db_path(&mut conn, &plan, path.to_path_buf())
                    .map_err(|e| e.to_string())?;
            }
            SchemaCompatibility::Current { .. } => {
                Self::set_wal_mode(&conn)?;
            }
            SchemaCompatibility::Migratable { stored } => {
                Self::set_wal_mode(&conn)?;
                let stored = Self::promote_existing_dual_id_schema5_if_present(&mut conn, stored)?;
                let plan = migrations::current_plan_from(stored).map_err(|e| e.to_string())?;
                migrations::run_with_db_path(&mut conn, &plan, path.to_path_buf())
                    .map_err(|e| e.to_string())?;
            }
            SchemaCompatibility::LegacyVersionless => {
                if migrations::classify_versionless(&conn)?.is_none() {
                    return Err(migrations::MigrationError::UnrecognizedShape {
                        db_path: path.to_path_buf(),
                    }
                    .to_string());
                }
                Self::set_wal_mode(&conn)?;
                let plan = migrations::current_plan_from(MINIMUM_SUPPORTED_SCHEMA_VERSION)
                    .map_err(|e| e.to_string())?;
                migrations::run_with_db_path(&mut conn, &plan, path.to_path_buf())
                    .map_err(|e| e.to_string())?;
            }
            SchemaCompatibility::Future { stored } => {
                return Err(migrations::MigrationError::Incompatible {
                    db_path: path.to_path_buf(),
                    stored,
                    current: CURRENT_SCHEMA_VERSION,
                }
                .to_string());
            }
            SchemaCompatibility::UnrecognizedVersionless => {
                return Err(migrations::MigrationError::UnrecognizedShape {
                    db_path: path.to_path_buf(),
                }
                .to_string());
            }
            SchemaCompatibility::Corrupt { reason } => {
                return Err(format!(
                    "Corrupt schema ({reason}); run `agents migrate --rebuild`. db={}",
                    path.display()
                ));
            }
        }

        Self::validate_providers_schema(&conn)?;
        Self::ensure_invocations_schema(&conn)?;
        Self::ensure_providers_schema(&mut conn)?;
        Self::ensure_provider_quotas_schema(&conn)?;
        Self::ensure_provider_quotas_topology_schema(&conn)?;
        Self::ensure_provider_quota_windows_schema(&conn)?;
        Self::ensure_session_turns_schema(&conn)?;
        if ran_open_migrations {
            conn.execute_batch(invocation_returned_artifacts_schema_sql!())
                .map_err(|e| format!("Failed to ensure returned-artifacts schema: {e}"))?;
        }
        let db = StateDb {
            conn,
            db_path: path.to_path_buf(),
        };
        db.backfill_session_chains()
            .map_err(|e| format!("{e}; run `agents migrate-db` first"))?;

        Ok(db)
    }

    pub fn open_read_only(path: &Path) -> Result<Self, ReadOnlyOpenError> {
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

        for sidecar in [wal_path(path), shm_path(path)] {
            if sidecar.exists() && path_is_unreadable(&sidecar) {
                return Err(ReadOnlyOpenError::WalSidecarError {
                    path: path.to_path_buf(),
                    message: format!("SQLite sidecar is not readable: {}", sidecar.display()),
                });
            }
        }

        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY;
        let conn = Connection::open_with_flags(path, flags)
            .map_err(|err| classify_read_only_open_error(path, err))?;

        conn.query_row("SELECT count(*) FROM sqlite_schema", [], |_row| Ok(()))
            .map_err(|err| classify_read_only_open_error(path, err))?;

        Ok(Self {
            conn,
            db_path: path.to_path_buf(),
        })
    }

    pub fn open_default() -> Result<Self, String> {
        let db_path = Self::default_path()?;
        Self::open(&db_path)
    }

    pub fn open_for_memory(path: impl AsRef<Path>) -> Result<Self, String> {
        Self::open(path.as_ref())
    }

    pub fn default_path() -> Result<PathBuf, String> {
        let data_dir =
            dirs::data_dir().ok_or_else(|| "Could not determine data directory".to_string())?;
        Ok(data_dir.join("oulipoly-agent-runner").join("state.db"))
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    fn set_wal_mode(conn: &Connection) -> Result<(), String> {
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

    // Legacy repair allow-list only. Durable schema changes belong in
    // crates/oulipoly-state/migrations/ and schema.rs owns the version.
    fn ensure_invocations_schema(conn: &Connection) -> Result<(), String> {
        let columns = Self::invocations_columns(conn)?;
        if columns.is_empty() {
            conn.execute_batch(Self::invocations_schema_sql())
                .map_err(|e| format!("Failed to initialize invocations schema: {e}"))?;
            Self::ensure_invocations_row_version_support(conn)?;
            return Ok(());
        }

        if columns.iter().any(|column| column == "invocation_uuid") {
            if !columns.iter().any(|column| column == "session_id") {
                conn.execute("ALTER TABLE invocations ADD COLUMN session_id TEXT", [])
                    .map_err(|e| format!("Failed to add invocations.session_id: {e}"))?;
            }
            if !columns
                .iter()
                .any(|column| column == "session_capture_method")
            {
                conn.execute(
                    "ALTER TABLE invocations ADD COLUMN session_capture_method TEXT",
                    [],
                )
                .map_err(|e| format!("Failed to add invocations.session_capture_method: {e}"))?;
            }
            if !columns
                .iter()
                .any(|column| column == "resume_acceptance_status")
            {
                conn.execute(
                    "ALTER TABLE invocations ADD COLUMN resume_acceptance_status TEXT",
                    [],
                )
                .map_err(|e| format!("Failed to add invocations.resume_acceptance_status: {e}"))?;
            }
            if !columns
                .iter()
                .any(|column| column == "resume_acceptance_evidence")
            {
                conn.execute(
                    "ALTER TABLE invocations ADD COLUMN resume_acceptance_evidence TEXT",
                    [],
                )
                .map_err(|e| {
                    format!("Failed to add invocations.resume_acceptance_evidence: {e}")
                })?;
            }
            if !columns.iter().any(|column| column == "terminal_reason") {
                match conn.execute(
                    "ALTER TABLE invocations ADD COLUMN terminal_reason TEXT",
                    [],
                ) {
                    Ok(_) => {}
                    Err(rusqlite::Error::SqliteFailure(_, message))
                        if message
                            .as_deref()
                            .is_some_and(|value| value.contains("duplicate column name")) => {}
                    Err(e) => {
                        return Err(format!("Failed to add invocations.terminal_reason: {e}"));
                    }
                }
            }
            if columns.iter().any(|column| column == "quota_tight_routing") {
                conn.execute(
                    "ALTER TABLE invocations DROP COLUMN quota_tight_routing",
                    [],
                )
                .map_err(|e| format!("Failed to drop invocations.quota_tight_routing: {e}"))?;
            }
            conn.execute_batch(Self::invocations_index_sql())
                .map_err(|e| format!("Failed to ensure invocation indexes: {e}"))?;
            Self::ensure_invocations_row_version_support(conn)?;
            return Ok(());
        }

        if Self::legacy_invocations_shape_is_pre_uuid(&columns) {
            return Self::migrate_legacy_invocations(conn);
        }

        Err(format!(
            "Refusing to rebuild populated invocations table with unrecognized pre-UUID shape: {columns:?}"
        ))
    }

    fn normalize_invocations_columns_excluding_maintenance(columns: &[String]) -> Vec<String> {
        let mut names = columns
            .iter()
            .filter(|column| column.as_str() != "row_version")
            .cloned()
            .collect::<Vec<_>>();
        names.sort();
        names
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

    fn ensure_invocations_row_version_support(conn: &Connection) -> Result<(), String> {
        let columns = Self::invocations_columns(conn)?;
        if !columns.iter().any(|column| column == "row_version") {
            conn.execute(
                "ALTER TABLE invocations ADD COLUMN row_version INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(|e| format!("Failed to add invocations.row_version during repair: {e}"))?;
        }
        let registration = crate::deployment::row_version::registry::lookup("invocations")
            .ok_or_else(|| {
                "Missing row-version registry entry for invocations during repair".to_string()
            })?;
        conn.execute_batch(
            &crate::deployment::row_version::triggers_sql::generate_triggers_for_table(
                registration,
            ),
        )
        .map_err(|e| format!("Failed to install invocation row-version triggers: {e}"))
    }

    fn invocations_columns(conn: &Connection) -> Result<Vec<String>, String> {
        let mut stmt = conn
            .prepare("PRAGMA table_info(invocations)")
            .map_err(|e| format!("Failed to inspect invocations schema: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| format!("Failed to inspect invocations columns: {e}"))?;

        let mut columns = Vec::new();
        for row in rows {
            columns.push(row.map_err(|e| format!("Failed to read invocations column: {e}"))?);
        }
        Ok(columns)
    }

    fn invocations_have_dual_id_columns(conn: &Connection) -> Result<bool, String> {
        let columns = Self::invocations_columns(conn)?;
        Ok(Self::columns_have_dual_id_columns(&columns))
    }

    fn columns_have_dual_id_columns(columns: &[String]) -> bool {
        columns.iter().any(|column| column == "provider_session_id")
            && columns.iter().any(|column| column == "resume_input_id")
            && columns
                .iter()
                .any(|column| column == "provider_session_capture_method")
    }

    fn promote_existing_dual_id_schema5_if_present(
        conn: &mut Connection,
        stored: i32,
    ) -> Result<i32, String> {
        if stored >= 5 {
            return Ok(stored);
        }
        let columns = Self::invocations_columns(conn)?;
        if !Self::columns_have_dual_id_columns(&columns) {
            return Ok(stored);
        }

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
        })?;
        Ok(5)
    }

    fn provider_session_expr(conn: &Connection, alias: Option<&str>) -> Result<String, String> {
        let prefix = alias.unwrap_or_default();
        if Self::invocations_have_dual_id_columns(conn)? {
            Ok(format!(
                "COALESCE({prefix}provider_session_id, {prefix}session_id)"
            ))
        } else {
            Ok(format!("{prefix}session_id"))
        }
    }

    fn invocation_record_select_sql(conn: &Connection, tail_sql: &str) -> Result<String, String> {
        let (provider_session_id, resume_input_id, provider_session_capture_method) =
            if Self::invocations_have_dual_id_columns(conn)? {
                (
                    "provider_session_id",
                    "resume_input_id",
                    "provider_session_capture_method",
                )
            } else {
                (
                    "NULL AS provider_session_id",
                    "NULL AS resume_input_id",
                    "NULL AS provider_session_capture_method",
                )
            };
        Ok(format!(
            "SELECT id, invocation_uuid, model_name, provider_name, provider_index,
                    parent_invocation_id, status, success, exit_code, error_category,
                    terminal_reason, session_id, session_capture_method,
                    {provider_session_id}, {resume_input_id}, {provider_session_capture_method},
                    resume_acceptance_status, resume_acceptance_evidence,
                    created_at, finished_at
             FROM invocations
             {tail_sql}"
        ))
    }

    fn ensure_providers_schema(conn: &mut Connection) -> Result<(), String> {
        let columns = Self::providers_columns(conn)?;
        if columns.is_empty() {
            conn.execute_batch(Self::providers_schema_sql())
                .map_err(|e| format!("Failed to initialize providers schema: {e}"))?;
            return Ok(());
        }

        if Self::providers_shape_is_post_fix(&columns) {
            return Ok(());
        }

        if Self::providers_shape_is_pre_fix(&columns) {
            let tx = conn
                .transaction()
                .map_err(|e| format!("Failed to begin providers migration: {e}"))?;
            tx.execute_batch("ALTER TABLE providers RENAME TO providers_legacy_index_keyed;")
                .map_err(|e| format!("Failed to rename legacy providers table: {e}"))?;
            tx.execute_batch(Self::providers_schema_sql())
                .map_err(|e| format!("Failed to create migrated providers table: {e}"))?;
            tx.execute_batch(
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
            .map_err(|e| format!("Failed to rebuild providers aggregate: {e}"))?;
            tx.execute_batch(
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
            .map_err(|e| format!("Failed to rebuild provider error metadata: {e}"))?;
            tx.execute_batch("DROP TABLE providers_legacy_index_keyed;")
                .map_err(|e| format!("Failed to drop legacy providers table: {e}"))?;
            tx.commit()
                .map_err(|e| format!("Failed to commit providers migration: {e}"))?;
            return Ok(());
        }

        Err(format!(
            "Unexpected providers schema shape: {}",
            Self::describe_columns(&columns)
        ))
    }

    fn validate_providers_schema(conn: &Connection) -> Result<(), String> {
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

    fn providers_object_type(conn: &Connection) -> Result<Option<String>, String> {
        conn.query_row(
            "SELECT type FROM sqlite_master WHERE name = 'providers'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| format!("Failed to inspect providers object type: {e}"))
    }

    fn providers_has_foreign_keys(conn: &Connection) -> Result<bool, String> {
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

    fn providers_columns(conn: &Connection) -> Result<Vec<ProviderColumn>, String> {
        let mut stmt = conn
            .prepare("PRAGMA table_info(providers)")
            .map_err(|e| format!("Failed to inspect providers schema: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ProviderColumn {
                    name: row.get(1)?,
                    data_type: row.get(2)?,
                    notnull: row.get(3)?,
                    pk: row.get(5)?,
                })
            })
            .map_err(|e| format!("Failed to inspect providers columns: {e}"))?;

        let mut columns = Vec::new();
        for row in rows {
            columns.push(row.map_err(|e| format!("Failed to read providers column: {e}"))?);
        }
        Ok(columns)
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
        columns
            .iter()
            .map(|column| {
                format!(
                    "{}(type={}, notnull={}, pk={})",
                    column.name, column.data_type, column.notnull, column.pk
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn ensure_session_turns_schema(conn: &Connection) -> Result<(), String> {
        let columns = Self::session_turns_columns(conn)?;
        if !columns.iter().any(|column| column == "parent_turn_id") {
            conn.execute(
                "ALTER TABLE session_turns ADD COLUMN parent_turn_id TEXT",
                [],
            )
            .map_err(|e| format!("Failed to add session_turns.parent_turn_id: {e}"))?;
        }
        if !columns.iter().any(|column| column == "is_sidechain") {
            conn.execute(
                "ALTER TABLE session_turns ADD COLUMN is_sidechain INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(|e| format!("Failed to add session_turns.is_sidechain: {e}"))?;
        }
        if !columns
            .iter()
            .any(|column| column == "is_compaction_boundary")
        {
            conn.execute(
                "ALTER TABLE session_turns ADD COLUMN is_compaction_boundary INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(|e| format!("Failed to add session_turns.is_compaction_boundary: {e}"))?;
        }
        if !columns.iter().any(|column| column == "body") {
            conn.execute("ALTER TABLE session_turns ADD COLUMN body TEXT", [])
                .map_err(|e| format!("Failed to add session_turns.body: {e}"))?;
        }
        conn.execute_batch(Self::session_turns_index_sql())
            .map_err(|e| format!("Failed to ensure session_turns indexes: {e}"))?;
        Ok(())
    }

    fn session_turns_columns(conn: &Connection) -> Result<Vec<String>, String> {
        let mut stmt = conn
            .prepare("PRAGMA table_info(session_turns)")
            .map_err(|e| format!("Failed to inspect session_turns schema: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| format!("Failed to inspect session_turns columns: {e}"))?;

        let mut columns = Vec::new();
        for row in rows {
            columns.push(row.map_err(|e| format!("Failed to read session_turns column: {e}"))?);
        }
        Ok(columns)
    }

    fn ensure_provider_quotas_schema(conn: &Connection) -> Result<(), String> {
        let columns = Self::provider_quotas_columns(conn)?;
        if !columns
            .iter()
            .any(|column| column == "last_empty_refresh_at")
        {
            conn.execute(
                "ALTER TABLE provider_quotas ADD COLUMN last_empty_refresh_at TEXT",
                [],
            )
            .map_err(|e| format!("Failed to add provider_quotas.last_empty_refresh_at: {e}"))?;
        }
        if !columns.iter().any(|column| column == "exhausted_at") {
            conn.execute(
                "ALTER TABLE provider_quotas ADD COLUMN exhausted_at TEXT NULL",
                [],
            )
            .map_err(|e| format!("Failed to add provider_quotas.exhausted_at: {e}"))?;
        }
        if columns.iter().any(|column| column == "last_delta_percent") {
            conn.execute(
                "ALTER TABLE provider_quotas DROP COLUMN last_delta_percent",
                [],
            )
            .map_err(|e| format!("Failed to drop provider_quotas.last_delta_percent: {e}"))?;
        }
        if columns.iter().any(|column| column == "last_delta_calls") {
            conn.execute(
                "ALTER TABLE provider_quotas DROP COLUMN last_delta_calls",
                [],
            )
            .map_err(|e| format!("Failed to drop provider_quotas.last_delta_calls: {e}"))?;
        }
        Ok(())
    }

    fn ensure_provider_quotas_topology_schema(conn: &Connection) -> Result<(), String> {
        let columns = Self::provider_quotas_columns(conn)?;
        let added_peak = !columns
            .iter()
            .any(|column| column == "topology_peak_live_window_count");
        if added_peak {
            conn.execute(
                "ALTER TABLE provider_quotas
                 ADD COLUMN topology_peak_live_window_count INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(|e| {
                format!("Failed to add provider_quotas.topology_peak_live_window_count: {e}")
            })?;
        }
        if !columns
            .iter()
            .any(|column| column == "last_topology_probe_at")
        {
            conn.execute(
                "ALTER TABLE provider_quotas ADD COLUMN last_topology_probe_at TEXT",
                [],
            )
            .map_err(|e| format!("Failed to add provider_quotas.last_topology_probe_at: {e}"))?;
        }
        conn.execute(
            "UPDATE provider_quotas
             SET topology_peak_live_window_count = MAX(
                topology_peak_live_window_count,
                (
                    SELECT COUNT(*)
                    FROM provider_quota_windows
                    WHERE provider_quota_windows.provider_name = provider_quotas.provider_name
                )
             )",
            [],
        )
        .map_err(|e| format!("Failed to backfill provider_quotas topology peak counts: {e}"))?;
        Ok(())
    }

    fn ensure_provider_quota_windows_schema(conn: &Connection) -> Result<(), String> {
        let columns = Self::provider_quota_windows_columns(conn)?;
        if !columns.iter().any(|column| column == "last_delta_percent") {
            conn.execute(
                "ALTER TABLE provider_quota_windows ADD COLUMN last_delta_percent REAL NULL",
                [],
            )
            .map_err(|e| format!("Failed to add provider_quota_windows.last_delta_percent: {e}"))?;
        }
        if !columns.iter().any(|column| column == "last_delta_calls") {
            conn.execute(
                "ALTER TABLE provider_quota_windows ADD COLUMN last_delta_calls INTEGER NULL",
                [],
            )
            .map_err(|e| format!("Failed to add provider_quota_windows.last_delta_calls: {e}"))?;
        }
        Ok(())
    }

    fn provider_quotas_columns(conn: &Connection) -> Result<Vec<String>, String> {
        let mut stmt = conn
            .prepare("PRAGMA table_info(provider_quotas)")
            .map_err(|e| format!("Failed to inspect provider_quotas schema: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| format!("Failed to inspect provider_quotas columns: {e}"))?;

        let mut columns = Vec::new();
        for row in rows {
            columns.push(row.map_err(|e| format!("Failed to read provider_quotas column: {e}"))?);
        }
        Ok(columns)
    }

    fn provider_quota_windows_columns(conn: &Connection) -> Result<Vec<String>, String> {
        let mut stmt = conn
            .prepare("PRAGMA table_info(provider_quota_windows)")
            .map_err(|e| format!("Failed to inspect provider_quota_windows schema: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| format!("Failed to inspect provider_quota_windows columns: {e}"))?;

        let mut columns = Vec::new();
        for row in rows {
            columns.push(
                row.map_err(|e| format!("Failed to read provider_quota_windows column: {e}"))?,
            );
        }
        Ok(columns)
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

    fn migrate_legacy_invocations(conn: &Connection) -> Result<(), String> {
        #[derive(Debug)]
        struct LegacyInvocationRow {
            model_name: String,
            provider_index: i64,
            success: i64,
            exit_code: i64,
            error_category: Option<String>,
            created_at: String,
        }

        let provider_names = Self::provider_name_lookup()?;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to begin invocation migration: {e}"))?;
        Self::validate_providers_schema(&tx)?;
        let old_count: i64 = tx
            .query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
            .map_err(|e| format!("Failed to count legacy invocations before rebuild: {e}"))?;

        let mut old_rows = Vec::new();
        {
            let mut stmt = tx
                .prepare(
                    "SELECT model_name, provider_index, success, exit_code, error_category, created_at
                     FROM invocations
                     ORDER BY id",
                )
                .map_err(|e| format!("Failed to read legacy invocations: {e}"))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(LegacyInvocationRow {
                        model_name: row.get(0)?,
                        provider_index: row.get(1)?,
                        success: row.get(2)?,
                        exit_code: row.get(3)?,
                        error_category: row.get(4)?,
                        created_at: row.get(5)?,
                    })
                })
                .map_err(|e| format!("Failed to scan legacy invocations: {e}"))?;
            for row in rows {
                old_rows.push(row.map_err(|e| format!("Failed to parse legacy invocation: {e}"))?);
            }
        }
        if old_rows.len() as i64 != old_count {
            return Err(format!(
                "Legacy invocation rebuild aborted before replacement: scanned {} rows but table count was {old_count}",
                old_rows.len()
            ));
        }

        tx.execute_batch(
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
                resume_acceptance_status TEXT,
                resume_acceptance_evidence TEXT,
                created_at TEXT NOT NULL,
                finished_at TEXT,
                row_version INTEGER NOT NULL DEFAULT 0
            );",
        )
        .map_err(|e| format!("Failed to create migrated invocations table: {e}"))?;

        {
            let mut insert = tx
                .prepare(
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
                        resume_acceptance_status,
                        resume_acceptance_evidence,
                        created_at,
                        finished_at
                     ) VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?7, ?8, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, ?9, ?9)",
                )
                .map_err(|e| format!("Failed to prepare migrated invocation insert: {e}"))?;

            for row in old_rows {
                let provider_name = provider_names
                    .get(&(row.model_name.clone(), row.provider_index as usize))
                    .cloned();
                let status = match provider_name {
                    Some(_) if row.success != 0 => InvocationStatus::Succeeded,
                    Some(_) => InvocationStatus::Failed,
                    None => InvocationStatus::Legacy,
                };

                insert
                    .execute(params![
                        Uuid::new_v4().to_string(),
                        row.model_name,
                        provider_name,
                        row.provider_index,
                        status.as_str(),
                        row.success,
                        row.exit_code,
                        row.error_category,
                        row.created_at,
                    ])
                    .map_err(|e| format!("Failed to copy legacy invocation: {e}"))?;
            }
        }

        let new_count: i64 = tx
            .query_row("SELECT COUNT(*) FROM invocations_new", [], |row| row.get(0))
            .map_err(|e| format!("Failed to count migrated invocations before replacement: {e}"))?;
        if new_count != old_count {
            return Err(format!(
                "Legacy invocation rebuild aborted before replacement: migrated {new_count} rows from {old_count}"
            ));
        }

        tx.execute_batch(
            "DROP TABLE invocations;
             ALTER TABLE invocations_new RENAME TO invocations;",
        )
        .map_err(|e| format!("Failed to replace invocations table: {e}"))?;

        tx.execute_batch(Self::invocations_index_sql())
            .map_err(|e| format!("Failed to create migrated invocation indexes: {e}"))?;
        Self::ensure_invocations_row_version_support(&tx)?;

        tx.commit()
            .map_err(|e| format!("Failed to commit invocation migration: {e}"))
    }

    /// Resolve `(model_name, provider_index) -> provider_name` from the
    /// installed models config, used by the legacy-row migration. A corrupt
    /// or missing models directory must not block DB open: log on stderr and
    /// return an empty lookup so unmappable rows fall through to
    /// `status='legacy'` with `provider_name=NULL` (per V10 — degradation
    /// is observable via the legacy status, not silent).
    fn provider_name_lookup() -> Result<std::collections::HashMap<(String, usize), String>, String>
    {
        let models_dir = dirs::config_dir()
            .map(|dir| dir.join("oulipoly-agent-runner").join("models"))
            .unwrap_or_else(|| std::path::PathBuf::from("models"));
        let models = match load_models(&models_dir, None) {
            Ok(m) => m,
            Err(e) => {
                eprintln!(
                    "Warning: failed to load models config during invocation migration ({e}); \
                     pre-existing invocation rows will migrate as status='legacy'."
                );
                return Ok(std::collections::HashMap::new());
            }
        };
        let mut lookup = std::collections::HashMap::new();
        for (model_name, model) in models {
            for (provider_index, provider) in model.providers.iter().enumerate() {
                lookup.insert((model_name.clone(), provider_index), provider.name.clone());
            }
        }
        Ok(lookup)
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
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("create_dir_all({}): {e}", dir.display()))?;
        let payload = serde_json::json!({
            "id": start.invocation_uuid,
            "status": "running",
            "pid": std::process::id(),
            "started_at": started_at,
            "model_name": start.model_name,
            "provider_name": start.provider_name,
        });
        let bytes = serde_json::to_vec(&payload)
            .map_err(|e| format!("serialize invocation artifact: {e}"))?;
        let tmp_path = dir.join(format!("{}.invocation.tmp", start.invocation_uuid));
        let final_path = dir.join(format!("{}.invocation", start.invocation_uuid));
        std::fs::write(&tmp_path, &bytes)
            .map_err(|e| format!("write({}): {e}", tmp_path.display()))?;
        std::fs::rename(&tmp_path, &final_path).map_err(|e| {
            format!(
                "rename({} -> {}): {e}",
                tmp_path.display(),
                final_path.display()
            )
        })?;
        Ok(())
    }

    fn write_result_artifact(
        &self,
        uuid: &str,
        success: bool,
        exit_code: i32,
        error_category: Option<&str>,
        terminal_reason: Option<&str>,
        finished_at: &str,
    ) -> Result<(), String> {
        let Some(dir) = self.invocations_dir() else {
            return Ok(());
        };
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("create_dir_all({}): {e}", dir.display()))?;
        let payload = serde_json::json!({
            "id": uuid,
            "status": if success { "succeeded" } else { "failed" },
            "success": success,
            "exit_code": exit_code,
            "terminal_reason": terminal_reason,
            "error_category": error_category,
            "finished_at": finished_at,
        });
        let bytes =
            serde_json::to_vec(&payload).map_err(|e| format!("serialize result artifact: {e}"))?;
        let tmp_path = dir.join(format!("{uuid}.result.tmp"));
        let final_path = dir.join(format!("{uuid}.result"));
        std::fs::write(&tmp_path, &bytes)
            .map_err(|e| format!("write({}): {e}", tmp_path.display()))?;
        std::fs::rename(&tmp_path, &final_path).map_err(|e| {
            format!(
                "rename({} -> {}): {e}",
                tmp_path.display(),
                final_path.display()
            )
        })?;
        Ok(())
    }

    pub fn start_invocation(&self, start: &InvocationStart) -> Result<i64, String> {
        let now = Utc::now().to_rfc3339();
        self.conn
            .execute(
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
                params![
                    &start.invocation_uuid,
                    &start.model_name,
                    &start.provider_name,
                    start.provider_index as i64,
                    start.parent_invocation_id,
                    InvocationStatus::Running.as_str(),
                    &now,
                ],
            )
            .map_err(|e| format!("Failed to insert invocation: {e}"))?;
        let row_id = self.conn.last_insert_rowid();
        if let Err(err) = self.write_invocation_artifact(start, &now) {
            eprintln!(
                "Warning: Failed to write invocation artifact for {}: {err}",
                start.invocation_uuid
            );
        }
        Ok(row_id)
    }

    pub fn finalize_invocation(
        &self,
        id: i64,
        success: bool,
        exit_code: i32,
        error_category: Option<&str>,
        terminal_reason: Option<&str>,
    ) -> Result<(), String> {
        let now = Utc::now().to_rfc3339();
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to begin invocation finalize tx: {e}"))?;

        let (invocation_uuid, model_name, provider_name, _provider_index, status) = tx
            .query_row(
                "SELECT invocation_uuid, model_name, provider_name, provider_index, status
                 FROM invocations WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| format!("Failed to load invocation {id}: {e}"))?
            .ok_or_else(|| format!("Invocation {id} not found"))?;

        if status.parse::<InvocationStatus>().ok() != Some(InvocationStatus::Running) {
            return Err(format!("Invocation {id} is already finalized"));
        }

        let updated = tx
            .execute(
                "UPDATE invocations
             SET status = ?1,
                 success = ?2,
                 exit_code = ?3,
                 error_category = ?4,
                 terminal_reason = ?5,
                 finished_at = ?6
             WHERE id = ?7 AND status = ?8",
                params![
                    if success {
                        InvocationStatus::Succeeded.as_str()
                    } else {
                        InvocationStatus::Failed.as_str()
                    },
                    success as i64,
                    exit_code,
                    error_category,
                    terminal_reason,
                    &now,
                    id,
                    InvocationStatus::Running.as_str(),
                ],
            )
            .map_err(|e| format!("Failed to finalize invocation {id}: {e}"))?;
        if updated == 0 {
            return Err(format!("Invocation {id} is already finalized"));
        }

        if let Some(provider_name) = provider_name {
            tx.execute(
                "INSERT INTO providers (
                    model_name, provider_name,
                    invocation_count, error_count, last_invoked_at
                 ) VALUES (?1, ?2, 1, ?3, ?4)
                 ON CONFLICT (model_name, provider_name)
                 DO UPDATE SET
                    invocation_count = invocation_count + 1,
                    error_count = error_count + ?3,
                    last_invoked_at = ?4",
                params![
                    &model_name,
                    &provider_name,
                    if success { 0i64 } else { 1i64 },
                    &now
                ],
            )
            .map_err(|e| format!("Failed to upsert provider: {e}"))?;

            if !success {
                let snippet =
                    terminal_reason.map(|value| value.chars().take(500).collect::<String>());
                tx.execute(
                    "UPDATE providers SET last_error = ?1, last_error_at = ?2
                     WHERE model_name = ?3 AND provider_name = ?4",
                    params![&snippet, &now, &model_name, &provider_name],
                )
                .map_err(|e| format!("Failed to update error info: {e}"))?;
            }
        }

        tx.commit()
            .map_err(|e| format!("Failed to commit invocation finalize tx: {e}"))?;
        if let Err(err) = self.write_result_artifact(
            &invocation_uuid,
            success,
            exit_code,
            error_category,
            terminal_reason,
            &now,
        ) {
            eprintln!("Warning: Failed to write result artifact for {invocation_uuid}: {err}");
        }
        Ok(())
    }

    pub fn record_returned_artifacts(
        &self,
        invocation_row_id: i64,
        refs: &[ReturnedArtifactRef],
    ) -> Result<(), DbError> {
        self.conn
            .execute_batch(invocation_returned_artifacts_schema_sql!())
            .map_err(|e| format!("Failed to ensure returned-artifacts schema: {e}"))?;
        let invocation_uuid_text: String = self
            .conn
            .query_row(
                "SELECT invocation_uuid FROM invocations WHERE id = ?1",
                params![invocation_row_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to load invocation for returned artifacts: {e}"))?
            .ok_or_else(|| format!("Invocation {invocation_row_id} not found"))?;
        let invocation_uuid = Uuid::parse_str(&invocation_uuid_text)
            .map_err(|e| format!("Invalid invocation UUID on row {invocation_row_id}: {e}"))?;

        for reference in refs {
            let derived_uuid =
                returned_artifact_producer_uuid(&reference.store_address.workflow_run_id)
                    .map_err(|e| format!("Invalid returned-artifact workflow_run_id: {e}"))?;
            if derived_uuid != reference.producer_invocation_uuid {
                return Err(format!(
                    "Returned artifact producer UUID mismatch: workflow_run_id encodes {derived_uuid}, ref carries {}",
                    reference.producer_invocation_uuid
                ));
            }
            if reference.producer_invocation_uuid != invocation_uuid {
                return Err(format!(
                    "Returned artifact belongs to {}, but invocation row {invocation_row_id} is {invocation_uuid}",
                    reference.producer_invocation_uuid
                ));
            }
            let expected_version_id = returned_artifact_version_id(
                derived_uuid,
                &reference.store_address.artifact_name,
                reference.store_address.version,
            );
            if reference.version_id != expected_version_id {
                return Err(format!(
                    "Returned artifact version_id mismatch: expected {expected_version_id}, ref carries {}",
                    reference.version_id
                ));
            }
        }

        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to begin returned-artifacts tx: {e}"))?;
        tx.execute(
            "DELETE FROM invocation_returned_artifacts WHERE invocation_id = ?1",
            params![invocation_row_id],
        )
        .map_err(|e| format!("Failed to reset returned artifacts: {e}"))?;
        for (ordinal, reference) in refs.iter().enumerate() {
            let version =
                returned_artifact_sql_integer(reference.store_address.version, "version")?;
            let content_len = returned_artifact_sql_integer(reference.content_len, "content_len")?;
            let source_json = serde_json::to_string(&reference.source)
                .map_err(|e| format!("Failed to encode returned-artifact source: {e}"))?;
            let source_kind = returned_source_kind(&reference.source);
            tx.execute(
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
                params![
                    invocation_row_id,
                    ordinal as i64,
                    &reference.version_id,
                    &reference.name,
                    &reference.store_address.workflow_run_id,
                    &reference.store_address.artifact_name,
                    version,
                    &reference.sha256,
                    content_len,
                    &reference.format_hint,
                    &reference.verdict_line,
                    source_kind,
                    source_json,
                    reference.returned_at.to_rfc3339(),
                ],
            )
            .map_err(|e| format!("Failed to record returned artifact: {e}"))?;
        }
        tx.commit()
            .map_err(|e| format!("Failed to commit returned-artifacts tx: {e}"))
    }

    pub fn list_returned_artifacts(
        &self,
        invocation_row_id: i64,
    ) -> Result<Vec<ReturnedArtifactRef>, DbError> {
        let object_type = self
            .conn
            .query_row(
                "SELECT type
                 FROM sqlite_master
                 WHERE name = 'invocation_returned_artifacts'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| format!("Failed to inspect returned-artifacts schema: {e}"))?;
        match object_type.as_deref() {
            None => return Ok(Vec::new()),
            Some("table") => {}
            Some(other) => {
                return Err(format!(
                    "Unexpected returned-artifacts schema shape: object type={other}"
                ));
            }
        }
        let columns = self
            .conn
            .prepare("PRAGMA table_info(invocation_returned_artifacts)")
            .map_err(|e| format!("Failed to inspect returned-artifacts schema: {e}"))?
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| format!("Failed to query returned-artifacts columns: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read returned-artifacts column: {e}"))?;
        if !columns.iter().any(|column| column == "version_id") {
            return Ok(Vec::new());
        }

        let mut stmt = self
            .conn
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
            .query_map(params![invocation_row_id], |row| {
                let source_json: String = row.get(9)?;
                let workflow_run_id: String = row.get(2)?;
                let returned_at_text: String = row.get(10)?;
                let source = serde_json::from_str(&source_json).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        9,
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })?;
                let returned_at = DateTime::parse_from_rfc3339(&returned_at_text)
                    .map_err(|err| {
                        rusqlite::Error::FromSqlConversionFailure(
                            10,
                            rusqlite::types::Type::Text,
                            Box::new(err),
                        )
                    })?
                    .with_timezone(&Utc);
                let producer_invocation_uuid = returned_artifact_producer_uuid(&workflow_run_id)?;
                let version = u64::try_from(row.get::<_, i64>(4)?).map_err(|_| {
                    rusqlite::Error::FromSqlConversionFailure(
                        4,
                        rusqlite::types::Type::Integer,
                        Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "negative returned artifact version",
                        )),
                    )
                })?;
                let content_len = u64::try_from(row.get::<_, i64>(6)?).map_err(|_| {
                    rusqlite::Error::FromSqlConversionFailure(
                        6,
                        rusqlite::types::Type::Integer,
                        Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "negative returned artifact content_len",
                        )),
                    )
                })?;
                Ok(ReturnedArtifactRef {
                    version_id: row.get(0)?,
                    name: row.get(1)?,
                    store_address: oulipoly_agent_messenger::StoreAddress {
                        workflow_run_id,
                        artifact_name: row.get(3)?,
                        version,
                    },
                    sha256: row.get(5)?,
                    content_len,
                    format_hint: row.get(7)?,
                    verdict_line: row.get(8)?,
                    source,
                    producer_invocation_uuid,
                    returned_at,
                })
            })
            .map_err(|e| format!("Failed to query returned artifacts: {e}"))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read returned artifact row: {e}"))
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
        let (provider_session_id, resume_input_id, provider_session_capture_method) =
            if method == "resumed" {
                (None, session_id, None)
            } else {
                (session_id, None, session_id.map(|_| method))
            };
        self.conn
            .execute(
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
                params![
                    session_id,
                    method,
                    provider_session_id,
                    resume_input_id,
                    provider_session_capture_method,
                    id
                ],
            )
            .map_err(|e| format!("Failed to update session capture for invocation {id}: {e}"))?;
        Ok(())
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

        let existing = tx
            .query_row(
                "SELECT provider_session_id FROM invocations WHERE id = ?1",
                params![invocation_row_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|e| format!("Failed to read invocation {invocation_row_id}: {e}"))?
            .ok_or_else(|| format!("Invocation {invocation_row_id} not found"))?;

        if let Some(existing) = existing
            && existing != binding.provider_session_id
        {
            return Err(format!(
                "Invocation {invocation_row_id} is already bound to provider session {existing}; refusing to bind {}",
                binding.provider_session_id
            ));
        }

        tx.execute(
            "UPDATE invocations
             SET provider_session_id = ?1,
                 provider_session_capture_method = ?2,
                 resume_input_id = COALESCE(?3, resume_input_id),
                 session_id = CASE
                     WHEN session_capture_method = 'resumed'
                          AND resume_input_id IS NOT NULL
                          AND session_id = resume_input_id
                     THEN session_id
                     ELSE ?1
                 END,
                 session_capture_method = ?2
             WHERE id = ?4",
            params![
                &binding.provider_session_id,
                binding.capture_method,
                binding.resume_input_id.as_deref(),
                invocation_row_id
            ],
        )
        .map_err(|e| {
            format!("Failed to bind provider session for invocation {invocation_row_id}: {e}")
        })?;

        if binding.resume_input_id.as_deref() != Some(binding.provider_session_id.as_str()) {
            Self::mint_chain_for_invocation_session_on(&tx, invocation_row_id)?;
        }
        tx.commit()
            .map_err(|e| format!("Failed to commit provider session binding tx: {e}"))
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
                params![resume_input_id, id],
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
                params![status, evidence, id],
            )
            .map_err(|e| format!("Failed to update resume acceptance for invocation {id}: {e}"))?;
        Ok(())
    }

    pub fn mint_chain_for_invocation_session(&self, invocation_row_id: i64) -> Result<(), DbError> {
        Self::mint_chain_for_invocation_session_on(&self.conn, invocation_row_id)
    }

    fn mint_chain_for_invocation_session_on(
        conn: &Connection,
        invocation_row_id: i64,
    ) -> Result<(), DbError> {
        let provider_session_expr = Self::provider_session_expr(conn, None)?;
        let sql = format!(
            "SELECT model_name, provider_name, {provider_session_expr}, COALESCE(finished_at, created_at)
             FROM invocations
             WHERE id = ?1
               AND provider_name IS NOT NULL
               AND {provider_session_expr} IS NOT NULL"
        );
        let row = conn
            .query_row(&sql, params![invocation_row_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .optional()
            .map_err(|e| format!("Failed to read invocation for chain mint: {e}"))?;
        let Some((model_name, provider_name, session_id, raw_ts)) = row else {
            return Ok(());
        };
        let ts = DateTime::parse_from_rfc3339(&raw_ts)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        let exists = conn
            .query_row(
                "SELECT chain_id FROM session_chain_segments
                 WHERE provider_name = ?1 AND session_id = ?2
                 LIMIT 1",
                params![provider_name, session_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| format!("Failed to check existing invocation chain: {e}"))?;
        if let Some(chain_id) = exists {
            conn.execute(
                "UPDATE session_chains
                     SET model_name = ?2
                     WHERE chain_id = ?1 AND model_name = '<unknown>'",
                params![chain_id, model_name],
            )
            .map_err(|e| format!("Failed to update invocation session chain model: {e}"))?;
            conn.execute(
                "UPDATE session_chain_segments
                     SET transition_reason = 'initial'
                     WHERE chain_id = ?1
                       AND provider_name = ?2
                       AND session_id = ?3
                       AND transition_reason = 'imported'",
                params![chain_id, provider_name, session_id],
            )
            .map_err(|e| format!("Failed to promote imported session chain segment: {e}"))?;
            return Ok(());
        }
        let chain_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, ?2, ?2, ?3)",
            params![chain_id, ts.to_rfc3339(), model_name],
        )
        .map_err(|e| format!("Failed to mint invocation session chain: {e}"))?;
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, ?4, 'initial')",
            params![chain_id, provider_name, session_id, ts.to_rfc3339()],
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

        let result = stmt.query_row(params![uuid], Self::map_invocation_row);
        match result {
            Ok(record) => Ok(Some(record)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
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
            .query_map(params![parent_id], Self::map_invocation_row)
            .map_err(|e| format!("Failed to query invocation children: {e}"))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to map invocation children: {e}"))
    }

    fn map_invocation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<InvocationRecord> {
        let created_at_raw: String = row.get(18)?;
        let finished_at_raw: Option<String> = row.get(19)?;
        let status_raw: String = row.get(6)?;
        let created_at = DateTime::parse_from_rfc3339(&created_at_raw)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    18,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
        let finished_at = finished_at_raw
            .map(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .map(|dt| dt.with_timezone(&Utc))
                    .map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            19,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })
            })
            .transpose()?;
        let status = status_raw.parse::<InvocationStatus>().map_err(|_| {
            rusqlite::Error::FromSqlConversionFailure(
                6,
                rusqlite::types::Type::Text,
                format!("Unknown invocation status: {status_raw}").into(),
            )
        })?;

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
            resume_acceptance_status: row.get(16)?,
            resume_acceptance_evidence: row.get(17)?,
            created_at,
            finished_at,
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

        let result = stmt.query_row(params![model_name, provider_name], |row| {
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
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
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
                params![model_name, provider_name, &cutoff],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to count recent errors: {e}"))?;

        Ok(count)
    }

    // --- Provider quota operations ---

    /// Fetch provider-level quota metadata. Windows live in a separate
    /// table — use `get_windows` to get the actual quota numbers.
    pub fn get_quota(&self, provider_name: &str) -> Result<Option<QuotaRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT calls_since_refresh, refreshed_at, exhausted_at,
                        topology_peak_live_window_count, last_topology_probe_at
                 FROM provider_quotas WHERE provider_name = ?1",
            )
            .map_err(|e| format!("Failed to prepare quota query: {e}"))?;

        let result = stmt.query_row(params![provider_name], |row| {
            Ok(QuotaRecord {
                provider_name: provider_name.to_string(),
                calls_since_refresh: row.get::<_, i64>(0)? as u64,
                refreshed_at: row
                    .get::<_, Option<String>>(1)?
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                exhausted_at: row
                    .get::<_, Option<String>>(2)?
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                topology_peak_live_window_count: usize::try_from(row.get::<_, i64>(3)?).map_err(
                    |_| {
                        rusqlite::Error::FromSqlConversionFailure(
                            3,
                            rusqlite::types::Type::Integer,
                            "negative topology_peak_live_window_count".into(),
                        )
                    },
                )?,
                last_topology_probe_at: row
                    .get::<_, Option<String>>(4)?
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
            })
        });

        match result {
            Ok(record) => Ok(Some(record)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("Failed to query quota: {e}")),
        }
    }

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
                params![provider_name, &now],
            )
            .map_err(|e| format!("Failed to mark provider exhausted: {e}"))?;
        Ok(())
    }

    pub fn clear_exhausted(&self, provider_name: &str) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE provider_quotas SET exhausted_at = NULL WHERE provider_name = ?1",
                params![provider_name],
            )
            .map_err(|e| format!("Failed to clear provider exhausted flag: {e}"))?;
        Ok(())
    }

    /// Fetch every rolling-quota window a provider has reported, ordered by
    /// `window_id`. Empty vec if the provider has never been refreshed.
    pub fn get_windows(&self, provider_name: &str) -> Result<Vec<QuotaWindow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT window_id, used_percent, resets_at, last_delta_percent, last_delta_calls
                 FROM provider_quota_windows
                 WHERE provider_name = ?1
                 ORDER BY window_id",
            )
            .map_err(|e| format!("Failed to prepare windows query: {e}"))?;
        let rows = stmt
            .query_map(params![provider_name], |row| {
                let resets_at_str: String = row.get(2)?;
                let resets_at = DateTime::parse_from_rfc3339(&resets_at_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            2,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?;
                Ok(QuotaWindow {
                    provider_name: provider_name.to_string(),
                    window_id: row.get::<_, i64>(0)? as u32,
                    used_percent: row.get(1)?,
                    resets_at,
                    last_delta_percent: row.get(3)?,
                    last_delta_calls: row.get::<_, Option<i64>>(4)?.map(|v| v as u64),
                })
            })
            .map_err(|e| format!("Failed to query windows: {e}"))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| format!("Row error: {e}"))?);
        }
        Ok(out)
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
            // Empty input: preserve any prior windows (the DELETE below only
            // runs on the non-empty branch) and do not reset calls_since_refresh.
            //
            // When prior windows exist we must ALSO preserve prior.refreshed_at,
            // because the per-window delta learner (PR 3) computes
            //   delta_percent = new.used_percent - prior.used_percent
            //   delta_calls   = count_assistant_turns_since(prior.refreshed_at)
            // on the next successful refresh. Advancing refreshed_at here
            // would count only the turns since the empty refresh while still
            // measuring delta against the older preserved window sample,
            // inflating the learned burn rate. Only last_empty_refresh_at
            // advances — that is the audit timestamp the empty refresh is
            // observed through.
            //
            // When no prior windows exist, we need refreshed_at populated so
            // is_stale returns the expected values — but the §5.1 empty-
            // windows guard already forces stale on this shape, so whatever
            // we write is diagnostic only.
            if prior_windows.is_empty() {
                tx.execute(
                    "INSERT INTO provider_quotas
                        (provider_name, refreshed_at, last_empty_refresh_at)
                     VALUES (?1, ?2, ?2)
                     ON CONFLICT (provider_name) DO UPDATE SET
                        refreshed_at = ?2,
                        last_empty_refresh_at = ?2",
                    params![provider_name, &now],
                )
                .map_err(|e| format!("Failed to record empty quota refresh: {e}"))?;
            } else {
                tx.execute(
                    "INSERT INTO provider_quotas
                        (provider_name, refreshed_at, last_empty_refresh_at)
                     VALUES (?1, ?2, ?2)
                     ON CONFLICT (provider_name) DO UPDATE SET
                        last_empty_refresh_at = ?2",
                    params![provider_name, &now],
                )
                .map_err(|e| format!("Failed to record empty quota refresh: {e}"))?;
            }

            tx.commit()
                .map_err(|e| format!("Failed to commit refresh: {e}"))?;
            return Ok(());
        }

        let longest_new = windows.iter().max_by_key(|w| w.resets_at);
        let turns_between_refreshes = prior
            .as_ref()
            .map(|p| {
                self.count_assistant_turns_since(provider_name, p.refreshed_at.as_ref())
                    .unwrap_or(p.calls_since_refresh)
            })
            .unwrap_or(0);
        let prior_windows_by_id: std::collections::HashMap<u32, &QuotaWindow> = prior_windows
            .iter()
            .map(|window| (window.window_id, window))
            .collect();

        // Backwards-compat: keep used_percent/resets_at on provider_quotas in sync
        // with the longest window so legacy readers see something sensible.
        let (legacy_used, legacy_resets) = match longest_new {
            Some(w) => (w.used_percent, Some(w.resets_at.to_rfc3339())),
            None => (0.0, None),
        };
        let topology_peak_live_window_count = prior
            .as_ref()
            .map(|quota| quota.topology_peak_live_window_count)
            .unwrap_or(0)
            .max(windows.len()) as i64;

        tx.execute(
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
            params![
                provider_name,
                legacy_used,
                legacy_resets,
                &now,
                topology_peak_live_window_count
            ],
        )
        .map_err(|e| format!("Failed to upsert quota: {e}"))?;

        tx.execute(
            "DELETE FROM provider_quota_windows WHERE provider_name = ?1",
            params![provider_name],
        )
        .map_err(|e| format!("Failed to clear windows: {e}"))?;

        for (i, w) in windows.iter().enumerate() {
            let prior_window = prior_windows_by_id.get(&(i as u32)).copied();
            let (delta_percent, delta_calls) = match prior_window {
                Some(prior_w) => {
                    let dp = (w.used_percent - prior_w.used_percent).max(0.0);
                    // Three guards on what we trust from a single refresh-to-
                    // refresh sample. Any failure → carry the prior learn
                    // forward (same path as dp == 0 / window reset).
                    //  1. `turns_between_refreshes >= MIN_LEARN_SAMPLE_CALLS`
                    //     — small samples are noise-dominated.
                    //  2. `w.used_percent < NEAR_EXHAUSTED_USED_PERCENT`
                    //     — a window pinned at 100% did not fill at a
                    //     natural rate; its dp is a cap-hit artifact.
                    //  3. `new_rate <= MAX_LEARNABLE_BURN_RATE` —
                    //     belt-and-suspenders absolute rate ceiling for
                    //     samples that passed (1) and (2) but still
                    //     imply an implausibly fast consumption.
                    let small_sample = turns_between_refreshes < MIN_LEARN_SAMPLE_CALLS;
                    let near_rail = w.used_percent >= NEAR_EXHAUSTED_USED_PERCENT;
                    let new_rate = if turns_between_refreshes > 0 {
                        dp / (turns_between_refreshes as f64)
                    } else {
                        f64::INFINITY
                    };
                    let rate_too_high = new_rate > MAX_LEARNABLE_BURN_RATE;
                    if dp > 0.0 && !small_sample && !near_rail && !rate_too_high {
                        (Some(dp), Some(turns_between_refreshes))
                    } else {
                        (prior_w.last_delta_percent, prior_w.last_delta_calls)
                    }
                }
                None => (None, None),
            };
            tx.execute(
                "INSERT INTO provider_quota_windows
                    (provider_name, window_id, used_percent, resets_at,
                     last_delta_percent, last_delta_calls)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    provider_name,
                    i as i64,
                    w.used_percent,
                    w.resets_at.to_rfc3339(),
                    delta_percent,
                    delta_calls.map(|v| v as i64),
                ],
            )
            .map_err(|e| format!("Failed to insert window: {e}"))?;
        }

        tx.commit()
            .map_err(|e| format!("Failed to commit refresh: {e}"))?;
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
                params![provider_name, &now],
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
                params![refreshed_at.to_rfc3339(), provider_name],
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
        let updated = self
            .conn
            .execute(
                "UPDATE providers
                 SET last_invoked_at = ?1
                 WHERE model_name = ?2 AND provider_name = ?3",
                params![last_invoked_at.to_rfc3339(), model_name, provider_name],
            )
            .map_err(|e| format!("Failed to set last_invoked_at: {e}"))?;
        if updated != 1 {
            return Err(format!(
                "Expected exactly one providers row for model_name={model_name}, provider_name={provider_name}, updated {updated}"
            ));
        }
        Ok(())
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
                params![
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
                params![provider_name, refreshed_at.to_rfc3339()],
            )
            .map_err(|e| format!("Failed to insert quota row: {e}"))?;
        self.conn
            .execute(
                "DELETE FROM provider_quota_windows WHERE provider_name = ?1",
                params![provider_name],
            )
            .map_err(|e| format!("Failed to clear quota windows: {e}"))?;
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
                params![provider_name],
            )
            .map_err(|e| format!("Failed to increment calls_since_refresh: {e}"))?;
        Ok(())
    }

    // --- CLI Provider operations ---

    /// Insert or update a CLI provider record.
    pub fn upsert_cli_provider(&self, provider: &CliProviderRecord) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO cli_providers (cli_name, display_name, installed, version, config_dir, last_synced)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (cli_name)
                 DO UPDATE SET
                    display_name = ?2,
                    installed = ?3,
                    version = ?4,
                    config_dir = ?5,
                    last_synced = ?6",
                params![
                    &provider.cli_name,
                    &provider.display_name,
                    provider.installed as i64,
                    &provider.version,
                    &provider.config_dir,
                    &provider.last_synced,
                ],
            )
            .map_err(|e| format!("Failed to upsert CLI provider: {e}"))?;
        Ok(())
    }

    /// List all known CLI providers.
    pub fn list_cli_providers(&self) -> Result<Vec<CliProviderRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT cli_name, display_name, installed, version, config_dir, last_synced
                 FROM cli_providers ORDER BY cli_name",
            )
            .map_err(|e| format!("Failed to prepare query: {e}"))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(CliProviderRecord {
                    cli_name: row.get(0)?,
                    display_name: row.get(1)?,
                    installed: row.get::<_, i64>(2)? != 0,
                    version: row.get(3)?,
                    config_dir: row.get(4)?,
                    last_synced: row.get(5)?,
                })
            })
            .map_err(|e| format!("Failed to query CLI providers: {e}"))?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| format!("Failed to read provider row: {e}"))?);
        }
        Ok(result)
    }

    /// Get a single CLI provider by cli_name.
    pub fn get_cli_provider(&self, cli_name: &str) -> Result<Option<CliProviderRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT cli_name, display_name, installed, version, config_dir, last_synced
                 FROM cli_providers WHERE cli_name = ?1",
            )
            .map_err(|e| format!("Failed to prepare query: {e}"))?;

        let result = stmt.query_row(params![cli_name], |row| {
            Ok(CliProviderRecord {
                cli_name: row.get(0)?,
                display_name: row.get(1)?,
                installed: row.get::<_, i64>(2)? != 0,
                version: row.get(3)?,
                config_dir: row.get(4)?,
                last_synced: row.get(5)?,
            })
        });

        match result {
            Ok(record) => Ok(Some(record)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("Failed to query CLI provider: {e}")),
        }
    }

    // --- Account operations ---

    /// Insert a new account. Fails if (id, provider) already exists.
    pub fn insert_account(&self, account: &AccountRecord) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO accounts (id, provider, profile_name, auth_method, auth_status, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    &account.id,
                    &account.provider,
                    &account.profile_name,
                    &account.auth_method.to_db_string(),
                    account.auth_status.as_str(),
                    &account.created_at,
                ],
            )
            .map_err(|e| format!("Failed to insert account: {e}"))?;
        Ok(())
    }

    /// List all accounts, optionally filtered by provider.
    pub fn list_accounts(&self, provider: Option<&str>) -> Result<Vec<AccountRecord>, String> {
        let (sql, bind_provider);
        match provider {
            Some(p) => {
                sql = "SELECT id, provider, profile_name, auth_method, auth_status, created_at
                       FROM accounts WHERE provider = ?1 ORDER BY id";
                bind_provider = Some(p.to_string());
            }
            None => {
                sql = "SELECT id, provider, profile_name, auth_method, auth_status, created_at
                       FROM accounts ORDER BY provider, id";
                bind_provider = None;
            }
        }

        let mut stmt = self
            .conn
            .prepare(sql)
            .map_err(|e| format!("Failed to prepare query: {e}"))?;

        let rows = if let Some(ref p) = bind_provider {
            stmt.query_map(params![p], Self::map_account_row)
                .map_err(|e| format!("Failed to query accounts: {e}"))?
        } else {
            stmt.query_map([], Self::map_account_row)
                .map_err(|e| format!("Failed to query accounts: {e}"))?
        };

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| format!("Failed to read account row: {e}"))?);
        }
        Ok(result)
    }

    /// Delete an account by (id, provider).
    pub fn delete_account(&self, id: &str, provider: &str) -> Result<bool, String> {
        let changed = self
            .conn
            .execute(
                "DELETE FROM accounts WHERE id = ?1 AND provider = ?2",
                params![id, provider],
            )
            .map_err(|e| format!("Failed to delete account: {e}"))?;
        Ok(changed > 0)
    }

    // --- Discovered model operations ---

    /// Insert or update a discovered model.
    pub fn upsert_discovered_model(&self, model: &DiscoveredModel) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO discovered_models (canonical_name, provider, discovered_at, cli_version)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (canonical_name, provider)
                 DO UPDATE SET
                    discovered_at = ?3,
                    cli_version = ?4",
                params![
                    &model.canonical_name,
                    &model.provider,
                    &model.discovered_at,
                    &model.cli_version,
                ],
            )
            .map_err(|e| format!("Failed to upsert discovered model: {e}"))?;
        Ok(())
    }

    /// List discovered models, optionally filtered by provider.
    pub fn list_discovered_models(
        &self,
        provider: Option<&str>,
    ) -> Result<Vec<DiscoveredModel>, String> {
        let (sql, bind_provider);
        match provider {
            Some(p) => {
                sql = "SELECT canonical_name, provider, discovered_at, cli_version
                       FROM discovered_models WHERE provider = ?1
                       ORDER BY canonical_name";
                bind_provider = Some(p.to_string());
            }
            None => {
                sql = "SELECT canonical_name, provider, discovered_at, cli_version
                       FROM discovered_models
                       ORDER BY provider, canonical_name";
                bind_provider = None;
            }
        }

        let mut stmt = self
            .conn
            .prepare(sql)
            .map_err(|e| format!("Failed to prepare query: {e}"))?;

        let rows = if let Some(ref p) = bind_provider {
            stmt.query_map(params![p], Self::map_discovered_model_row)
                .map_err(|e| format!("Failed to query discovered models: {e}"))?
        } else {
            stmt.query_map([], Self::map_discovered_model_row)
                .map_err(|e| format!("Failed to query discovered models: {e}"))?
        };

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| format!("Failed to read model row: {e}"))?);
        }
        Ok(result)
    }

    /// Delete models for a provider that were discovered with an older CLI version.
    pub fn delete_stale_models(
        &self,
        provider: &str,
        current_cli_version: &str,
    ) -> Result<u64, String> {
        let changed = self
            .conn
            .execute(
                "DELETE FROM discovered_models
                 WHERE provider = ?1 AND cli_version != ?2",
                params![provider, current_cli_version],
            )
            .map_err(|e| format!("Failed to delete stale models: {e}"))?;
        Ok(changed as u64)
    }

    /// Insert or update a model parameter.
    pub fn upsert_model_parameter(
        &self,
        model_name: &str,
        provider: &str,
        param: &ModelParameter,
    ) -> Result<(), String> {
        let param_type_json = serde_json::to_string(&param.param_type)
            .map_err(|e| format!("Failed to serialize param_type: {e}"))?;
        let cli_mapping_json = serde_json::to_string(&param.cli_mapping)
            .map_err(|e| format!("Failed to serialize cli_mapping: {e}"))?;

        self.conn
            .execute(
                "INSERT INTO model_parameters (model_name, provider, name, display_name, param_type, description, cli_mapping)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (model_name, provider, name)
                 DO UPDATE SET
                    display_name = ?4,
                    param_type = ?5,
                    description = ?6,
                    cli_mapping = ?7",
                params![
                    model_name,
                    provider,
                    &param.name,
                    &param.display_name,
                    &param_type_json,
                    &param.description,
                    &cli_mapping_json,
                ],
            )
            .map_err(|e| format!("Failed to upsert model parameter: {e}"))?;
        Ok(())
    }

    /// List all parameters for a given model and provider.
    pub fn list_model_parameters(
        &self,
        model_name: &str,
        provider: &str,
    ) -> Result<Vec<ModelParameter>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT name, display_name, param_type, description, cli_mapping
                 FROM model_parameters
                 WHERE model_name = ?1 AND provider = ?2
                 ORDER BY name",
            )
            .map_err(|e| format!("Failed to prepare query: {e}"))?;

        let rows = stmt
            .query_map(params![model_name, provider], |row| {
                let param_type_str: String = row.get(2)?;
                let cli_mapping_str: String = row.get(4)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    param_type_str,
                    row.get::<_, String>(3)?,
                    cli_mapping_str,
                ))
            })
            .map_err(|e| format!("Failed to query model parameters: {e}"))?;

        let mut result = Vec::new();
        for row in rows {
            let (name, display_name, param_type_str, description, cli_mapping_str) =
                row.map_err(|e| format!("Failed to read parameter row: {e}"))?;

            let param_type: ParamType = serde_json::from_str(&param_type_str)
                .map_err(|e| format!("Failed to deserialize param_type: {e}"))?;
            let cli_mapping: CliMapping = serde_json::from_str(&cli_mapping_str)
                .map_err(|e| format!("Failed to deserialize cli_mapping: {e}"))?;

            result.push(ModelParameter {
                name,
                display_name,
                param_type,
                description,
                cli_mapping,
            });
        }
        Ok(result)
    }

    /// Helper: map a rusqlite row to a DiscoveredModel.
    fn map_discovered_model_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DiscoveredModel> {
        Ok(DiscoveredModel {
            canonical_name: row.get(0)?,
            provider: row.get(1)?,
            discovered_at: row.get(2)?,
            cli_version: row.get(3)?,
        })
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
                params![
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
        if turns.is_empty() {
            return Ok(0);
        }
        let now = Utc::now().to_rfc3339();
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to begin transaction: {e}"))?;
        let mut new_count: u64 = 0;
        {
            let mut stmt = tx
                .prepare(
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
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, '', ?9, ?10)",
                )
                .map_err(|e| format!("Failed to prepare batch insert: {e}"))?;
            for turn in turns {
                let n = stmt
                    .execute(params![
                        provider_name,
                        turn.session_id,
                        turn.turn_id,
                        turn.timestamp.to_rfc3339(),
                        turn.role,
                        turn.parent_turn_id,
                        if turn.is_sidechain { 1i64 } else { 0i64 },
                        if turn.is_compaction_boundary {
                            1i64
                        } else {
                            0i64
                        },
                        &now,
                        turn.body.as_deref(),
                    ])
                    .map_err(|e| format!("Batch insert row failed: {e}"))?;
                new_count += n as u64;
            }
        }
        tx.commit()
            .map_err(|e| format!("Failed to commit batch: {e}"))?;
        Ok(new_count)
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
                params![provider_name, session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|e| format!("Failed to count session turns for trace: {e}"))?;

        Ok(SessionTurnCounts {
            total: total.max(0) as u64,
            assistant: assistant.max(0) as u64,
            sidechain: sidechain.max(0) as u64,
        })
    }

    pub fn backfill_session_chains(&self) -> Result<BackfillReport, DbError> {
        let exists: i64 = self
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session_chains LIMIT 1)",
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to check session chain backfill state: {e}"))?;
        if exists != 0 {
            return Ok(BackfillReport {
                skipped_existing: true,
                chains_inserted: 0,
                segments_inserted: 0,
            });
        }

        #[derive(Debug)]
        struct Row {
            provider: String,
            session: String,
            started_at: String,
            last_used_at: String,
            last_turn_id: String,
        }

        let rows = {
            let mut stmt = self
                .conn
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
                .query_map([], |row| {
                    Ok(Row {
                        provider: row.get(0)?,
                        session: row.get(1)?,
                        started_at: row.get(2)?,
                        last_used_at: row.get(3)?,
                        last_turn_id: row.get(4)?,
                    })
                })
                .map_err(|e| format!("Failed to query session chain backfill rows: {e}"))?;
            iter.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to read session chain backfill rows: {e}"))?
        };

        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to begin session chain backfill: {e}"))?;
        let provider_session_expr = Self::provider_session_expr(&tx, None)?;
        let mut chains_inserted = 0;
        let mut segments_inserted = 0;
        for row in rows {
            let model_sql = format!(
                "SELECT model_name
                 FROM invocations
                 WHERE {provider_session_expr} = ?1
                 ORDER BY COALESCE(finished_at, created_at) DESC, id DESC
                 LIMIT 1"
            );
            let model_name = tx
                .query_row(&model_sql, params![row.session], |r| r.get::<_, String>(0))
                .optional()
                .map_err(|e| format!("Failed to infer model during backfill: {e}"))?
                .unwrap_or_else(|| "<unknown>".to_string());
            let chain_id = Uuid::new_v4().to_string();
            chains_inserted += tx
                .execute(
                    "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![chain_id, row.started_at, row.last_used_at, model_name],
                )
                .map_err(|e| format!("Failed to insert session chain during backfill: {e}"))?
                as u64;
            segments_inserted += tx
                .execute(
                    "INSERT INTO session_chain_segments
                        (chain_id, provider_name, session_id, started_at, ended_at, last_turn_id, transition_reason)
                     VALUES (?1, ?2, ?3, ?4, NULL, ?5, 'imported')",
                    params![chain_id, row.provider, row.session, row.started_at, row.last_turn_id],
                )
                .map_err(|e| format!("Failed to insert session chain segment during backfill: {e}"))?
                as u64;
        }
        tx.commit()
            .map_err(|e| format!("Failed to commit session chain backfill: {e}"))?;
        Ok(BackfillReport {
            skipped_existing: false,
            chains_inserted,
            segments_inserted,
        })
    }

    pub fn open_chain_segment(
        &self,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
        started_at: &DateTime<Utc>,
        reason: TransitionReason,
    ) -> Result<i64, DbError> {
        self.conn
            .execute(
                "INSERT INTO session_chain_segments
                    (chain_id, provider_name, session_id, started_at, transition_reason)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (chain_id, provider_name, session_id)
                 DO UPDATE SET
                    started_at = excluded.started_at,
                    ended_at = NULL,
                    last_turn_id = NULL,
                    transition_reason = excluded.transition_reason",
                params![
                    chain_id,
                    provider_name,
                    session_id,
                    started_at.to_rfc3339(),
                    reason.as_str()
                ],
            )
            .map_err(|e| format!("Failed to open session chain segment: {e}"))?;
        self.conn
            .query_row(
                "SELECT id FROM session_chain_segments
                 WHERE chain_id = ?1 AND provider_name = ?2 AND session_id = ?3
                 ORDER BY id DESC LIMIT 1",
                params![chain_id, provider_name, session_id],
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
                params![provider_name, session_id, own_chain_id],
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
        let exists: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM session_chain_segments
                 WHERE provider_name = ?1 AND session_id = ?2
                 LIMIT 1",
                params![provider_name, session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to check existing session chain segment: {e}"))?;
        if exists.is_some() {
            return Ok(());
        }
        let chain_id = Uuid::new_v4().to_string();
        let ts = started_at.to_rfc3339();
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to begin imported chain mint: {e}"))?;
        tx.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, ?2, ?2, ?3)
             ON CONFLICT DO NOTHING",
            params![chain_id, ts, model_name],
        )
        .map_err(|e| format!("Failed to mint imported session chain: {e}"))?;
        tx.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, ?4, 'imported')
             ON CONFLICT DO NOTHING",
            params![chain_id, provider_name, session_id, ts],
        )
        .map_err(|e| format!("Failed to mint imported session chain segment: {e}"))?;
        tx.commit()
            .map_err(|e| format!("Failed to commit imported chain mint: {e}"))?;
        Ok(())
    }

    pub fn close_active_segment_returning(
        &self,
        chain_id: &str,
        ended_at: &DateTime<Utc>,
    ) -> Result<Option<i64>, DbError> {
        self.conn
            .query_row(
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
                 WHERE chain_id = ?1 AND ended_at IS NULL
                 RETURNING id",
                params![chain_id, ended_at.to_rfc3339()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to close active session chain segment: {e}"))
    }

    pub fn update_chain_last_used(&self, chain_id: &str) -> Result<(), DbError> {
        self.conn
            .execute(
                "UPDATE session_chains SET last_used_at = ?2 WHERE chain_id = ?1",
                params![chain_id, Utc::now().to_rfc3339()],
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
                params![provider_name, session_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|e| format!("Failed to query latest compaction boundary: {e}"))?;
        row.map(|(turn_id, raw_ts)| {
            DateTime::parse_from_rfc3339(&raw_ts)
                .map(|dt| (turn_id, dt.with_timezone(&Utc)))
                .map_err(|e| format!("Bad compaction boundary timestamp {raw_ts}: {e}"))
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
                params![provider_name, session_id, turn_id],
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
        Uuid::try_parse(input).map_err(|_| ResumeError::InvalidUuid {
            input: input.to_string(),
        })?;

        if let Some(wrong_id) = self
            .wrong_id_kind_invocation_match(input)
            .map_err(|message| ResumeError::Db { message })?
        {
            return Err(ResumeError::WrongIdKind {
                input: input.to_string(),
                input_kind: WrongIdKindInput::AgentRunnerInvocationId,
                provider_session_id: wrong_id.provider_session_id,
                agent_runner_invocation_id: wrong_id.invocation_uuid,
                chain_id: wrong_id.chain_id,
                provider_name: wrong_id.provider_name,
            });
        }

        let chain_ids = self
            .candidate_chain_ids(input)
            .map_err(|message| ResumeError::Db { message })?;
        if chain_ids.is_empty() {
            return Err(ResumeError::NoChainFound {
                input: input.to_string(),
            });
        }
        let chain_id = self
            .choose_resume_chain(input, chain_ids)
            .map_err(|message| ResumeError::Db { message })?;

        let Some(chain_id) = chain_id else {
            let previews = self
                .chain_previews(input)
                .map_err(|message| ResumeError::Db { message })?;
            return Err(ResumeError::Ambiguous {
                input: input.to_string(),
                previews,
            });
        };

        let (active_provider, active_session_id) = self
            .active_segment_for_chain(&chain_id)
            .map_err(|message| ResumeError::Db { message })?
            .ok_or_else(|| ResumeError::ActiveSegmentMissing {
                chain_id: chain_id.clone(),
            })?;

        let model_name = if let Some(model_override) = model_override {
            Some(model_override.to_string())
        } else {
            self.latest_invocation_model_for_chain(&chain_id)
                .map_err(|message| ResumeError::Db { message })?
                .filter(|name| name != "<unknown>")
                .or(self
                    .chain_model_name(&chain_id)
                    .map_err(|message| ResumeError::Db { message })?
                    .filter(|name| name != "<unknown>"))
        };

        let model = if let Some(model_name) = model_name.as_ref() {
            let model =
                models
                    .get(model_name)
                    .cloned()
                    .ok_or_else(|| ResumeError::UnknownModel {
                        model_name: model_name.clone(),
                    })?;
            let Some(_active_provider_index) = model
                .providers
                .iter()
                .position(|provider| provider.name == active_provider)
            else {
                let mut suggestions = models
                    .iter()
                    .filter(|(_, model)| {
                        model
                            .providers
                            .iter()
                            .any(|provider| provider.name == active_provider)
                    })
                    .map(|(name, _)| name.clone())
                    .collect::<Vec<_>>();
                suggestions.sort();
                return Err(ResumeError::ProviderModelMismatch {
                    model_name: model_name.clone(),
                    active_provider,
                    suggestions,
                });
            };
            Some(model)
        } else {
            None
        };

        Ok(ResolvedResume {
            chain_id,
            model_name,
            model,
            active_provider,
            active_session_id,
        })
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
                params![provider_name, session_id],
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
            .query_map(params![input], |row| row.get::<_, String>(0))
            .map_err(|e| format!("Failed to query resume chain lookup: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read resume chain lookup: {e}"))
    }

    fn wrong_id_kind_invocation_match(
        &self,
        input: &str,
    ) -> Result<Option<WrongIdKindInvocationMatch>, String> {
        let provider_session_select = if Self::invocations_have_dual_id_columns(&self.conn)? {
            "provider_session_id"
        } else {
            "NULL AS provider_session_id"
        };
        let sql = format!(
            "SELECT invocation_uuid, provider_name, {provider_session_select}
             FROM invocations
             WHERE invocation_uuid = ?1"
        );
        let row = self
            .conn
            .query_row(&sql, params![input], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .optional()
            .map_err(|e| format!("Failed to query invocation id-kind match: {e}"))?;

        let Some((invocation_uuid, provider_name, provider_session_id)) = row else {
            return Ok(None);
        };
        let chain_id = match (provider_name.as_deref(), provider_session_id.as_deref()) {
            (Some(provider_name), Some(provider_session_id)) => self
                .chain_id_for_segment(provider_name, provider_session_id)
                .map_err(|e| format!("Failed to resolve chain for wrong-id-kind match: {e}"))?,
            _ => None,
        };
        Ok(Some(WrongIdKindInvocationMatch {
            invocation_uuid,
            provider_name,
            provider_session_id,
            chain_id,
        }))
    }

    fn choose_resume_chain(
        &self,
        _input: &str,
        mut chain_ids: Vec<String>,
    ) -> Result<Option<String>, String> {
        if chain_ids.len() == 1 {
            return Ok(chain_ids.pop());
        }

        struct ResumeChainCandidate {
            chain_id: String,
            last_used_at: DateTime<Utc>,
            latest_segment_started_at: DateTime<Utc>,
        }

        let mut rows = Vec::new();
        for chain_id in chain_ids {
            let raw: String = self
                .conn
                .query_row(
                    "SELECT last_used_at FROM session_chains WHERE chain_id = ?1",
                    params![chain_id],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Failed to read chain last_used_at: {e}"))?;
            let last_used = DateTime::parse_from_rfc3339(&raw)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|e| format!("Bad chain last_used_at {raw}: {e}"))?;

            let raw_started: String = self
                .conn
                .query_row(
                    "SELECT started_at
                     FROM session_chain_segments
                     WHERE chain_id = ?1
                     ORDER BY started_at DESC, id DESC
                     LIMIT 1",
                    params![chain_id],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Failed to read chain latest segment started_at: {e}"))?;
            let latest_segment_started_at = DateTime::parse_from_rfc3339(&raw_started)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|e| format!("Bad chain segment started_at {raw_started}: {e}"))?;

            rows.push(ResumeChainCandidate {
                chain_id,
                last_used_at: last_used,
                latest_segment_started_at,
            });
        }

        rows.sort_by(|a, b| {
            b.last_used_at
                .cmp(&a.last_used_at)
                .then_with(|| {
                    b.latest_segment_started_at
                        .cmp(&a.latest_segment_started_at)
                })
                .then_with(|| a.chain_id.cmp(&b.chain_id))
        });
        Ok(rows.into_iter().next().map(|row| row.chain_id))
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
                params![chain_id, provider_name, session_id],
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
                params![chain_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| format!("Failed to read active chain segment: {e}"))
    }

    fn chain_model_name(&self, chain_id: &str) -> Result<Option<String>, String> {
        self.conn
            .query_row(
                "SELECT model_name FROM session_chains WHERE chain_id = ?1",
                params![chain_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to read session chain model: {e}"))
    }

    fn latest_invocation_model_for_chain(&self, chain_id: &str) -> Result<Option<String>, String> {
        let provider_session_expr = Self::provider_session_expr(&self.conn, Some("i."))?;
        let sql = format!(
            "SELECT i.model_name
             FROM invocations i
             WHERE {provider_session_expr} IN (
                SELECT session_id FROM session_chain_segments WHERE chain_id = ?1
             )
             ORDER BY COALESCE(i.finished_at, i.created_at) DESC, i.id DESC
             LIMIT 1"
        );
        self.conn
            .query_row(&sql, params![chain_id], |row| row.get(0))
            .optional()
            .map_err(|e| format!("Failed to infer session chain model from invocations: {e}"))
    }

    fn chain_previews(&self, input: &str) -> Result<Vec<ChainPreview>, String> {
        let chain_ids = self.candidate_chain_ids(input)?;
        let mut out = Vec::new();
        for chain_id in chain_ids {
            let raw_last: String = self
                .conn
                .query_row(
                    "SELECT last_used_at FROM session_chains WHERE chain_id = ?1",
                    params![chain_id],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Failed to read chain preview: {e}"))?;
            let last_used_at = DateTime::parse_from_rfc3339(&raw_last)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|e| format!("Bad chain preview timestamp {raw_last}: {e}"))?;
            let (active_provider, active_session_id) = self
                .active_segment_for_chain(&chain_id)?
                .unwrap_or_else(|| ("<none>".to_string(), "<none>".to_string()));
            let turn_count: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM session_turns WHERE provider_name = ?1 AND session_id = ?2",
                params![active_provider, active_session_id],
                |row| row.get(0),
            ).unwrap_or(0);
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
                .query_map(params![active_provider, active_session_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| format!("Failed to query recent turns preview: {e}"))?;
            let mut recent_turns = Vec::new();
            for row in rows {
                let (role, raw_ts) = row.map_err(|e| format!("Failed to read recent turn: {e}"))?;
                let timestamp = DateTime::parse_from_rfc3339(&raw_ts)
                    .map(|dt| dt.with_timezone(&Utc))
                    .map_err(|e| format!("Bad recent turn timestamp {raw_ts}: {e}"))?;
                recent_turns.push(TurnPreview {
                    role,
                    timestamp,
                    snippet: None,
                });
            }
            recent_turns.reverse();
            out.push(ChainPreview {
                chain_id,
                last_used_at,
                active_provider,
                active_session_id,
                turn_count: turn_count.max(0) as usize,
                recent_turns,
            });
        }
        out.sort_by_key(|preview| std::cmp::Reverse(preview.last_used_at));
        Ok(out)
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
        let mut stmt = self
            .conn
            .prepare(
                "SELECT session_id, timestamp
                 FROM session_turns
                 WHERE provider_name = ?1",
            )
            .map_err(|e| format!("Failed to prepare invocation session lookup: {e}"))?;

        let rows = stmt
            .query_map(params![provider_name], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| format!("Failed to query invocation session lookup: {e}"))?;

        let mut candidates: HashMap<String, (DateTime<Utc>, u64)> = HashMap::new();
        for row in rows {
            let (session_id, timestamp_raw) =
                row.map_err(|e| format!("Failed to read invocation session lookup row: {e}"))?;
            let timestamp = DateTime::parse_from_rfc3339(&timestamp_raw)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|e| format!("Bad session turn timestamp {timestamp_raw}: {e}"))?;
            if timestamp <= *started_at || timestamp > *finished_at {
                continue;
            }

            candidates
                .entry(session_id)
                .and_modify(|(earliest, in_window)| {
                    if timestamp < *earliest {
                        *earliest = timestamp;
                    }
                    *in_window += 1;
                })
                .or_insert((timestamp, 1));
        }

        let mut ranked = candidates.into_iter().collect::<Vec<_>>();
        ranked.sort_by(
            |(session_a, (earliest_a, count_a)), (session_b, (earliest_b, count_b))| {
                count_b
                    .cmp(count_a)
                    .then_with(|| earliest_a.cmp(earliest_b))
                    .then_with(|| session_a.cmp(session_b))
            },
        );
        Ok(ranked
            .into_iter()
            .map(|(session_id, _)| session_id)
            .collect())
    }

    /// Count assistant turns ingested for a provider since `since` (exclusive).
    /// `None` means count everything we've ever ingested for that provider.
    pub fn count_assistant_turns_since(
        &self,
        provider_name: &str,
        since: Option<&DateTime<Utc>>,
    ) -> Result<u64, String> {
        let count: i64 = match since {
            Some(ts) => self
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM session_turns
                     WHERE provider_name = ?1 AND role = 'assistant' AND timestamp > ?2",
                    params![provider_name, ts.to_rfc3339()],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Failed to count session turns: {e}"))?,
            None => self
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM session_turns
                     WHERE provider_name = ?1 AND role = 'assistant'",
                    params![provider_name],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Failed to count session turns: {e}"))?,
        };
        Ok(count.max(0) as u64)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn drop_provider_quotas_for_test(&self) {
        self.conn
            .execute_batch("DROP TABLE provider_quotas")
            .unwrap();
    }

    /// Helper: map a rusqlite row to an AccountRecord.
    fn map_account_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AccountRecord> {
        let auth_method_str: String = row.get(3)?;
        let auth_status_str: String = row.get(4)?;
        Ok(AccountRecord {
            id: row.get(0)?,
            provider: row.get(1)?,
            profile_name: row.get(2)?,
            auth_method: AuthMethod::from_db_string(&auth_method_str),
            auth_status: AuthStatus::from_str(&auth_status_str),
            created_at: row.get(5)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::env_lock;
    use rusqlite::Connection;
    use tempfile::TempDir;
    use uuid::Uuid;

    mod failing_migration {
        include!("../tests/fixtures/failing_migration.rs");
    }

    fn test_db() -> StateDb {
        StateDb::open(Path::new(":memory:")).unwrap()
    }

    fn mark_current_schema_version(conn: &Connection) {
        seed_current_drift_required_tables(conn);
        conn.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION)
            .unwrap();
    }

    fn seed_current_drift_required_tables(conn: &Connection) {
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
                params![provider_name],
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
                params![provider_name],
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
                params![provider_name],
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
                params![provider_name],
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
                params![created_at, id],
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
        let conn = Connection::open(&path).unwrap();
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
                params![
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
        let conn = Connection::open(&path).unwrap();
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
                params![
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
        let conn = Connection::open(&path).unwrap();
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
            params![Uuid::new_v4().to_string()],
        )
        .unwrap();
        mark_current_schema_version(&conn);
        dir
    }

    fn malformed_providers_affinity_db() -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let conn = Connection::open(&path).unwrap();
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
        let conn = Connection::open(&path).unwrap();
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

    fn table_columns_with_pk(conn: &Connection, table_name: &str) -> Vec<(String, i64)> {
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

    fn provider_aggregate_snapshot(conn: &Connection) -> Vec<ProviderAggregateSnapshot> {
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

    fn quoted_snapshot(conn: &Connection, schema_sql: &str, rows_sql: &str) -> Vec<String> {
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

    fn malformed_providers_snapshot(conn: &Connection) -> Vec<String> {
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

    fn invocations_snapshot(conn: &Connection) -> Vec<String> {
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

    fn legacy_invocations_snapshot(conn: &Connection) -> Vec<String> {
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
        let conn = Connection::open(&path).unwrap();
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
                params![provider],
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
        let conn = Connection::open(&path).unwrap();
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
        let conn = Connection::open(&path).unwrap();
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

        let conn = Connection::open(&path).unwrap();
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
        let conn = Connection::open(&path).unwrap();
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

        let conn = Connection::open(&path).unwrap();
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
        let conn = Connection::open(&path).unwrap();
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
        let conn = Connection::open(&path).unwrap();
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

        let conn = Connection::open(&path).unwrap();
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
        let conn = Connection::open(&path).unwrap();
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
        let conn = Connection::open(&path).unwrap();
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
        let conn = Connection::open(&path).unwrap();
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

        let conn = Connection::open(&path).unwrap();
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
            let conn = Connection::open(&path).unwrap();
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
        let conn = Connection::open(&path).unwrap();
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
        let conn = Connection::open(&path).unwrap();
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
                params!["p", -1],
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
                params![id],
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
            },
        )
        .unwrap();

        assert_eq!(segment_count(&db), 0);
        let row: (Option<String>, Option<String>) = db
            .conn
            .query_row(
                "SELECT provider_session_id, resume_input_id FROM invocations WHERE id = ?1",
                params![id],
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
                params![id],
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
                    params![
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
                params![resumed],
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
                params![non_resumed],
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
                params![resumed],
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
                params![resumed],
                |row| row.get(0),
            )
            .unwrap();
        let non_resumed_session: Option<String> = db
            .conn
            .query_row(
                "SELECT session_id FROM invocations WHERE id = ?1",
                params![non_resumed],
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
                params!["fixture-provider", "session-a", "child-turn"],
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
                params![chain_id, last_used_at, model_name],
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
                params![
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
                params![created_at, id],
            )
            .unwrap();
    }

    fn pre_chain_db_with_turns(rows: &[(&str, &str, &str, &str, &str)]) -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let conn = Connection::open(&path).unwrap();
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
                params![provider, session, turn, timestamp, role],
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
        let conn = Connection::open(&db_path).unwrap();
        seed_current_drift_required_tables(&conn);
        conn.execute(
            "INSERT INTO invocations
                (invocation_uuid, model_name, provider_name, provider_index, status, success,
                 exit_code, error_category, terminal_reason, session_id, session_capture_method,
                 created_at, finished_at)
             VALUES (?1, 'claude-opus', 'claude', 0, ?2, NULL, NULL, ?3, ?4, ?5, ?6,
                     '2026-04-17T08:00:00Z', NULL)",
            params![
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
                params![invocation_uuid],
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
            let conn = Connection::open(dir.path().join("state.db")).unwrap();
            conn.execute(
                "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
                 VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', 'claude-opus')",
                params![CHAIN_A],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO session_chain_segments
                    (chain_id, provider_name, session_id, started_at, transition_reason)
                 VALUES (?1, 'claude', ?2, '2026-04-17T08:00:00Z', 'initial')",
                params![CHAIN_A, SESSION_A],
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
                params![invocation_uuid],
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
                params![invocation_uuid],
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
                params![CHAIN_A],
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
                params![SESSION_A],
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
                params![SESSION_A],
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
                params![CHAIN_A],
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
                params![SESSION_A],
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
                params![CHAIN_A],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(active, 0);
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
        let mut conn = Connection::open(&db_path).unwrap();
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
}
