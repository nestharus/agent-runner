//! ## Declared roles
//!
//! - accessor
//! - mapper
//! - orchestration
//! - formatter
//! - validator
//!
//! Role set: { accessor, mapper, orchestration, formatter, validator }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/providers.rs
//!     role: intrinsic-surface
//!     Domain: providers-persistence
//!     Owns:
//!       - StateDb providers persistence: the providers table/rows and provider-aggregate SQL this concern owns
//!       - Intrinsic StateDb/rusqlite carriers and DTOs referenced via `use super::*`, subordinate to this domain: StateDb, sqlite, params, DbError, DateTime, Utc
//! ```
//!
//! Provider aggregate and round-robin cursor persistence methods for `StateDb`.

use super::*;

const PROVIDER_RECORD_SQL: &str = "SELECT model_name, provider_name, invocation_count, error_count,
                        last_error, last_error_at, last_invoked_at
                 FROM providers
                 WHERE model_name = ?1 AND provider_name = ?2";

const ROUND_ROBIN_CURSOR_SQL: &str =
    "SELECT last_index FROM model_round_robin_cursor WHERE model_name = ?1";

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
            .prepare(PROVIDER_RECORD_SQL)
            .map_err(Self::format_provider_query_prepare_error)?;

        let result = stmt.query_row(
            sqlite::params![model_name, provider_name],
            Self::map_provider_record_row,
        );

        match result {
            Ok(record) => Ok(Some(record)),
            Err(sqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(Self::format_provider_query_error(e)),
        }
    }

    fn map_provider_record_row(row: &sqlite::Row<'_>) -> sqlite::Result<ProviderRecord> {
        Ok(ProviderRecord {
            model_name: row.get(0)?,
            provider_name: row.get(1)?,
            invocation_count: row.get(2)?,
            error_count: row.get(3)?,
            last_error: row.get(4)?,
            last_error_at: row.get(5)?,
            last_invoked_at: row.get(6)?,
        })
    }

    fn format_provider_query_prepare_error(e: sqlite::Error) -> String {
        format!("Failed to prepare query: {e}")
    }

    fn format_provider_query_error(e: sqlite::Error) -> String {
        format!("Failed to query provider: {e}")
    }

    pub fn recent_error_count(
        &self,
        model_name: &str,
        provider_name: &str,
        window_minutes: i64,
    ) -> Result<i64, String> {
        let cutoff = Self::recent_error_cutoff(window_minutes);

        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM invocations
                 WHERE model_name = ?1 AND provider_name = ?2
                   AND success = 0 AND created_at > ?3",
                sqlite::params![model_name, provider_name, &cutoff],
                Self::map_recent_error_count_row,
            )
            .map_err(Self::format_recent_error_count_error)?;

        Ok(count)
    }

    fn map_recent_error_count_row(row: &sqlite::Row<'_>) -> sqlite::Result<i64> {
        row.get(0)
    }

    fn recent_error_cutoff(window_minutes: i64) -> String {
        (Utc::now() - chrono::Duration::minutes(window_minutes)).to_rfc3339()
    }

    fn format_recent_error_count_error(e: sqlite::Error) -> String {
        format!("Failed to count recent errors: {e}")
    }

    pub fn next_round_robin_index_for_model(
        &self,
        model_name: &str,
    ) -> Result<Option<usize>, String> {
        let Some(value) = self.raw_round_robin_index_for_model(model_name)? else {
            return Ok(None);
        };
        Self::validate_round_robin_index(model_name, value).map(Some)
    }

    fn raw_round_robin_index_for_model(&self, model_name: &str) -> Result<Option<i64>, String> {
        let result = self.conn.query_row(
            ROUND_ROBIN_CURSOR_SQL,
            params![model_name],
            Self::map_round_robin_index_row,
        );
        match result {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(Self::format_round_robin_cursor_query_error(e)),
        }
    }

    fn map_round_robin_index_row(row: &sqlite::Row<'_>) -> sqlite::Result<i64> {
        row.get(0)
    }

    fn validate_round_robin_index(model_name: &str, value: i64) -> Result<usize, String> {
        usize::try_from(value)
            .map_err(|_| Self::format_negative_round_robin_cursor_error(model_name))
    }

    fn format_negative_round_robin_cursor_error(model_name: &str) -> String {
        format!("Negative round-robin cursor for {model_name}")
    }

    fn format_round_robin_cursor_query_error(e: sqlite::Error) -> String {
        format!("Failed to query round-robin cursor: {e}")
    }

    pub fn advance_round_robin_index(
        &self,
        model_name: &str,
        new_index: usize,
        now: DateTime<Utc>,
    ) -> Result<(), String> {
        let cursor = Self::round_robin_cursor_write(new_index, now);
        self.conn
            .execute(
                "INSERT INTO model_round_robin_cursor (model_name, last_index, updated_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT (model_name) DO UPDATE SET
                    last_index = excluded.last_index,
                    updated_at = excluded.updated_at",
                params![model_name, cursor.last_index, cursor.updated_at],
            )
            .map_err(Self::format_advance_round_robin_cursor_error)?;
        Ok(())
    }

    fn round_robin_cursor_write(new_index: usize, now: DateTime<Utc>) -> RoundRobinCursorWrite {
        RoundRobinCursorWrite {
            last_index: new_index as i64,
            updated_at: now.to_rfc3339(),
        }
    }

    fn format_advance_round_robin_cursor_error(e: sqlite::Error) -> String {
        format!("Failed to advance round-robin cursor: {e}")
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
        let last_invoked_at = Self::last_invoked_at_test_timestamp(last_invoked_at);
        self.conn
            .execute(
                "UPDATE providers
                 SET last_invoked_at = ?1
                 WHERE model_name = ?2 AND provider_name = ?3",
                sqlite::params![last_invoked_at, model_name, provider_name],
            )
            .map_err(Self::format_set_last_invoked_at_error)
    }

    #[cfg(any(test, feature = "test-support"))]
    fn last_invoked_at_test_timestamp(timestamp: &DateTime<Utc>) -> String {
        timestamp.to_rfc3339()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn format_set_last_invoked_at_error(e: sqlite::Error) -> String {
        format!("Failed to set last_invoked_at: {e}")
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
            Err(Self::format_last_invoked_at_test_update_error(
                model_name,
                provider_name,
                updated,
            ))
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    fn format_last_invoked_at_test_update_error(
        model_name: &str,
        provider_name: &str,
        updated: usize,
    ) -> String {
        format!(
            "Expected exactly one providers row for model_name={model_name}, provider_name={provider_name}, updated {updated}"
        )
    }
}

struct RoundRobinCursorWrite {
    last_index: i64,
    updated_at: String,
}
