//! Durable bounded session-turn page checkpoints and atomic page application.

use super::*;
use chrono::SecondsFormat;
use sha2::{Digest, Sha256};

pub const SESSION_TURN_PAGES_PROTOCOL: &str = "oulipoly.session_turn_pages/v1";
const MAX_PAGE_TURNS: usize = 256;
const MAX_ID_BYTES: usize = 1024;
const MAX_TOKEN_BYTES: usize = 4096;
const MAX_LEASE_OWNER_BYTES: usize = 256;
const MAX_STREAM_ERROR_BYTES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTurnStreamProjection {
    CanonicalIngest,
    UserObservation,
}

impl SessionTurnStreamProjection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CanonicalIngest => "canonical_ingest",
            Self::UserObservation => "user_observation",
        }
    }

    fn parse(value: &str) -> Result<Self, DbError> {
        match value {
            "canonical_ingest" => Ok(Self::CanonicalIngest),
            "user_observation" => Ok(Self::UserObservation),
            _ => Err(format!("invalid session turn stream projection: {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTurnPageBodyState {
    Inline,
    Absent,
    OmittedOversize,
}

impl SessionTurnPageBodyState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::Absent => "absent",
            Self::OmittedOversize => "omitted_oversize",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTurnIngestStreamKey {
    pub provider_name: String,
    pub provider_instance_id: String,
    pub settings_id: String,
    pub session_id: String,
    pub projection: SessionTurnStreamProjection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTurnIngestStream {
    pub key: SessionTurnIngestStreamKey,
    pub checkpoint_generation: u64,
    pub after_token: Option<String>,
    pub snapshot_id: Option<String>,
    pub next_page_token: Option<String>,
    pub expected_page_index: u64,
    pub expected_turn_sequence: u64,
    pub status: String,
    pub committed_page_count: u64,
    pub committed_turn_count: u64,
    pub retry_count: u64,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTurnIngestFreshness {
    pub tracked_streams: u64,
    pub caught_up_streams: u64,
    pub latest_success_at: Option<String>,
    pub latest_updated_at: Option<String>,
}

impl SessionTurnIngestFreshness {
    pub fn is_caught_up(&self) -> bool {
        self.tracked_streams > 0 && self.caught_up_streams == self.tracked_streams
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTurnPageTurnIngest {
    pub session_id: String,
    pub turn_id: String,
    pub snapshot_sequence: u64,
    pub timestamp: DateTime<Utc>,
    pub role: String,
    pub parent_turn_id: Option<String>,
    pub is_sidechain: bool,
    pub is_compaction_boundary: bool,
    pub body_state: SessionTurnPageBodyState,
    pub body: Option<String>,
    pub body_bytes: Option<u64>,
    pub body_sha256: Option<String>,
    pub canonical_text_sha256: Option<String>,
    pub canonical_text_digest_verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTurnPageApply {
    pub key: SessionTurnIngestStreamKey,
    pub lease_owner: String,
    pub expected_generation: u64,
    pub request_token_sha256: String,
    pub snapshot_id: String,
    pub page_index: u64,
    pub page_start_sequence: u64,
    pub page_turn_count: u64,
    pub scan_progress: bool,
    pub snapshot_complete: bool,
    pub next_page_token: Option<String>,
    pub resume_token: Option<String>,
    pub page_digest: String,
    pub turns: Vec<SessionTurnPageTurnIngest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTurnPageApplyOutcome {
    pub inserted_turns: u64,
    pub duplicate_turns: u64,
    pub replayed: bool,
    pub checkpoint_generation: u64,
}

enum ExistingTurnDisposition {
    Insert,
    Duplicate,
    AdoptLegacy { retain_existing_body: bool },
}

struct ExistingTurnRow {
    timestamp: String,
    role: String,
    parent_turn_id: Option<String>,
    is_sidechain: bool,
    is_compaction_boundary: bool,
    ingest_digest: Option<String>,
    body_is_null: bool,
    body_matches: bool,
}

impl StateDb {
    pub fn enqueue_session_turn_ingest_stream(
        &self,
        key: &SessionTurnIngestStreamKey,
    ) -> Result<(), DbError> {
        validate_stream_key(key)?;
        let now = Utc::now().to_rfc3339();
        upsert_stream_on(&self.conn, key, &now)
    }

    /// Explicit recovery after an operator resolves a fixed paging terminal.
    /// Routine enqueue/import deliberately cannot invoke this transition.
    /// Compare both the retained checkpoint generation and sanitized reason;
    /// never reset tokens, turns, page indexes, or quarantine state.
    pub fn rearm_session_turn_ingest_after_capacity_resolution(
        &self,
        key: &SessionTurnIngestStreamKey,
        expected_generation: u64,
        expected_error: &str,
    ) -> Result<bool, DbError> {
        validate_stream_key(key)?;
        let generation = i64::try_from(expected_generation)
            .map_err(|_| "invalid checkpoint generation".to_string())?;
        let changed = self
            .conn
            .execute(
                "UPDATE session_turn_ingest_streams SET status = 'ready',
                 next_attempt_at = NULL, updated_at = ?8
             WHERE provider_name = ?1 AND provider_instance_id = ?2
                 AND settings_id = ?3 AND session_id = ?4 AND projection = ?5
                 AND checkpoint_generation = ?6 AND last_error = ?7
                 AND status = 'unsupported' AND lease_owner IS NULL
                 AND last_error IN ('codex_rollout_capacity',
                     'session_turn_page_budget_too_small', 'session_turn_record_ceiling_exceeded',
                     'session_turn_staging_capacity_exceeded', 'session_turn_paging_paused')",
                params![
                    key.provider_name,
                    key.provider_instance_id,
                    key.settings_id,
                    key.session_id,
                    key.projection.as_str(),
                    generation,
                    expected_error,
                    Utc::now().to_rfc3339()
                ],
            )
            .map_err(|error| format!("Failed to rearm resolved paging capacity: {error}"))?;
        Ok(changed == 1)
    }

    pub fn session_turn_ingest_stream(
        &self,
        key: &SessionTurnIngestStreamKey,
    ) -> Result<Option<SessionTurnIngestStream>, DbError> {
        validate_stream_key(key)?;
        read_stream_on(&self.conn, key)
    }

    pub fn canonical_session_turn_ingest_freshness(
        &self,
        provider_name: &str,
        session_id: &str,
    ) -> Result<SessionTurnIngestFreshness, DbError> {
        validate_id("provider_name", provider_name)?;
        validate_id("session_id", session_id)?;
        self.conn
            .query_row(
                "SELECT COUNT(*),
                        COALESCE(SUM(CASE WHEN status = 'caught_up' THEN 1 ELSE 0 END), 0),
                        MAX(last_success_at),
                        MAX(updated_at)
                 FROM session_turn_ingest_streams
                 WHERE provider_name = ?1 AND session_id = ?2
                   AND projection = 'canonical_ingest'",
                params![provider_name, session_id],
                |row| {
                    Ok(SessionTurnIngestFreshness {
                        tracked_streams: sqlite_u64(row.get(0)?, 0)?,
                        caught_up_streams: sqlite_u64(row.get(1)?, 1)?,
                        latest_success_at: row.get(2)?,
                        latest_updated_at: row.get(3)?,
                    })
                },
            )
            .map_err(|error| format!("Failed to read session turn ingest freshness: {error}"))
    }

    pub fn canonical_provider_turn_ingest_freshness(
        &self,
        provider_name: &str,
    ) -> Result<SessionTurnIngestFreshness, DbError> {
        validate_id("provider_name", provider_name)?;
        self.conn
            .query_row(
                "SELECT COUNT(*),
                        COALESCE(SUM(CASE WHEN status = 'caught_up' THEN 1 ELSE 0 END), 0),
                        MAX(last_success_at),
                        MAX(updated_at)
                 FROM session_turn_ingest_streams
                 WHERE provider_name = ?1 AND projection = 'canonical_ingest'",
                params![provider_name],
                |row| {
                    Ok(SessionTurnIngestFreshness {
                        tracked_streams: sqlite_u64(row.get(0)?, 0)?,
                        caught_up_streams: sqlite_u64(row.get(1)?, 1)?,
                        latest_success_at: row.get(2)?,
                        latest_updated_at: row.get(3)?,
                    })
                },
            )
            .map_err(|error| {
                format!("Failed to read provider session turn ingest freshness: {error}")
            })
    }

    pub fn lease_ready_session_turn_ingest_stream(
        &self,
        projection: SessionTurnStreamProjection,
        lease_owner: &str,
        now: DateTime<Utc>,
        lease_expires_at: DateTime<Utc>,
    ) -> Result<Option<SessionTurnIngestStream>, DbError> {
        validate_lease(lease_owner, now, lease_expires_at)?;
        let now = timestamp_text(now);
        let lease_expires_at = timestamp_text(lease_expires_at);
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|error| format!("Failed to begin session turn lease transaction: {error}"))?;
        let Some(key) = next_leaseable_stream_key(&tx, projection, &now)? else {
            tx.commit()
                .map_err(|error| format!("Failed to commit empty session turn lease: {error}"))?;
            return Ok(None);
        };
        let changed = tx
            .execute(
                "UPDATE session_turn_ingest_streams
                 SET status = 'active', lease_owner = ?6, lease_expires_at = ?7, updated_at = ?8
                 WHERE provider_name = ?1 AND provider_instance_id = ?2 AND settings_id = ?3
                   AND session_id = ?4 AND projection = ?5
                   AND (lease_owner IS NULL OR lease_expires_at <= ?8)
                   AND (status IN ('ready', 'retry_wait')
                        OR (status = 'active' AND lease_expires_at <= ?8))",
                params![
                    key.provider_name,
                    key.provider_instance_id,
                    key.settings_id,
                    key.session_id,
                    key.projection.as_str(),
                    lease_owner,
                    lease_expires_at,
                    now,
                ],
            )
            .map_err(|error| format!("Failed to acquire session turn stream lease: {error}"))?;
        if changed != 1 {
            return Err("session_turn_stream_lease_contended".to_string());
        }
        let stream = read_stream_on(&tx, &key)?
            .ok_or_else(|| "leased session turn stream disappeared".to_string())?;
        tx.commit()
            .map_err(|error| format!("Failed to commit session turn stream lease: {error}"))?;
        Ok(Some(stream))
    }

    pub fn lease_session_turn_ingest_stream(
        &self,
        key: &SessionTurnIngestStreamKey,
        lease_owner: &str,
        now: DateTime<Utc>,
        lease_expires_at: DateTime<Utc>,
    ) -> Result<Option<SessionTurnIngestStream>, DbError> {
        validate_stream_key(key)?;
        validate_lease(lease_owner, now, lease_expires_at)?;
        let now = timestamp_text(now);
        let lease_expires_at = timestamp_text(lease_expires_at);
        let changed = self
            .conn
            .execute(
                "UPDATE session_turn_ingest_streams
                 SET status = 'active', lease_owner = ?6, lease_expires_at = ?7, updated_at = ?8
                 WHERE provider_name = ?1 AND provider_instance_id = ?2 AND settings_id = ?3
                   AND session_id = ?4 AND projection = ?5
                   AND (lease_owner IS NULL OR lease_expires_at <= ?8)
                   AND (status = 'ready'
                        OR (status = 'retry_wait' AND (next_attempt_at IS NULL OR next_attempt_at <= ?8))
                        OR (status = 'active' AND lease_expires_at <= ?8))",
                params![
                    key.provider_name,
                    key.provider_instance_id,
                    key.settings_id,
                    key.session_id,
                    key.projection.as_str(),
                    lease_owner,
                    lease_expires_at,
                    now,
                ],
            )
            .map_err(|error| format!("Failed to acquire requested session turn stream lease: {error}"))?;
        if changed == 0 {
            return Ok(None);
        }
        read_stream_on(&self.conn, key)
    }

    pub fn retry_session_turn_ingest_stream(
        &self,
        key: &SessionTurnIngestStreamKey,
        lease_owner: &str,
        expected_generation: u64,
        next_attempt_at: DateTime<Utc>,
        error: &str,
    ) -> Result<(), DbError> {
        update_stream_after_worker_failure(
            &self.conn,
            key,
            lease_owner,
            expected_generation,
            "retry_wait",
            Some(timestamp_text(next_attempt_at)),
            error,
        )
    }

    pub fn mark_session_turn_ingest_unsupported(
        &self,
        key: &SessionTurnIngestStreamKey,
        lease_owner: &str,
        expected_generation: u64,
        error: &str,
    ) -> Result<(), DbError> {
        update_stream_after_worker_failure(
            &self.conn,
            key,
            lease_owner,
            expected_generation,
            "unsupported",
            None,
            error,
        )
    }

    pub fn quarantine_session_turn_ingest_stream(
        &self,
        key: &SessionTurnIngestStreamKey,
        lease_owner: &str,
        expected_generation: u64,
        error: &str,
    ) -> Result<(), DbError> {
        update_stream_after_worker_failure(
            &self.conn,
            key,
            lease_owner,
            expected_generation,
            "quarantined",
            None,
            error,
        )
    }

    pub fn import_session_and_enqueue_turn_ingest(
        &self,
        metadata: &ImportedSessionDisplayMetadataUpsert,
        key: &SessionTurnIngestStreamKey,
        started_at: &DateTime<Utc>,
        model_name: &str,
    ) -> Result<bool, DbError> {
        validate_stream_key(key)?;
        if metadata.provider_name != key.provider_name
            || metadata.provider_session_id != key.session_id
        {
            return Err("import metadata and turn stream identity mismatch".to_string());
        }
        let turn_count = metadata
            .turn_count
            .map(|value| {
                i64::try_from(value)
                    .map_err(|_| "Imported session turn_count exceeds SQLite INTEGER".to_string())
            })
            .transpose()?;
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|error| format!("Failed to begin session import transaction: {error}"))?;
        let existed = Self::session_chain_segment_exists(
            &tx,
            &metadata.provider_name,
            &metadata.provider_session_id,
        )?;
        if !existed {
            mint_imported_chain_on(&tx, key, started_at, model_name)?;
        }
        StateDb::bind_session_provider_authority_on(
            &tx,
            &key.provider_name,
            &key.session_id,
            &key.provider_instance_id,
            &key.settings_id,
        )?;
        upsert_import_metadata_on(&tx, metadata, turn_count)?;
        upsert_stream_on(&tx, key, &metadata.seen_at.to_rfc3339())?;
        tx.commit()
            .map_err(|error| format!("Failed to commit session import transaction: {error}"))?;
        Ok(!existed)
    }

    pub fn apply_session_turn_page(
        &self,
        page: &SessionTurnPageApply,
    ) -> Result<SessionTurnPageApplyOutcome, DbError> {
        validate_page(page)?;
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|error| format!("Failed to begin session turn page transaction: {error}"))?;
        let stream = read_stream_on(&tx, &page.key)?
            .ok_or_else(|| "session turn ingest stream does not exist".to_string())?;
        validate_page_lease(&stream, page)?;

        if is_replay_attempt(&tx, page)? {
            if replay_digest_matches(&tx, page)? {
                release_replayed_page_lease(&tx, page)?;
                tx.commit().map_err(|error| {
                    format!("Failed to commit replayed session turn page lease release: {error}")
                })?;
                return Ok(SessionTurnPageApplyOutcome {
                    inserted_turns: 0,
                    duplicate_turns: page.page_turn_count,
                    replayed: true,
                    checkpoint_generation: stream.checkpoint_generation,
                });
            }
            quarantine_stream(&tx, &page.key, &page.lease_owner, "page_replay_mismatch")?;
            tx.commit().map_err(|error| {
                format!("Failed to commit session turn page quarantine: {error}")
            })?;
            return Err("page_replay_mismatch".to_string());
        }

        validate_checkpoint(&stream, page)?;
        let mut prepared = Vec::with_capacity(page.turns.len());
        for turn in &page.turns {
            let digest = turn_ingest_digest(turn);
            match existing_turn_disposition(&tx, &page.key.provider_name, turn, &digest) {
                Ok(disposition) => prepared.push((turn, digest, disposition)),
                Err(error) => {
                    quarantine_stream(&tx, &page.key, &page.lease_owner, "turn_content_conflict")?;
                    tx.commit().map_err(|commit_error| {
                        format!("Failed to commit session turn conflict: {commit_error}")
                    })?;
                    return Err(error);
                }
            }
        }

        ensure_imported_chain_on(&tx, page)?;
        let now = Utc::now().to_rfc3339();
        let mut inserted_turns = 0;
        let mut duplicate_turns = 0;
        for (turn, digest, disposition) in prepared {
            match disposition {
                ExistingTurnDisposition::Insert => {
                    insert_page_turn(&tx, &page.key.provider_name, turn, &digest, &now)?;
                    inserted_turns += 1;
                }
                ExistingTurnDisposition::Duplicate => duplicate_turns += 1,
                ExistingTurnDisposition::AdoptLegacy {
                    retain_existing_body,
                } => {
                    adopt_legacy_turn(
                        &tx,
                        &page.key.provider_name,
                        turn,
                        &digest,
                        retain_existing_body,
                    )?;
                    duplicate_turns += 1;
                }
            }
            insert_owned_page_turn_event(&tx, turn, &now)?;
        }
        update_imported_segment_tail(&tx, page)?;
        let generation = advance_stream_checkpoint(&tx, &stream, page, &now)?;
        tx.commit()
            .map_err(|error| format!("Failed to commit session turn page: {error}"))?;
        Ok(SessionTurnPageApplyOutcome {
            inserted_turns,
            duplicate_turns,
            replayed: false,
            checkpoint_generation: generation,
        })
    }
}

fn validate_stream_key(key: &SessionTurnIngestStreamKey) -> Result<(), DbError> {
    for (name, value) in [
        ("provider_name", key.provider_name.as_str()),
        ("provider_instance_id", key.provider_instance_id.as_str()),
        ("settings_id", key.settings_id.as_str()),
        ("session_id", key.session_id.as_str()),
    ] {
        validate_id(name, value)?;
    }
    Ok(())
}

fn validate_id(name: &str, value: &str) -> Result<(), DbError> {
    if value.is_empty() || value.len() > MAX_ID_BYTES {
        Err(format!("invalid session turn stream {name}"))
    } else {
        Ok(())
    }
}

fn validate_lease(
    lease_owner: &str,
    now: DateTime<Utc>,
    lease_expires_at: DateTime<Utc>,
) -> Result<(), DbError> {
    if lease_owner.is_empty() || lease_owner.len() > MAX_LEASE_OWNER_BYTES {
        return Err("invalid session turn stream lease owner".to_string());
    }
    if lease_expires_at <= now {
        return Err("session turn stream lease must expire after acquisition".to_string());
    }
    Ok(())
}

fn timestamp_text(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Nanos, true)
}

fn next_leaseable_stream_key(
    conn: &Connection,
    projection: SessionTurnStreamProjection,
    now: &str,
) -> Result<Option<SessionTurnIngestStreamKey>, DbError> {
    conn.query_row(
        "SELECT provider_name, provider_instance_id, settings_id, session_id, projection
         FROM session_turn_ingest_streams
         WHERE projection = ?2
           AND (lease_owner IS NULL OR lease_expires_at <= ?1)
           AND (status = 'ready'
                OR (status = 'retry_wait' AND (next_attempt_at IS NULL OR next_attempt_at <= ?1))
                OR (status = 'active' AND lease_expires_at <= ?1))
         ORDER BY priority DESC, updated_at, provider_name, provider_instance_id, settings_id,
                  session_id, projection
         LIMIT 1",
        params![now, projection.as_str()],
        |row| {
            let projection_text = row.get::<_, String>(4)?;
            let projection =
                SessionTurnStreamProjection::parse(&projection_text).map_err(|error| {
                    sqlite::Error::FromSqlConversionFailure(
                        4,
                        sqlite::Type::Text,
                        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
                    )
                })?;
            Ok(SessionTurnIngestStreamKey {
                provider_name: row.get(0)?,
                provider_instance_id: row.get(1)?,
                settings_id: row.get(2)?,
                session_id: row.get(3)?,
                projection,
            })
        },
    )
    .optional()
    .map_err(|error| format!("Failed to select ready session turn stream: {error}"))
}

fn update_stream_after_worker_failure(
    conn: &Connection,
    key: &SessionTurnIngestStreamKey,
    lease_owner: &str,
    expected_generation: u64,
    status: &str,
    next_attempt_at: Option<String>,
    error: &str,
) -> Result<(), DbError> {
    if lease_owner.is_empty() || lease_owner.len() > MAX_LEASE_OWNER_BYTES {
        return Err("invalid session turn stream lease owner".to_string());
    }
    let error = truncate_utf8(error, MAX_STREAM_ERROR_BYTES);
    let changed = conn
        .execute(
            "UPDATE session_turn_ingest_streams
             SET status = ?6, next_attempt_at = ?7, retry_count = retry_count + 1,
                 last_error = ?8, lease_owner = NULL, lease_expires_at = NULL, updated_at = ?9
             WHERE provider_name = ?1 AND provider_instance_id = ?2 AND settings_id = ?3
               AND session_id = ?4 AND projection = ?5 AND lease_owner = ?10
               AND checkpoint_generation = ?11",
            params![
                key.provider_name,
                key.provider_instance_id,
                key.settings_id,
                key.session_id,
                key.projection.as_str(),
                status,
                next_attempt_at,
                error,
                timestamp_text(Utc::now()),
                lease_owner,
                i64::try_from(expected_generation)
                    .map_err(|_| "checkpoint generation overflow".to_string())?,
            ],
        )
        .map_err(|db_error| format!("Failed to update session turn stream failure: {db_error}"))?;
    if changed != 1 {
        return Err("session_turn_stream_lease_lost".to_string());
    }
    Ok(())
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut boundary = max_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    &value[..boundary]
}

fn validate_page(page: &SessionTurnPageApply) -> Result<(), DbError> {
    validate_stream_key(&page.key)?;
    if page.turns.len() > MAX_PAGE_TURNS || page.page_turn_count != page.turns.len() as u64 {
        return Err("invalid session turn page count".to_string());
    }
    validate_sha256("request_token_sha256", &page.request_token_sha256)?;
    validate_sha256("page_digest", &page.page_digest)?;
    validate_token("snapshot_id", Some(page.snapshot_id.as_str()))?;
    validate_token("next_page_token", page.next_page_token.as_deref())?;
    validate_token("resume_token", page.resume_token.as_deref())?;
    if page.snapshot_complete != page.resume_token.is_some()
        || page.snapshot_complete == page.next_page_token.is_some()
    {
        return Err("invalid session turn page completion tokens".to_string());
    }
    if page.scan_progress && !page.turns.is_empty() {
        return Err("scan-progress page contains turns".to_string());
    }
    for (offset, turn) in page.turns.iter().enumerate() {
        if turn.session_id != page.key.session_id
            || turn.snapshot_sequence != page.page_start_sequence + offset as u64
        {
            return Err("invalid session turn page sequence or identity".to_string());
        }
        validate_turn(turn)?;
    }
    Ok(())
}

fn validate_turn(turn: &SessionTurnPageTurnIngest) -> Result<(), DbError> {
    for (name, value) in [
        ("session_id", turn.session_id.as_str()),
        ("turn_id", turn.turn_id.as_str()),
        ("role", turn.role.as_str()),
    ] {
        if value.is_empty() || value.len() > MAX_ID_BYTES {
            return Err(format!("invalid session page turn {name}"));
        }
    }
    if let Some(parent) = turn.parent_turn_id.as_deref()
        && (parent.is_empty() || parent.len() > MAX_ID_BYTES)
    {
        return Err("invalid session page parent turn id".to_string());
    }
    if let Some(hash) = turn.body_sha256.as_deref() {
        validate_sha256("body_sha256", hash)?;
    }
    if let Some(hash) = turn.canonical_text_sha256.as_deref() {
        validate_sha256("canonical_text_sha256", hash)?;
    }
    match turn.body_state {
        SessionTurnPageBodyState::Inline
            if turn.body.is_some() && turn.body_bytes.is_some() && turn.body_sha256.is_some() => {}
        SessionTurnPageBodyState::Absent
            if turn.body.is_none()
                && turn.body_bytes.is_none()
                && turn.body_sha256.is_none()
                && turn.canonical_text_sha256.is_none() => {}
        SessionTurnPageBodyState::OmittedOversize if turn.body.is_none() => {}
        _ => return Err("invalid session page turn body metadata".to_string()),
    }
    Ok(())
}

fn validate_sha256(name: &str, value: &str) -> Result<(), DbError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(format!("invalid {name}"))
    }
}

fn validate_token(name: &str, value: Option<&str>) -> Result<(), DbError> {
    if value.is_some_and(|token| token.is_empty() || token.len() > MAX_TOKEN_BYTES) {
        Err(format!("invalid {name}"))
    } else {
        Ok(())
    }
}

fn upsert_stream_on(
    conn: &Connection,
    key: &SessionTurnIngestStreamKey,
    now: &str,
) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO session_turn_ingest_streams (
            provider_name, provider_instance_id, settings_id, session_id, projection,
            paging_protocol, status, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'ready', ?7, ?7)
         ON CONFLICT(provider_name, provider_instance_id, settings_id, session_id, projection)
         DO UPDATE SET
            status = CASE
                WHEN session_turn_ingest_streams.status = 'quarantined' THEN 'quarantined'
                WHEN session_turn_ingest_streams.status = 'unsupported'
                    AND session_turn_ingest_streams.last_error IN (
                        'codex_rollout_capacity', 'session_turn_page_budget_too_small',
                        'session_turn_record_ceiling_exceeded',
                        'session_turn_staging_capacity_exceeded', 'session_turn_paging_paused'
                    ) THEN 'unsupported'
                ELSE 'ready'
            END,
            next_attempt_at = NULL,
            updated_at = excluded.updated_at",
        params![
            key.provider_name,
            key.provider_instance_id,
            key.settings_id,
            key.session_id,
            key.projection.as_str(),
            SESSION_TURN_PAGES_PROTOCOL,
            now,
        ],
    )
    .map(|_| ())
    .map_err(|error| format!("Failed to enqueue session turn ingest stream: {error}"))
}

fn read_stream_on(
    conn: &Connection,
    key: &SessionTurnIngestStreamKey,
) -> Result<Option<SessionTurnIngestStream>, DbError> {
    conn.query_row(
        "SELECT checkpoint_generation, after_token, snapshot_id, next_page_token,
                expected_page_index, expected_turn_sequence, status,
                committed_page_count, committed_turn_count, projection,
                retry_count, lease_owner, lease_expires_at
         FROM session_turn_ingest_streams
         WHERE provider_name = ?1 AND provider_instance_id = ?2 AND settings_id = ?3
           AND session_id = ?4 AND projection = ?5",
        params![
            key.provider_name,
            key.provider_instance_id,
            key.settings_id,
            key.session_id,
            key.projection.as_str(),
        ],
        |row| {
            let projection = SessionTurnStreamProjection::parse(row.get::<_, String>(9)?.as_str())
                .map_err(|error| {
                    sqlite::Error::FromSqlConversionFailure(
                        9,
                        sqlite::Type::Text,
                        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
                    )
                })?;
            Ok(SessionTurnIngestStream {
                key: SessionTurnIngestStreamKey {
                    projection,
                    ..key.clone()
                },
                checkpoint_generation: sqlite_u64(row.get(0)?, 0)?,
                after_token: row.get(1)?,
                snapshot_id: row.get(2)?,
                next_page_token: row.get(3)?,
                expected_page_index: sqlite_u64(row.get(4)?, 4)?,
                expected_turn_sequence: sqlite_u64(row.get(5)?, 5)?,
                status: row.get(6)?,
                committed_page_count: sqlite_u64(row.get(7)?, 7)?,
                committed_turn_count: sqlite_u64(row.get(8)?, 8)?,
                retry_count: sqlite_u64(row.get(10)?, 10)?,
                lease_owner: row.get(11)?,
                lease_expires_at: row.get(12)?,
            })
        },
    )
    .optional()
    .map_err(|error| format!("Failed to read session turn ingest stream: {error}"))
}

fn sqlite_u64(value: i64, column: usize) -> sqlite::Result<u64> {
    u64::try_from(value).map_err(|error| {
        sqlite::Error::FromSqlConversionFailure(column, sqlite::Type::Integer, Box::new(error))
    })
}

fn mint_imported_chain_on(
    conn: &Connection,
    key: &SessionTurnIngestStreamKey,
    started_at: &DateTime<Utc>,
    model_name: &str,
) -> Result<(), DbError> {
    let chain_id = Uuid::new_v4().to_string();
    let timestamp = started_at.to_rfc3339();
    StateDb::insert_imported_chain(conn, &chain_id, &timestamp, model_name)?;
    StateDb::insert_imported_segment(
        conn,
        &chain_id,
        &key.provider_name,
        &key.session_id,
        &timestamp,
    )
}

fn upsert_import_metadata_on(
    conn: &Connection,
    input: &ImportedSessionDisplayMetadataUpsert,
    turn_count: Option<i64>,
) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO imported_session_display_metadata (
            provider_name, provider_session_id, title, cwd, turn_count,
            provider_updated_at, first_seen_at, last_seen_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
         ON CONFLICT(provider_name, provider_session_id) DO UPDATE SET
            title = excluded.title,
            cwd = excluded.cwd,
            turn_count = excluded.turn_count,
            provider_updated_at = excluded.provider_updated_at,
            last_seen_at = excluded.last_seen_at",
        params![
            input.provider_name,
            input.provider_session_id,
            input.title,
            input.cwd,
            turn_count,
            input.provider_updated_at.map(|value| value.to_rfc3339()),
            input.seen_at.to_rfc3339(),
        ],
    )
    .map(|_| ())
    .map_err(|error| format!("Failed to upsert imported session display metadata: {error}"))
}

