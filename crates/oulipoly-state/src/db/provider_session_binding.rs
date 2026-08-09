//! ## Declared roles
//!
//! - orchestration
//! - accessor
//! - validator
//! - predicate
//! - mapper
//! - formatter
//!
//! Role set: { orchestration, accessor, validator, predicate, mapper, formatter }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/provider_session_binding.rs
//!     role: intrinsic-surface
//!     Domain: provider-session-binding-persistence
//!     Owns:
//!       - StateDb provider-session binding upsert/lookup persistence
//!       - sqlite (rusqlite re-export) and RusqliteOptionalExtension optional-row access
//!       - DbError construction for binding persistence failures
//!       - external contract symbols referenced by this concern via its `use`
//!         declarations, intrinsic and subordinate to this persistence domain: DateTime, DbError, RusqliteOptionalExtension, StateDb, Utc, Uuid, sqlite
//! ```
//!
//! Provider-session binding persistence and chain minting coordination.

use super::{DbError, RusqliteOptionalExtension, StateDb, sqlite};
use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ProviderSessionBinding {
    pub provider_session_id: String,
    pub capture_method: &'static str,
    pub resume_input_id: Option<String>,
    pub provider_session_resolved_account: Option<String>,
}

struct InvocationChainMintRow {
    model_name: String,
    provider_name: String,
    session_id: String,
    raw_ts: String,
}

impl StateDb {
    pub fn latest_provider_session_resolved_account(
        &self,
        provider_name: &str,
        session_id: &str,
    ) -> Result<Option<String>, String> {
        self.conn
            .query_row(
                "SELECT provider_session_resolved_account
                 FROM invocations
                 WHERE provider_name = ?1
                   AND (provider_session_id = ?2 OR session_id = ?2)
                   AND provider_session_resolved_account IS NOT NULL
                   AND trim(provider_session_resolved_account) <> ''
                 ORDER BY COALESCE(finished_at, created_at) DESC, id DESC
                 LIMIT 1",
                sqlite::params![provider_name, session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| {
                format!(
                    "Failed to read provider_session_resolved_account for {provider_name}/{session_id}: {e}"
                )
            })
    }

    pub fn bind_invocation_provider_session_start(
        &self,
        invocation_row_id: i64,
        binding: &ProviderSessionBinding,
    ) -> Result<(), String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(Self::format_provider_session_binding_begin_error)?;

        let existing = Self::load_existing_provider_session_binding(&tx, invocation_row_id)?;
        Self::validate_provider_session_rebind(invocation_row_id, binding, existing.as_deref())?;
        Self::write_provider_session_binding(&tx, invocation_row_id, binding)?;

        if Self::provider_session_binding_should_mint_chain(binding) {
            Self::mint_chain_for_invocation_session_on(&tx, invocation_row_id)?;
        }
        tx.commit()
            .map_err(Self::format_provider_session_binding_commit_error)
    }

    fn format_provider_session_binding_begin_error(e: sqlite::Error) -> String {
        format!("Failed to begin provider session binding tx: {e}")
    }

    fn format_provider_session_binding_commit_error(e: sqlite::Error) -> String {
        format!("Failed to commit provider session binding tx: {e}")
    }

    fn load_existing_provider_session_binding(
        conn: &sqlite::Connection,
        invocation_row_id: i64,
    ) -> Result<Option<String>, String> {
        let existing = Self::query_existing_provider_session_binding(conn, invocation_row_id)
            .map_err(|e| Self::format_provider_session_binding_read_error(invocation_row_id, e))?;
        Self::require_provider_session_binding_row(invocation_row_id, existing)
    }

    fn query_existing_provider_session_binding(
        conn: &sqlite::Connection,
        invocation_row_id: i64,
    ) -> sqlite::Result<Option<Option<String>>> {
        conn.query_row(
            "SELECT provider_session_id FROM invocations WHERE id = ?1",
            sqlite::params![invocation_row_id],
            Self::map_existing_provider_session_binding_row,
        )
        .optional()
    }

    fn map_existing_provider_session_binding_row(
        row: &sqlite::Row<'_>,
    ) -> sqlite::Result<Option<String>> {
        row.get(0)
    }

    fn require_provider_session_binding_row(
        invocation_row_id: i64,
        existing: Option<Option<String>>,
    ) -> Result<Option<String>, String> {
        existing.ok_or_else(|| {
            Self::format_provider_session_binding_missing_invocation_error(invocation_row_id)
        })
    }

    fn format_provider_session_binding_read_error(
        invocation_row_id: i64,
        e: sqlite::Error,
    ) -> String {
        format!("Failed to read invocation {invocation_row_id}: {e}")
    }

    fn format_provider_session_binding_missing_invocation_error(invocation_row_id: i64) -> String {
        format!("Invocation {invocation_row_id} not found")
    }

