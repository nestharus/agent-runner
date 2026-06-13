//! ## Declared roles
//!
//! - accessor
//! - orchestration
//! - mapper
//! - predicate
//! - formatter
//!
//! Role set: { accessor, orchestration, mapper, predicate, formatter }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/session_capture.rs
//!     role: intrinsic-surface
//!     Domain: session-capture-persistence
//!     Owns:
//!       - StateDb session-capture write/read methods over the invocations table
//!       - sqlite (rusqlite re-export) and RusqliteOptionalExtension row access used by session capture
//!       - LifecycleInvocationRow projection and lc_log_adapter lifecycle emission for capture events
//!       - external contract symbols referenced by this concern via its `use`
//!         declarations, intrinsic and subordinate to this persistence domain: LifecycleInvocationRow, RusqliteOptionalExtension, StateDb, lc_log_adapter, sqlite
//! ```
//!
//! Invocation session-capture projection, persistence, and lifecycle reporting.

use super::{LifecycleInvocationRow, RusqliteOptionalExtension, StateDb, lc_log_adapter, sqlite};

struct SessionCaptureProjection<'a> {
    provider_session_id: Option<&'a str>,
    resume_input_id: Option<&'a str>,
    provider_session_capture_method: Option<&'a str>,
}

impl StateDb {
    pub(super) fn lifecycle_context_for_row_or_none(
        &self,
        row_id: i64,
    ) -> Option<LifecycleInvocationRow> {
        self.lifecycle_context_for_row(row_id).ok().flatten()
    }

    /// Update an invocation row's session correlation columns. Per
    /// `tmp/01-pr-c-contract.md` §"DB method additions", this method
    /// takes `method` as a `&str` so the DB layer stays decoupled from
    /// `SessionCaptureMethod` (an executor-internal type).
    ///
    /// Always writes both columns. Per V10 (failures observable, never
    /// silent), a completed invocation with no capture attempted
    /// records `("None", "none")` explicitly — that's a positive
    /// signal distinct from NULL (the row was never finalized). The
    /// last call wins, which matches the multi-call safety semantics.
    pub fn update_session_capture(
        &self,
        id: i64,
        session_id: Option<&str>,
        method: &str,
    ) -> Result<(), String> {
        let lifecycle_row = self.lifecycle_context_for_row_or_none(id);
        let timer = lc_log_adapter::start_timer();
        let projection = Self::project_session_capture(session_id, method);
        let sql_result =
            self.execute_session_capture_persistence(id, session_id, method, projection);
        let result = Self::translate_session_capture_result(id, sql_result);
        let context = self.optional_session_context(id, lifecycle_row.as_ref(), session_id, method);
        lc_log_adapter::emit_session_capture(&self.lifecycle_sink, timer, context, &result);
        result
    }