fn is_replay_attempt(conn: &Connection, page: &SessionTurnPageApply) -> Result<bool, DbError> {
    conn.query_row(
        "SELECT EXISTS (
            SELECT 1 FROM session_turn_ingest_streams
            WHERE provider_name = ?1 AND provider_instance_id = ?2 AND settings_id = ?3
              AND session_id = ?4 AND projection = ?5
              AND last_committed_snapshot_id = ?6 AND last_committed_page_index = ?7
              AND last_request_token_sha256 = ?8
         )",
        params![
            page.key.provider_name,
            page.key.provider_instance_id,
            page.key.settings_id,
            page.key.session_id,
            page.key.projection.as_str(),
            page.snapshot_id,
            page.page_index as i64,
            page.request_token_sha256,
        ],
        |row| row.get::<_, i64>(0),
    )
    .map(|value| value != 0)
    .map_err(|error| format!("Failed to inspect session turn page replay guard: {error}"))
}

fn replay_digest_matches(conn: &Connection, page: &SessionTurnPageApply) -> Result<bool, DbError> {
    conn.query_row(
        "SELECT last_page_digest = ?6
         FROM session_turn_ingest_streams
         WHERE provider_name = ?1 AND provider_instance_id = ?2 AND settings_id = ?3
           AND session_id = ?4 AND projection = ?5",
        params![
            page.key.provider_name,
            page.key.provider_instance_id,
            page.key.settings_id,
            page.key.session_id,
            page.key.projection.as_str(),
            page.page_digest,
        ],
        |row| row.get::<_, bool>(0),
    )
    .map_err(|error| format!("Failed to compare session turn page replay digest: {error}"))
}

