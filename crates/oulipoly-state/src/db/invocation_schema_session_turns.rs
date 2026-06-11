//! ## Declared roles
//!
//! - validator
//! - orchestration
//!
//! Role set: { validator, orchestration }
//!
//! Session-turn validator repair helpers.

use super::*;

impl StateDb {
    pub(super) fn ensure_session_turns_schema(conn: &sqlite::Connection) -> Result<(), String> {
        let columns = Self::session_turns_columns(conn)?;
        Self::execute_column_repairs(
            conn,
            &columns,
            Self::session_turns_column_repairs().as_slice(),
        )?;
        conn.execute_batch(Self::session_turns_index_sql())
            .map_err(|e| format!("Failed to ensure session_turns indexes: {e}"))?;
        Ok(())
    }

    pub(super) fn session_turns_column_repairs() -> [ColumnRepair; 4] {
        [
            ColumnRepair {
                column_name: "parent_turn_id",
                sql: "ALTER TABLE session_turns ADD COLUMN parent_turn_id TEXT",
                error_context: "Failed to add session_turns.parent_turn_id",
            },
            ColumnRepair {
                column_name: "is_sidechain",
                sql: "ALTER TABLE session_turns ADD COLUMN is_sidechain INTEGER NOT NULL DEFAULT 0",
                error_context: "Failed to add session_turns.is_sidechain",
            },
            ColumnRepair {
                column_name: "is_compaction_boundary",
                sql: "ALTER TABLE session_turns ADD COLUMN is_compaction_boundary INTEGER NOT NULL DEFAULT 0",
                error_context: "Failed to add session_turns.is_compaction_boundary",
            },
            ColumnRepair {
                column_name: "body",
                sql: "ALTER TABLE session_turns ADD COLUMN body TEXT",
                error_context: "Failed to add session_turns.body",
            },
        ]
    }

    pub(super) fn session_turns_columns(conn: &sqlite::Connection) -> Result<Vec<String>, String> {
        Self::table_column_names(
            conn,
            "session_turns",
            "Failed to inspect session_turns schema",
            "Failed to inspect session_turns columns",
            "Failed to read session_turns column",
        )
    }

    pub(super) fn execute_column_repairs(
        conn: &sqlite::Connection,
        columns: &[String],
        repairs: &[ColumnRepair],
    ) -> Result<(), String> {
        for repair in repairs {
            Self::execute_column_repair_if_absent(conn, columns, repair)?;
        }
        Ok(())
    }

    pub(super) fn execute_column_repair_if_absent(
        conn: &sqlite::Connection,
        columns: &[String],
        repair: &ColumnRepair,
    ) -> Result<(), String> {
        if Self::has_column(columns, repair.column_name) {
            return Ok(());
        }
        conn.execute(repair.sql, [])
            .map_err(|e| format!("{}: {e}", repair.error_context))?;
        Ok(())
    }

    pub(super) fn execute_drop_column_repairs(
        conn: &sqlite::Connection,
        columns: &[String],
        repairs: &[DropColumnRepair],
    ) -> Result<(), String> {
        for repair in repairs {
            Self::execute_drop_column_repair_if_present(conn, columns, repair)?;
        }
        Ok(())
    }

    pub(super) fn execute_drop_column_repair_if_present(
        conn: &sqlite::Connection,
        columns: &[String],
        repair: &DropColumnRepair,
    ) -> Result<(), String> {
        if !Self::has_column(columns, repair.column_name) {
            return Ok(());
        }
        conn.execute(repair.sql, [])
            .map_err(|e| format!("{}: {e}", repair.error_context))?;
        Ok(())
    }

    pub(super) fn invocations_index_sql() -> &'static str {
        "CREATE INDEX IF NOT EXISTS idx_invocations_uuid
            ON invocations (invocation_uuid);
        CREATE INDEX IF NOT EXISTS idx_invocations_parent
            ON invocations (parent_invocation_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_invocations_provider_created
            ON invocations (provider_name, created_at);
        CREATE INDEX IF NOT EXISTS idx_invocations_provider_session
            ON invocations (provider_name, session_id)
            WHERE session_id IS NOT NULL;"
    }

    pub(super) fn session_turns_index_sql() -> &'static str {
        "CREATE INDEX IF NOT EXISTS idx_session_turns_provider_ts
            ON session_turns (provider_name, role, timestamp);
        CREATE INDEX IF NOT EXISTS idx_session_turns_session_ts
            ON session_turns (provider_name, session_id, timestamp);
        CREATE INDEX IF NOT EXISTS idx_session_turns_session_lookup
            ON session_turns (session_id, timestamp);
        CREATE INDEX IF NOT EXISTS idx_session_turns_parent
            ON session_turns (provider_name, session_id, parent_turn_id, timestamp);"
    }
}
