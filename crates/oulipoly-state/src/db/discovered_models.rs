//! ## Declared roles
//!
//! - accessor
//! - filter
//! - formatter
//! - mapper
//! - orchestration
//!
//! Role set: { accessor, filter, formatter, mapper, orchestration }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/discovered_models.rs
//!     role: intrinsic-surface
//!     Domain: discovered-models-persistence
//!     Owns:
//!       - the StateDb discovered-models surface this concern owns, split from the StateDb
//!         facade by the WU #65 decomposition with the public API preserved
//!       - all StateDb/rusqlite carriers and concern-owned DTOs/macros referenced
//!         via `use super::*`, subordinate to this domain
//! ```
//!
//! Discovered model persistence methods for `StateDb`.

use super::*;

impl StateDb {
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
                sqlite::params![
                    &model.canonical_name,
                    &model.provider,
                    &model.discovered_at,
                    &model.cli_version,
                ],
            )
            .map_err(Self::format_discovered_model_upsert_error)?;
        Ok(())
    }

    /// List discovered models, optionally filtered by provider.
    pub fn list_discovered_models(
        &self,
        provider: Option<&str>,
    ) -> Result<Vec<DiscoveredModel>, String> {
        self.load_discovered_model_rows(Self::discovered_model_query(provider))
    }

    fn discovered_model_query(provider: Option<&str>) -> (&'static str, Option<&str>) {
        match provider {
            Some(provider) => (
                "SELECT canonical_name, provider, discovered_at, cli_version
                 FROM discovered_models WHERE provider = ?1
                 ORDER BY canonical_name",
                Some(provider),
            ),
            None => (
                "SELECT canonical_name, provider, discovered_at, cli_version
                 FROM discovered_models
                 ORDER BY provider, canonical_name",
                None,
            ),
        }
    }

    fn load_discovered_model_rows(
        &self,
        query: (&'static str, Option<&str>),
    ) -> Result<Vec<DiscoveredModel>, String> {
        let (sql, bind_provider) = query;
        let mut stmt = self
            .conn
            .prepare(sql)
            .map_err(Self::format_discovered_model_query_prepare_error)?;

        let rows = if let Some(provider) = bind_provider {
            stmt.query_map(sqlite::params![provider], Self::map_discovered_model_row)
                .map_err(Self::format_discovered_models_query_error)?
        } else {
            stmt.query_map([], Self::map_discovered_model_row)
                .map_err(Self::format_discovered_models_query_error)?
        };

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(Self::format_discovered_model_row_read_error)?);
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
                Self::stale_discovered_models_delete_sql(),
                sqlite::params![provider, current_cli_version],
            )
            .map_err(Self::format_stale_discovered_models_delete_error)?;
        Ok(changed as u64)
    }

    /// Helper: map a rusqlite row to a DiscoveredModel.
    fn map_discovered_model_row(row: &sqlite::Row<'_>) -> sqlite::Result<DiscoveredModel> {
        Ok(DiscoveredModel {
            canonical_name: row.get(0)?,
            provider: row.get(1)?,
            discovered_at: row.get(2)?,
            cli_version: row.get(3)?,
        })
    }

    fn stale_discovered_models_delete_sql() -> &'static str {
        "DELETE FROM discovered_models
                 WHERE provider = ?1 AND cli_version != ?2"
    }

    fn format_discovered_model_upsert_error(e: sqlite::Error) -> String {
        format!("Failed to upsert discovered model: {e}")
    }

    fn format_discovered_model_query_prepare_error(e: sqlite::Error) -> String {
        format!("Failed to prepare query: {e}")
    }

    fn format_discovered_models_query_error(e: sqlite::Error) -> String {
        format!("Failed to query discovered models: {e}")
    }

    fn format_discovered_model_row_read_error(e: sqlite::Error) -> String {
        format!("Failed to read model row: {e}")
    }

    fn format_stale_discovered_models_delete_error(e: sqlite::Error) -> String {
        format!("Failed to delete stale models: {e}")
    }
}