fn release_replayed_page_lease(
    conn: &Connection,
    page: &SessionTurnPageApply,
) -> Result<(), DbError> {
    let status = if page.snapshot_complete {
        "caught_up"
    } else {
        "ready"
    };
    let changed = conn
        .execute(
            "UPDATE session_turn_ingest_streams
             SET status = ?6, lease_owner = NULL, lease_expires_at = NULL, updated_at = ?7
             WHERE provider_name = ?1 AND provider_instance_id = ?2 AND settings_id = ?3
               AND session_id = ?4 AND projection = ?5 AND lease_owner = ?8",
            params![
                page.key.provider_name,
                page.key.provider_instance_id,
                page.key.settings_id,
                page.key.session_id,
                page.key.projection.as_str(),
                status,
                timestamp_text(Utc::now()),
                page.lease_owner,
            ],
        )
        .map_err(|error| format!("Failed to release replayed session turn page lease: {error}"))?;
    if changed != 1 {
        return Err("session_turn_stream_lease_lost".to_string());
    }
    Ok(())
}

fn quarantine_stream(
    conn: &Connection,
    key: &SessionTurnIngestStreamKey,
    lease_owner: &str,
    error: &str,
) -> Result<(), DbError> {
    let changed = conn
        .execute(
            "UPDATE session_turn_ingest_streams
         SET status = 'quarantined', last_error = ?6, lease_owner = NULL,
             lease_expires_at = NULL, updated_at = ?7
         WHERE provider_name = ?1 AND provider_instance_id = ?2 AND settings_id = ?3
           AND session_id = ?4 AND projection = ?5 AND lease_owner = ?8",
            params![
                key.provider_name,
                key.provider_instance_id,
                key.settings_id,
                key.session_id,
                key.projection.as_str(),
                error,
                timestamp_text(Utc::now()),
                lease_owner,
            ],
        )
        .map_err(|db_error| format!("Failed to quarantine session turn stream: {db_error}"))?;
    if changed != 1 {
        return Err("session_turn_stream_lease_lost".to_string());
    }
    Ok(())
}

