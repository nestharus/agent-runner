//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - orchestration
//! - mapper
//!
//! Role set: { accessor, formatter, orchestration, mapper }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/invocation_lifecycle_start.rs
//!     role: intrinsic-surface
//!     Domain: invocation-lifecycle-start-persistence
//!     Owns:
//!       - StateDb invocation-lifecycle-start persistence surface: the StateDb methods, owned
//!         tables/rows, and SQL this concern extends, split out of the StateDb
//!         facade by the WU #65 decomposition with the public API preserved
//!       - Intrinsic StateDb/rusqlite carriers and concern-owned DTOs referenced
//!         via `use super::*`, subordinate to this domain: InvocationStart, InvocationStatus, StateDb, lc_log_adapter, params, sqlite
//! ```
//!
//! Invocation start persistence and lifecycle-log context.

use super::*;

pub(super) struct FinalizeInvocationRow {
    pub(super) invocation_uuid: String,
    pub(super) model_name: String,
    pub(super) provider_name: Option<String>,
    pub(super) provider_session_id: Option<String>,
    pub(super) status: String,
}

pub(super) type FinalizeInvocationRowColumns =
    (String, String, Option<String>, Option<String>, String);

pub(super) type OperationResult = &'static str;

pub(super) struct FinalizeLifecycleInput<'a> {
    pub(super) terminal_status_attempt: &'a str,
    pub(super) exit_code: i32,
    pub(super) error_category: Option<&'a str>,
    pub(super) terminal_reason: Option<&'a str>,
    pub(super) operation_result: OperationResult,
}

pub(super) fn lifecycle_terminal_status(success: bool) -> &'static str {
    if success { "success" } else { "failed" }
}

pub(super) fn active_lifecycle_session_id(row: &LifecycleInvocationRow) -> Option<String> {
    row.provider_session_id
        .clone()
        .or_else(|| row.session_id.clone())
}

impl StateDb {
    pub(super) fn lifecycle_context(
        &self,
        start: &InvocationStart,
    ) -> lc_log_adapter::StartContext {
        let parent_invocation_uuid = self.load_parent_invocation_uuid(start.parent_invocation_id);
        Self::build_start_context(start, parent_invocation_uuid)
    }

    pub(super) fn load_parent_invocation_uuid(&self, parent_id: Option<i64>) -> Option<String> {
        let parent_id = parent_id?;
        self.conn
            .query_row(
                "SELECT invocation_uuid FROM invocations WHERE id = ?1",
                sqlite::params![parent_id],
                Self::map_parent_invocation_uuid_row,
            )
            .optional()
            .ok()
            .flatten()
    }

    fn map_parent_invocation_uuid_row(row: &sqlite::Row<'_>) -> sqlite::Result<String> {
        row.get(0)
    }

    pub(super) fn build_start_context(
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

    pub(super) fn execute_start_invocation_sql(
        &self,
        start: &InvocationStart,
        started_at: &str,
    ) -> Result<i64, std::io::Error> {
        self.insert_invocation_start_row_raw(start, started_at)
            .map_err(Self::start_invocation_io_error)
    }

    pub(super) fn translate_start_invocation_result(
        result: Result<i64, std::io::Error>,
    ) -> Result<i64, String> {
        result.map_err(|err| err.to_string())
    }

    pub(super) fn start_invocation_io_error(err: sqlite::Error) -> std::io::Error {
        lc_log_adapter::start_invocation_io_error(err)
    }

    pub(super) fn insert_invocation_start_row_raw(
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

    pub(super) fn warn_invocation_artifact_for_start_result(
        &self,
        start: &InvocationStart,
        started_at: &str,
        result: &Result<i64, std::io::Error>,
    ) {
        if result.is_ok() {
            self.warn_invocation_artifact_failure(start, started_at);
        }
    }

    pub(super) fn warn_invocation_artifact_failure(
        &self,
        start: &InvocationStart,
        started_at: &str,
    ) {
        if let Err(err) = self.write_invocation_artifact(start, started_at) {
            let message =
                Self::format_invocation_artifact_warning_message(&start.invocation_uuid, &err);
            Self::emit_artifact_warning(&message);
        }
    }

    pub(super) fn format_invocation_artifact_warning_message(
        invocation_uuid: &str,
        err: &dyn std::fmt::Display,
    ) -> String {
        format!("Warning: Failed to write invocation artifact for {invocation_uuid}: {err}")
    }

    pub(super) fn emit_artifact_warning(message: &str) {
        eprintln!("{message}");
    }
}
