//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - validator
//! - mapper
//! - orchestration
//! - predicate
//!
//! Role set: { accessor, formatter, validator, mapper, orchestration, predicate }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/invocation_schema_projection.rs
//!     role: intrinsic-surface
//!     Domain: invocation-schema-projection-persistence
//!     Owns:
//!       - the StateDb invocation-schema-projection persistence surface this concern extends, split
//!         from the StateDb facade with the public API preserved
//!       - intrinsic StateDb/rusqlite carriers and concern-owned DTOs referenced
//!         via `use super::*`, subordinate to this domain: StateDb, sqlite, and the invocation schema/column-projection helper symbols this concern owns
//! ```
//!
//! Invocation dual-id projection SQL helpers.

use super::*;

impl StateDb {
    pub(super) fn invocations_columns(conn: &sqlite::Connection) -> Result<Vec<String>, String> {
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

    pub(super) fn invocations_have_resolved_account_column(
        conn: &sqlite::Connection,
    ) -> Result<bool, String> {
        let columns = Self::invocations_columns(conn)?;
        Ok(Self::columns_have_resolved_account_column(&columns))
    }

    fn columns_have_resolved_account_column(columns: &[String]) -> bool {
        Self::has_column(columns, "provider_session_resolved_account")
    }

    pub(super) fn columns_have_dual_id_columns(columns: &[String]) -> bool {
        Self::has_column(columns, "provider_session_id")
            && Self::has_column(columns, "resume_input_id")
            && Self::has_column(columns, "provider_session_capture_method")
    }

    pub(super) fn promote_existing_dual_id_schema5_if_present(
        conn: &mut sqlite::Connection,
        stored: i32,
    ) -> Result<i32, String> {
        if Self::stored_schema_is_at_least_dual_id_schema5(stored) {
            return Ok(stored);
        }
        let columns = Self::invocations_columns(conn)?;
        if !Self::existing_schema_can_promote_to_dual_id_schema5(&columns) {
            return Ok(stored);
        }
        Self::promote_existing_dual_id_schema5(conn)?;
        Ok(5)
    }

    fn stored_schema_is_at_least_dual_id_schema5(stored: i32) -> bool {
        stored >= 5
    }

    fn existing_schema_can_promote_to_dual_id_schema5(columns: &[String]) -> bool {
        Self::columns_have_dual_id_columns(columns)
    }

    pub(super) fn promote_existing_dual_id_schema5(
        conn: &mut sqlite::Connection,
    ) -> Result<(), String> {
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
        .map_err(|e| Self::report_dual_id_schema5_promotion_error(conn, e))
    }

    fn report_dual_id_schema5_promotion_error(
        conn: &mut sqlite::Connection,
        err: sqlite::Error,
    ) -> String {
        Self::rollback_dual_id_schema5_promotion(conn);
        Self::format_dual_id_schema5_promotion_error(err)
    }

    fn rollback_dual_id_schema5_promotion(conn: &mut sqlite::Connection) {
        let _ = conn.execute_batch("ROLLBACK;");
    }

    fn format_dual_id_schema5_promotion_error(err: sqlite::Error) -> String {
        format!("Failed to promote existing dual-id invocation schema to version 5: {err}")
    }

    pub(super) fn provider_session_expr(
        conn: &sqlite::Connection,
        alias: Option<&str>,
    ) -> Result<String, String> {
        let projection = Self::select_provider_session_projection(conn)?;
        Ok(Self::format_provider_session_expr(projection, alias))
    }

    fn select_provider_session_projection(
        conn: &sqlite::Connection,
    ) -> Result<ProviderSessionProjection, String> {
        Ok(if Self::invocations_have_dual_id_columns(conn)? {
            ProviderSessionProjection::DualId
        } else {
            ProviderSessionProjection::LegacySessionId
        })
    }

    pub(super) fn format_provider_session_expr(
        projection: ProviderSessionProjection,
        alias: Option<&str>,
    ) -> String {
        let prefix = alias.unwrap_or_default();
        match projection {
            ProviderSessionProjection::DualId => Self::dual_id_session_expr(prefix),
            ProviderSessionProjection::LegacySessionId => Self::legacy_session_id_expr(prefix),
        }
    }

    fn dual_id_session_expr(prefix: &str) -> String {
        format!("COALESCE({prefix}provider_session_id, {prefix}session_id)")
    }

    fn legacy_session_id_expr(prefix: &str) -> String {
        format!("{prefix}session_id")
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

    pub(super) fn invocation_dual_id_projection(
        conn: &sqlite::Connection,
    ) -> Result<InvocationDualIdProjection, String> {
        let has_dual_id = Self::invocations_have_dual_id_columns(conn)?;
        let has_resolved_account = Self::invocations_have_resolved_account_column(conn)?;
        Ok(Self::map_invocation_dual_id_projection(
            has_dual_id,
            has_resolved_account,
        ))
    }

    fn map_invocation_dual_id_projection(
        has_dual_id: bool,
        has_resolved_account: bool,
    ) -> InvocationDualIdProjection {
        if has_dual_id {
            if has_resolved_account {
                InvocationDualIdProjection::Current
            } else {
                InvocationDualIdProjection::CurrentWithoutResolvedAccount
            }
        } else {
            InvocationDualIdProjection::Legacy
        }
    }

    pub(super) fn format_invocation_record_select_sql(
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
}