fn validate_page_lease(
    stream: &SessionTurnIngestStream,
    page: &SessionTurnPageApply,
) -> Result<(), DbError> {
    if page.lease_owner.is_empty() || page.lease_owner.len() > MAX_LEASE_OWNER_BYTES {
        return Err("invalid session turn page lease owner".to_string());
    }
    if stream.lease_owner.as_deref() != Some(page.lease_owner.as_str()) {
        return Err("session_turn_stream_lease_lost".to_string());
    }
    let expires_at = stream
        .lease_expires_at
        .as_deref()
        .ok_or_else(|| "session_turn_stream_lease_lost".to_string())?
        .parse::<DateTime<Utc>>()
        .map_err(|_| "session_turn_stream_lease_expiry_invalid".to_string())?;
    if expires_at <= Utc::now() {
        return Err("session_turn_stream_lease_expired".to_string());
    }
    Ok(())
}

fn validate_checkpoint(
    stream: &SessionTurnIngestStream,
    page: &SessionTurnPageApply,
) -> Result<(), DbError> {
    if stream.checkpoint_generation != page.expected_generation {
        return Err("session_turn_page_stale_generation".to_string());
    }
    if stream.expected_page_index != page.page_index
        || stream.expected_turn_sequence != page.page_start_sequence
    {
        return Err("session_turn_page_checkpoint_mismatch".to_string());
    }
    if let Some(snapshot_id) = stream.snapshot_id.as_deref() {
        if snapshot_id != page.snapshot_id {
            return Err("session_turn_page_snapshot_mismatch".to_string());
        }
    } else if page.page_index != 0 {
        return Err("session_turn_page_initial_index_mismatch".to_string());
    }
    Ok(())
}

