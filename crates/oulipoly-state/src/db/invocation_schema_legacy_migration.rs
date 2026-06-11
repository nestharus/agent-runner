//! ## Declared roles
//!
//! - validator
//! - orchestration
//! - mapper
//!
//! Role set: { validator, orchestration, mapper }
//!
//! Legacy invocation table rebuild migration helpers.

use super::*;

impl StateDb {
    pub(super) fn migrate_legacy_invocations(conn: &sqlite::Connection) -> Result<(), String> {
        let provider_names = Self::provider_name_lookup()?;
        let tx = Self::begin_invocation_migration(conn)?;
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
        Self::create_migrated_invocation_indexes(&tx)?;
        Self::ensure_invocations_row_version_support(&tx)?;

        Self::commit_invocation_migration(tx)
    }

    fn begin_invocation_migration(conn: &sqlite::Connection) -> Result<Transaction<'_>, String> {
        conn.unchecked_transaction()
            .map_err(Self::format_invocation_migration_begin_error)
    }

    fn format_invocation_migration_begin_error(err: sqlite::Error) -> String {
        format!("Failed to begin invocation migration: {err}")
    }

    fn commit_invocation_migration(tx: Transaction<'_>) -> Result<(), String> {
        tx.commit()
            .map_err(Self::format_invocation_migration_commit_error)
    }

    fn format_invocation_migration_commit_error(err: sqlite::Error) -> String {
        format!("Failed to commit invocation migration: {err}")
    }

    fn create_migrated_invocation_indexes(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch(Self::invocations_index_sql())
            .map_err(Self::format_migrated_invocation_indexes_error)
    }

    fn format_migrated_invocation_indexes_error(err: sqlite::Error) -> String {
        format!("Failed to create migrated invocation indexes: {err}")
    }

    pub(super) fn legacy_invocations_count(conn: &sqlite::Connection) -> Result<i64, String> {
        conn.query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
            .map_err(Self::format_legacy_invocations_count_error)
    }

    fn format_legacy_invocations_count_error(err: sqlite::Error) -> String {
        format!("Failed to count legacy invocations before rebuild: {err}")
    }

    pub(super) fn load_legacy_invocation_rows(
        conn: &sqlite::Connection,
    ) -> Result<Vec<LegacyInvocationRow>, String> {
        let mut stmt = Self::prepare_legacy_invocation_rows_query(conn)?;
        Self::read_legacy_invocation_rows(&mut stmt)
    }

    fn prepare_legacy_invocation_rows_query(
        conn: &sqlite::Connection,
    ) -> Result<sqlite::Statement<'_>, String> {
        conn.prepare(
            "SELECT model_name, provider_index, success, exit_code, error_category, created_at
                 FROM invocations
                 ORDER BY id",
        )
        .map_err(Self::format_legacy_invocations_read_error)
    }

    fn format_legacy_invocations_read_error(err: sqlite::Error) -> String {
        format!("Failed to read legacy invocations: {err}")
    }

    fn read_legacy_invocation_rows(
        stmt: &mut sqlite::Statement<'_>,
    ) -> Result<Vec<LegacyInvocationRow>, String> {
        let rows = stmt
            .query_map([], Self::map_legacy_invocation_row)
            .map_err(Self::format_legacy_invocations_scan_error)?;
        Self::collect_legacy_invocation_rows(rows)
    }

    fn format_legacy_invocations_scan_error(err: sqlite::Error) -> String {
        format!("Failed to scan legacy invocations: {err}")
    }

    fn collect_legacy_invocation_rows<I>(rows: I) -> Result<Vec<LegacyInvocationRow>, String>
    where
        I: IntoIterator<Item = sqlite::Result<LegacyInvocationRow>>,
    {
        rows.into_iter()
            .collect::<Result<Vec<_>, _>>()
            .map_err(Self::format_legacy_invocation_parse_error)
    }

    fn format_legacy_invocation_parse_error(err: sqlite::Error) -> String {
        format!("Failed to parse legacy invocation: {err}")
    }

    pub(super) fn map_legacy_invocation_row(
        row: &sqlite::Row<'_>,
    ) -> sqlite::Result<LegacyInvocationRow> {
        Ok(LegacyInvocationRow {
            model_name: row.get(0)?,
            provider_index: row.get(1)?,
            success: row.get(2)?,
            exit_code: row.get(3)?,
            error_category: row.get(4)?,
            created_at: row.get(5)?,
        })
    }

    pub(super) fn validate_legacy_invocation_scan_count(
        scanned: usize,
        old_count: i64,
    ) -> Result<(), String> {
        if Self::legacy_invocation_scan_count_matches(scanned, old_count) {
            Ok(())
        } else {
            Err(Self::format_legacy_invocation_scan_count_error(
                scanned, old_count,
            ))
        }
    }

    fn legacy_invocation_scan_count_matches(scanned: usize, old_count: i64) -> bool {
        scanned as i64 == old_count
    }

    fn format_legacy_invocation_scan_count_error(scanned: usize, old_count: i64) -> String {
        format!(
            "Legacy invocation rebuild aborted before replacement: scanned {scanned} rows but table count was {old_count}"
        )
    }

    pub(super) fn create_migrated_invocations_table(
        conn: &sqlite::Connection,
    ) -> Result<(), String> {
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
        .map_err(Self::format_migrated_invocations_table_create_error)
    }

    fn format_migrated_invocations_table_create_error(err: sqlite::Error) -> String {
        format!("Failed to create migrated invocations table: {err}")
    }

    pub(super) fn insert_migrated_invocation_rows(
        conn: &sqlite::Connection,
        rows: Vec<LegacyInvocationRow>,
        provider_names: &HashMap<(String, usize), String>,
    ) -> Result<(), String> {
        let mut insert = Self::prepare_migrated_invocation_insert(conn)?;
        for row in rows {
            let migrated = Self::map_legacy_invocation_insert(row, provider_names);
            Self::execute_migrated_invocation_insert(&mut insert, migrated)?;
        }
        Ok(())
    }

    fn prepare_migrated_invocation_insert(
        conn: &sqlite::Connection,
    ) -> Result<sqlite::Statement<'_>, String> {
        conn.prepare(Self::migrated_invocation_insert_sql())
            .map_err(Self::format_migrated_invocation_insert_prepare_error)
    }

    fn format_migrated_invocation_insert_prepare_error(err: sqlite::Error) -> String {
        format!("Failed to prepare migrated invocation insert: {err}")
    }

    fn execute_migrated_invocation_insert(
        insert: &mut sqlite::Statement<'_>,
        migrated: LegacyInvocationInsert,
    ) -> Result<(), String> {
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
            .map_err(Self::format_legacy_invocation_copy_error)?;
        Ok(())
    }

    fn format_legacy_invocation_copy_error(err: sqlite::Error) -> String {
        format!("Failed to copy legacy invocation: {err}")
    }

    pub(super) fn migrated_invocation_insert_sql() -> &'static str {
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

    pub(super) fn map_legacy_invocation_insert(
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

    pub(super) fn legacy_invocation_status(
        provider_name: Option<&String>,
        success: i64,
    ) -> InvocationStatus {
        match provider_name {
            Some(_) if success != 0 => InvocationStatus::Succeeded,
            Some(_) => InvocationStatus::Failed,
            None => InvocationStatus::Legacy,
        }
    }

    pub(super) fn migrated_invocations_count(conn: &sqlite::Connection) -> Result<i64, String> {
        conn.query_row("SELECT COUNT(*) FROM invocations_new", [], |row| row.get(0))
            .map_err(Self::format_migrated_invocations_count_error)
    }

    fn format_migrated_invocations_count_error(err: sqlite::Error) -> String {
        format!("Failed to count migrated invocations before replacement: {err}")
    }

    pub(super) fn validate_migrated_invocation_count(
        new_count: i64,
        old_count: i64,
    ) -> Result<(), String> {
        if Self::migrated_invocation_count_matches(new_count, old_count) {
            Ok(())
        } else {
            Err(Self::format_migrated_invocation_count_mismatch_error(
                new_count, old_count,
            ))
        }
    }

    fn migrated_invocation_count_matches(new_count: i64, old_count: i64) -> bool {
        new_count == old_count
    }

    fn format_migrated_invocation_count_mismatch_error(new_count: i64, old_count: i64) -> String {
        format!(
            "Legacy invocation rebuild aborted before replacement: migrated {new_count} rows from {old_count}"
        )
    }

    pub(super) fn replace_invocations_with_migrated_table(
        conn: &sqlite::Connection,
    ) -> Result<(), String> {
        conn.execute_batch(
            "DROP TABLE invocations;
             ALTER TABLE invocations_new RENAME TO invocations;",
        )
        .map_err(Self::format_invocations_table_replace_error)
    }

    fn format_invocations_table_replace_error(err: sqlite::Error) -> String {
        format!("Failed to replace invocations table: {err}")
    }

    /// Resolve `(model_name, provider_index) -> provider_name` from the
    /// installed models config, used by the legacy-row migration. A corrupt
    /// or missing models directory must not block DB open: log on stderr and
    /// return an empty lookup so unmappable rows fall through to
    /// `status='legacy'` with `provider_name=NULL` (per V10 — degradation
    /// is observable via the legacy status, not silent).
    pub(super) fn provider_name_lookup()
    -> Result<std::collections::HashMap<(String, usize), String>, String> {
        let models = Self::load_models_for_invocation_migration()?;
        Ok(Self::build_provider_name_lookup(models))
    }

    pub(super) fn load_models_for_invocation_migration() -> Result<ModelStore, String> {
        let models_dir = Self::migration_models_dir();
        match load_models(&models_dir, None) {
            Ok(models) => Ok(models),
            Err(e) => {
                Self::warn_model_config_load_failed(&e.to_string());
                Ok(HashMap::new())
            }
        }
    }

    pub(super) fn migration_models_dir() -> PathBuf {
        dirs::config_dir()
            .map(|dir| dir.join("oulipoly-agent-runner").join("models"))
            .unwrap_or_else(|| PathBuf::from("models"))
    }

    pub(super) fn warn_model_config_load_failed(error: &str) {
        eprintln!(
            "Warning: failed to load models config during invocation migration ({error}); \
             pre-existing invocation rows will migrate as status='legacy'."
        );
    }

    pub(super) fn build_provider_name_lookup(
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
