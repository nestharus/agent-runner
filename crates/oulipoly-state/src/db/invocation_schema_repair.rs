//! ## Declared roles
//!
//! - accessor
//! - filter
//! - formatter
//! - mapper
//! - validator
//! - orchestration
//! - predicate
//!
//! Role set: { accessor, filter, formatter, mapper, validator, orchestration, predicate }
//!
//! Invocation schema shape classification and repair.

use super::*;

// Legacy repair allow-list only. Durable schema changes belong in
// crates/oulipoly-state/migrations/ and schema.rs owns the version.
impl StateDb {
    pub(super) fn ensure_invocations_schema(conn: &sqlite::Connection) -> Result<(), String> {
        let columns = Self::invocations_columns(conn)?;
        match Self::classify_invocations_schema(&columns) {
            InvocationsSchemaShape::Empty => Self::initialize_invocations_schema(conn),
            InvocationsSchemaShape::Current => {
                Self::repair_current_invocations_schema(conn, &columns)
            }
            InvocationsSchemaShape::LegacyPreUuid => Self::migrate_legacy_invocations(conn),
            InvocationsSchemaShape::UnrecognizedPreUuid(columns) => {
                Err(Self::unrecognized_invocations_shape_error(&columns))
            }
        }
    }

    pub(super) fn classify_invocations_schema(columns: &[String]) -> InvocationsSchemaShape {
        if columns.is_empty() {
            InvocationsSchemaShape::Empty
        } else if Self::has_column(columns, "invocation_uuid") {
            InvocationsSchemaShape::Current
        } else if Self::legacy_invocations_shape_is_pre_uuid(columns) {
            InvocationsSchemaShape::LegacyPreUuid
        } else {
            InvocationsSchemaShape::UnrecognizedPreUuid(columns.to_vec())
        }
    }

    pub(super) fn initialize_invocations_schema(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch(Self::invocations_schema_sql())
            .map_err(Self::format_initialize_invocations_schema_error)?;
        Self::ensure_invocations_row_version_support(conn)
    }

    fn format_initialize_invocations_schema_error(err: sqlite::Error) -> String {
        format!("Failed to initialize invocations schema: {err}")
    }

    pub(super) fn repair_current_invocations_schema(
        conn: &sqlite::Connection,
        columns: &[String],
    ) -> Result<(), String> {
        Self::execute_column_repairs(conn, columns, Self::invocations_column_repairs().as_slice())?;
        Self::execute_drop_column_repairs(
            conn,
            columns,
            Self::invocations_drop_column_repairs().as_slice(),
        )?;
        Self::ensure_invocation_indexes(conn)?;
        Self::ensure_invocations_row_version_support(conn)
    }

    fn invocations_drop_column_repairs() -> [DropColumnRepair; 1] {
        [DropColumnRepair {
            column_name: "quota_tight_routing",
            sql: "ALTER TABLE invocations DROP COLUMN quota_tight_routing",
            error_context: "Failed to drop invocations.quota_tight_routing",
        }]
    }

    fn ensure_invocation_indexes(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch(Self::invocations_index_sql())
            .map_err(Self::format_invocation_indexes_error)
    }

    fn format_invocation_indexes_error(err: sqlite::Error) -> String {
        format!("Failed to ensure invocation indexes: {err}")
    }

    pub(super) fn invocations_column_repairs() -> [ColumnRepair; 5] {
        [
            ColumnRepair {
                column_name: "session_id",
                sql: "ALTER TABLE invocations ADD COLUMN session_id TEXT",
                error_context: "Failed to add invocations.session_id",
            },
            ColumnRepair {
                column_name: "session_capture_method",
                sql: "ALTER TABLE invocations ADD COLUMN session_capture_method TEXT",
                error_context: "Failed to add invocations.session_capture_method",
            },
            ColumnRepair {
                column_name: "resume_acceptance_status",
                sql: "ALTER TABLE invocations ADD COLUMN resume_acceptance_status TEXT",
                error_context: "Failed to add invocations.resume_acceptance_status",
            },
            ColumnRepair {
                column_name: "resume_acceptance_evidence",
                sql: "ALTER TABLE invocations ADD COLUMN resume_acceptance_evidence TEXT",
                error_context: "Failed to add invocations.resume_acceptance_evidence",
            },
            ColumnRepair {
                column_name: "terminal_reason",
                sql: "ALTER TABLE invocations ADD COLUMN terminal_reason TEXT",
                error_context: "Failed to add invocations.terminal_reason",
            },
        ]
    }

