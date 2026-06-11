//! ## Declared roles
//!
//! - accessor
//! - filter
//! - mapper
//! - orchestration
//!
//! Role set: { accessor, filter, mapper, orchestration }
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
            .map_err(|e| format!("Failed to upsert discovered model: {e}"))?;
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
            .map_err(|e| format!("Failed to prepare query: {e}"))?;

        let rows = if let Some(provider) = bind_provider {
            stmt.query_map(sqlite::params![provider], Self::map_discovered_model_row)
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
                sqlite::params![provider, current_cli_version],
            )
            .map_err(|e| format!("Failed to delete stale models: {e}"))?;
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
}
