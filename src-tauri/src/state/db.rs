use crate::config::load_models;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, named_params, params};
use serde::{Deserialize, Serialize};
use std::path::Path;
use uuid::Uuid;

/// Ceiling on the per-turn burn rate that `upsert_quota_refresh` is willing
/// to learn from a single refresh-to-refresh sample. A transient upstream
/// spike (observed on the ChatGPT usage endpoint: `used_percent` briefly
/// reported as 1.0 before the window reset) paired with a small turn count
/// produced a learned rate of ~0.05/turn that then got carried forward across
/// subsequent no-change refreshes, projecting every provider over the 95%
/// failure threshold and blocking the whole pool. The highest plausible real
/// rate observed in live data is ~5e-4/turn on a 5h Claude window; 0.1/turn
/// is a 200× safety margin that still filters the spike case.
const MAX_LEARNABLE_BURN_RATE: f64 = 0.1;

/// Minimum assistant-turn sample size before a refresh-to-refresh delta is
/// accepted as a burn-rate learn. Below this, a 1%-on-6-turns observation
/// extrapolates to rates that are dominated by sample noise — when the
/// rate is then multiplied by `turns_since_refresh` at scoring time, a
/// 65%-used window can project to 97% on nothing but measurement error,
/// and `score_by_density` hard-blocks the provider as if it were actually
/// exhausted. Live-caught 2026-04-21 on `claude2` with
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
/// over `failure_threshold` on nothing but a bad sample. User intuition:
/// "turns barely budge weekly" — so any single sample imputing a weekly
/// move > 1 point is suspect, and the cleanest marker of "suspect" is
/// "the sample is at the rail." Matching ceiling from score_by_density.
const NEAR_EXHAUSTED_USED_PERCENT: f64 = 0.99;