    fn project_session_capture<'a>(
        session_id: Option<&'a str>,
        method: &'a str,
    ) -> SessionCaptureProjection<'a> {
        let provider_session_id = Self::map_capture_provider_session_id(session_id, method);
        let resume_input_id = Self::map_capture_resume_input_id(session_id, method);
        let provider_session_capture_method =
            Self::map_provider_session_capture_method(session_id, method);
        SessionCaptureProjection {
            provider_session_id,
            resume_input_id,
            provider_session_capture_method,
        }
    }

    fn map_capture_provider_session_id<'a>(
        session_id: Option<&'a str>,
        method: &str,
    ) -> Option<&'a str> {
        if Self::is_resumed_session_method(method) {
            None
        } else {
            session_id
        }
    }

    fn map_capture_resume_input_id<'a>(
        session_id: Option<&'a str>,
        method: &str,
    ) -> Option<&'a str> {
        if Self::is_resumed_session_method(method) {
            session_id
        } else {
            None
        }
    }

    fn map_provider_session_capture_method<'a>(
        session_id: Option<&str>,
        method: &'a str,
    ) -> Option<&'a str> {
        if Self::is_resumed_session_method(method) {
            None
        } else {
            session_id.map(|_| method)
        }
    }

    fn execute_session_capture_persistence(
        &self,
        id: i64,
        session_id: Option<&str>,
        method: &str,
        projection: SessionCaptureProjection<'_>,
    ) -> Result<i64, sqlite::Error> {
        let updated = self.conn.execute(
            "UPDATE invocations
                 SET session_id = CASE
                         WHEN ?2 = 'resumed' THEN COALESCE(session_id, ?1)
                         ELSE ?1
                     END,
                     session_capture_method = ?2,
                     provider_session_id = COALESCE(provider_session_id, ?3),
                     resume_input_id = COALESCE(resume_input_id, ?4),
                     provider_session_capture_method = COALESCE(provider_session_capture_method, ?5)
                 WHERE id = ?6",
            sqlite::params![
                session_id,
                method,
                projection.provider_session_id,
                projection.resume_input_id,
                projection.provider_session_capture_method,
                id
            ],
        )?;
        Ok(updated as i64)
    }

    fn translate_session_capture_result(
        id: i64,
        result: Result<i64, sqlite::Error>,
    ) -> Result<(), String> {
        result
            .map(|_| ())
            .map_err(|err| Self::format_session_capture_error(id, err))
    }

    fn format_session_capture_error(id: i64, err: sqlite::Error) -> String {
        format!("Failed to update session capture for invocation {id}: {err}")
    }

    fn optional_session_context(
        &self,
        id: i64,
        row: Option<&LifecycleInvocationRow>,
        session_id: Option<&str>,
        method: &str,
    ) -> Option<lc_log_adapter::SessionContext> {
        row.map(|row| self.session_context(id, row, session_id, method))
    }

    fn lifecycle_context_for_row(
        &self,
        row_id: i64,
    ) -> Result<Option<LifecycleInvocationRow>, String> {
        self.query_lifecycle_context_for_row(row_id)
            .map_err(|err| Self::format_lifecycle_context_lookup_error(row_id, err))
    }

    fn query_lifecycle_context_for_row(
        &self,
        row_id: i64,
    ) -> sqlite::Result<Option<LifecycleInvocationRow>> {
        self.conn
            .query_row(
                "SELECT i.invocation_uuid,
                        i.provider_name,
                        i.session_id,
                        i.provider_session_id,
                        i.resume_input_id
                 FROM invocations i
                 WHERE i.id = ?1",
                sqlite::params![row_id],
                Self::map_lifecycle_invocation_row,
            )
            .optional()
    }

    fn format_lifecycle_context_lookup_error(row_id: i64, err: sqlite::Error) -> String {
        format!("Failed to load invocation lifecycle context {row_id}: {err}")
    }

    fn map_lifecycle_invocation_row(
        row: &sqlite::Row<'_>,
    ) -> sqlite::Result<LifecycleInvocationRow> {
        Ok(LifecycleInvocationRow {
            invocation_uuid: row.get(0)?,
            provider_name: row.get(1)?,
            session_id: row.get(2)?,
            provider_session_id: row.get(3)?,
            resume_input_id: row.get(4)?,
        })
    }

    fn session_context(
        &self,
        id: i64,
        row: &LifecycleInvocationRow,
        session_id: Option<&str>,
        method: &str,
    ) -> lc_log_adapter::SessionContext {
        let event_session_id = Self::map_resumed_session_id(method, session_id);
        let resume_input_id = Self::map_session_resume_input_id(method, session_id, row);
        let chain_id_result = self.load_chain_id_for_invocation(id);
        let chain_id = Self::map_lifecycle_chain_id(chain_id_result);
        Self::build_session_context(id, row, event_session_id, method, resume_input_id, chain_id)
    }

    fn is_resumed_session_method(method: &str) -> bool {
        method == "resumed"
    }

    fn map_resumed_session_id(method: &str, session_id: Option<&str>) -> Option<String> {
        if Self::is_resumed_session_method(method) {
            None
        } else {
            session_id.map(str::to_string)
        }
    }

    fn map_session_resume_input_id(
        method: &str,
        session_id: Option<&str>,
        row: &LifecycleInvocationRow,
    ) -> Option<String> {
        if Self::is_resumed_session_method(method) {
            session_id.map(str::to_string)
        } else {
            row.resume_input_id.clone()
        }
    }

    pub(super) fn load_chain_id_for_invocation(
        &self,
        invocation_id: i64,
    ) -> Result<Option<String>, sqlite::Error> {
        self.conn
            .query_row(
                "SELECT s.chain_id
                 FROM invocations i
                 JOIN session_chain_segments s
                   ON s.provider_name = i.provider_name
                  AND s.session_id = COALESCE(i.provider_session_id, i.session_id)
                 WHERE i.id = ?1
                 LIMIT 1",
                sqlite::params![invocation_id],
                Self::map_chain_id_for_invocation_row,
            )
            .optional()
    }

    fn map_chain_id_for_invocation_row(row: &sqlite::Row<'_>) -> sqlite::Result<String> {
        row.get(0)
    }

    pub(super) fn map_lifecycle_chain_id(
        chain_id_result: Result<Option<String>, sqlite::Error>,
    ) -> Option<String> {
        chain_id_result.ok().flatten()
    }

    fn build_session_context(
        id: i64,
        row: &LifecycleInvocationRow,
        event_session_id: Option<String>,
        method: &str,
        resume_input_id: Option<String>,
        chain_id: Option<String>,
    ) -> lc_log_adapter::SessionContext {
        lc_log_adapter::SessionContext {
            invocation_uuid: row.invocation_uuid.clone(),
            provider_source: row.provider_name.clone(),
            chain_id,
            session_id: event_session_id,
            latency_us: 0,
            invocation_row_id: id,
            capture_method: method.to_string(),
            marker_emitted: true,
            resume_input_id,
        }
    }
}
