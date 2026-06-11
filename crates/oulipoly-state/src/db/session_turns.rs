use super::{SessionTurnCounts, SessionTurnIngest, StateDb, sqlite};
use chrono::{DateTime, Utc};

struct SessionTurnBindValues<'a> {
    session_id: &'a str,
    turn_id: &'a str,
    timestamp: String,
    role: &'a str,
    parent_turn_id: Option<&'a str>,
    is_sidechain: i64,
    is_compaction_boundary: i64,
    body: Option<&'a str>,
}

impl StateDb {
    // --- Session log ingestion ---

    /// Insert one parsed turn. Idempotent: re-running a scan against an
    /// unchanged log is a no-op for already-seen turns.
    pub fn ingest_session_turn(
        &self,
        provider_name: &str,
        session_id: &str,
        turn_id: &str,
        timestamp: &DateTime<Utc>,
        role: &str,
        source_file: &str,
    ) -> Result<bool, String> {
        let now = Utc::now().to_rfc3339();
        let ts = timestamp.to_rfc3339();
        let changed = self
            .conn
            .execute(
                "INSERT OR IGNORE INTO session_turns
                    (provider_name, session_id, turn_id, timestamp, role, is_compaction_boundary, source_file, ingested_at, body)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?8)",
                sqlite::params![
                    provider_name,
                    session_id,
                    turn_id,
                    &ts,
                    role,
                    source_file,
                    &now,
                    Option::<&str>::None,
                ],
            )
            .map_err(|e| format!("Failed to ingest session turn: {e}"))?;
        Ok(changed > 0)
    }

    /// Bulk-insert turns inside a single transaction with a prepared
    /// statement. Hundreds of thousands of rows go from minutes to seconds
    /// vs the per-row method. Returns the count of newly-inserted rows
    /// (duplicates collapsed by the UNIQUE constraint don't count).
    pub fn ingest_session_turns_batch(
        &self,
        provider_name: &str,
        turns: &[SessionTurnIngest],
    ) -> Result<u64, String> {
        if Self::session_turn_batch_is_empty(turns) {
            return Ok(0);
        }
        let now = Utc::now().to_rfc3339();
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to begin transaction: {e}"))?;
        let new_count = Self::insert_session_turn_batch_rows(&tx, provider_name, turns, &now)?;
        tx.commit()
            .map_err(|e| format!("Failed to commit batch: {e}"))?;
        Ok(new_count)
    }

    fn session_turn_batch_is_empty(turns: &[SessionTurnIngest]) -> bool {
        turns.is_empty()
    }

    fn insert_session_turn_batch_rows(
        conn: &sqlite::Connection,
        provider_name: &str,
        turns: &[SessionTurnIngest],
        ingested_at: &str,
    ) -> Result<u64, String> {
        let mut stmt = Self::prepare_session_turn_batch_insert(conn)?;
        Self::execute_session_turn_writes(&mut stmt, provider_name, turns, ingested_at)
    }

    fn prepare_session_turn_batch_insert(
        conn: &sqlite::Connection,
    ) -> Result<sqlite::Statement<'_>, String> {
        conn.prepare(Self::session_turn_batch_insert_sql())
            .map_err(Self::format_session_turn_prepare_error)
    }

    fn execute_session_turn_writes(
        stmt: &mut sqlite::Statement<'_>,
        provider_name: &str,
        turns: &[SessionTurnIngest],
        ingested_at: &str,
    ) -> Result<u64, String> {
        let mut new_count: u64 = 0;
        for turn in turns {
            let binds = Self::bind_session_turn_row_params(turn);
            let n =
                Self::execute_session_turn_batch_insert(stmt, provider_name, &binds, ingested_at)?;
            new_count += n as u64;
        }
        Ok(new_count)
    }

    fn format_session_turn_prepare_error(err: sqlite::Error) -> String {
        format!("Failed to prepare batch insert: {err}")
    }

    fn session_turn_batch_insert_sql() -> &'static str {
        "INSERT OR IGNORE INTO session_turns
            (
                provider_name,
                session_id,
                turn_id,
                timestamp,
                role,
                parent_turn_id,
                is_sidechain,
                is_compaction_boundary,
                source_file,
                ingested_at,
                body
            )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, '', ?9, ?10)"
    }

    fn bind_session_turn_row_params(turn: &SessionTurnIngest) -> SessionTurnBindValues<'_> {
        SessionTurnBindValues {
            session_id: &turn.session_id,
            turn_id: &turn.turn_id,
            timestamp: turn.timestamp.to_rfc3339(),
            role: &turn.role,
            parent_turn_id: turn.parent_turn_id.as_deref(),
            is_sidechain: Self::sqlite_bool(turn.is_sidechain),
            is_compaction_boundary: Self::sqlite_bool(turn.is_compaction_boundary),
            body: turn.body.as_deref(),
        }
    }

    pub(super) fn sqlite_bool(value: bool) -> i64 {
        if value { 1 } else { 0 }
    }

    fn execute_session_turn_batch_insert(
        stmt: &mut sqlite::Statement<'_>,
        provider_name: &str,
        binds: &SessionTurnBindValues<'_>,
        ingested_at: &str,
    ) -> Result<usize, String> {
        stmt.execute(sqlite::params![
            provider_name,
            binds.session_id,
            binds.turn_id,
            &binds.timestamp,
            binds.role,
            binds.parent_turn_id,
            binds.is_sidechain,
            binds.is_compaction_boundary,
            ingested_at,
            binds.body,
        ])
        .map_err(|e| format!("Batch insert row failed: {e}"))
    }

    pub fn count_session_turns(
        &self,
        provider_name: &str,
        session_id: &str,
    ) -> Result<SessionTurnCounts, String> {
        let (total, assistant, sidechain): (i64, i64, i64) = self
            .conn
            .query_row(
                "SELECT
                    COUNT(*) AS total,
                    COUNT(CASE WHEN role = 'assistant' THEN 1 END) AS assistant,
                    COUNT(CASE WHEN is_sidechain = 1 THEN 1 END) AS sidechain
                 FROM session_turns
                 WHERE provider_name = ?1 AND session_id = ?2",
                sqlite::params![provider_name, session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|e| format!("Failed to count session turns for trace: {e}"))?;

        Ok(SessionTurnCounts {
            total: total.max(0) as u64,
            assistant: assistant.max(0) as u64,
            sidechain: sidechain.max(0) as u64,
        })
    }

    /// Count assistant turns ingested for a provider since `since` (exclusive).
    /// `None` means count everything we've ever ingested for that provider.
    pub fn count_assistant_turns_since(
        &self,
        provider_name: &str,
        since: Option<&DateTime<Utc>>,
    ) -> Result<u64, String> {
        let count = self.query_assistant_turn_count(provider_name, since)?;
        Ok(count.max(0) as u64)
    }

    fn query_assistant_turn_count(
        &self,
        provider_name: &str,
        since: Option<&DateTime<Utc>>,
    ) -> Result<i64, String> {
        match since {
            Some(ts) => self.query_assistant_turn_count_after(provider_name, ts),
            None => self.query_all_assistant_turn_count(provider_name),
        }
    }

    fn query_assistant_turn_count_after(
        &self,
        provider_name: &str,
        since: &DateTime<Utc>,
    ) -> Result<i64, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM session_turns
                 WHERE provider_name = ?1 AND role = 'assistant' AND timestamp > ?2",
                sqlite::params![provider_name, since.to_rfc3339()],
                |row| row.get(0),
            )
            .map_err(Self::session_turn_count_error)
    }

    fn query_all_assistant_turn_count(&self, provider_name: &str) -> Result<i64, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM session_turns
                 WHERE provider_name = ?1 AND role = 'assistant'",
                sqlite::params![provider_name],
                |row| row.get(0),
            )
            .map_err(Self::session_turn_count_error)
    }

    fn session_turn_count_error(e: sqlite::Error) -> String {
        format!("Failed to count session turns: {e}")
    }

    pub fn has_session_user_text_turn(
        &self,
        provider_name: &str,
        session_id: &str,
        text: &str,
    ) -> Result<bool, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT body
                 FROM session_turns
                 WHERE provider_name = ?1
                   AND session_id = ?2
                   AND role = 'user'
                   AND body IS NOT NULL",
            )
            .map_err(|e| format!("Failed to prepare session user turn lookup: {e}"))?;
        let rows = stmt
            .query_map(sqlite::params![provider_name, session_id], |row| {
                Self::session_user_turn_body(row)
            })
            .map_err(|e| format!("Failed to query session user turns: {e}"))?;

        for row in rows {
            let body = row.map_err(|e| format!("Failed to read session user turn body: {e}"))?;
            if Self::session_user_turn_body_matches(&body, text) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn has_session_user_turn_containing(
        &self,
        provider_name: &str,
        session_id: &str,
        needle: &str,
    ) -> Result<bool, String> {
        if needle.is_empty() {
            return Ok(false);
        }
        let found: i64 = self
            .conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1
                    FROM session_turns
                    WHERE provider_name = ?1
                      AND session_id = ?2
                      AND role = 'user'
                      AND body IS NOT NULL
                      AND instr(body, ?3) > 0
                )",
                sqlite::params![provider_name, session_id, needle],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to query session user turn substring: {e}"))?;
        Ok(found != 0)
    }

    fn session_user_turn_body(row: &sqlite::Row<'_>) -> sqlite::Result<String> {
        row.get(0)
    }

    fn session_user_turn_body_matches(body: &str, text: &str) -> bool {
        Self::session_turn_body_has_exact_text(body, text)
    }

    pub(super) fn session_turn_body_has_exact_text(body: &str, text: &str) -> bool {
        Self::parse_session_turn_body(body)
            .as_ref()
            .is_some_and(|value| Self::parsed_session_turn_body_has_exact_text(value, text))
    }

    fn parse_session_turn_body(body: &str) -> Option<serde_json::Value> {
        serde_json::from_str::<serde_json::Value>(body).ok()
    }

    fn parsed_session_turn_body_has_exact_text(body: &serde_json::Value, text: &str) -> bool {
        Self::canonical_body_has_exact_text(body, text)
    }

    fn canonical_body_has_exact_text(body: &serde_json::Value, text: &str) -> bool {
        let serde_json::Value::Array(chunks) = body else {
            return false;
        };
        let canonical_text =
            Self::canonical_text_from_chunks(Self::session_turn_text_chunks(chunks));
        Self::canonical_text_equals(canonical_text.as_deref(), text)
    }

    fn session_turn_text_chunks(
        chunks: &[serde_json::Value],
    ) -> impl Iterator<Item = &serde_json::Value> + '_ {
        chunks
            .iter()
            .filter(|chunk| Self::session_turn_chunk_is_text(chunk))
    }

    fn canonical_text_from_chunks<'a>(
        chunks: impl Iterator<Item = &'a serde_json::Value>,
    ) -> Option<String> {
        let mut canonical_text = String::new();
        let mut has_text = false;
        for chunk in chunks {
            if let Some(candidate) = chunk.get("text").and_then(serde_json::Value::as_str) {
                canonical_text.push_str(candidate);
                has_text = true;
            }
        }
        has_text.then_some(canonical_text)
    }

    fn canonical_text_equals(candidate: Option<&str>, text: &str) -> bool {
        candidate == Some(text)
    }

    fn session_turn_chunk_is_text(chunk: &serde_json::Value) -> bool {
        chunk
            .get("type")
            .and_then(serde_json::Value::as_str)
            .is_none_or(|chunk_type| chunk_type == "text")
    }
}
