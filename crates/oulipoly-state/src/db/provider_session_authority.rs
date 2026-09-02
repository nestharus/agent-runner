//! Persisted selected-endpoint authority for provider-owned sessions.

use super::{DbError, RusqliteOptionalExtension, StateDb, sqlite};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredProviderSessionAuthority {
    pub provider_instance_id: String,
    pub settings_id: String,
}

pub struct FinalizedProviderSessionAuthority<'a> {
    pub provider_session_id: &'a str,
    pub capture_method: &'a str,
    pub provider_instance_id: &'a str,
    pub settings_id: &'a str,
}

impl StateDb {
    pub fn commit_finalized_provider_session_authority(
        &self,
        invocation_row_id: i64,
        authority: &FinalizedProviderSessionAuthority<'_>,
    ) -> Result<(), DbError> {
        validate_authority(authority.provider_instance_id, authority.settings_id)?;
        let tx = self.conn.unchecked_transaction().map_err(|error| {
            format!("Failed to begin provider session authority commit: {error}")
        })?;
        let provider_name = update_finalized_invocation_capture(
            &tx,
            invocation_row_id,
            authority.provider_session_id,
            authority.capture_method,
        )?;
        Self::mint_chain_for_invocation_session_on(&tx, invocation_row_id)?;
        bind_segment_authority_on(
            &tx,
            &provider_name,
            authority.provider_session_id,
            authority.provider_instance_id,
            authority.settings_id,
        )?;
        bind_invocation_authority_on(
            &tx,
            invocation_row_id,
            authority.provider_instance_id,
            authority.settings_id,
        )?;
        tx.commit()
            .map_err(|error| format!("Failed to commit provider session authority: {error}"))
    }

    pub fn active_provider_session_authority(
        &self,
        chain_id: &str,
    ) -> Result<Option<StoredProviderSessionAuthority>, DbError> {
        self.conn
            .query_row(
                "SELECT authority.provider_instance_id, authority.settings_id
                 FROM session_chain_segments AS segment
                 JOIN session_chain_segment_provider_authority AS authority
                   ON authority.segment_id = segment.id
                 WHERE segment.chain_id = ?1 AND segment.ended_at IS NULL
                 ORDER BY segment.started_at DESC, segment.id DESC
                 LIMIT 1",
                sqlite::params![chain_id],
                map_stored_authority,
            )
            .optional()
            .map_err(|error| format!("Failed to read active provider session authority: {error}"))
    }