fn existing_turn_disposition(
    conn: &Connection,
    provider_name: &str,
    turn: &SessionTurnPageTurnIngest,
    digest: &str,
) -> Result<ExistingTurnDisposition, DbError> {
    let existing = read_existing_turn(conn, provider_name, turn)?;
    let Some(existing) = existing else {
        return Ok(ExistingTurnDisposition::Insert);
    };
    if let Some(existing_digest) = existing.ingest_digest.as_deref() {
        return if existing_digest == digest {
            Ok(ExistingTurnDisposition::Duplicate)
        } else {
            Err(format!("turn_content_conflict: {}", turn.turn_id))
        };
    }
    if !legacy_metadata_matches(&existing, turn) {
        return Err(format!("turn_content_conflict: {}", turn.turn_id));
    }
    match turn.body_state {
        SessionTurnPageBodyState::Inline if existing.body_is_null || existing.body_matches => {
            Ok(ExistingTurnDisposition::AdoptLegacy {
                retain_existing_body: false,
            })
        }
        SessionTurnPageBodyState::Absent if existing.body_is_null => {
            Ok(ExistingTurnDisposition::AdoptLegacy {
                retain_existing_body: false,
            })
        }
        SessionTurnPageBodyState::OmittedOversize => Ok(ExistingTurnDisposition::AdoptLegacy {
            retain_existing_body: !existing.body_is_null,
        }),
        _ => Err(format!("turn_content_conflict: {}", turn.turn_id)),
    }
}

