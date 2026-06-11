//! ## Declared roles
//!
//! - validator
//! - validator
//! - mapper
//!
//! Role set: { validator, mapper }
//!
//! Provider aggregate schema validation and shape matching.

use super::*;

impl StateDb {
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

    pub(super) fn providers_object_type(
        conn: &sqlite::Connection,
    ) -> Result<Option<String>, String> {
        conn.query_row(
            "SELECT type FROM sqlite_master WHERE name = 'providers'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| format!("Failed to inspect providers object type: {e}"))
    }

    pub(super) fn providers_has_foreign_keys(conn: &sqlite::Connection) -> Result<bool, String> {
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

    pub(super) fn providers_columns(
        conn: &sqlite::Connection,
    ) -> Result<Vec<ProviderColumn>, String> {
        Self::query_provider_columns(conn)
    }

    pub(super) fn query_provider_columns(
        conn: &sqlite::Connection,
    ) -> Result<Vec<ProviderColumn>, String> {
        let mut stmt = conn
            .prepare("PRAGMA table_info(providers)")
            .map_err(|e| Self::format_provider_column_error("inspect providers schema", e))?;
        let rows = stmt
            .query_map([], Self::provider_column_row_mapper)
            .map_err(|e| Self::format_provider_column_error("inspect providers columns", e))?;
        Self::collect_provider_columns(rows)
    }

    pub(super) fn provider_column_row_mapper(
        row: &sqlite::Row<'_>,
    ) -> sqlite::Result<ProviderColumn> {
        Ok(ProviderColumn {
            name: row.get(1)?,
            data_type: row.get(2)?,
            notnull: row.get(3)?,
            pk: row.get(5)?,
        })
    }

    pub(super) fn collect_provider_columns<I>(rows: I) -> Result<Vec<ProviderColumn>, String>
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

    pub(super) fn format_provider_column_error(operation: &str, err: sqlite::Error) -> String {
        format!("Failed to {operation}: {err}")
    }

    pub(super) fn providers_shape_is_post_fix(columns: &[ProviderColumn]) -> bool {
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

    pub(super) fn providers_shape_is_pre_fix(columns: &[ProviderColumn]) -> bool {
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

    pub(super) fn columns_match_allowing_row_version(
        columns: &[ProviderColumn],
        expected: &[(&str, &str, i64, i64)],
    ) -> bool {
        Self::columns_match(columns, expected)
            || columns.len() == expected.len() + 1
                && Self::columns_match(&columns[..expected.len()], expected)
                && Self::column_matches(&columns[expected.len()], "row_version", "INTEGER", 1, 0)
    }

    pub(super) fn columns_match(
        columns: &[ProviderColumn],
        expected: &[(&str, &str, i64, i64)],
    ) -> bool {
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

    pub(super) fn column_matches(
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

    pub(super) fn describe_columns(columns: &[ProviderColumn]) -> String {
        Self::provider_column_descriptions(columns).join(", ")
    }

    pub(super) fn provider_column_descriptions(columns: &[ProviderColumn]) -> Vec<String> {
        columns
            .iter()
            .map(Self::provider_column_description)
            .collect::<Vec<_>>()
    }

    pub(super) fn provider_column_description(column: &ProviderColumn) -> String {
        format!(
            "{}(type={}, notnull={}, pk={})",
            column.name, column.data_type, column.notnull, column.pk
        )
    }

    pub(super) fn providers_schema_sql() -> &'static str {
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
