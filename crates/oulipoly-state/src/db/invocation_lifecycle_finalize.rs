//! ## Declared roles
//!
//! - orchestration
//!
//! Role set: { orchestration }
//!
//! Invocation finalize orchestration and lifecycle-log classification.

use super::*;
use crate::result_envelope::ResultEnvelopeInput;

impl StateDb {
    pub fn finalize_invocation(
        &self,
        id: i64,
        success: bool,
        exit_code: i32,
        error_category: Option<&str>,
        terminal_reason: Option<&str>,
    ) -> Result<(), String> {
        let lifecycle_row = self.lifecycle_context_for_row_or_none(id);
        let timer = lc_log_adapter::start_timer();
        let finished_at = Self::current_rfc3339_timestamp();
        let transaction_result = self.finalize_invocation_transaction(
            id,
            success,
            exit_code,
            error_category,
            terminal_reason,
            &finished_at,
        );
        self.warn_result_artifact_for_finalize_result(
            success,
            exit_code,
            error_category,
            terminal_reason,
            &finished_at,
            &transaction_result,
        );
        let result = Self::translate_finalize_invocation_result(transaction_result);
        let finalize_success = Self::is_finalize_result_success(&result);
        let sqlite_error = Self::is_finalize_sqlite_error(id, lifecycle_row.as_ref(), &result);
        let operation_result =
            Self::classify_finalize_operation_result(finalize_success, sqlite_error);
        let terminal_status = Self::format_terminal_status(success, exit_code, terminal_reason);
        let input = Self::finalize_lifecycle_input(
            &terminal_status,
            exit_code,
            error_category,
            terminal_reason,
            operation_result,
        );
        let context = self.finalize_context(id, lifecycle_row.as_ref(), input);
        lc_log_adapter::emit_finalize(
            &self.lifecycle_sink,
            timer,
            context,
            &result,
            terminal_status,
        );
        result
    }

    pub(super) fn warn_result_artifact_for_finalize_result(
        &self,
        success: bool,
        exit_code: i32,
        error_category: Option<&str>,
        terminal_reason: Option<&str>,
        finished_at: &str,
        result: &Result<FinalizeInvocationRow, String>,
    ) {
        if let Ok(invocation) = result {
            let failure_identity =
                (!success).then(|| self.result_artifact_failure_identity(invocation));
            let input = ResultEnvelopeInput {
                id: &invocation.invocation_uuid,
                success,
                exit_code,
                error_category,
                terminal_reason,
                finished_at,
                failure_identity: failure_identity.as_ref(),
            };
            self.warn_result_artifact_failure(input);
        }
    }

    pub(super) fn translate_finalize_invocation_result(
        result: Result<FinalizeInvocationRow, String>,
    ) -> Result<(), String> {
        result.map(|_| ())
    }

    pub(super) fn is_finalize_result_success(result: &Result<(), String>) -> bool {
        result.is_ok()
    }

    pub(super) fn is_finalize_sqlite_error(
        id: i64,
        lifecycle_row: Option<&LifecycleInvocationRow>,
        result: &Result<(), String>,
    ) -> bool {
        result.as_ref().err().is_some_and(|message| {
            !Self::is_finalize_context_resolution_error(id, lifecycle_row, message)
        })
    }

    pub(super) fn is_finalize_context_resolution_error(
        id: i64,
        lifecycle_row: Option<&LifecycleInvocationRow>,
        message: &str,
    ) -> bool {
        lifecycle_row.is_none() && Self::is_invocation_not_found_error(id, message)
    }

    pub(super) fn finalize_lifecycle_input<'a>(
        terminal_status_attempt: &'a str,
        exit_code: i32,
        error_category: Option<&'a str>,
        terminal_reason: Option<&'a str>,
        operation_result: OperationResult,
    ) -> FinalizeLifecycleInput<'a> {
        FinalizeLifecycleInput {
            terminal_status_attempt,
            exit_code,
            error_category,
            terminal_reason,
            operation_result,
        }
    }

    pub(super) fn format_terminal_status(
        success: bool,
        _exit_code: i32,
        _terminal_reason: Option<&str>,
    ) -> String {
        lifecycle_terminal_status(success).to_string()
    }

    pub(super) fn finalize_invocation_transaction(
        &self,
        id: i64,
        success: bool,
        exit_code: i32,
        error_category: Option<&str>,
        terminal_reason: Option<&str>,
        finished_at: &str,
    ) -> Result<FinalizeInvocationRow, String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(Self::format_begin_transaction_error)?;

        let invocation = Self::load_invocation_for_finalize(&tx, id)?;
        Self::validate_invocation_is_running(id, &invocation.status)?;
        Self::write_invocation_final_row(
            &tx,
            id,
            success,
            exit_code,
            error_category,
            terminal_reason,
            finished_at,
        )?;
        Self::upsert_provider_finalize_aggregate(
            &tx,
            &invocation.model_name,
            invocation.provider_name.as_deref(),
            success,
            terminal_reason,
            finished_at,
        )?;

        tx.commit().map_err(Self::format_commit_transaction_error)?;
        Ok(invocation)
    }

    pub(super) fn format_begin_transaction_error(err: sqlite::Error) -> String {
        format!("Failed to begin invocation finalize tx: {err}")
    }

    pub(super) fn format_commit_transaction_error(err: sqlite::Error) -> String {
        format!("Failed to commit invocation finalize tx: {err}")
    }

    pub(super) fn classify_finalize_operation_result(
        success: bool,
        sqlite_error: bool,
    ) -> OperationResult {
        if success {
            lc_log_adapter::finalize_operation_result(true, false)
        } else {
            lc_log_adapter::finalize_operation_result(false, sqlite_error)
        }
    }

    pub(super) fn is_invocation_not_found_error(id: i64, message: &str) -> bool {
        message == Self::format_invocation_not_found_error(id)
    }

    pub(super) fn format_invocation_not_found_error(id: i64) -> String {
        format!("Invocation {id} not found")
    }
}