fn read_existing_turn(
    conn: &Connection,
    provider_name: &str,
    turn: &SessionTurnPageTurnIngest,
) -> Result<Option<ExistingTurnRow>, DbError> {
    conn.query_row(
        "SELECT timestamp, role, parent_turn_id, is_sidechain, is_compaction_boundary,
                ingest_digest, body IS NULL,
                CASE WHEN ?4 IS NULL THEN 0 ELSE body = ?4 END
         FROM session_turns
         WHERE provider_name = ?1 AND session_id = ?2 AND turn_id = ?3",
        params![provider_name, turn.session_id, turn.turn_id, turn.body],
        |row| {
            Ok(ExistingTurnRow {
                timestamp: row.get(0)?,
                role: row.get(1)?,
                parent_turn_id: row.get(2)?,
                is_sidechain: row.get::<_, i64>(3)? != 0,
                is_compaction_boundary: row.get::<_, i64>(4)? != 0,
                ingest_digest: row.get(5)?,
                body_is_null: row.get(6)?,
                body_matches: row.get(7)?,
            })
        },
    )
    .optional()
    .map_err(|error| format!("Failed to inspect existing session turn: {error}"))
}

fn legacy_metadata_matches(existing: &ExistingTurnRow, turn: &SessionTurnPageTurnIngest) -> bool {
    existing.timestamp == turn.timestamp.to_rfc3339()
        && existing.role == turn.role
        && existing.parent_turn_id == turn.parent_turn_id
        && existing.is_sidechain == turn.is_sidechain
        && existing.is_compaction_boundary == turn.is_compaction_boundary
}