    fn validate_provider_session_rebind(
        invocation_row_id: i64,
        binding: &ProviderSessionBinding,
        existing: Option<&str>,
    ) -> Result<(), String> {
        if let Some(existing) = existing
            && existing != binding.provider_session_id
        {
            return Err(Self::format_provider_session_rebind_error(
                invocation_row_id,
                existing,
                &binding.provider_session_id,
            ));
        }
        Ok(())
    }

    fn format_provider_session_rebind_error(
        invocation_row_id: i64,
        existing: &str,
        provider_session_id: &str,
    ) -> String {
        format!(
            "Invocation {invocation_row_id} is already bound to provider session {existing}; refusing to bind {provider_session_id}"
        )
    }

    fn write_provider_session_binding(
        conn: &sqlite::Connection,
        invocation_row_id: i64,
        binding: &ProviderSessionBinding,
    ) -> Result<(), String> {
        let updated = conn
            .execute(
            "UPDATE invocations
             SET provider_session_id = ?1,
                 provider_session_capture_method = ?2,
                 provider_session_resolved_account = COALESCE(?3, provider_session_resolved_account),
                 resume_input_id = COALESCE(?4, resume_input_id),
                 session_id = CASE
                     WHEN session_capture_method = 'resumed'
                          AND resume_input_id IS NOT NULL
                          AND session_id = resume_input_id
                     THEN session_id
                     ELSE ?1
                 END,
                 session_capture_method = ?2
             WHERE id = ?5
               AND status = 'running'",
            sqlite::params![
                &binding.provider_session_id,
                binding.capture_method,
                binding.provider_session_resolved_account.as_deref(),
                binding.resume_input_id.as_deref(),
                invocation_row_id
            ],
            )
            .map_err(|e| Self::format_provider_session_binding_write_error(invocation_row_id, e))?;
        if updated == 1 {
            Ok(())
        } else {
            Err(format!(
                "Invocation {invocation_row_id} is no longer running; refusing provider session binding"
            ))
        }
    }

    fn format_provider_session_binding_write_error(
        invocation_row_id: i64,
        e: sqlite::Error,
    ) -> String {
        format!("Failed to bind provider session for invocation {invocation_row_id}: {e}")
    }

    pub fn transition_invocation_provider_session_capture_method(
        &self,
        invocation_row_id: i64,
        provider_session_id: &str,
        expected_method: &str,
        next_method: &str,
    ) -> Result<(), String> {
        let updated = self
            .conn
            .execute(
                "UPDATE invocations
                 SET provider_session_capture_method = ?1,
                     session_capture_method = ?1
                 WHERE id = ?2
                   AND status = 'running'
                   AND provider_session_id = ?3
                   AND provider_session_capture_method = ?4",
                sqlite::params![
                    next_method,
                    invocation_row_id,
                    provider_session_id,
                    expected_method
                ],
            )
            .map_err(|err| {
                format!(
                    "Failed to transition provider session capture method for invocation {invocation_row_id}: {err}"
                )
            })?;
        if updated == 1 {
            Ok(())
        } else {
            Err(format!(
                "Invocation {invocation_row_id} is not a running {expected_method} binding for provider session {provider_session_id}"
            ))
        }
    }

    fn provider_session_binding_should_mint_chain(binding: &ProviderSessionBinding) -> bool {
        binding.resume_input_id.as_deref() != Some(binding.provider_session_id.as_str())
    }

    pub fn mint_chain_for_invocation_session(&self, invocation_row_id: i64) -> Result<(), DbError> {
        Self::mint_chain_for_invocation_session_on(&self.conn, invocation_row_id)
    }

    fn mint_chain_for_invocation_session_on(
        conn: &sqlite::Connection,
        invocation_row_id: i64,
    ) -> Result<(), DbError> {
        let Some(row) = Self::load_invocation_chain_mint_row(conn, invocation_row_id)? else {
            return Ok(());
        };
        let ts = Self::fallback_now_rfc3339(&row.raw_ts);
        if let Some(chain_id) =
            Self::existing_chain_for_provider_session(conn, &row.provider_name, &row.session_id)?
        {
            Self::promote_existing_invocation_chain(
                conn,
                &chain_id,
                &row.model_name,
                &row.provider_name,
                &row.session_id,
            )?;
            return Ok(());
        }
        Self::insert_invocation_chain(conn, &row, &ts)
    }

    fn load_invocation_chain_mint_row(
        conn: &sqlite::Connection,
        invocation_row_id: i64,
    ) -> Result<Option<InvocationChainMintRow>, DbError> {
        let provider_session_expr = Self::provider_session_expr(conn, None)?;
        let sql = Self::invocation_chain_mint_sql(&provider_session_expr);
        conn.query_row(
            &sql,
            sqlite::params![invocation_row_id],
            Self::map_invocation_chain_mint_row,
        )
        .optional()
        .map_err(Self::format_invocation_chain_mint_read_error)
    }

