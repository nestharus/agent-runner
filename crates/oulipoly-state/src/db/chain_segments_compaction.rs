//! ## Declared roles
//!
//! - accessor
//! - orchestration
//!
//! Role set: { accessor, orchestration }
//!
//! Compaction boundary and chain segment list helpers.

use super::*;
use chrono::{DateTime, Utc};

impl StateDb {
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
