//! ## Declared roles
//!
//! - formatter
//! - mapper
//! - validator
//! - accessor
//! - predicate
//!
//! Role set: { formatter, mapper, validator, accessor, predicate }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/invocation_schema_table.rs
//!     role: intrinsic-surface
//!     Domain: invocation-schema-table-persistence
//!     Owns:
//!       - the StateDb invocation-schema-table surface this concern owns, split from the StateDb
//!         facade by the WU #65 decomposition with the public API preserved
//!       - all StateDb/rusqlite carriers and concern-owned DTOs/macros referenced
//!         via `use super::*`, subordinate to this domain
//! ```
//!
//! Invocation table DDL and table-column inspection helpers.

use super::*;

pub(super) struct LegacyInvocationRow {
    pub(super) model_name: String,
    pub(super) provider_index: i64,
    pub(super) success: i64,
    pub(super) exit_code: i64,
    pub(super) error_category: Option<String>,
    pub(super) created_at: String,
}

pub(super) struct LegacyInvocationInsert {
    pub(super) invocation_uuid: String,
    pub(super) model_name: String,
    pub(super) provider_name: Option<String>,
    pub(super) provider_index: i64,
    pub(super) status: InvocationStatus,
    pub(super) success: i64,
    pub(super) exit_code: i64,
    pub(super) error_category: Option<String>,
    pub(super) created_at: String,
}

impl StateDb {
    pub(super) fn invocations_schema_sql() -> &'static str {
        concat!(
            "CREATE TABLE IF NOT EXISTS invocations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            invocation_uuid TEXT NOT NULL UNIQUE,
            model_name TEXT NOT NULL,
            provider_name TEXT,
            provider_index INTEGER NOT NULL,
            parent_invocation_id INTEGER REFERENCES invocations(id),
            status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'legacy')),
            success INTEGER,
            exit_code INTEGER,
            error_category TEXT,
            terminal_reason TEXT,
            session_id TEXT,
            session_capture_method TEXT,
            provider_session_id TEXT,
            resume_input_id TEXT,
            provider_session_capture_method TEXT,
            provider_session_resolved_account TEXT,
            resume_acceptance_status TEXT,
            resume_acceptance_evidence TEXT,
            created_at TEXT NOT NULL,
            finished_at TEXT,
            row_version INTEGER NOT NULL DEFAULT 0,
            completion_registration_capability_digest TEXT
                CONSTRAINT invocation_completion_registration_capability_digest_shape
                CHECK (
                    completion_registration_capability_digest IS NULL
                    OR (
                        length(completion_registration_capability_digest) = 64
                        AND completion_registration_capability_digest NOT GLOB '*[^0-9a-f]*'
                    )
                )
        );

        CREATE INDEX IF NOT EXISTS idx_invocations_uuid
            ON invocations (invocation_uuid);
        CREATE INDEX IF NOT EXISTS idx_invocations_parent
            ON invocations (parent_invocation_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_invocations_parent_running_created
            ON invocations (
                parent_invocation_id,
                (status = 'running') DESC,
                created_at,
                id
            );
        CREATE INDEX IF NOT EXISTS idx_invocations_running_parent
            ON invocations (parent_invocation_id, id)
            WHERE status = 'running';
        CREATE INDEX IF NOT EXISTS idx_invocations_provider_created
            ON invocations (provider_name, created_at);
        CREATE INDEX IF NOT EXISTS idx_invocations_provider_session
            ON invocations (provider_name, session_id)
            WHERE session_id IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_invocations_provider_provider_session
            ON invocations (provider_name, provider_index, provider_session_id)
            WHERE provider_session_id IS NOT NULL;",
            invocation_returned_artifacts_schema_sql!()
        )
    }

    pub(super) fn table_column_names(
        conn: &sqlite::Connection,
        table_name: &str,
        inspect_context: &str,
        query_context: &str,
        read_context: &str,
    ) -> Result<Vec<String>, String> {
        let pragma = Self::pragma_table_info_sql(table_name);
        Self::query_table_column_names(conn, &pragma, inspect_context, query_context, read_context)
    }

    pub(super) fn pragma_table_info_sql(table_name: &str) -> String {
        format!("PRAGMA table_info({table_name})")
    }

    pub(super) fn query_table_column_names(
        conn: &sqlite::Connection,
        pragma: &str,
        inspect_context: &str,
        query_context: &str,
        read_context: &str,
    ) -> Result<Vec<String>, String> {
        let mut stmt = Self::prepare_table_column_names_query(conn, pragma, inspect_context)?;
        Self::read_table_column_names(&mut stmt, query_context, read_context)
    }

    fn prepare_table_column_names_query<'conn>(
        conn: &'conn sqlite::Connection,
        pragma: &str,
        inspect_context: &str,
    ) -> Result<sqlite::Statement<'conn>, String> {
        conn.prepare(pragma)
            .map_err(|e| Self::format_contextual_sqlite_error(inspect_context, e))
    }

    fn read_table_column_names(
        stmt: &mut sqlite::Statement<'_>,
        query_context: &str,
        read_context: &str,
    ) -> Result<Vec<String>, String> {
        let rows = stmt
            .query_map([], Self::column_name_row_mapper)
            .map_err(|e| Self::format_contextual_sqlite_error(query_context, e))?;
        Self::collect_table_column_rows(rows, read_context)
    }

    pub(super) fn column_name_row_mapper(row: &sqlite::Row<'_>) -> sqlite::Result<String> {
        row.get::<_, String>(1)
    }

    pub(super) fn collect_table_column_rows<I>(
        rows: I,
        read_context: &str,
    ) -> Result<Vec<String>, String>
    where
        I: IntoIterator<Item = sqlite::Result<String>>,
    {
        let mut columns = Vec::new();
        for row in rows {
            columns.push(row.map_err(|e| Self::format_contextual_sqlite_error(read_context, e))?);
        }
        Ok(columns)
    }

    pub(super) fn format_contextual_sqlite_error(context: &str, err: sqlite::Error) -> String {
        format!("{context}: {err}")
    }

    pub(super) fn has_column(columns: &[String], name: &str) -> bool {
        columns.iter().any(|column| column == name)
    }
}