fn ensure_imported_chain_on(conn: &Connection, page: &SessionTurnPageApply) -> Result<(), DbError> {
    if StateDb::session_chain_segment_exists(conn, &page.key.provider_name, &page.key.session_id)? {
        return Ok(());
    }
    let started_at = page
        .turns
        .first()
        .map(|turn| turn.timestamp)
        .unwrap_or_else(Utc::now);
    mint_imported_chain_on(conn, &page.key, &started_at, "<unknown>")
}

fn insert_page_turn(
    conn: &Connection,
    provider_name: &str,
    turn: &SessionTurnPageTurnIngest,
    digest: &str,
    now: &str,
) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO session_turns (
            provider_name, session_id, turn_id, timestamp, role, parent_turn_id,
            is_sidechain, is_compaction_boundary, source_file, ingested_at, body,
            ingest_digest, body_state, body_sha256, body_bytes,
            canonical_text_sha256, canonical_text_digest_verified
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, '', ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        params![
            provider_name,
            turn.session_id,
            turn.turn_id,
            turn.timestamp.to_rfc3339(),
            turn.role,
            turn.parent_turn_id,
            StateDb::sqlite_bool(turn.is_sidechain),
            StateDb::sqlite_bool(turn.is_compaction_boundary),
            now,
            turn.body,
            digest,
            turn.body_state.as_str(),
            turn.body_sha256,
            optional_u64_sql(turn.body_bytes)?,
            turn.canonical_text_sha256,
            StateDb::sqlite_bool(turn.canonical_text_digest_verified),
        ],
    )
    .map(|_| ())
    .map_err(|error| format!("Failed to insert session turn page row: {error}"))
}

fn adopt_legacy_turn(
    conn: &Connection,
    provider_name: &str,
    turn: &SessionTurnPageTurnIngest,
    digest: &str,
    retain_existing_body: bool,
) -> Result<(), DbError> {
    conn.execute(
        "UPDATE session_turns
         SET body = CASE WHEN body IS NULL THEN ?4 ELSE body END,
             ingest_digest = CASE WHEN ?5 THEN NULL ELSE ?6 END,
             body_state = CASE WHEN ?5 THEN body_state ELSE ?7 END,
             body_sha256 = CASE WHEN ?5 THEN body_sha256 ELSE ?8 END,
             body_bytes = CASE WHEN ?5 THEN body_bytes ELSE ?9 END,
             canonical_text_sha256 = CASE WHEN ?5 THEN canonical_text_sha256 ELSE ?10 END,
             canonical_text_digest_verified = CASE WHEN ?5 THEN canonical_text_digest_verified ELSE ?11 END
         WHERE provider_name = ?1 AND session_id = ?2 AND turn_id = ?3",
        params![
            provider_name,
            turn.session_id,
            turn.turn_id,
            turn.body,
            retain_existing_body,
            digest,
            turn.body_state.as_str(),
            turn.body_sha256,
            optional_u64_sql(turn.body_bytes)?,
            turn.canonical_text_sha256,
            StateDb::sqlite_bool(turn.canonical_text_digest_verified),
        ],
    )
    .map(|_| ())
    .map_err(|error| format!("Failed to adopt legacy session turn row: {error}"))
}

fn optional_u64_sql(value: Option<u64>) -> Result<Option<i64>, DbError> {
    value
        .map(|value| i64::try_from(value).map_err(|_| "value exceeds SQLite INTEGER".to_string()))
        .transpose()
}

fn insert_owned_page_turn_event(
    conn: &Connection,
    turn: &SessionTurnPageTurnIngest,
    now: &str,
) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO owned_turn_events (
            session_id, turn_uuid, is_compaction_boundary, summary_metadata_json, ingested_at
         ) VALUES (?1, ?2, ?3, NULL, ?4)
         ON CONFLICT(session_id, turn_uuid) DO NOTHING",
        params![
            turn.session_id,
            turn.turn_id,
            StateDb::sqlite_bool(turn.is_compaction_boundary),
            now,
        ],
    )
    .map(|_| ())
    .map_err(|error| format!("Failed to insert bounded owned turn event: {error}"))
}

fn update_imported_segment_tail(
    conn: &Connection,
    page: &SessionTurnPageApply,
) -> Result<(), DbError> {
    let Some(last_turn) = page.turns.last() else {
        return Ok(());
    };
    conn.execute(
        "UPDATE session_chain_segments
         SET last_turn_id = ?3
         WHERE provider_name = ?1 AND session_id = ?2",
        params![
            page.key.provider_name,
            page.key.session_id,
            last_turn.turn_id
        ],
    )
    .map(|_| ())
    .map_err(|error| format!("Failed to advance imported session segment: {error}"))
}

