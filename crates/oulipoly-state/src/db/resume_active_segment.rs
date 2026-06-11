//! ## Declared roles
//!
//! - accessor
//! - formatter
//!
//! Role set: { accessor, formatter }
//!
//! Active segment and chain model lookup helpers.

use super::*;

impl StateDb {
    pub fn active_segment_id_for_chain_provider_session(
        &self,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
    ) -> Result<Option<i64>, String> {
        self.conn
            .query_row(
                "SELECT id
                 FROM session_chain_segments
                 WHERE chain_id = ?1
                   AND provider_name = ?2
                   AND session_id = ?3
                   AND ended_at IS NULL
                 ORDER BY started_at DESC, id DESC
                 LIMIT 1",
                sqlite::params![chain_id, provider_name, session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to read active chain segment id: {e}"))
    }

    pub(super) fn active_segment_for_chain(
        &self,
        chain_id: &str,
    ) -> Result<Option<(String, String)>, String> {
        self.conn
            .query_row(
                "SELECT provider_name, session_id
                 FROM session_chain_segments
                 WHERE chain_id = ?1 AND ended_at IS NULL
                 ORDER BY started_at DESC, id DESC
                 LIMIT 1",
                sqlite::params![chain_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| format!("Failed to read active chain segment: {e}"))
    }

    pub(super) fn chain_model_name(&self, chain_id: &str) -> Result<Option<String>, String> {
        self.conn
            .query_row(
                "SELECT model_name FROM session_chains WHERE chain_id = ?1",
                sqlite::params![chain_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to read session chain model: {e}"))
    }

    pub(super) fn latest_invocation_model_for_chain(
        &self,
        chain_id: &str,
    ) -> Result<Option<String>, String> {
        let provider_session_expr = Self::provider_session_expr(&self.conn, Some("i."))?;
        let sql = Self::latest_invocation_model_sql(&provider_session_expr);
        self.conn
            .query_row(&sql, sqlite::params![chain_id], |row| row.get(0))
            .optional()
            .map_err(|e| format!("Failed to infer session chain model from invocations: {e}"))
    }

    pub(super) fn latest_invocation_model_sql(provider_session_expr: &str) -> String {
        format!(
            "SELECT i.model_name
             FROM invocations i
             WHERE {provider_session_expr} IN (
                SELECT session_id FROM session_chain_segments WHERE chain_id = ?1
             )
             ORDER BY COALESCE(i.finished_at, i.created_at) DESC, i.id DESC
             LIMIT 1"
        )
    }
}