pub struct StateDb {
    conn: Connection,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ProviderRecord {
    pub model_name: String,
    pub provider_index: usize,
    pub invocation_count: u64,
    pub error_count: u64,
    pub last_error: Option<String>,
    pub last_error_at: Option<DateTime<Utc>>,
    pub last_invoked_at: Option<DateTime<Utc>>,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTurnCounts {
    pub total: u64,
    pub assistant: u64,
    pub sidechain: u64,
}

#[derive(Debug, Clone)]
pub struct ProviderSessionMatch {
    pub provider_name: String,
    pub latest_timestamp: DateTime<Utc>,
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
    pub session_id: Option<String>,
    pub session_capture_method: Option<String>,
    pub quota_tight_routing: bool,
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
    pub quota_tight_routing: bool,
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
        let parsed: CompositeInvocationId =
            serde_json::from_str(s).map_err(|e| format!("Invalid invocation JSON: {e}"))?;
        Uuid::parse_str(&parsed.id).map_err(|e| format!("Invalid invocation UUID: {e}"))?;
        Ok(parsed)
    }
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

        let conn = Connection::open(path).map_err(|e| format!("Failed to open state DB: {e}"))?;

        conn.execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(|e| format!("Failed to set WAL mode: {e}"))?;

        Self::ensure_invocations_schema(&conn)?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS providers (
                model_name TEXT NOT NULL,
                provider_index INTEGER NOT NULL,
                invocation_count INTEGER NOT NULL DEFAULT 0,
                error_count INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                last_error_at TEXT,
                last_invoked_at TEXT,
                PRIMARY KEY (model_name, provider_index)
            );

            CREATE TABLE IF NOT EXISTS provider_quotas (
                provider_name TEXT PRIMARY KEY,
                used_percent REAL NOT NULL DEFAULT 0,
                resets_at TEXT,
                calls_since_refresh INTEGER NOT NULL DEFAULT 0,
                refreshed_at TEXT,
                last_empty_refresh_at TEXT
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

            -- Migrate pre-multi-window rows: any provider_quotas row with a
            -- resets_at that doesn't yet have a window becomes window 0.
            INSERT OR IGNORE INTO provider_quota_windows (provider_name, window_id, used_percent, resets_at)
            SELECT provider_name, 0, used_percent, resets_at
            FROM provider_quotas
            WHERE resets_at IS NOT NULL;

            CREATE TABLE IF NOT EXISTS memory_nodes (
                id TEXT PRIMARY KEY,
                node_type TEXT NOT NULL,
                label TEXT NOT NULL,
                data TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS memory_edges (
                source_id TEXT NOT NULL REFERENCES memory_nodes(id),
                target_id TEXT NOT NULL REFERENCES memory_nodes(id),
                edge_type TEXT NOT NULL,
                data TEXT,
                created_at TEXT NOT NULL,
                PRIMARY KEY (source_id, target_id, edge_type)
            );

            CREATE TABLE IF NOT EXISTS setup_sessions (
                id TEXT PRIMARY KEY,
                started_at TEXT NOT NULL,
                ended_at TEXT,
                outcome TEXT,
                turn_count INTEGER DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS setup_turns (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                turn_number INTEGER NOT NULL,
                agent_prompt TEXT NOT NULL,
                agent_response TEXT NOT NULL,
                events_emitted TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS cli_providers (
                cli_name TEXT PRIMARY KEY,
                display_name TEXT NOT NULL,
                installed INTEGER NOT NULL DEFAULT 0,
                version TEXT,
                config_dir TEXT,
                last_synced TEXT
            );

            CREATE TABLE IF NOT EXISTS accounts (
                id TEXT NOT NULL,
                provider TEXT NOT NULL REFERENCES cli_providers(cli_name),
                profile_name TEXT NOT NULL,
                auth_method TEXT NOT NULL,
                auth_status TEXT NOT NULL DEFAULT 'unknown',
                created_at TEXT NOT NULL,
                PRIMARY KEY (id, provider)
            );

            CREATE INDEX IF NOT EXISTS idx_accounts_provider
                ON accounts (provider);

            CREATE TABLE IF NOT EXISTS discovered_models (
                canonical_name TEXT NOT NULL,
                provider TEXT NOT NULL,
                discovered_at TEXT NOT NULL,
                cli_version TEXT NOT NULL,
                PRIMARY KEY (canonical_name, provider)
            );

            CREATE TABLE IF NOT EXISTS model_parameters (
                model_name TEXT NOT NULL,
                provider TEXT NOT NULL,
                name TEXT NOT NULL,
                display_name TEXT NOT NULL,
                param_type TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                cli_mapping TEXT NOT NULL,
                PRIMARY KEY (model_name, provider, name)
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
                source_file TEXT NOT NULL,
                ingested_at TEXT NOT NULL,
                UNIQUE (provider_name, session_id, turn_id)
            );
            ",
        )
        .map_err(|e| format!("Failed to initialize schema: {e}"))?;
        Self::ensure_provider_quotas_schema(&conn)?;
        Self::ensure_provider_quota_windows_schema(&conn)?;
        Self::ensure_session_turns_schema(&conn)?;

        Ok(StateDb { conn })
    }

    pub fn open_default() -> Result<Self, String> {
        let data_dir =
            dirs::data_dir().ok_or_else(|| "Could not determine data directory".to_string())?;
        let db_path = data_dir.join("oulipoly-agent-runner").join("state.db");
        Self::open(&db_path)
    }

    fn ensure_invocations_schema(conn: &Connection) -> Result<(), String> {
        let columns = Self::invocations_columns(conn)?;
        if columns.is_empty() {
            conn.execute_batch(Self::invocations_schema_sql())
                .map_err(|e| format!("Failed to initialize invocations schema: {e}"))?;
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
            if !columns.iter().any(|column| column == "quota_tight_routing") {
                conn.execute(
                    "ALTER TABLE invocations ADD COLUMN quota_tight_routing BOOLEAN NOT NULL DEFAULT 0",
                    [],
                )
                .map_err(|e| format!("Failed to add invocations.quota_tight_routing: {e}"))?;
            }
            conn.execute_batch(Self::invocations_index_sql())
                .map_err(|e| format!("Failed to ensure invocation indexes: {e}"))?;
            return Ok(());
        }

        Self::migrate_legacy_invocations(conn)
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

    fn invocations_schema_sql() -> &'static str {
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
            session_id TEXT,
            session_capture_method TEXT,
            quota_tight_routing BOOLEAN NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            finished_at TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_invocations_uuid
            ON invocations (invocation_uuid);
        CREATE INDEX IF NOT EXISTS idx_invocations_parent
            ON invocations (parent_invocation_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_invocations_provider_created
            ON invocations (provider_name, created_at);
        CREATE INDEX IF NOT EXISTS idx_invocations_provider_session
            ON invocations (provider_name, session_id)
            WHERE session_id IS NOT NULL;"
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
                session_id TEXT,
                session_capture_method TEXT,
                quota_tight_routing BOOLEAN NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                finished_at TEXT
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
                        session_id,
                        session_capture_method,
                        quota_tight_routing,
                        created_at,
                        finished_at
                     ) VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?7, ?8, NULL, NULL, 0, ?9, ?9)",
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

        tx.execute_batch(
            "DROP TABLE invocations;
             ALTER TABLE invocations_new RENAME TO invocations;",
        )
        .map_err(|e| format!("Failed to replace invocations table: {e}"))?;

        tx.execute_batch(Self::invocations_index_sql())
            .map_err(|e| format!("Failed to create migrated invocation indexes: {e}"))?;

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
        let models = match load_models(&models_dir) {
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
                    quota_tight_routing,
                    created_at,
                    finished_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, NULL, ?7, ?8, NULL)",
                params![
                    &start.invocation_uuid,
                    &start.model_name,
                    &start.provider_name,
                    start.provider_index as i64,
                    start.parent_invocation_id,
                    InvocationStatus::Running.as_str(),
                    start.quota_tight_routing as i64,
                    &now,
                ],
            )
            .map_err(|e| format!("Failed to insert invocation: {e}"))?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn finalize_invocation(
        &self,
        id: i64,
        success: bool,
        exit_code: i32,
        error_category: Option<&str>,
        stderr_snippet: Option<&str>,
    ) -> Result<(), String> {
        let now = Utc::now().to_rfc3339();
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to begin invocation finalize tx: {e}"))?;

        let (model_name, provider_index, status) = tx
            .query_row(
                "SELECT model_name, provider_index, status FROM invocations WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| format!("Failed to load invocation {id}: {e}"))?
            .ok_or_else(|| format!("Invocation {id} not found"))?;

        if status.parse::<InvocationStatus>().ok() != Some(InvocationStatus::Running) {
            return Err(format!("Invocation {id} is already finalized"));
        }

        tx.execute(
            "UPDATE invocations
             SET status = ?1,
                 success = ?2,
                 exit_code = ?3,
                 error_category = ?4,
                 finished_at = ?5
             WHERE id = ?6",
            params![
                if success {
                    InvocationStatus::Succeeded.as_str()
                } else {
                    InvocationStatus::Failed.as_str()
                },
                success as i64,
                exit_code,
                error_category,
                &now,
                id,
            ],
        )
        .map_err(|e| format!("Failed to finalize invocation {id}: {e}"))?;

        tx.execute(
            "INSERT INTO providers (model_name, provider_index, invocation_count, error_count, last_invoked_at)
             VALUES (?1, ?2, 1, ?3, ?4)
             ON CONFLICT (model_name, provider_index)
             DO UPDATE SET
                invocation_count = invocation_count + 1,
                error_count = error_count + ?3,
                last_invoked_at = ?4",
            params![
                &model_name,
                provider_index,
                if success { 0i64 } else { 1i64 },
                &now
            ],
        )
        .map_err(|e| format!("Failed to upsert provider: {e}"))?;

        if !success {
            let snippet = stderr_snippet
                .unwrap_or("")
                .chars()
                .take(500)
                .collect::<String>();
            tx.execute(
                "UPDATE providers SET last_error = ?1, last_error_at = ?2
                 WHERE model_name = ?3 AND provider_index = ?4",
                params![&snippet, &now, &model_name, provider_index],
            )
            .map_err(|e| format!("Failed to update error info: {e}"))?;
        }

        tx.commit()
            .map_err(|e| format!("Failed to commit invocation finalize tx: {e}"))
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
        self.conn
            .execute(
                "UPDATE invocations
                 SET session_id = ?1,
                     session_capture_method = ?2
                 WHERE id = ?3",
                params![session_id, method, id],
            )
            .map_err(|e| format!("Failed to update session capture for invocation {id}: {e}"))?;
        Ok(())
    }

    pub fn get_invocation_by_uuid(&self, uuid: &str) -> Result<Option<InvocationRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, invocation_uuid, model_name, provider_name, provider_index,
                        parent_invocation_id, status, success, exit_code, error_category,
                        session_id, session_capture_method, quota_tight_routing, created_at,
                        finished_at
                 FROM invocations
                 WHERE invocation_uuid = ?1",
            )
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
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, invocation_uuid, model_name, provider_name, provider_index,
                        parent_invocation_id, status, success, exit_code, error_category,
                        session_id, session_capture_method, quota_tight_routing, created_at,
                        finished_at
                 FROM invocations
                 WHERE parent_invocation_id = ?1
                 ORDER BY created_at, id",
            )
            .map_err(|e| format!("Failed to prepare invocation child lookup: {e}"))?;

        let rows = stmt
            .query_map(params![parent_id], Self::map_invocation_row)
            .map_err(|e| format!("Failed to query invocation children: {e}"))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to map invocation children: {e}"))
    }

