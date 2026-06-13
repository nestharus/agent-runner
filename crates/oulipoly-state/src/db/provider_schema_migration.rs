//! ## Declared roles
//!
//! - formatter
//! - mapper
//! - validator
//! - orchestration
//!
//! Role set: { formatter, mapper, validator, orchestration }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/provider_schema_migration.rs
//!     role: intrinsic-surface
//!     Domain: provider-schema-migration-persistence
//!     Owns:
//!       - the StateDb provider-schema-migration surface this concern owns, split from the StateDb
//!         facade by the WU #65 decomposition with the public API preserved
//!       - all StateDb/rusqlite carriers and concern-owned DTOs/macros referenced
//!         via `use super::*`, subordinate to this domain
//! ```
//!
//! Provider aggregate validator creation and legacy migration.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProviderColumn {
    pub(super) name: String,
    pub(super) data_type: String,
    pub(super) notnull: i64,
    pub(super) pk: i64,
}

impl StateDb {
    pub(super) fn ensure_providers_schema(conn: &mut sqlite::Connection) -> Result<(), String> {
        let columns = Self::providers_columns(conn)?;
        match Self::classify_providers_schema(&columns) {
            ProvidersSchemaShape::Empty => Self::initialize_providers_schema(conn),
            ProvidersSchemaShape::Current => Ok(()),
            ProvidersSchemaShape::LegacyIndexKeyed => Self::migrate_legacy_providers_schema(conn),
            ProvidersSchemaShape::Unexpected(description) => {
                Err(Self::unexpected_providers_schema_error(&description))
            }
        }
    }

    pub(super) fn classify_providers_schema(columns: &[ProviderColumn]) -> ProvidersSchemaShape {
        if columns.is_empty() {
            ProvidersSchemaShape::Empty
        } else if Self::providers_shape_is_post_fix(columns) {
            ProvidersSchemaShape::Current
        } else if Self::providers_shape_is_pre_fix(columns) {
            ProvidersSchemaShape::LegacyIndexKeyed
        } else {
            ProvidersSchemaShape::Unexpected(Self::describe_columns(columns))
        }
    }

    pub(super) fn initialize_providers_schema(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch(Self::providers_schema_sql())
            .map_err(Self::format_initialize_providers_schema_error)
    }

    pub(super) fn migrate_legacy_providers_schema(
        conn: &mut sqlite::Connection,
    ) -> Result<(), String> {
        let tx = conn
            .transaction()
            .map_err(Self::format_begin_providers_migration_error)?;
        Self::rename_legacy_providers_table(&tx)?;
        Self::create_migrated_providers_table(&tx)?;
        Self::rebuild_providers_aggregate(&tx)?;
        Self::rebuild_provider_error_metadata(&tx)?;
        Self::drop_legacy_providers_table(&tx)?;
        tx.commit()
            .map_err(Self::format_commit_providers_migration_error)
    }

    pub(super) fn rename_legacy_providers_table(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch("ALTER TABLE providers RENAME TO providers_legacy_index_keyed;")
            .map_err(Self::format_rename_legacy_providers_table_error)
    }

    pub(super) fn create_migrated_providers_table(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch(Self::providers_schema_sql())
            .map_err(Self::format_create_migrated_providers_table_error)
    }

    pub(super) fn rebuild_providers_aggregate(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch(
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
        .map_err(Self::format_rebuild_providers_aggregate_error)
    }

    pub(super) fn rebuild_provider_error_metadata(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch(
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
        .map_err(Self::format_rebuild_provider_error_metadata_error)
    }

    pub(super) fn drop_legacy_providers_table(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch("DROP TABLE providers_legacy_index_keyed;")
            .map_err(Self::format_drop_legacy_providers_table_error)
    }

    fn format_initialize_providers_schema_error(e: sqlite::Error) -> String {
        format!("Failed to initialize providers schema: {e}")
    }

    fn format_begin_providers_migration_error(e: sqlite::Error) -> String {
        format!("Failed to begin providers migration: {e}")
    }

    fn format_commit_providers_migration_error(e: sqlite::Error) -> String {
        format!("Failed to commit providers migration: {e}")
    }

    fn format_rename_legacy_providers_table_error(e: sqlite::Error) -> String {
        format!("Failed to rename legacy providers table: {e}")
    }

    fn format_create_migrated_providers_table_error(e: sqlite::Error) -> String {
        format!("Failed to create migrated providers table: {e}")
    }

    fn format_rebuild_providers_aggregate_error(e: sqlite::Error) -> String {
        format!("Failed to rebuild providers aggregate: {e}")
    }

    fn format_rebuild_provider_error_metadata_error(e: sqlite::Error) -> String {
        format!("Failed to rebuild provider error metadata: {e}")
    }

    fn format_drop_legacy_providers_table_error(e: sqlite::Error) -> String {
        format!("Failed to drop legacy providers table: {e}")
    }

    pub(super) fn unexpected_providers_schema_error(description: &str) -> String {
        format!("Unexpected providers schema shape: {description}")
    }
}
