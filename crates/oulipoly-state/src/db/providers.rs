//! ## Declared roles
//!
//! - accessor
//! - mapper
//! - mutator
//!
//! Role set: { accessor, mapper, mutator }
//!
//! Provider aggregate and round-robin cursor persistence methods for `StateDb`.

use super::*;

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

impl StateDb {
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
}
