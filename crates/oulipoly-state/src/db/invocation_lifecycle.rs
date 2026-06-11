//! ## Declared roles
//!
//! - orchestration
//! - mapper
//! - accessor
//!
//! Role set: { orchestration, mapper, accessor }

use super::{
    InvocationStart, InvocationStatus, LifecycleInvocationRow, RusqliteOptionalExtension, StateDb,
    lc_log_adapter, sqlite,
};
use crate::result_envelope::{ResultEnvelopeFailureIdentity, ResultEnvelopeInput};

struct FinalizeInvocationRow {
    invocation_uuid: String,
    model_name: String,
    provider_name: Option<String>,
    provider_session_id: Option<String>,
    status: String,
}

type FinalizeInvocationRowColumns = (String, String, Option<String>, Option<String>, String);

type OperationResult = &'static str;

struct FinalizeLifecycleInput<'a> {
    terminal_status_attempt: &'a str,
    exit_code: i32,
    error_category: Option<&'a str>,
    terminal_reason: Option<&'a str>,
    operation_result: OperationResult,
}

fn lifecycle_terminal_status(success: bool) -> &'static str {
    if success { "success" } else { "failed" }
}

fn active_lifecycle_session_id(row: &LifecycleInvocationRow) -> Option<String> {
    row.provider_session_id
        .clone()
        .or_else(|| row.session_id.clone())
}

impl StateDb {
    fn lifecycle_context(&self, start: &InvocationStart) -> lc_log_adapter::StartContext {
        let parent_invocation_uuid = self.load_parent_invocation_uuid(start.parent_invocation_id);
        Self::build_start_context(start, parent_invocation_uuid)
    }

