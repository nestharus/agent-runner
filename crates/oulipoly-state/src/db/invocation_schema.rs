//! ## Declared roles
//!
//! - accessor
//! - mapper
//! - mutator
//! - schema
//! - validator
//!
//! Role set: { accessor, mapper, mutator, schema, validator }
//!
//! Invocation and session-turn schema repair plus legacy invocation migration helpers.

use super::*;

struct LegacyInvocationRow {
    model_name: String,
    provider_index: i64,
    success: i64,
    exit_code: i64,
    error_category: Option<String>,
    created_at: String,
}

struct LegacyInvocationInsert {
    invocation_uuid: String,
    model_name: String,
    provider_name: Option<String>,
    provider_index: i64,
    status: InvocationStatus,
    success: i64,
    exit_code: i64,
    error_category: Option<String>,
    created_at: String,
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
            row_version INTEGER NOT NULL DEFAULT 0
        );

        CREATE INDEX IF NOT EXISTS idx_invocations_uuid
            ON invocations (invocation_uuid);
        CREATE INDEX IF NOT EXISTS idx_invocations_parent
            ON invocations (parent_invocation_id, created_at);
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

    fn pragma_table_info_sql(table_name: &str) -> String {
        format!("PRAGMA table_info({table_name})")
    }

    fn query_table_column_names(
        conn: &sqlite::Connection,
        pragma: &str,
        inspect_context: &str,
        query_context: &str,
        read_context: &str,
    ) -> Result<Vec<String>, String> {
        let mut stmt = conn
            .prepare(pragma)
            .map_err(|e| Self::format_contextual_sqlite_error(inspect_context, e))?;
        let rows = stmt
            .query_map([], Self::column_name_row_mapper)
            .map_err(|e| Self::format_contextual_sqlite_error(query_context, e))?;
        Self::collect_table_column_rows(rows, read_context)
    }

    fn column_name_row_mapper(row: &sqlite::Row<'_>) -> sqlite::Result<String> {
        row.get::<_, String>(1)
    }

    fn collect_table_column_rows<I>(rows: I, read_context: &str) -> Result<Vec<String>, String>
    where
        I: IntoIterator<Item = sqlite::Result<String>>,
    {
        let mut columns = Vec::new();
        for row in rows {
            columns.push(row.map_err(|e| Self::format_contextual_sqlite_error(read_context, e))?);
        }
        Ok(columns)
    }

    fn format_contextual_sqlite_error(context: &str, err: sqlite::Error) -> String {
        format!("{context}: {err}")
    }

    pub(super) fn has_column(columns: &[String], name: &str) -> bool {
        columns.iter().any(|column| column == name)
    }

    // Legacy repair allow-list only. Durable schema changes belong in
    // crates/oulipoly-state/migrations/ and schema.rs owns the version.
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

    fn classify_invocations_schema(columns: &[String]) -> InvocationsSchemaShape {
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

    fn initialize_invocations_schema(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch(Self::invocations_schema_sql())
            .map_err(|e| format!("Failed to initialize invocations schema: {e}"))?;
        Self::ensure_invocations_row_version_support(conn)
    }

    fn repair_current_invocations_schema(
        conn: &sqlite::Connection,
        columns: &[String],
    ) -> Result<(), String> {
        Self::execute_column_repairs(conn, columns, Self::invocations_column_repairs().as_slice())?;
        let drop_repairs = [DropColumnRepair {
            column_name: "quota_tight_routing",
            sql: "ALTER TABLE invocations DROP COLUMN quota_tight_routing",
            error_context: "Failed to drop invocations.quota_tight_routing",
        }];
        Self::execute_drop_column_repairs(conn, columns, drop_repairs.as_slice())?;
        conn.execute_batch(Self::invocations_index_sql())
            .map_err(|e| format!("Failed to ensure invocation indexes: {e}"))?;
        Self::ensure_invocations_row_version_support(conn)
    }

    fn invocations_column_repairs() -> [ColumnRepair; 5] {
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

    fn unrecognized_invocations_shape_error(columns: &[String]) -> String {
        format!(
            "Refusing to rebuild populated invocations table with unrecognized pre-UUID shape: {columns:?}"
        )
    }

    fn normalize_invocations_columns_excluding_maintenance(columns: &[String]) -> Vec<String> {
        let mut names = Self::invocation_columns_without_maintenance(columns);
        names.sort();
        names
    }

    fn invocation_columns_without_maintenance(columns: &[String]) -> Vec<String> {
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

    fn legacy_invocations_shape_is_pre_uuid(columns: &[String]) -> bool {
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

    fn ensure_invocations_row_version_support(conn: &sqlite::Connection) -> Result<(), String> {
        let columns = Self::invocations_columns(conn)?;
        Self::repair_invocations_row_version_column(conn, &columns)?;
        Self::install_invocations_row_version_triggers(conn)
    }

    fn repair_invocations_row_version_column(
        conn: &sqlite::Connection,
        columns: &[String],
    ) -> Result<(), String> {
        if Self::has_column(columns, "row_version") {
            return Ok(());
        }
        conn.execute(
            "ALTER TABLE invocations ADD COLUMN row_version INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .map_err(|e| format!("Failed to add invocations.row_version during repair: {e}"))?;
        Ok(())
    }

    fn install_invocations_row_version_triggers(conn: &sqlite::Connection) -> Result<(), String> {
        let registration = Self::invocations_row_version_registration()?;
        let trigger_sql = Self::row_version_trigger_sql(registration);
        conn.execute_batch(&trigger_sql)
            .map_err(|e| format!("Failed to install invocation row-version triggers: {e}"))
    }

    fn invocations_row_version_registration()
    -> Result<&'static crate::deployment::row_version::registry::TableRegistration, String> {
        crate::deployment::row_version::registry::lookup("invocations").ok_or_else(|| {
            "Missing row-version registry entry for invocations during repair".to_string()
        })
    }

    fn row_version_trigger_sql(
        registration: &crate::deployment::row_version::registry::TableRegistration,
    ) -> String {
        crate::deployment::row_version::triggers_sql::generate_triggers_for_table(registration)
    }

    fn invocations_columns(conn: &sqlite::Connection) -> Result<Vec<String>, String> {
        Self::table_column_names(
            conn,
            "invocations",
            "Failed to inspect invocations schema",
            "Failed to inspect invocations columns",
            "Failed to read invocations column",
        )
    }

    pub(super) fn invocations_have_dual_id_columns(
        conn: &sqlite::Connection,
    ) -> Result<bool, String> {
        let columns = Self::invocations_columns(conn)?;
        Ok(Self::columns_have_dual_id_columns(&columns))
    }

    fn invocations_have_resolved_account_column(conn: &sqlite::Connection) -> Result<bool, String> {
        let columns = Self::invocations_columns(conn)?;
        Ok(columns
            .iter()
            .any(|column| column == "provider_session_resolved_account"))
    }

    fn columns_have_dual_id_columns(columns: &[String]) -> bool {
        Self::has_column(columns, "provider_session_id")
            && Self::has_column(columns, "resume_input_id")
            && Self::has_column(columns, "provider_session_capture_method")
    }

    pub(super) fn promote_existing_dual_id_schema5_if_present(
        conn: &mut sqlite::Connection,
        stored: i32,
    ) -> Result<i32, String> {
        if stored >= 5 {
            return Ok(stored);
        }
        let columns = Self::invocations_columns(conn)?;
        if !Self::columns_have_dual_id_columns(&columns) {
            return Ok(stored);
        }
        Self::promote_existing_dual_id_schema5(conn)?;
        Ok(5)
    }

    fn promote_existing_dual_id_schema5(conn: &mut sqlite::Connection) -> Result<(), String> {
        conn.execute_batch(
            "BEGIN IMMEDIATE;
             UPDATE invocations
             SET provider_session_id = COALESCE(provider_session_id, session_id),
                 provider_session_capture_method = COALESCE(provider_session_capture_method, session_capture_method)
             WHERE session_id IS NOT NULL
               AND (session_capture_method IS NULL OR session_capture_method <> 'resumed');

             UPDATE invocations
             SET resume_input_id = COALESCE(resume_input_id, session_id)
             WHERE session_id IS NOT NULL
               AND session_capture_method = 'resumed';

             CREATE INDEX IF NOT EXISTS idx_invocations_provider_provider_session
               ON invocations(provider_name, provider_index, provider_session_id)
               WHERE provider_session_id IS NOT NULL;

             PRAGMA user_version = 5;
             COMMIT;",
        )
        .map_err(|e| {
            let _ = conn.execute_batch("ROLLBACK;");
            format!("Failed to promote existing dual-id invocation schema to version 5: {e}")
        })
    }

    pub(super) fn provider_session_expr(
        conn: &sqlite::Connection,
        alias: Option<&str>,
    ) -> Result<String, String> {
        let projection = if Self::invocations_have_dual_id_columns(conn)? {
            ProviderSessionProjection::DualId
        } else {
            ProviderSessionProjection::LegacySessionId
        };
        Ok(Self::format_provider_session_expr(projection, alias))
    }

    fn format_provider_session_expr(
        projection: ProviderSessionProjection,
        alias: Option<&str>,
    ) -> String {
        let prefix = alias.unwrap_or_default();
        match projection {
            ProviderSessionProjection::DualId => {
                format!("COALESCE({prefix}provider_session_id, {prefix}session_id)")
            }
            ProviderSessionProjection::LegacySessionId => format!("{prefix}session_id"),
        }
    }

    pub(super) fn invocation_record_select_sql(
        conn: &sqlite::Connection,
        tail_sql: &str,
    ) -> Result<String, String> {
        let projection = Self::invocation_dual_id_projection(conn)?;
        Ok(Self::format_invocation_record_select_sql(
            projection, tail_sql,
        ))
    }

    fn invocation_dual_id_projection(
        conn: &sqlite::Connection,
    ) -> Result<InvocationDualIdProjection, String> {
        if Self::invocations_have_dual_id_columns(conn)? {
            if Self::invocations_have_resolved_account_column(conn)? {
                Ok(InvocationDualIdProjection::Current)
            } else {
                Ok(InvocationDualIdProjection::CurrentWithoutResolvedAccount)
            }
        } else {
            Ok(InvocationDualIdProjection::Legacy)
        }
    }

    fn format_invocation_record_select_sql(
        projection: InvocationDualIdProjection,
        tail_sql: &str,
    ) -> String {
        let (
            provider_session_id,
            resume_input_id,
            provider_session_capture_method,
            provider_session_resolved_account,
        ) = projection.select_columns();
        format!(
            "SELECT id, invocation_uuid, model_name, provider_name, provider_index,
                    parent_invocation_id, status, success, exit_code, error_category,
                    terminal_reason, session_id, session_capture_method,
                    {provider_session_id}, {resume_input_id}, {provider_session_capture_method},
                    {provider_session_resolved_account},
                    resume_acceptance_status, resume_acceptance_evidence,
                    created_at, finished_at
             FROM invocations
             {tail_sql}"
        )
    }

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

    fn session_turns_column_repairs() -> [ColumnRepair; 4] {
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

    fn session_turns_columns(conn: &sqlite::Connection) -> Result<Vec<String>, String> {
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

    fn execute_column_repair_if_absent(
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

    fn execute_drop_column_repair_if_present(
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

    fn invocations_index_sql() -> &'static str {
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

    fn session_turns_index_sql() -> &'static str {
        "CREATE INDEX IF NOT EXISTS idx_session_turns_provider_ts
            ON session_turns (provider_name, role, timestamp);
        CREATE INDEX IF NOT EXISTS idx_session_turns_session_ts
            ON session_turns (provider_name, session_id, timestamp);
        CREATE INDEX IF NOT EXISTS idx_session_turns_session_lookup
            ON session_turns (session_id, timestamp);
        CREATE INDEX IF NOT EXISTS idx_session_turns_parent
            ON session_turns (provider_name, session_id, parent_turn_id, timestamp);"
    }

    fn migrate_legacy_invocations(conn: &sqlite::Connection) -> Result<(), String> {
        let provider_names = Self::provider_name_lookup()?;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to begin invocation migration: {e}"))?;
        Self::validate_providers_schema(&tx)?;
        // Guardrail order: SELECT COUNT(*) FROM invocations
        let old_count = Self::legacy_invocations_count(&tx)?;
        let old_rows = Self::load_legacy_invocation_rows(&tx)?;
        // Guardrail order: scanned {} rows but table count was {old_count}
        Self::validate_legacy_invocation_scan_count(old_rows.len(), old_count)?;
        // Guardrail order: CREATE TABLE invocations_new
        Self::create_migrated_invocations_table(&tx)?;
        Self::insert_migrated_invocation_rows(&tx, old_rows, &provider_names)?;
        // Guardrail order: SELECT COUNT(*) FROM invocations_new
        let new_count = Self::migrated_invocations_count(&tx)?;
        // Guardrail order: migrated {new_count} rows from {old_count}
        Self::validate_migrated_invocation_count(new_count, old_count)?;
        // Guardrail order: DROP TABLE invocations;
        Self::replace_invocations_with_migrated_table(&tx)?;
        tx.execute_batch(Self::invocations_index_sql())
            .map_err(|e| format!("Failed to create migrated invocation indexes: {e}"))?;
        Self::ensure_invocations_row_version_support(&tx)?;

        tx.commit()
            .map_err(|e| format!("Failed to commit invocation migration: {e}"))
    }

    fn legacy_invocations_count(conn: &sqlite::Connection) -> Result<i64, String> {
        conn.query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
            .map_err(|e| format!("Failed to count legacy invocations before rebuild: {e}"))
    }

    fn load_legacy_invocation_rows(
        conn: &sqlite::Connection,
    ) -> Result<Vec<LegacyInvocationRow>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT model_name, provider_index, success, exit_code, error_category, created_at
                 FROM invocations
                 ORDER BY id",
            )
            .map_err(|e| format!("Failed to read legacy invocations: {e}"))?;
        let rows = stmt
            .query_map([], Self::map_legacy_invocation_row)
            .map_err(|e| format!("Failed to scan legacy invocations: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to parse legacy invocation: {e}"))
    }

    fn map_legacy_invocation_row(row: &sqlite::Row<'_>) -> sqlite::Result<LegacyInvocationRow> {
        Ok(LegacyInvocationRow {
            model_name: row.get(0)?,
            provider_index: row.get(1)?,
            success: row.get(2)?,
            exit_code: row.get(3)?,
            error_category: row.get(4)?,
            created_at: row.get(5)?,
        })
    }

    fn validate_legacy_invocation_scan_count(scanned: usize, old_count: i64) -> Result<(), String> {
        if scanned as i64 == old_count {
            Ok(())
        } else {
            Err(format!(
                "Legacy invocation rebuild aborted before replacement: scanned {scanned} rows but table count was {old_count}"
            ))
        }
    }

    fn create_migrated_invocations_table(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch(
            "CREATE TABLE invocations_new (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                invocation_uuid TEXT NOT NULL UNIQUE,
                model_name TEXT NOT NULL,
                provider_name TEXT,
                provider_index INTEGER NOT NULL,
                parent_invocation_id INTEGER REFERENCES invocations_new(id),
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
                row_version INTEGER NOT NULL DEFAULT 0
            );",
        )
        .map_err(|e| format!("Failed to create migrated invocations table: {e}"))
    }

    fn insert_migrated_invocation_rows(
        conn: &sqlite::Connection,
        rows: Vec<LegacyInvocationRow>,
        provider_names: &HashMap<(String, usize), String>,
    ) -> Result<(), String> {
        let mut insert = conn
            .prepare(Self::migrated_invocation_insert_sql())
            .map_err(|e| format!("Failed to prepare migrated invocation insert: {e}"))?;
        for row in rows {
            let migrated = Self::map_legacy_invocation_insert(row, provider_names);
            insert
                .execute(sqlite::params![
                    migrated.invocation_uuid,
                    migrated.model_name,
                    migrated.provider_name,
                    migrated.provider_index,
                    migrated.status.as_str(),
                    migrated.success,
                    migrated.exit_code,
                    migrated.error_category,
                    migrated.created_at,
                ])
                .map_err(|e| format!("Failed to copy legacy invocation: {e}"))?;
        }
        Ok(())
    }

    fn migrated_invocation_insert_sql() -> &'static str {
        "INSERT INTO invocations_new (
            invocation_uuid,
            model_name,
            provider_name,
            provider_index,
            parent_invocation_id,
            status,
            success,
            exit_code,
            error_category,
            terminal_reason,
            session_id,
            session_capture_method,
            provider_session_id,
            resume_input_id,
            provider_session_capture_method,
            provider_session_resolved_account,
            resume_acceptance_status,
            resume_acceptance_evidence,
            created_at,
            finished_at
         ) VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?7, ?8, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, ?9, ?9)"
    }

    fn map_legacy_invocation_insert(
        row: LegacyInvocationRow,
        provider_names: &HashMap<(String, usize), String>,
    ) -> LegacyInvocationInsert {
        let provider_name = provider_names
            .get(&(row.model_name.clone(), row.provider_index as usize))
            .cloned();
        let status = Self::legacy_invocation_status(provider_name.as_ref(), row.success);
        LegacyInvocationInsert {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: row.model_name,
            provider_name,
            provider_index: row.provider_index,
            status,
            success: row.success,
            exit_code: row.exit_code,
            error_category: row.error_category,
            created_at: row.created_at,
        }
    }

    fn legacy_invocation_status(provider_name: Option<&String>, success: i64) -> InvocationStatus {
        match provider_name {
            Some(_) if success != 0 => InvocationStatus::Succeeded,
            Some(_) => InvocationStatus::Failed,
            None => InvocationStatus::Legacy,
        }
    }

    fn migrated_invocations_count(conn: &sqlite::Connection) -> Result<i64, String> {
        conn.query_row("SELECT COUNT(*) FROM invocations_new", [], |row| row.get(0))
            .map_err(|e| format!("Failed to count migrated invocations before replacement: {e}"))
    }

    fn validate_migrated_invocation_count(new_count: i64, old_count: i64) -> Result<(), String> {
        if new_count == old_count {
            Ok(())
        } else {
            Err(format!(
                "Legacy invocation rebuild aborted before replacement: migrated {new_count} rows from {old_count}"
            ))
        }
    }

    fn replace_invocations_with_migrated_table(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch(
            "DROP TABLE invocations;
             ALTER TABLE invocations_new RENAME TO invocations;",
        )
        .map_err(|e| format!("Failed to replace invocations table: {e}"))
    }

    /// Resolve `(model_name, provider_index) -> provider_name` from the
    /// installed models config, used by the legacy-row migration. A corrupt
    /// or missing models directory must not block DB open: log on stderr and
    /// return an empty lookup so unmappable rows fall through to
    /// `status='legacy'` with `provider_name=NULL` (per V10 — degradation
    /// is observable via the legacy status, not silent).
    fn provider_name_lookup() -> Result<std::collections::HashMap<(String, usize), String>, String>
    {
        let models = Self::load_models_for_invocation_migration()?;
        Ok(Self::build_provider_name_lookup(models))
    }

    fn load_models_for_invocation_migration() -> Result<ModelStore, String> {
        let models_dir = Self::migration_models_dir();
        match load_models(&models_dir, None) {
            Ok(models) => Ok(models),
            Err(e) => {
                Self::warn_model_config_load_failed(&e.to_string());
                Ok(HashMap::new())
            }
        }
    }

    fn migration_models_dir() -> PathBuf {
        dirs::config_dir()
            .map(|dir| dir.join("oulipoly-agent-runner").join("models"))
            .unwrap_or_else(|| PathBuf::from("models"))
    }

    fn warn_model_config_load_failed(error: &str) {
        eprintln!(
            "Warning: failed to load models config during invocation migration ({error}); \
             pre-existing invocation rows will migrate as status='legacy'."
        );
    }

    fn build_provider_name_lookup(
        models: ModelStore,
    ) -> std::collections::HashMap<(String, usize), String> {
        let mut lookup = std::collections::HashMap::new();
        for (model_name, model) in models {
            for (provider_index, provider) in model.providers.iter().enumerate() {
                lookup.insert((model_name.clone(), provider_index), provider.name.clone());
            }
        }
        lookup
    }
}