    fn map_invocation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<InvocationRecord> {
        let created_at_raw: String = row.get(13)?;
        let finished_at_raw: Option<String> = row.get(14)?;
        let status_raw: String = row.get(6)?;
        let created_at = DateTime::parse_from_rfc3339(&created_at_raw)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    13,
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
                            14,
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
            session_id: row.get(10)?,
            session_capture_method: row.get(11)?,
            quota_tight_routing: row.get::<_, i64>(12)? != 0,
            created_at,
            finished_at,
        })
    }

    pub fn get_provider(
        &self,
        model_name: &str,
        provider_index: usize,
    ) -> Result<Option<ProviderRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT invocation_count, error_count, last_error, last_error_at, last_invoked_at
                 FROM providers WHERE model_name = ?1 AND provider_index = ?2",
            )
            .map_err(|e| format!("Failed to prepare query: {e}"))?;

        let result = stmt.query_row(params![model_name, provider_index as i64], |row| {
            Ok(ProviderRecord {
                model_name: model_name.to_string(),
                provider_index,
                invocation_count: row.get::<_, i64>(0)? as u64,
                error_count: row.get::<_, i64>(1)? as u64,
                last_error: row.get(2)?,
                last_error_at: row
                    .get::<_, Option<String>>(3)?
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                last_invoked_at: row
                    .get::<_, Option<String>>(4)?
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
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
        provider_index: usize,
        window_minutes: i64,
    ) -> Result<u64, String> {
        let cutoff = (Utc::now() - chrono::Duration::minutes(window_minutes)).to_rfc3339();

        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM invocations
                 WHERE model_name = ?1 AND provider_index = ?2
                   AND success = 0 AND created_at > ?3",
                params![model_name, provider_index as i64, &cutoff],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to count recent errors: {e}"))?;

        Ok(count as u64)
    }

    // --- Provider quota operations ---

    /// Fetch provider-level quota metadata. Windows live in a separate
    /// table — use `get_windows` to get the actual quota numbers.
    pub fn get_quota(&self, provider_name: &str) -> Result<Option<QuotaRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT calls_since_refresh, refreshed_at
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
            })
        });

        match result {
            Ok(record) => Ok(Some(record)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("Failed to query quota: {e}")),
        }
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

        tx.execute(
            "INSERT INTO provider_quotas
                (provider_name, used_percent, resets_at, calls_since_refresh, refreshed_at)
             VALUES (?1, ?2, ?3, 0, ?4)
             ON CONFLICT (provider_name) DO UPDATE SET
                used_percent = ?2,
                resets_at = ?3,
                calls_since_refresh = 0,
                refreshed_at = ?4",
            params![provider_name, legacy_used, legacy_resets, &now],
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

    /// Test-only: backdate a provider's `refreshed_at` so tests can seed
    /// turns whose timestamps are "after" the refresh.
    #[cfg(test)]
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

    /// Test-only: seed the PR 3 per-window burn-rate learning columns without
    /// adding a migration here. This intentionally fails at runtime until the
    /// production schema owns these columns.
    #[cfg(test)]
    pub(crate) fn set_window_delta_for_test(
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
    #[cfg(test)]
    pub(crate) fn insert_quota_row_without_windows_for_test(
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
                    (provider_name, session_id, turn_id, timestamp, role, source_file, ingested_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    provider_name,
                    session_id,
                    turn_id,
                    &ts,
                    role,
                    source_file,
                    &now,
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
                            source_file,
                            ingested_at
                        )
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, '', ?8)",
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
                        &now,
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

    pub fn find_provider_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<ProviderSessionMatch>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT provider_name, MAX(timestamp) AS latest_timestamp
                 FROM session_turns
                 WHERE session_id = :session_id
                 GROUP BY provider_name
                 ORDER BY latest_timestamp DESC, provider_name ASC",
            )
            .map_err(|e| format!("Failed to prepare session provider lookup: {e}"))?;

        let rows = stmt
            .query_map(named_params! { ":session_id": session_id }, |row| {
                let latest_timestamp_raw: String = row.get(1)?;
                let latest_timestamp = DateTime::parse_from_rfc3339(&latest_timestamp_raw)
                    .map(|dt| dt.with_timezone(&Utc))
                    .map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?;

                Ok(ProviderSessionMatch {
                    provider_name: row.get(0)?,
                    latest_timestamp,
                })
            })
            .map_err(|e| format!("Failed to query session provider lookup: {e}"))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read session provider lookup: {e}"))
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
    use rusqlite::Connection;
    use std::sync::{Mutex, OnceLock};
    use tempfile::TempDir;
    use uuid::Uuid;

    fn test_db() -> StateDb {
        StateDb::open(Path::new(":memory:")).unwrap()
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
                quota_tight_routing: false,
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

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
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
        dir
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
                "idx_invocations_provider_session".to_string(),
                "idx_invocations_uuid".to_string(),
                "sqlite_autoindex_invocations_1".to_string(),
            ]
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
prompt_mode = "arg"

[[providers]]
name = "fixture-provider"
command = "fixture"
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
        assert!(crate::quota::is_stale(&db, provider));
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
        // provider above failure_threshold. The sanity cap at
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
        // which then projected every future invocation over
        // failure_threshold. User framing: "turns barely budge weekly" —
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

    #[test]
    fn quota_tight_routing_column_persisted_to_invocations() {
        let db = test_db();
        let start = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "test-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
            quota_tight_routing: true,
        };

        let id = db.start_invocation(&start).unwrap();
        let persisted: i64 = db
            .conn
            .query_row(
                "SELECT quota_tight_routing FROM invocations WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(persisted, 1);
    }

    #[test]
    fn start_invocation_inserts_running_row_with_null_terminal_fields() {
        let db = test_db();
        let start = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "test-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
            quota_tight_routing: false,
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
        assert_eq!(row.finished_at, None);
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
            quota_tight_routing: false,
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
            quota_tight_routing: false,
        };
        let parent_id = db.start_invocation(&parent).unwrap();

        let child = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "test-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: Some(parent_id),
            quota_tight_routing: false,
        };
        db.start_invocation(&child).unwrap();

        let row = db
            .get_invocation_by_uuid(&child.invocation_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(row.parent_invocation_id, Some(parent_id));
    }

    #[test]
    fn finalize_invocation_sets_terminal_fields() {
        let db = test_db();
        let start = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "test-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
            quota_tight_routing: false,
        };
        let id = db.start_invocation(&start).unwrap();

        db.finalize_invocation(id, true, 0, None, None).unwrap();

        let row = db
            .get_invocation_by_uuid(&start.invocation_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(row.status, InvocationStatus::Succeeded);
        assert_eq!(row.success, Some(true));
        assert_eq!(row.exit_code, Some(0));
        assert_eq!(row.error_category, None);
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
            quota_tight_routing: false,
        };
        let succeeded = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "test-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
            quota_tight_routing: false,
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

        let provider = db.get_provider("test-model", 0).unwrap().unwrap();
        assert_eq!(provider.invocation_count, 2);
        assert_eq!(provider.error_count, 1);
        assert_eq!(
            provider.last_error.as_deref(),
            Some("429 Too Many Requests")
        );
        assert!(provider.last_invoked_at.is_some());
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
            quota_tight_routing: false,
        };
        let id = db.start_invocation(&start).unwrap();
        db.finalize_invocation(id, true, 0, None, None).unwrap();

        let err = db.finalize_invocation(id, true, 0, None, None).unwrap_err();
        assert!(err.contains("already"));
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
            quota_tight_routing: false,
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
            quota_tight_routing: false,
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
            quota_tight_routing: false,
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
            quota_tight_routing: false,
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
    fn recent_errors() {
        let db = test_db();
        let failed = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "m".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
            quota_tight_routing: false,
        };
        let succeeded = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "m".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
            quota_tight_routing: false,
        };
        let failed_id = db.start_invocation(&failed).unwrap();
        db.finalize_invocation(failed_id, false, 1, None, None)
            .unwrap();
        let succeeded_id = db.start_invocation(&succeeded).unwrap();
        db.finalize_invocation(succeeded_id, true, 0, None, None)
            .unwrap();

        let count = db.recent_error_count("m", 0, 60).unwrap();
        assert_eq!(count, 1);
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
                },
                SessionTurnIngest {
                    session_id: "session-a".to_string(),
                    turn_id: "assistant-main".to_string(),
                    timestamp: ts("2026-04-17T08:00:01Z"),
                    role: "assistant".to_string(),
                    parent_turn_id: Some("root".to_string()),
                    is_sidechain: false,
                },
                SessionTurnIngest {
                    session_id: "session-a".to_string(),
                    turn_id: "assistant-side".to_string(),
                    timestamp: ts("2026-04-17T08:00:02Z"),
                    role: "assistant".to_string(),
                    parent_turn_id: Some("assistant-main".to_string()),
                    is_sidechain: true,
                },
                SessionTurnIngest {
                    session_id: "session-b".to_string(),
                    turn_id: "other-session".to_string(),
                    timestamp: ts("2026-04-17T08:00:03Z"),
                    role: "assistant".to_string(),
                    parent_turn_id: None,
                    is_sidechain: true,
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
    fn find_provider_for_session_returns_empty_for_unknown_session() {
        let db = test_db();

        let matches = db.find_provider_for_session("missing-session").unwrap();

        assert!(matches.is_empty());
    }

    #[test]
    fn find_provider_for_session_returns_single_provider_match_with_latest_timestamp() {
        let db = test_db();
        db.ingest_session_turns_batch(
            "claude2",
            &[
                SessionTurnIngest {
                    session_id: "session-a".to_string(),
                    turn_id: "turn-1".to_string(),
                    timestamp: ts("2026-04-17T08:00:00Z"),
                    role: "user".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                },
                SessionTurnIngest {
                    session_id: "session-a".to_string(),
                    turn_id: "turn-2".to_string(),
                    timestamp: ts("2026-04-17T08:00:05Z"),
                    role: "assistant".to_string(),
                    parent_turn_id: Some("turn-1".to_string()),
                    is_sidechain: false,
                },
            ],
        )
        .unwrap();

        let matches = db.find_provider_for_session("session-a").unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].provider_name, "claude2");
        assert_eq!(matches[0].latest_timestamp, ts("2026-04-17T08:00:05Z"));
    }

    #[test]
    fn find_provider_for_session_orders_by_latest_timestamp_then_provider_name() {
        let db = test_db();
        db.ingest_session_turns_batch(
            "claude2",
            &[SessionTurnIngest {
                session_id: "shared-session".to_string(),
                turn_id: "claude-turn".to_string(),
                timestamp: ts("2026-04-17T08:00:10Z"),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
            }],
        )
        .unwrap();
        db.ingest_session_turns_batch(
            "codex-a",
            &[SessionTurnIngest {
                session_id: "shared-session".to_string(),
                turn_id: "codex-a-turn".to_string(),
                timestamp: ts("2026-04-17T08:00:05Z"),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
            }],
        )
        .unwrap();
        db.ingest_session_turns_batch(
            "codex-b",
            &[SessionTurnIngest {
                session_id: "shared-session".to_string(),
                turn_id: "codex-b-turn".to_string(),
                timestamp: ts("2026-04-17T08:00:05Z"),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
            }],
        )
        .unwrap();

        let matches = db.find_provider_for_session("shared-session").unwrap();
        let providers: Vec<&str> = matches.iter().map(|m| m.provider_name.as_str()).collect();

        assert_eq!(providers, vec!["claude2", "codex-a", "codex-b"]);
        assert_eq!(matches[0].latest_timestamp, ts("2026-04-17T08:00:10Z"));
        assert_eq!(matches[1].latest_timestamp, ts("2026-04-17T08:00:05Z"));
        assert_eq!(matches[2].latest_timestamp, ts("2026-04-17T08:00:05Z"));
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
prompt_mode = "arg"

[[providers]]
name = "fixture-provider"
command = "fixture"
"#,
            || {
                let db = test_db();
                let start = InvocationStart {
                    invocation_uuid: Uuid::new_v4().to_string(),
                    model_name: "legacy-model".to_string(),
                    provider_name: "fixture-provider".to_string(),
                    provider_index: 0,
                    parent_invocation_id: None,
                    quota_tight_routing: false,
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
        assert!(db.get_provider("nonexistent", 0).unwrap().is_none());
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
}
