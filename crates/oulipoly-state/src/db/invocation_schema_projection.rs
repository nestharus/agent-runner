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
        Ok(columns
            .iter()
            .any(|column| column == "provider_session_resolved_account"))
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

    pub(super) fn format_provider_session_expr(
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

    pub(super) fn invocation_dual_id_projection(
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
