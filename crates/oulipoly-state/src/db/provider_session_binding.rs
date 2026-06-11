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
    pub fn bind_invocation_provider_session_start(
        &self,
        invocation_row_id: i64,
        binding: &ProviderSessionBinding,
    ) -> Result<(), String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to begin provider session binding tx: {e}"))?;

        let existing = Self::load_existing_provider_session_binding(&tx, invocation_row_id)?;
        Self::validate_provider_session_rebind(invocation_row_id, binding, existing.as_deref())?;
        Self::write_provider_session_binding(&tx, invocation_row_id, binding)?;

        if Self::provider_session_binding_should_mint_chain(binding) {
            Self::mint_chain_for_invocation_session_on(&tx, invocation_row_id)?;
        }
        tx.commit()
            .map_err(|e| format!("Failed to commit provider session binding tx: {e}"))
    }

    fn load_existing_provider_session_binding(
        conn: &sqlite::Connection,
        invocation_row_id: i64,
    ) -> Result<Option<String>, String> {
        conn.query_row(
            "SELECT provider_session_id FROM invocations WHERE id = ?1",
            sqlite::params![invocation_row_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|e| format!("Failed to read invocation {invocation_row_id}: {e}"))?
        .ok_or_else(|| format!("Invocation {invocation_row_id} not found"))
    }

    fn validate_provider_session_rebind(
        invocation_row_id: i64,
        binding: &ProviderSessionBinding,
        existing: Option<&str>,
    ) -> Result<(), String> {
        if let Some(existing) = existing
            && existing != binding.provider_session_id
        {
            return Err(format!(
                "Invocation {invocation_row_id} is already bound to provider session {existing}; refusing to bind {}",
                binding.provider_session_id
            ));
        }
        Ok(())
    }

    fn write_provider_session_binding(
        conn: &sqlite::Connection,
        invocation_row_id: i64,
        binding: &ProviderSessionBinding,
    ) -> Result<(), String> {
        conn.execute(
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
             WHERE id = ?5",
            sqlite::params![
                &binding.provider_session_id,
                binding.capture_method,
                binding.provider_session_resolved_account.as_deref(),
                binding.resume_input_id.as_deref(),
                invocation_row_id
            ],
        )
        .map_err(|e| {
            format!("Failed to bind provider session for invocation {invocation_row_id}: {e}")
        })?;
        Ok(())
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
        let sql = format!(
            "SELECT model_name, provider_name, {provider_session_expr}, COALESCE(finished_at, created_at)
             FROM invocations
             WHERE id = ?1
               AND provider_name IS NOT NULL
               AND {provider_session_expr} IS NOT NULL"
        );
        conn.query_row(&sql, sqlite::params![invocation_row_id], |row| {
            Ok(InvocationChainMintRow {
                model_name: row.get(0)?,
                provider_name: row.get(1)?,
                session_id: row.get(2)?,
                raw_ts: row.get(3)?,
            })
        })
        .optional()
        .map_err(|e| format!("Failed to read invocation for chain mint: {e}"))
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
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| format!("Failed to check existing invocation chain: {e}"))
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
        .map_err(|e| format!("Failed to update invocation session chain model: {e}"))?;
        conn.execute(
            "UPDATE session_chain_segments
                 SET transition_reason = 'initial'
                 WHERE chain_id = ?1
                   AND provider_name = ?2
                   AND session_id = ?3
                   AND transition_reason = 'imported'",
            sqlite::params![chain_id, provider_name, session_id],
        )
        .map_err(|e| format!("Failed to promote imported session chain segment: {e}"))?;
        Ok(())
    }

    fn insert_invocation_chain(
        conn: &sqlite::Connection,
        row: &InvocationChainMintRow,
        ts: &DateTime<Utc>,
    ) -> Result<(), DbError> {
        let chain_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, ?2, ?2, ?3)",
            sqlite::params![chain_id, ts.to_rfc3339(), row.model_name],
        )
        .map_err(|e| format!("Failed to mint invocation session chain: {e}"))?;
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, ?4, 'initial')",
            sqlite::params![chain_id, row.provider_name, row.session_id, ts.to_rfc3339()],
        )
        .map_err(|e| format!("Failed to mint invocation session segment: {e}"))?;
        Ok(())
    }
}