    pub fn imported_session_cwd_for_authority(
        &self,
        provider_name: &str,
        provider_session_id: &str,
        authority: &StoredProviderSessionAuthority,
    ) -> Result<Option<String>, DbError> {
        self.conn
            .query_row(
                "SELECT metadata.cwd
                 FROM imported_session_display_metadata AS metadata
                 JOIN session_chain_segments AS segment
                   ON segment.provider_name = metadata.provider_name
                  AND segment.session_id = metadata.provider_session_id
                 JOIN session_chain_segment_provider_authority AS stored
                   ON stored.segment_id = segment.id
                 WHERE metadata.provider_name = ?1
                   AND metadata.provider_session_id = ?2
                   AND stored.provider_instance_id = ?3
                   AND stored.settings_id = ?4
                   AND metadata.cwd IS NOT NULL
                   AND trim(metadata.cwd) <> ''
                 ORDER BY segment.ended_at IS NULL DESC, segment.started_at DESC, segment.id DESC
                 LIMIT 1",
                sqlite::params![
                    provider_name,
                    provider_session_id,
                    authority.provider_instance_id,
                    authority.settings_id,
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| format!("Failed to read imported provider session cwd: {error}"))
    }

    pub fn latest_provider_session_resolved_account_for_authority(
        &self,
        provider_name: &str,
        provider_session_id: &str,
        authority: &StoredProviderSessionAuthority,
    ) -> Result<Option<String>, DbError> {
        self.conn
            .query_row(
                "SELECT invocation.provider_session_resolved_account
                 FROM invocations AS invocation
                 JOIN invocation_provider_session_authority AS stored
                   ON stored.invocation_id = invocation.id
                 WHERE invocation.provider_name = ?1
                   AND invocation.provider_session_id = ?2
                   AND stored.provider_instance_id = ?3
                   AND stored.settings_id = ?4
                   AND invocation.provider_session_resolved_account IS NOT NULL
                   AND trim(invocation.provider_session_resolved_account) <> ''
                 ORDER BY COALESCE(invocation.finished_at, invocation.created_at) DESC,
                          invocation.id DESC
                 LIMIT 1",
                sqlite::params![
                    provider_name,
                    provider_session_id,
                    authority.provider_instance_id,
                    authority.settings_id,
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| format!("Failed to read authoritative provider session cwd: {error}"))
    }

    pub(super) fn bind_session_provider_authority_on(
        conn: &sqlite::Connection,
        provider_name: &str,
        provider_session_id: &str,
        provider_instance_id: &str,
        settings_id: &str,
    ) -> Result<(), DbError> {
        validate_authority(provider_instance_id, settings_id)?;
        bind_segment_authority_on(
            conn,
            provider_name,
            provider_session_id,
            provider_instance_id,
            settings_id,
        )
    }
}

fn update_finalized_invocation_capture(
    conn: &sqlite::Connection,
    invocation_row_id: i64,
    provider_session_id: &str,
    capture_method: &str,
) -> Result<String, DbError> {
    let provider_name = conn
        .query_row(
            "SELECT provider_name FROM invocations WHERE id = ?1",
            sqlite::params![invocation_row_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|error| format!("Failed to read invocation provider authority: {error}"))?
        .flatten()
        .ok_or_else(|| format!("Invocation {invocation_row_id} has no provider authority"))?;
    let updated = conn
        .execute(
            "UPDATE invocations
             SET session_id = ?1,
                 session_capture_method = ?2,
                 provider_session_id = ?1,
                 provider_session_capture_method = ?2
             WHERE id = ?3",
            sqlite::params![provider_session_id, capture_method, invocation_row_id],
        )
        .map_err(|error| {
            format!("Failed to persist authenticated provider session capture: {error}")
        })?;
    if updated != 1 {
        return Err(format!("Invocation {invocation_row_id} not found"));
    }
    Ok(provider_name)
}

fn bind_segment_authority_on(
    conn: &sqlite::Connection,
    provider_name: &str,
    provider_session_id: &str,
    provider_instance_id: &str,
    settings_id: &str,
) -> Result<(), DbError> {
    let segment_id = conn
        .query_row(
            "SELECT id
             FROM session_chain_segments
             WHERE provider_name = ?1 AND session_id = ?2
             ORDER BY ended_at IS NULL DESC, started_at DESC, id DESC
             LIMIT 1",
            sqlite::params![provider_name, provider_session_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|error| format!("Failed to locate provider session segment authority: {error}"))?
        .ok_or_else(|| {
            format!("Provider session segment not found for {provider_name}/{provider_session_id}")
        })?;
    insert_or_validate_authority(
        conn,
        "session_chain_segment_provider_authority",
        "segment_id",
        segment_id,
        provider_instance_id,
        settings_id,
    )
}

fn bind_invocation_authority_on(
    conn: &sqlite::Connection,
    invocation_row_id: i64,
    provider_instance_id: &str,
    settings_id: &str,
) -> Result<(), DbError> {
    insert_or_validate_authority(
        conn,
        "invocation_provider_session_authority",
        "invocation_id",
        invocation_row_id,
        provider_instance_id,
        settings_id,
    )
}

fn insert_or_validate_authority(
    conn: &sqlite::Connection,
    table: &str,
    key_column: &str,
    key: i64,
    provider_instance_id: &str,
    settings_id: &str,
) -> Result<(), DbError> {
    let insert = format!(
        "INSERT INTO {table} ({key_column}, provider_instance_id, settings_id)
         VALUES (?1, ?2, ?3)
         ON CONFLICT({key_column}) DO NOTHING"
    );
    conn.execute(
        &insert,
        sqlite::params![key, provider_instance_id, settings_id],
    )
    .map_err(|error| format!("Failed to bind provider session authority: {error}"))?;
    let select =
        format!("SELECT provider_instance_id, settings_id FROM {table} WHERE {key_column} = ?1");
    let stored = conn
        .query_row(&select, sqlite::params![key], map_stored_authority)
        .map_err(|error| format!("Failed to validate provider session authority: {error}"))?;
    if stored.provider_instance_id != provider_instance_id || stored.settings_id != settings_id {
        return Err("provider_session_authority_mismatch".to_string());
    }
    Ok(())
}

fn validate_authority(provider_instance_id: &str, settings_id: &str) -> Result<(), DbError> {
    if provider_instance_id.trim().is_empty() {
        return Err("provider session instance identity is empty".to_string());
    }
    if settings_id.trim().is_empty() {
        return Err("provider session settings identity is empty".to_string());
    }
    Ok(())
}

fn map_stored_authority(row: &sqlite::Row<'_>) -> sqlite::Result<StoredProviderSessionAuthority> {
    Ok(StoredProviderSessionAuthority {
        provider_instance_id: row.get(0)?,
        settings_id: row.get(1)?,
    })
}
