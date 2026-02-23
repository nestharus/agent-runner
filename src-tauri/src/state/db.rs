use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::Path;

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

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct InvocationRecord {
    pub model_name: String,
    pub provider_index: usize,
    pub success: bool,
    pub exit_code: i32,
    pub error_category: Option<String>,
    pub created_at: DateTime<Utc>,
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
    Number {
        min: Option<f64>,
        max: Option<f64>,
    },
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

            CREATE TABLE IF NOT EXISTS invocations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                model_name TEXT NOT NULL,
                provider_index INTEGER NOT NULL,
                success INTEGER NOT NULL,
                exit_code INTEGER NOT NULL,
                error_category TEXT,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_invocations_model
                ON invocations (model_name, provider_index, created_at);

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
            ",
        )
        .map_err(|e| format!("Failed to initialize schema: {e}"))?;

        Ok(StateDb { conn })
    }

    pub fn open_default() -> Result<Self, String> {
        let data_dir =
            dirs::data_dir().ok_or_else(|| "Could not determine data directory".to_string())?;
        let db_path = data_dir.join("oulipoly-agent-runner").join("state.db");
        Self::open(&db_path)
    }

    pub fn record_invocation(
        &self,
        model_name: &str,
        provider_index: usize,
        success: bool,
        exit_code: i32,
        error_category: Option<&str>,
        stderr_snippet: Option<&str>,
    ) -> Result<(), String> {
        let now = Utc::now().to_rfc3339();

        // Upsert provider stats
        self.conn
            .execute(
                "INSERT INTO providers (model_name, provider_index, invocation_count, error_count, last_invoked_at)
                 VALUES (?1, ?2, 1, ?3, ?4)
                 ON CONFLICT (model_name, provider_index)
                 DO UPDATE SET
                    invocation_count = invocation_count + 1,
                    error_count = error_count + ?3,
                    last_invoked_at = ?4",
                params![model_name, provider_index as i64, if success { 0i64 } else { 1 }, &now],
            )
            .map_err(|e| format!("Failed to upsert provider: {e}"))?;

        // Record error details if failed
        if !success {
            let snippet = stderr_snippet
                .unwrap_or("")
                .chars()
                .take(500)
                .collect::<String>();
            self.conn
                .execute(
                    "UPDATE providers SET last_error = ?1, last_error_at = ?2
                     WHERE model_name = ?3 AND provider_index = ?4",
                    params![&snippet, &now, model_name, provider_index as i64],
                )
                .map_err(|e| format!("Failed to update error info: {e}"))?;
        }

        // Insert invocation log
        self.conn
            .execute(
                "INSERT INTO invocations (model_name, provider_index, success, exit_code, error_category, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    model_name,
                    provider_index as i64,
                    success as i64,
                    exit_code,
                    error_category,
                    &now,
                ],
            )
            .map_err(|e| format!("Failed to insert invocation: {e}"))?;

        Ok(())
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

    fn test_db() -> StateDb {
        StateDb::open(Path::new(":memory:")).unwrap()
    }

    #[test]
    fn schema_creation() {
        let _db = test_db();
    }

    #[test]
    fn record_and_query() {
        let db = test_db();
        db.record_invocation("test-model", 0, true, 0, None, None)
            .unwrap();
        db.record_invocation(
            "test-model",
            0,
            false,
            1,
            Some("rate_limit"),
            Some("429 Too Many Requests"),
        )
        .unwrap();

        let provider = db.get_provider("test-model", 0).unwrap().unwrap();
        assert_eq!(provider.invocation_count, 2);
        assert_eq!(provider.error_count, 1);
        assert!(provider.last_error.is_some());
    }

    #[test]
    fn recent_errors() {
        let db = test_db();
        db.record_invocation("m", 0, false, 1, None, None).unwrap();
        db.record_invocation("m", 0, true, 0, None, None).unwrap();

        let count = db.recent_error_count("m", 0, 60).unwrap();
        assert_eq!(count, 1);
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

        let params = db
            .list_model_parameters("claude-opus-4", "claude")
            .unwrap();
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