    pub(super) fn unrecognized_invocations_shape_error(columns: &[String]) -> String {
        format!(
            "Refusing to rebuild populated invocations table with unrecognized pre-UUID shape: {columns:?}"
        )
    }

    pub(super) fn normalize_invocations_columns_excluding_maintenance(
        columns: &[String],
    ) -> Vec<String> {
        let mut names = Self::invocation_columns_without_maintenance(columns);
        names.sort();
        names
    }

    pub(super) fn invocation_columns_without_maintenance(columns: &[String]) -> Vec<String> {
        columns
            .iter()
            .filter(|column| {
                !matches!(
                    column.as_str(),
                    "row_version" | "provider_session_resolved_account"
                )
            })
            .cloned()
            .collect()
    }

    pub(super) fn legacy_invocations_shape_is_pre_uuid(columns: &[String]) -> bool {
        Self::normalize_invocations_columns_excluding_maintenance(columns)
            == [
                "created_at",
                "error_category",
                "exit_code",
                "id",
                "model_name",
                "provider_index",
                "success",
            ]
    }

    pub(super) fn ensure_invocations_row_version_support(
        conn: &sqlite::Connection,
    ) -> Result<(), String> {
        let columns = Self::invocations_columns(conn)?;
        Self::repair_invocations_row_version_column(conn, &columns)?;
        Self::install_invocations_row_version_triggers(conn)
    }

    pub(super) fn repair_invocations_row_version_column(
        conn: &sqlite::Connection,
        columns: &[String],
    ) -> Result<(), String> {
        if Self::invocations_row_version_column_exists(columns) {
            return Ok(());
        }
        Self::add_invocations_row_version_column(conn)
    }

    fn invocations_row_version_column_exists(columns: &[String]) -> bool {
        Self::has_column(columns, "row_version")
    }

    fn add_invocations_row_version_column(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute(
            "ALTER TABLE invocations ADD COLUMN row_version INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .map_err(Self::format_invocations_row_version_add_error)?;
        Ok(())
    }

    fn format_invocations_row_version_add_error(err: sqlite::Error) -> String {
        format!("Failed to add invocations.row_version during repair: {err}")
    }

    pub(super) fn install_invocations_row_version_triggers(
        conn: &sqlite::Connection,
    ) -> Result<(), String> {
        let registration = Self::invocations_row_version_registration()?;
        let trigger_sql = Self::row_version_trigger_sql(registration);
        Self::execute_invocations_row_version_triggers(conn, &trigger_sql)
    }

    fn execute_invocations_row_version_triggers(
        conn: &sqlite::Connection,
        trigger_sql: &str,
    ) -> Result<(), String> {
        conn.execute_batch(trigger_sql)
            .map_err(Self::format_invocations_row_version_triggers_error)
    }

    fn format_invocations_row_version_triggers_error(err: sqlite::Error) -> String {
        format!("Failed to install invocation row-version triggers: {err}")
    }

    pub(super) fn invocations_row_version_registration()
    -> Result<&'static crate::deployment::row_version::registry::TableRegistration, String> {
        crate::deployment::row_version::registry::lookup("invocations")
            .ok_or_else(Self::format_missing_invocations_row_version_registration_error)
    }

    fn format_missing_invocations_row_version_registration_error() -> String {
        "Missing row-version registry entry for invocations during repair".to_string()
    }

    pub(super) fn row_version_trigger_sql(
        registration: &crate::deployment::row_version::registry::TableRegistration,
    ) -> String {
        crate::deployment::row_version::triggers_sql::generate_triggers_for_table(registration)
    }
}
