//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - orchestration
//! - mapper
//! - predicate
//! - validator
//!
//! Role set: { accessor, formatter, orchestration, mapper, predicate, validator }
//!
//! Invocation finalize row and provider aggregate writes.

use super::*;
use crate::result_envelope::{ResultEnvelopeFailureIdentity, ResultEnvelopeInput};

impl StateDb {
    pub(super) fn load_invocation_for_finalize(
        conn: &sqlite::Connection,
        id: i64,
    ) -> Result<FinalizeInvocationRow, String> {
        let columns = Self::query_invocation_row_for_finalize(conn, id)
            .map_err(|err| Self::format_load_invocation_for_finalize_error(id, err))?
            .ok_or_else(|| Self::format_invocation_not_found_error(id))?;
        Ok(Self::map_invocation_row_for_finalize(columns))
    }

    pub(super) fn query_invocation_row_for_finalize(
        conn: &sqlite::Connection,
        id: i64,
    ) -> sqlite::Result<Option<FinalizeInvocationRowColumns>> {
        conn.query_row(
            "SELECT invocation_uuid, model_name, provider_name, provider_session_id, status
             FROM invocations WHERE id = ?1",
            sqlite::params![id],
            Self::read_invocation_row_for_finalize,
        )
        .optional()
    }

    pub(super) fn read_invocation_row_for_finalize(
        row: &sqlite::Row<'_>,
    ) -> sqlite::Result<FinalizeInvocationRowColumns> {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        ))
    }

    pub(super) fn map_invocation_row_for_finalize(
        columns: FinalizeInvocationRowColumns,
    ) -> FinalizeInvocationRow {
        let (invocation_uuid, model_name, provider_name, provider_session_id, status) = columns;
        FinalizeInvocationRow {
            invocation_uuid,
            model_name,
            provider_name,
            provider_session_id,
            status,
        }
    }

    pub(super) fn format_load_invocation_for_finalize_error(id: i64, err: sqlite::Error) -> String {
        format!("Failed to load invocation {id}: {err}")
    }

    pub(super) fn validate_invocation_is_running(id: i64, status: &str) -> Result<(), String> {
        if status.parse::<InvocationStatus>().ok() == Some(InvocationStatus::Running) {
            Ok(())
        } else {
            Err(format!("Invocation {id} is already finalized"))
        }
    }

    pub(super) fn write_invocation_final_row(
        conn: &sqlite::Connection,
        id: i64,
        success: bool,
        exit_code: i32,
        error_category: Option<&str>,
        terminal_reason: Option<&str>,
        finished_at: &str,
    ) -> Result<(), String> {
        let updated = Self::execute_update_invocation_final_row(
            conn,
            id,
            success,
            exit_code,
            error_category,
            terminal_reason,
            finished_at,
        )
        .map_err(|err| Self::format_invocation_final_row_update_error(id, err))?;
        Self::validate_invocation_final_row_updated(id, updated)
    }

    pub(super) fn execute_update_invocation_final_row(
        conn: &sqlite::Connection,
        id: i64,
        success: bool,
        exit_code: i32,
        error_category: Option<&str>,
        terminal_reason: Option<&str>,
        finished_at: &str,
    ) -> sqlite::Result<usize> {
        conn.execute(
            "UPDATE invocations
             SET status = ?1,
                 success = ?2,
                 exit_code = ?3,
                 error_category = ?4,
                 terminal_reason = ?5,
                 finished_at = ?6
             WHERE id = ?7 AND status = ?8",
            sqlite::params![
                Self::terminal_invocation_status(success).as_str(),
                success as i64,
                exit_code,
                error_category,
                terminal_reason,
                finished_at,
                id,
                InvocationStatus::Running.as_str(),
            ],
        )
    }

    pub(super) fn validate_invocation_final_row_updated(
        id: i64,
        updated: usize,
    ) -> Result<(), String> {
        if updated == 0 {
            return Err(format!("Invocation {id} is already finalized"));
        }
        Ok(())
    }

    pub(super) fn format_invocation_final_row_update_error(id: i64, err: sqlite::Error) -> String {
        format!("Failed to finalize invocation {id}: {err}")
    }

    pub(super) fn terminal_invocation_status(success: bool) -> InvocationStatus {
        if success {
            InvocationStatus::Succeeded
        } else {
            InvocationStatus::Failed
        }
    }

    pub(super) fn upsert_provider_finalize_aggregate(
        conn: &sqlite::Connection,
        model_name: &str,
        provider_name: Option<&str>,
        success: bool,
        terminal_reason: Option<&str>,
        finished_at: &str,
    ) -> Result<(), String> {
        let Some(provider_name) = Self::eligible_provider_name(provider_name) else {
            return Ok(());
        };
        Self::execute_provider_finalize_aggregate_sql(
            conn,
            model_name,
            provider_name,
            success,
            finished_at,
        )
        .map_err(Self::format_provider_finalize_aggregate_sql_error)?;
        if Self::is_finalize_failure(success) {
            Self::update_provider_last_error(
                conn,
                model_name,
                provider_name,
                terminal_reason,
                finished_at,
            )?;
        }
        Ok(())
    }

    pub(super) fn eligible_provider_name(provider_name: Option<&str>) -> Option<&str> {
        provider_name
    }

    pub(super) fn is_finalize_failure(success: bool) -> bool {
        !success
    }

    pub(super) fn execute_provider_finalize_aggregate_sql(
        conn: &sqlite::Connection,
        model_name: &str,
        provider_name: &str,
        success: bool,
        finished_at: &str,
    ) -> sqlite::Result<()> {
        conn.execute(
            "INSERT INTO providers (
                    model_name, provider_name,
                    invocation_count, error_count, last_invoked_at
                 ) VALUES (?1, ?2, 1, ?3, ?4)
                 ON CONFLICT (model_name, provider_name)
                 DO UPDATE SET
                    invocation_count = invocation_count + 1,
                    error_count = error_count + ?3,
                    last_invoked_at = ?4",
            sqlite::params![
                model_name,
                provider_name,
                Self::provider_error_count_increment(success),
                finished_at
            ],
        )?;
        Ok(())
    }

    pub(super) fn provider_error_count_increment(success: bool) -> i64 {
        if success { 0 } else { 1 }
    }

    pub(super) fn format_provider_finalize_aggregate_sql_error(err: sqlite::Error) -> String {
        format!("Failed to upsert provider: {err}")
    }

    pub(super) fn update_provider_last_error(
        conn: &sqlite::Connection,
        model_name: &str,
        provider_name: &str,
        terminal_reason: Option<&str>,
        finished_at: &str,
    ) -> Result<(), String> {
        let snippet = Self::map_provider_error_snippet(terminal_reason);
        Self::execute_update_provider_last_error_sql(
            conn,
            model_name,
            provider_name,
            snippet.as_deref(),
            finished_at,
        )
        .map_err(Self::format_update_provider_last_error_sql_error)?;
        Ok(())
    }

    pub(super) fn map_provider_error_snippet(terminal_reason: Option<&str>) -> Option<String> {
        terminal_reason.map(Self::provider_error_snippet)
    }

    pub(super) fn execute_update_provider_last_error_sql(
        conn: &sqlite::Connection,
        model_name: &str,
        provider_name: &str,
        snippet: Option<&str>,
        finished_at: &str,
    ) -> sqlite::Result<()> {
        conn.execute(
            "UPDATE providers SET last_error = ?1, last_error_at = ?2
             WHERE model_name = ?3 AND provider_name = ?4",
            sqlite::params![snippet, finished_at, model_name, provider_name],
        )?;
        Ok(())
    }

    pub(super) fn format_update_provider_last_error_sql_error(err: sqlite::Error) -> String {
        format!("Failed to update error info: {err}")
    }

    pub(super) fn provider_error_snippet(value: &str) -> String {
        value.chars().take(500).collect()
    }

    pub(super) fn warn_result_artifact_failure(&self, input: ResultEnvelopeInput<'_>) {
        if let Err(err) = self.write_result_artifact(input) {
            let message = Self::format_result_artifact_warning_message(input.id, &err);
            Self::emit_artifact_warning(&message);
        }
    }

    pub(super) fn result_artifact_failure_identity(
        &self,
        invocation: &FinalizeInvocationRow,
    ) -> ResultEnvelopeFailureIdentity {
        let agent_runner_chain_id =
            match (&invocation.provider_name, &invocation.provider_session_id) {
                (Some(provider_name), Some(provider_session_id)) => self
                    .chain_id_for_segment(provider_name, provider_session_id)
                    .ok()
                    .flatten(),
                _ => None,
            };
        ResultEnvelopeFailureIdentity {
            agent_runner_invocation_id: invocation.invocation_uuid.clone(),
            provider_name: invocation.provider_name.clone(),
            provider_session_id: invocation.provider_session_id.clone(),
            agent_runner_chain_id,
        }
    }

    pub(super) fn format_result_artifact_warning_message(
        invocation_uuid: &str,
        err: &dyn std::fmt::Display,
    ) -> String {
        format!("Warning: Failed to write result artifact for {invocation_uuid}: {err}")
    }
}