    fn invocation_chain_mint_sql(provider_session_expr: &str) -> String {
        format!(
            "SELECT model_name, provider_name, {provider_session_expr}, COALESCE(finished_at, created_at)
             FROM invocations
             WHERE id = ?1
               AND provider_name IS NOT NULL
               AND {provider_session_expr} IS NOT NULL"
        )
    }

    fn map_invocation_chain_mint_row(
        row: &sqlite::Row<'_>,
    ) -> sqlite::Result<InvocationChainMintRow> {
        Ok(InvocationChainMintRow {
            model_name: row.get(0)?,
            provider_name: row.get(1)?,
            session_id: row.get(2)?,
            raw_ts: row.get(3)?,
        })
    }

    fn format_invocation_chain_mint_read_error(e: sqlite::Error) -> DbError {
        format!("Failed to read invocation for chain mint: {e}")
    }

    fn existing_chain_for_provider_session(
        conn: &sqlite::Connection,
        provider_name: &str,
        session_id: &str,
    ) -> Result<Option<String>, DbError> {
        conn.query_row(
            "SELECT chain_id FROM session_chain_segments
                 WHERE provider_name = ?1 AND session_id = ?2
                 LIMIT 1",
            sqlite::params![provider_name, session_id],
            Self::map_existing_chain_for_provider_session_row,
        )
        .optional()
        .map_err(Self::format_existing_invocation_chain_error)
    }

    fn map_existing_chain_for_provider_session_row(
        row: &sqlite::Row<'_>,
    ) -> sqlite::Result<String> {
        row.get(0)
    }

    fn format_existing_invocation_chain_error(e: sqlite::Error) -> DbError {
        format!("Failed to check existing invocation chain: {e}")
    }

    fn promote_existing_invocation_chain(
        conn: &sqlite::Connection,
        chain_id: &str,
        model_name: &str,
        provider_name: &str,
        session_id: &str,
    ) -> Result<(), DbError> {
        conn.execute(
            "UPDATE session_chains
                 SET model_name = ?2
                 WHERE chain_id = ?1 AND model_name = '<unknown>'",
            sqlite::params![chain_id, model_name],
        )
        .map_err(Self::format_update_invocation_session_chain_model_error)?;
        conn.execute(
            "UPDATE session_chain_segments
                 SET transition_reason = 'initial'
                 WHERE chain_id = ?1
                   AND provider_name = ?2
                   AND session_id = ?3
                   AND transition_reason = 'imported'",
            sqlite::params![chain_id, provider_name, session_id],
        )
        .map_err(Self::format_promote_imported_session_chain_segment_error)?;
        Ok(())
    }

    fn format_update_invocation_session_chain_model_error(e: sqlite::Error) -> DbError {
        format!("Failed to update invocation session chain model: {e}")
    }

    fn format_promote_imported_session_chain_segment_error(e: sqlite::Error) -> DbError {
        format!("Failed to promote imported session chain segment: {e}")
    }

    fn insert_invocation_chain(
        conn: &sqlite::Connection,
        row: &InvocationChainMintRow,
        ts: &DateTime<Utc>,
    ) -> Result<(), DbError> {
        let insert = Self::invocation_chain_insert(row, ts);
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, ?2, ?2, ?3)",
            sqlite::params![&insert.chain_id, &insert.started_at, &insert.model_name],
        )
        .map_err(Self::format_mint_invocation_session_chain_error)?;
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, ?4, 'initial')",
            sqlite::params![
                &insert.chain_id,
                row.provider_name,
                row.session_id,
                &insert.started_at
            ],
        )
        .map_err(Self::format_mint_invocation_session_segment_error)?;
        Ok(())
    }

    fn invocation_chain_insert(
        row: &InvocationChainMintRow,
        ts: &DateTime<Utc>,
    ) -> InvocationChainInsert {
        InvocationChainInsert {
            chain_id: Self::new_invocation_chain_id(),
            started_at: Self::invocation_chain_timestamp(ts),
            model_name: row.model_name.clone(),
        }
    }

    fn new_invocation_chain_id() -> String {
        Uuid::new_v4().to_string()
    }

    fn invocation_chain_timestamp(ts: &DateTime<Utc>) -> String {
        ts.to_rfc3339()
    }

    fn format_mint_invocation_session_chain_error(e: sqlite::Error) -> DbError {
        format!("Failed to mint invocation session chain: {e}")
    }

    fn format_mint_invocation_session_segment_error(e: sqlite::Error) -> DbError {
        format!("Failed to mint invocation session segment: {e}")
    }
}

struct InvocationChainInsert {
    chain_id: String,
    started_at: String,
    model_name: String,
}
