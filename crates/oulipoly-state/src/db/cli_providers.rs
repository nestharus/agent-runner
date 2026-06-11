//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - mapper
//! - orchestration
//!
//! Role set: { accessor, formatter, mapper, orchestration }
//!
//! CLI provider discovery persistence methods for `StateDb`.

use super::*;

impl StateDb {
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
                sqlite::params![
                    &provider.cli_name,
                    &provider.display_name,
                    provider.installed as i64,
                    &provider.version,
                    &provider.config_dir,
                    &provider.last_synced,
                ],
            )
            .map_err(Self::format_cli_provider_upsert_error)?;
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
            .map_err(Self::format_cli_providers_query_prepare_error)?;

        let rows = stmt
            .query_map([], Self::map_cli_provider_row)
            .map_err(Self::format_cli_providers_query_error)?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(Self::format_cli_provider_row_read_error)?);
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
            .map_err(Self::format_cli_providers_query_prepare_error)?;

        let result = stmt.query_row(sqlite::params![cli_name], Self::map_cli_provider_row);

        match result {
            Ok(record) => Ok(Some(record)),
            Err(sqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(Self::format_cli_provider_query_error(e)),
        }
    }

    fn map_cli_provider_row(row: &sqlite::Row<'_>) -> sqlite::Result<CliProviderRecord> {
        Ok(CliProviderRecord {
            cli_name: row.get(0)?,
            display_name: row.get(1)?,
            installed: row.get::<_, i64>(2)? != 0,
            version: row.get(3)?,
            config_dir: row.get(4)?,
            last_synced: row.get(5)?,
        })
    }

    fn format_cli_provider_upsert_error(e: sqlite::Error) -> String {
        format!("Failed to upsert CLI provider: {e}")
    }

    fn format_cli_providers_query_prepare_error(e: sqlite::Error) -> String {
        format!("Failed to prepare query: {e}")
    }

    fn format_cli_providers_query_error(e: sqlite::Error) -> String {
        format!("Failed to query CLI providers: {e}")
    }

    fn format_cli_provider_row_read_error(e: sqlite::Error) -> String {
        format!("Failed to read provider row: {e}")
    }

    fn format_cli_provider_query_error(e: sqlite::Error) -> String {
        format!("Failed to query CLI provider: {e}")
    }
}
