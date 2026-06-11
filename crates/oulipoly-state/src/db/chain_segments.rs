use super::{DbError, RusqliteOptionalExtension, StateDb, sqlite};
use chrono::{DateTime, Utc};
use oulipoly_core::TransitionReason;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveChainSegmentSnapshot {
    pub chain_id: String,
    pub active_provider: String,
    pub active_session_id: String,
    pub active_started_at: String,
    pub active_ended_at: Option<String>,
    pub active_last_turn_id: Option<String>,
    pub latest_turn_at: Option<String>,
}

pub struct ChainSegmentRotationInput<'a> {
    pub chain_id: &'a str,
    pub source_provider_name: &'a str,
    pub source_session_id: &'a str,
    pub target_provider_name: &'a str,
    pub target_session_id: &'a str,
    pub changed_at: &'a DateTime<Utc>,
    pub reason: TransitionReason,
}

/// Compact-summary evidence consumed by `migrate-db`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactSummaryEvidence {
    pub session_id: String,
    pub compact_turn_uuids: Vec<String>,
}

impl StateDb {
    pub fn open_chain_segment(
        &self,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
        started_at: &DateTime<Utc>,
        reason: TransitionReason,
    ) -> Result<i64, DbError> {
        Self::upsert_open_chain_segment(
            &self.conn,
            chain_id,
            provider_name,
            session_id,
            &started_at.to_rfc3339(),
            reason,
        )?;
        Self::read_open_chain_segment_id(&self.conn, chain_id, provider_name, session_id)
    }