fn advance_stream_checkpoint(
    conn: &Connection,
    stream: &SessionTurnIngestStream,
    page: &SessionTurnPageApply,
    now: &str,
) -> Result<u64, DbError> {
    let generation = stream
        .checkpoint_generation
        .checked_add(1)
        .ok_or_else(|| "session turn checkpoint generation overflow".to_string())?;
    let prefix_digest = rolling_prefix_digest(conn, &page.key, page.page_index, &page.page_digest)?;
    let (
        after_token,
        after_token_sha256,
        snapshot_id,
        next_page_token,
        expected_page,
        expected_sequence,
        status,
    ) = if page.snapshot_complete {
        let resume = page.resume_token.clone();
        (
            resume.clone(),
            resume.as_deref().map(sha256_text),
            None,
            None,
            0_i64,
            0_i64,
            "caught_up",
        )
    } else {
        (
            stream.after_token.clone(),
            stream.after_token.as_deref().map(sha256_text),
            Some(page.snapshot_id.clone()),
            page.next_page_token.clone(),
            i64::try_from(page.page_index + 1).map_err(|_| "page index overflow".to_string())?,
            i64::try_from(page.page_start_sequence + page.page_turn_count)
                .map_err(|_| "turn sequence overflow".to_string())?,
            "ready",
        )
    };
    let changed = conn
        .execute(
            "UPDATE session_turn_ingest_streams
             SET checkpoint_generation = ?6,
                 after_token = ?7,
                 after_token_sha256 = ?8,
                 snapshot_id = ?9,
                 next_page_token = ?10,
                 expected_page_index = ?11,
                 expected_turn_sequence = ?12,
                 last_committed_snapshot_id = ?13,
                 last_committed_page_index = ?14,
                 last_request_token_sha256 = ?15,
                 last_page_digest = ?16,
                 committed_prefix_digest = ?17,
                 committed_page_count = committed_page_count + 1,
                 committed_turn_count = committed_turn_count + ?18,
                 status = ?19,
                 retry_count = 0,
                 last_success_at = ?20,
                 last_error = NULL,
                 lease_owner = NULL,
                 lease_expires_at = NULL,
                 updated_at = ?20
             WHERE provider_name = ?1 AND provider_instance_id = ?2 AND settings_id = ?3
               AND session_id = ?4 AND projection = ?5 AND checkpoint_generation = ?21
               AND lease_owner = ?22",
            params![
                page.key.provider_name,
                page.key.provider_instance_id,
                page.key.settings_id,
                page.key.session_id,
                page.key.projection.as_str(),
                i64::try_from(generation)
                    .map_err(|_| "checkpoint generation overflow".to_string())?,
                after_token,
                after_token_sha256,
                snapshot_id,
                next_page_token,
                expected_page,
                expected_sequence,
                page.snapshot_id,
                i64::try_from(page.page_index).map_err(|_| "page index overflow".to_string())?,
                page.request_token_sha256,
                page.page_digest,
                prefix_digest,
                i64::try_from(page.page_turn_count)
                    .map_err(|_| "page turn count overflow".to_string())?,
                status,
                now,
                i64::try_from(stream.checkpoint_generation)
                    .map_err(|_| "checkpoint generation overflow".to_string())?,
                page.lease_owner,
            ],
        )
        .map_err(|error| format!("Failed to advance session turn checkpoint: {error}"))?;
    if changed != 1 {
        return Err("session_turn_page_stale_generation".to_string());
    }
    Ok(generation)
}

fn rolling_prefix_digest(
    conn: &Connection,
    key: &SessionTurnIngestStreamKey,
    page_index: u64,
    page_digest: &str,
) -> Result<String, DbError> {
    let previous = conn
        .query_row(
            "SELECT committed_prefix_digest FROM session_turn_ingest_streams
             WHERE provider_name = ?1 AND provider_instance_id = ?2 AND settings_id = ?3
               AND session_id = ?4 AND projection = ?5",
            params![
                key.provider_name,
                key.provider_instance_id,
                key.settings_id,
                key.session_id,
                key.projection.as_str(),
            ],
            |row| row.get::<_, Option<String>>(0),
        )
        .map_err(|error| format!("Failed to read session turn prefix digest: {error}"))?;
    let mut digest = Sha256::new();
    digest.update(previous.as_deref().unwrap_or("").as_bytes());
    digest.update(page_index.to_be_bytes());
    digest.update(page_digest.as_bytes());
    Ok(format!("{:x}", digest.finalize()))
}

fn turn_ingest_digest(turn: &SessionTurnPageTurnIngest) -> String {
    let mut digest = Sha256::new();
    update_digest_part(&mut digest, turn.session_id.as_bytes());
    update_digest_part(&mut digest, turn.turn_id.as_bytes());
    update_digest_part(&mut digest, turn.timestamp.to_rfc3339().as_bytes());
    update_digest_part(&mut digest, turn.role.as_bytes());
    update_digest_part(
        &mut digest,
        turn.parent_turn_id.as_deref().unwrap_or("").as_bytes(),
    );
    update_digest_part(&mut digest, if turn.is_sidechain { b"1" } else { b"0" });
    update_digest_part(
        &mut digest,
        if turn.is_compaction_boundary {
            b"1"
        } else {
            b"0"
        },
    );
    update_digest_part(&mut digest, turn.body_state.as_str().as_bytes());
    update_digest_part(&mut digest, turn.body.as_deref().unwrap_or("").as_bytes());
    update_digest_part(
        &mut digest,
        turn.body_sha256.as_deref().unwrap_or("").as_bytes(),
    );
    update_digest_part(
        &mut digest,
        turn.canonical_text_sha256
            .as_deref()
            .unwrap_or("")
            .as_bytes(),
    );
    digest.update(turn.body_bytes.unwrap_or(u64::MAX).to_be_bytes());
    digest.update([u8::from(turn.canonical_text_digest_verified)]);
    format!("{:x}", digest.finalize())
}

fn update_digest_part(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

fn sha256_text(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