    fn load_parent_invocation_uuid(&self, parent_id: Option<i64>) -> Option<String> {
        let parent_id = parent_id?;
        self.conn
            .query_row(
                "SELECT invocation_uuid FROM invocations WHERE id = ?1",
                sqlite::params![parent_id],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    fn build_start_context(
        start: &InvocationStart,
        parent_invocation_uuid: Option<String>,
    ) -> lc_log_adapter::StartContext {
        lc_log_adapter::StartContext {
            invocation_uuid: start.invocation_uuid.clone(),
            provider_source: Some(start.provider_name.clone()),
            chain_id: None,
            session_id: None,
            latency_us: 0,
            model: Some(start.model_name.clone()),
            provider: Some(start.provider_name.clone()),
            parent_invocation_uuid,
        }
    }

    pub fn start_invocation(&self, start: &InvocationStart) -> Result<i64, String> {
        let timer = lc_log_adapter::start_timer();
        let context = self.lifecycle_context(start);
        let started_at = Self::current_rfc3339_timestamp();
        let sql_result = self.execute_start_invocation_sql(start, &started_at);
        self.warn_invocation_artifact_for_start_result(start, &started_at, &sql_result);
        lc_log_adapter::emit_start(&self.lifecycle_sink, timer, context, &sql_result);
        Self::translate_start_invocation_result(sql_result)
    }

    fn execute_start_invocation_sql(
        &self,
        start: &InvocationStart,
        started_at: &str,
    ) -> Result<i64, std::io::Error> {
        self.insert_invocation_start_row_raw(start, started_at)
            .map_err(Self::start_invocation_io_error)
    }

    fn translate_start_invocation_result(
        result: Result<i64, std::io::Error>,
    ) -> Result<i64, String> {
        result.map_err(|err| err.to_string())
    }

    fn start_invocation_io_error(err: sqlite::Error) -> std::io::Error {
        lc_log_adapter::start_invocation_io_error(err)
    }

    fn insert_invocation_start_row_raw(
        &self,
        start: &InvocationStart,
        started_at: &str,
    ) -> Result<i64, sqlite::Error> {
        self.conn.execute(
            "INSERT INTO invocations (
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
                    created_at,
                    finished_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, NULL, NULL, ?7, NULL)",
            sqlite::params![
                &start.invocation_uuid,
                &start.model_name,
                &start.provider_name,
                start.provider_index as i64,
                start.parent_invocation_id,
                InvocationStatus::Running.as_str(),
                started_at,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    fn warn_invocation_artifact_for_start_result(
        &self,
        start: &InvocationStart,
        started_at: &str,
        result: &Result<i64, std::io::Error>,
    ) {
        if result.is_ok() {
            self.warn_invocation_artifact_failure(start, started_at);
        }
    }

    fn warn_invocation_artifact_failure(&self, start: &InvocationStart, started_at: &str) {
        if let Err(err) = self.write_invocation_artifact(start, started_at) {
            let message =
                Self::format_invocation_artifact_warning_message(&start.invocation_uuid, &err);
            Self::emit_artifact_warning(&message);
        }
    }

    fn format_invocation_artifact_warning_message(
        invocation_uuid: &str,
        err: &dyn std::fmt::Display,
    ) -> String {
        format!("Warning: Failed to write invocation artifact for {invocation_uuid}: {err}")
    }

    fn emit_artifact_warning(message: &str) {
        eprintln!("{message}");
    }

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

    fn warn_result_artifact_for_finalize_result(
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

    fn translate_finalize_invocation_result(
        result: Result<FinalizeInvocationRow, String>,
    ) -> Result<(), String> {
        result.map(|_| ())
    }

    fn is_finalize_result_success(result: &Result<(), String>) -> bool {
        result.is_ok()
    }

    fn is_finalize_sqlite_error(
        id: i64,
        lifecycle_row: Option<&LifecycleInvocationRow>,
        result: &Result<(), String>,
    ) -> bool {
        result.as_ref().err().is_some_and(|message| {
            !Self::is_finalize_context_resolution_error(id, lifecycle_row, message)
        })
    }

    fn is_finalize_context_resolution_error(
        id: i64,
        lifecycle_row: Option<&LifecycleInvocationRow>,
        message: &str,
    ) -> bool {
        lifecycle_row.is_none() && Self::is_invocation_not_found_error(id, message)
    }

    fn finalize_lifecycle_input<'a>(
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

    fn format_terminal_status(
        success: bool,
        _exit_code: i32,
        _terminal_reason: Option<&str>,
    ) -> String {
        lifecycle_terminal_status(success).to_string()
    }

    fn finalize_invocation_transaction(
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

    fn format_begin_transaction_error(err: sqlite::Error) -> String {
        format!("Failed to begin invocation finalize tx: {err}")
    }

    fn format_commit_transaction_error(err: sqlite::Error) -> String {
        format!("Failed to commit invocation finalize tx: {err}")
    }

    fn classify_finalize_operation_result(success: bool, sqlite_error: bool) -> OperationResult {
        if success {
            lc_log_adapter::finalize_operation_result(true, false)
        } else {
            lc_log_adapter::finalize_operation_result(false, sqlite_error)
        }
    }

    fn is_invocation_not_found_error(id: i64, message: &str) -> bool {
        message == Self::format_invocation_not_found_error(id)
    }

    fn format_invocation_not_found_error(id: i64) -> String {
        format!("Invocation {id} not found")
    }

    fn load_invocation_for_finalize(
        conn: &sqlite::Connection,
        id: i64,
    ) -> Result<FinalizeInvocationRow, String> {
        let columns = Self::query_invocation_row_for_finalize(conn, id)
            .map_err(|err| Self::format_load_invocation_for_finalize_error(id, err))?
            .ok_or_else(|| Self::format_invocation_not_found_error(id))?;
        Ok(Self::map_invocation_row_for_finalize(columns))
    }

    fn query_invocation_row_for_finalize(
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

    fn read_invocation_row_for_finalize(
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

    fn map_invocation_row_for_finalize(
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

    fn format_load_invocation_for_finalize_error(id: i64, err: sqlite::Error) -> String {
        format!("Failed to load invocation {id}: {err}")
    }

    fn validate_invocation_is_running(id: i64, status: &str) -> Result<(), String> {
        if status.parse::<InvocationStatus>().ok() == Some(InvocationStatus::Running) {
            Ok(())
        } else {
            Err(format!("Invocation {id} is already finalized"))
        }
    }

    fn write_invocation_final_row(
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

    fn execute_update_invocation_final_row(
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

    fn validate_invocation_final_row_updated(id: i64, updated: usize) -> Result<(), String> {
        if updated == 0 {
            return Err(format!("Invocation {id} is already finalized"));
        }
        Ok(())
    }

    fn format_invocation_final_row_update_error(id: i64, err: sqlite::Error) -> String {
        format!("Failed to finalize invocation {id}: {err}")
    }

    fn terminal_invocation_status(success: bool) -> InvocationStatus {
        if success {
            InvocationStatus::Succeeded
        } else {
            InvocationStatus::Failed
        }
    }

    fn upsert_provider_finalize_aggregate(
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

    fn eligible_provider_name(provider_name: Option<&str>) -> Option<&str> {
        provider_name
    }

    fn is_finalize_failure(success: bool) -> bool {
        !success
    }

    fn execute_provider_finalize_aggregate_sql(
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

    fn provider_error_count_increment(success: bool) -> i64 {
        if success { 0 } else { 1 }
    }

    fn format_provider_finalize_aggregate_sql_error(err: sqlite::Error) -> String {
        format!("Failed to upsert provider: {err}")
    }

    fn update_provider_last_error(
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

    fn map_provider_error_snippet(terminal_reason: Option<&str>) -> Option<String> {
        terminal_reason.map(Self::provider_error_snippet)
    }

    fn execute_update_provider_last_error_sql(
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

    fn format_update_provider_last_error_sql_error(err: sqlite::Error) -> String {
        format!("Failed to update error info: {err}")
    }

    fn provider_error_snippet(value: &str) -> String {
        value.chars().take(500).collect()
    }

    fn warn_result_artifact_failure(&self, input: ResultEnvelopeInput<'_>) {
        if let Err(err) = self.write_result_artifact(input) {
            let message = Self::format_result_artifact_warning_message(input.id, &err);
            Self::emit_artifact_warning(&message);
        }
    }

    fn result_artifact_failure_identity(
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

    fn format_result_artifact_warning_message(
        invocation_uuid: &str,
        err: &dyn std::fmt::Display,
    ) -> String {
        format!("Warning: Failed to write result artifact for {invocation_uuid}: {err}")
    }

    fn finalize_context(
        &self,
        id: i64,
        row: Option<&LifecycleInvocationRow>,
        input: FinalizeLifecycleInput<'_>,
    ) -> lc_log_adapter::FinalizeContext {
        let row_invocation_uuid = Self::load_invocation_uuid_for_finalize(row);
        let fallback_invocation_uuid = Self::format_fallback_invocation_uuid(id);
        let invocation_uuid =
            Self::select_finalize_invocation_uuid(row_invocation_uuid, fallback_invocation_uuid);
        let session_id = Self::load_session_id_for_invocation(row);
        let chain_id_result = self.load_chain_id_for_invocation(id);
        let chain_id = Self::map_lifecycle_chain_id(chain_id_result);
        let raw_artifact_paths = self.load_raw_paths_for_finalize(&invocation_uuid);
        Self::build_finalize_context(
            id,
            row,
            invocation_uuid,
            session_id,
            chain_id,
            raw_artifact_paths,
            input,
        )
    }

    fn load_invocation_uuid_for_finalize(row: Option<&LifecycleInvocationRow>) -> Option<String> {
        row.map(Self::clone_lifecycle_invocation_uuid)
    }

    fn select_finalize_invocation_uuid(
        row_invocation_uuid: Option<String>,
        fallback_invocation_uuid: String,
    ) -> String {
        row_invocation_uuid.unwrap_or(fallback_invocation_uuid)
    }

    fn clone_lifecycle_invocation_uuid(row: &LifecycleInvocationRow) -> String {
        row.invocation_uuid.clone()
    }

    fn format_fallback_invocation_uuid(row_id: i64) -> String {
        format!("unresolved-invocation-row-{row_id}")
    }

    fn load_session_id_for_invocation(row: Option<&LifecycleInvocationRow>) -> Option<String> {
        row.and_then(active_lifecycle_session_id)
    }

    fn load_raw_paths_for_finalize(
        &self,
        invocation_uuid: &str,
    ) -> Option<lc_log_adapter::RawArtifactPaths> {
        self.raw_paths_for(invocation_uuid)
    }

    fn build_finalize_context(
        id: i64,
        row: Option<&LifecycleInvocationRow>,
        invocation_uuid: String,
        session_id: Option<String>,
        chain_id: Option<String>,
        raw_artifact_paths: Option<lc_log_adapter::RawArtifactPaths>,
        input: FinalizeLifecycleInput<'_>,
    ) -> lc_log_adapter::FinalizeContext {
        lc_log_adapter::FinalizeContext {
            invocation_uuid,
            provider_source: row.and_then(|row| row.provider_name.clone()),
            chain_id,
            session_id,
            latency_us: 0,
            invocation_row_id: row.map(|_| id),
            terminal_status_attempt: input.terminal_status_attempt.to_string(),
            exit_code: input.exit_code,
            error_category: input.error_category.map(str::to_string),
            terminal_reason: input.terminal_reason.map(str::to_string),
            raw_artifact_paths,
            operation_result: input.operation_result,
        }
    }
}
