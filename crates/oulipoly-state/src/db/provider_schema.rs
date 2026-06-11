//! ## Declared roles
//!
//! - accessor
//! - mapper
//! - mutator
//! - validator
//!
//! Role set: { accessor, mapper, mutator, validator }
//!
//! Provider aggregate schema validation, creation, and legacy migration helpers.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderColumn {
    name: String,
    data_type: String,
    notnull: i64,
    pk: i64,
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

    fn classify_providers_schema(columns: &[ProviderColumn]) -> ProvidersSchemaShape {
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

    fn initialize_providers_schema(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch(Self::providers_schema_sql())
            .map_err(|e| format!("Failed to initialize providers schema: {e}"))
    }

    fn migrate_legacy_providers_schema(conn: &mut sqlite::Connection) -> Result<(), String> {
        let tx = conn
            .transaction()
            .map_err(|e| format!("Failed to begin providers migration: {e}"))?;
        Self::rename_legacy_providers_table(&tx)?;
        Self::create_migrated_providers_table(&tx)?;
        Self::rebuild_providers_aggregate(&tx)?;
        Self::rebuild_provider_error_metadata(&tx)?;
        Self::drop_legacy_providers_table(&tx)?;
        tx.commit()
            .map_err(|e| format!("Failed to commit providers migration: {e}"))
    }

    fn rename_legacy_providers_table(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch("ALTER TABLE providers RENAME TO providers_legacy_index_keyed;")
            .map_err(|e| format!("Failed to rename legacy providers table: {e}"))
    }

    fn create_migrated_providers_table(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch(Self::providers_schema_sql())
            .map_err(|e| format!("Failed to create migrated providers table: {e}"))
    }

    fn rebuild_providers_aggregate(conn: &sqlite::Connection) -> Result<(), String> {
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
        .map_err(|e| format!("Failed to rebuild providers aggregate: {e}"))
    }

    fn rebuild_provider_error_metadata(conn: &sqlite::Connection) -> Result<(), String> {
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
        .map_err(|e| format!("Failed to rebuild provider error metadata: {e}"))
    }

    fn drop_legacy_providers_table(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch("DROP TABLE providers_legacy_index_keyed;")
            .map_err(|e| format!("Failed to drop legacy providers table: {e}"))
    }

    fn unexpected_providers_schema_error(description: &str) -> String {
        format!("Unexpected providers schema shape: {description}")
    }

    pub(super) fn validate_providers_schema(conn: &sqlite::Connection) -> Result<(), String> {
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

    fn providers_object_type(conn: &sqlite::Connection) -> Result<Option<String>, String> {
        conn.query_row(
            "SELECT type FROM sqlite_master WHERE name = 'providers'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| format!("Failed to inspect providers object type: {e}"))
    }

    fn providers_has_foreign_keys(conn: &sqlite::Connection) -> Result<bool, String> {
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

    fn providers_columns(conn: &sqlite::Connection) -> Result<Vec<ProviderColumn>, String> {
        Self::query_provider_columns(conn)
    }

    fn query_provider_columns(conn: &sqlite::Connection) -> Result<Vec<ProviderColumn>, String> {
        let mut stmt = conn
            .prepare("PRAGMA table_info(providers)")
            .map_err(|e| Self::format_provider_column_error("inspect providers schema", e))?;
        let rows = stmt
            .query_map([], Self::provider_column_row_mapper)
            .map_err(|e| Self::format_provider_column_error("inspect providers columns", e))?;
        Self::collect_provider_columns(rows)
    }

    fn provider_column_row_mapper(row: &sqlite::Row<'_>) -> sqlite::Result<ProviderColumn> {
        Ok(ProviderColumn {
            name: row.get(1)?,
            data_type: row.get(2)?,
            notnull: row.get(3)?,
            pk: row.get(5)?,
        })
    }

    fn collect_provider_columns<I>(rows: I) -> Result<Vec<ProviderColumn>, String>
    where
        I: IntoIterator<Item = sqlite::Result<ProviderColumn>>,
    {
        let mut columns = Vec::new();
        for row in rows {
            columns.push(
                row.map_err(|e| Self::format_provider_column_error("read providers column", e))?,
            );
        }
        Ok(columns)
    }

    fn format_provider_column_error(operation: &str, err: sqlite::Error) -> String {
        format!("Failed to {operation}: {err}")
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
        Self::provider_column_descriptions(columns).join(", ")
    }

    fn provider_column_descriptions(columns: &[ProviderColumn]) -> Vec<String> {
        columns
            .iter()
            .map(Self::provider_column_description)
            .collect::<Vec<_>>()
    }

    fn provider_column_description(column: &ProviderColumn) -> String {
        format!(
            "{}(type={}, notnull={}, pk={})",
            column.name, column.data_type, column.notnull, column.pk
        )
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
}
