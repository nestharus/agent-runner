use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
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

impl StateDb {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create state directory: {e}"))?;
        }

        let conn = Connection::open(path)
            .map_err(|e| format!("Failed to open state DB: {e}"))?;

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
            "
        ).map_err(|e| format!("Failed to initialize schema: {e}"))?;

        Ok(StateDb { conn })
    }

    pub fn open_default() -> Result<Self, String> {
        let data_dir = dirs::data_dir()
            .ok_or_else(|| "Could not determine data directory".to_string())?;
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
            let snippet = stderr_snippet.unwrap_or("").chars().take(500).collect::<String>();
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

    pub fn get_provider(&self, model_name: &str, provider_index: usize) -> Result<Option<ProviderRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT invocation_count, error_count, last_error, last_error_at, last_invoked_at
                 FROM providers WHERE model_name = ?1 AND provider_index = ?2",
            )
            .map_err(|e| format!("Failed to prepare query: {e}"))?;

        let result = stmt
            .query_row(params![model_name, provider_index as i64], |row| {
                Ok(ProviderRecord {
                    model_name: model_name.to_string(),
                    provider_index,
                    invocation_count: row.get::<_, i64>(0)? as u64,
                    error_count: row.get::<_, i64>(1)? as u64,
                    last_error: row.get(2)?,
                    last_error_at: row.get::<_, Option<String>>(3)?
                        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                        .map(|dt| dt.with_timezone(&Utc)),
                    last_invoked_at: row.get::<_, Option<String>>(4)?
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
        db.record_invocation("test-model", 0, true, 0, None, None).unwrap();
        db.record_invocation("test-model", 0, false, 1, Some("rate_limit"), Some("429 Too Many Requests")).unwrap();

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
}