    pub fn rotate_chain_segment_transactionally(
        &self,
        input: ChainSegmentRotationInput<'_>,
    ) -> Result<(i64, i64), DbError> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to begin session chain rotation transaction: {e}"))?;
        let closed_id = Self::close_expected_active_segment_returning_on(
            &tx,
            input.chain_id,
            input.source_provider_name,
            input.source_session_id,
            input.changed_at,
        )?
        .ok_or_else(|| "validated source segment was no longer active".to_string())?;
        Self::upsert_open_chain_segment(
            &tx,
            input.chain_id,
            input.target_provider_name,
            input.target_session_id,
            &input.changed_at.to_rfc3339(),
            input.reason,
        )?;
        let opened_id = Self::read_open_chain_segment_id(
            &tx,
            input.chain_id,
            input.target_provider_name,
            input.target_session_id,
        )?;
        tx.commit()
            .map_err(|e| format!("Failed to commit session chain rotation transaction: {e}"))?;
        Ok((closed_id, opened_id))
    }

    pub fn active_chain_segment_snapshot(
        &self,
        chain_id: &str,
    ) -> Result<Option<ActiveChainSegmentSnapshot>, DbError> {
        self.conn
            .query_row(
                "SELECT sc.chain_id,
                        s.provider_name,
                        s.session_id,
                        s.started_at,
                        s.ended_at,
                        s.last_turn_id,
                        (
                            SELECT st.timestamp
                            FROM session_turns st
                            WHERE st.provider_name = s.provider_name
                              AND st.session_id = s.session_id
                            ORDER BY st.timestamp DESC, st.id DESC
                            LIMIT 1
                        )
                 FROM session_chains sc
                 JOIN session_chain_segments s ON s.chain_id = sc.chain_id
                 WHERE sc.chain_id = ?1 AND s.ended_at IS NULL
                 ORDER BY s.started_at DESC, s.id DESC
                 LIMIT 1",
                sqlite::params![chain_id],
                |row| {
                    Ok(ActiveChainSegmentSnapshot {
                        chain_id: row.get(0)?,
                        active_provider: row.get(1)?,
                        active_session_id: row.get(2)?,
                        active_started_at: row.get(3)?,
                        active_ended_at: row.get(4)?,
                        active_last_turn_id: row.get(5)?,
                        latest_turn_at: row.get(6)?,
                    })
                },
            )
            .optional()
            .map_err(|e| format!("Failed to read active chain segment snapshot: {e}"))
    }

    fn upsert_open_chain_segment(
        conn: &sqlite::Connection,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
        started_at: &str,
        reason: TransitionReason,
    ) -> Result<(), DbError> {
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (chain_id, provider_name, session_id)
             DO UPDATE SET
                started_at = excluded.started_at,
                ended_at = NULL,
                last_turn_id = NULL,
                transition_reason = excluded.transition_reason",
            sqlite::params![
                chain_id,
                provider_name,
                session_id,
                started_at,
                reason.as_str()
            ],
        )
        .map_err(|e| format!("Failed to open session chain segment: {e}"))?;
        Ok(())
    }

    fn read_open_chain_segment_id(
        conn: &sqlite::Connection,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
    ) -> Result<i64, DbError> {
        conn.query_row(
            "SELECT id FROM session_chain_segments
                 WHERE chain_id = ?1 AND provider_name = ?2 AND session_id = ?3
                 ORDER BY id DESC LIMIT 1",
            sqlite::params![chain_id, provider_name, session_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Failed to read session chain segment id: {e}"))
    }

    pub fn find_conflicting_active_segment(
        &self,
        provider_name: &str,
        session_id: &str,
        own_chain_id: &str,
    ) -> Result<Option<String>, DbError> {
        self.conn
            .query_row(
                "SELECT chain_id
                 FROM session_chain_segments
                 WHERE provider_name = ?1
                   AND session_id = ?2
                   AND chain_id != ?3
                   AND ended_at IS NULL
                 ORDER BY started_at DESC, id DESC
                 LIMIT 1",
                sqlite::params![provider_name, session_id, own_chain_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to check conflicting active session segment: {e}"))
    }

    pub fn mint_imported_chain_if_absent(
        &self,
        provider_name: &str,
        session_id: &str,
        started_at: &DateTime<Utc>,
        model_name: &str,
    ) -> Result<(), DbError> {
        if Self::session_chain_segment_exists(&self.conn, provider_name, session_id)? {
            return Ok(());
        }
        let chain_id = Uuid::new_v4().to_string();
        let ts = started_at.to_rfc3339();
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to begin imported chain mint: {e}"))?;
        Self::insert_imported_chain(&tx, &chain_id, &ts, model_name)?;
        Self::insert_imported_segment(&tx, &chain_id, provider_name, session_id, &ts)?;
        tx.commit()
            .map_err(|e| format!("Failed to commit imported chain mint: {e}"))?;
        Ok(())
    }

    fn session_chain_segment_exists(
        conn: &sqlite::Connection,
        provider_name: &str,
        session_id: &str,
    ) -> Result<bool, DbError> {
        let exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM session_chain_segments
                 WHERE provider_name = ?1 AND session_id = ?2
                 LIMIT 1",
                sqlite::params![provider_name, session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to check existing session chain segment: {e}"))?;
        Ok(exists.is_some())
    }

    fn insert_imported_chain(
        conn: &sqlite::Connection,
        chain_id: &str,
        started_at: &str,
        model_name: &str,
    ) -> Result<(), DbError> {
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, ?2, ?2, ?3)
             ON CONFLICT DO NOTHING",
            sqlite::params![chain_id, started_at, model_name],
        )
        .map_err(|e| format!("Failed to mint imported session chain: {e}"))?;
        Ok(())
    }

    fn insert_imported_segment(
        conn: &sqlite::Connection,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
        started_at: &str,
    ) -> Result<(), DbError> {
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, ?4, 'imported')
             ON CONFLICT DO NOTHING",
            sqlite::params![chain_id, provider_name, session_id, started_at],
        )
        .map_err(|e| format!("Failed to mint imported session chain segment: {e}"))?;
        Ok(())
    }

    pub fn close_active_segment_returning(
        &self,
        chain_id: &str,
        ended_at: &DateTime<Utc>,
    ) -> Result<Option<i64>, DbError> {
        Self::close_active_segment_returning_on(&self.conn, chain_id, ended_at)
    }

    fn close_active_segment_returning_on(
        conn: &sqlite::Connection,
        chain_id: &str,
        ended_at: &DateTime<Utc>,
    ) -> Result<Option<i64>, DbError> {
        Self::close_matching_active_segment_returning_on(conn, chain_id, None, None, ended_at)
    }

    fn close_expected_active_segment_returning_on(
        conn: &sqlite::Connection,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
        ended_at: &DateTime<Utc>,
    ) -> Result<Option<i64>, DbError> {
        Self::close_matching_active_segment_returning_on(
            conn,
            chain_id,
            Some(provider_name),
            Some(session_id),
            ended_at,
        )
    }

    fn close_matching_active_segment_returning_on(
        conn: &sqlite::Connection,
        chain_id: &str,
        provider_name: Option<&str>,
        session_id: Option<&str>,
        ended_at: &DateTime<Utc>,
    ) -> Result<Option<i64>, DbError> {
        conn.query_row(
            "UPDATE session_chain_segments
             SET ended_at = ?2,
                 last_turn_id = (
                    SELECT st.turn_id
                    FROM session_turns st
                    WHERE st.provider_name = session_chain_segments.provider_name
                      AND st.session_id = session_chain_segments.session_id
                    ORDER BY st.timestamp DESC, st.id DESC
                    LIMIT 1
                 )
             WHERE chain_id = ?1
               AND ended_at IS NULL
               AND (?3 IS NULL OR provider_name = ?3)
               AND (?4 IS NULL OR session_id = ?4)
             RETURNING id",
            sqlite::params![chain_id, ended_at.to_rfc3339(), provider_name, session_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("Failed to close active session chain segment: {e}"))
    }

    pub fn update_chain_last_used(&self, chain_id: &str) -> Result<(), DbError> {
        self.conn
            .execute(
                "UPDATE session_chains SET last_used_at = ?2 WHERE chain_id = ?1",
                sqlite::params![chain_id, Utc::now().to_rfc3339()],
            )
            .map_err(|e| format!("Failed to update session chain last_used_at: {e}"))?;
        Ok(())
    }

    pub fn latest_compaction_boundary(
        &self,
        provider_name: &str,
        session_id: &str,
    ) -> Result<Option<(String, DateTime<Utc>)>, DbError> {
        let row = self
            .conn
            .query_row(
                "SELECT turn_id, timestamp
                 FROM session_turns
                 WHERE provider_name = ?1
                   AND session_id = ?2
                   AND is_compaction_boundary = 1
                 ORDER BY timestamp DESC, id DESC
                 LIMIT 1",
                sqlite::params![provider_name, session_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|e| format!("Failed to query latest compaction boundary: {e}"))?;
        row.map(|(turn_id, raw_ts)| {
            Self::strict_rfc3339_message(&raw_ts, "compaction boundary timestamp")
                .map(|timestamp| (turn_id, timestamp))
        })
        .transpose()
    }

    pub fn distinct_chain_segments(&self) -> Result<Vec<(String, String)>, DbError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT DISTINCT provider_name, session_id
                 FROM session_chain_segments
                 ORDER BY provider_name, session_id",
            )
            .map_err(|e| format!("Failed to prepare chain segment list: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| format!("Failed to query chain segment list: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read chain segment list: {e}"))
    }

    pub fn flag_compaction_boundary(
        &self,
        provider_name: &str,
        session_id: &str,
        turn_id: &str,
    ) -> Result<bool, DbError> {
        let changed = self
            .conn
            .execute(
                "UPDATE session_turns
                 SET is_compaction_boundary = 1
                 WHERE provider_name = ?1
                   AND session_id = ?2
                   AND turn_id = ?3
                   AND is_compaction_boundary = 0",
                sqlite::params![provider_name, session_id, turn_id],
            )
            .map_err(|e| format!("Failed to flag compaction boundary: {e}"))?;
        Ok(changed > 0)
    }
}
