use std::error::Error;
use std::fmt;
use std::io;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::types::Type;
use rusqlite::{Connection, ErrorCode, OptionalExtension, params};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactKey {
    pub workflow_run_id: String,
    pub artifact_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PutRequest {
    pub key: ArtifactKey,
    pub producer_invocation_uuid: Option<Uuid>,
    pub format_hint: Option<String>,
    pub verdict_line: Option<String>,
    pub predecessor_version: Option<u64>,
    pub content: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PutReceipt {
    pub key: ArtifactKey,
    pub version: u64,
    pub producer_invocation_uuid: Option<Uuid>,
    pub sha256: String,
    pub content_len: u64,
    pub format_hint: Option<String>,
    pub verdict_line: Option<String>,
    pub predecessor_version: Option<u64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactMeta {
    pub key: ArtifactKey,
    pub version: u64,
    pub producer_invocation_uuid: Option<Uuid>,
    pub sha256: String,
    pub content_len: u64,
    pub format_hint: Option<String>,
    pub verdict_line: Option<String>,
    pub predecessor_version: Option<u64>,
    pub created_at: DateTime<Utc>,
    pub tombstone: Option<TombstoneMeta>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactRecord {
    pub meta: ArtifactMeta,
    pub content: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListFilter {
    pub workflow_run_id: Option<String>,
    pub artifact_name: Option<String>,
    pub include_tombstoned: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TombstoneMeta {
    pub tombstoned_at: DateTime<Utc>,
    pub actor: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TombstoneStatus {
    Tombstoned,
    AlreadyTombstoned,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TombstoneReceipt {
    pub key: ArtifactKey,
    pub version: u64,
    pub tombstone: TombstoneMeta,
    pub status: TombstoneStatus,
}

#[derive(Debug)]
pub enum StoreError {
    InvalidInput(String),
    NotFound,
    Collision,
    Io(io::Error),
    Database(rusqlite::Error),
    MigrationRequired,
    IncompatibleSchema(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(message) => write!(f, "invalid input: {message}"),
            Self::NotFound => write!(f, "artifact not found"),
            Self::Collision => write!(f, "artifact version collision"),
            Self::Io(err) => write!(f, "io error: {err}"),
            Self::Database(err) => write!(f, "database error: {err}"),
            Self::MigrationRequired => write!(f, "database schema migration required"),
            Self::IncompatibleSchema(version) => {
                write!(f, "incompatible database schema version: {version}")
            }
        }
    }
}

impl Error for StoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Database(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for StoreError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Database(value)
    }
}

pub struct Store {
    conn: Connection,
}

struct PreparedPut {
    sha256: String,
    content_len: u64,
    created_at_text: String,
    created_at: DateTime<Utc>,
    producer: Option<String>,
    predecessor: Option<i64>,
}

struct TombstoneState {
    tombstoned_at: Option<String>,
    actor: Option<String>,
    reason: Option<String>,
}

struct NewTombstone {
    timestamp_text: String,
    meta: TombstoneMeta,
}

impl Store {
    pub fn open(db_path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let db_path = db_path.as_ref();
        require_existing_db(db_path)?;

        open_verified_connection(db_path)
    }

    pub fn init(db_path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let conn = open_store_connection(db_path.as_ref())?;
        configure_connection(&conn)?;
        install_schema(&conn)?;
        initialize_schema_version(&conn)?;
        verify_schema(&conn)?;
        Ok(Self { conn })
    }

    pub fn put(&self, req: PutRequest) -> Result<PutReceipt, StoreError> {
        validate_key(&req.key)?;
        let prepared = prepare_put(&req)?;

        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = self.insert_put(req, prepared);
        finish_transaction(&self.conn, result)
    }

    pub fn get(
        &self,
        key: &ArtifactKey,
        version: Option<u64>,
    ) -> Result<ArtifactRecord, StoreError> {
        validate_key(key)?;
        self.get_validated(key, version)
    }

    pub fn get_meta(
        &self,
        key: &ArtifactKey,
        version: Option<u64>,
    ) -> Result<ArtifactMeta, StoreError> {
        validate_key(key)?;
        self.get_meta_validated(key, version)
    }

    pub fn list(&self, filter: ListFilter) -> Result<Vec<ArtifactMeta>, StoreError> {
        let mut stmt = self.conn.prepare(&meta_projection(
            "WHERE (?1 IS NULL OR workflow_run_id = ?1)
             AND (?2 IS NULL OR artifact_name = ?2)
             AND (?3 = 1 OR tombstoned_at IS NULL)
             ORDER BY workflow_run_id ASC, artifact_name ASC, version ASC",
        ))?;
        let rows = stmt.query_map(
            params![
                filter.workflow_run_id,
                filter.artifact_name,
                filter.include_tombstoned
            ],
            artifact_meta_from_row,
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn tombstone(
        &self,
        key: &ArtifactKey,
        version: u64,
        actor: &str,
        reason: &str,
    ) -> Result<TombstoneReceipt, StoreError> {
        validate_tombstone_request(key, actor, reason)?;

        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = self.tombstone_in_transaction(key, version, actor, reason);
        finish_transaction(&self.conn, result)
    }

    fn insert_put(&self, req: PutRequest, prepared: PreparedPut) -> Result<PutReceipt, StoreError> {
        let next_version = next_artifact_version(&self.conn, &req.key)?;
        insert_artifact_version(&self.conn, &req, &prepared, next_version)?;
        put_receipt(req, prepared, next_version)
    }

    fn get_validated(
        &self,
        key: &ArtifactKey,
        version: Option<u64>,
    ) -> Result<ArtifactRecord, StoreError> {
        let record = match version {
            Some(version) => self.record_by_version(key, version)?,
            None => self.latest_record(key)?,
        };
        record.ok_or(StoreError::NotFound)
    }

    fn record_by_version(
        &self,
        key: &ArtifactKey,
        version: u64,
    ) -> Result<Option<ArtifactRecord>, StoreError> {
        self.conn
            .query_row(
                &record_projection(
                    "WHERE workflow_run_id = ?1
                     AND artifact_name = ?2
                     AND version = ?3
                     AND tombstoned_at IS NULL",
                ),
                params![key.workflow_run_id, key.artifact_name, u64_to_i64(version)?],
                artifact_record_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    fn latest_record(&self, key: &ArtifactKey) -> Result<Option<ArtifactRecord>, StoreError> {
        self.conn
            .query_row(
                &record_projection(
                    "WHERE workflow_run_id = ?1
                     AND artifact_name = ?2
                     AND tombstoned_at IS NULL
                     ORDER BY version DESC
                     LIMIT 1",
                ),
                params![key.workflow_run_id, key.artifact_name],
                artifact_record_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    fn get_meta_validated(
        &self,
        key: &ArtifactKey,
        version: Option<u64>,
    ) -> Result<ArtifactMeta, StoreError> {
        let meta = match version {
            Some(version) => self.meta_by_version(key, version)?,
            None => self.latest_meta(key)?,
        };
        meta.ok_or(StoreError::NotFound)
    }

    fn meta_by_version(
        &self,
        key: &ArtifactKey,
        version: u64,
    ) -> Result<Option<ArtifactMeta>, StoreError> {
        self.conn
            .query_row(
                &meta_projection(
                    "WHERE workflow_run_id = ?1
                     AND artifact_name = ?2
                     AND version = ?3",
                ),
                params![key.workflow_run_id, key.artifact_name, u64_to_i64(version)?],
                artifact_meta_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    fn latest_meta(&self, key: &ArtifactKey) -> Result<Option<ArtifactMeta>, StoreError> {
        self.conn
            .query_row(
                &meta_projection(
                    "WHERE workflow_run_id = ?1
                     AND artifact_name = ?2
                     AND tombstoned_at IS NULL
                     ORDER BY version DESC
                     LIMIT 1",
                ),
                params![key.workflow_run_id, key.artifact_name],
                artifact_meta_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    fn tombstone_in_transaction(
        &self,
        key: &ArtifactKey,
        version: u64,
        actor: &str,
        reason: &str,
    ) -> Result<TombstoneReceipt, StoreError> {
        let state = self.tombstone_state(key, version)?;
        let TombstoneState {
            tombstoned_at,
            actor: tombstone_actor,
            reason: tombstone_reason,
        } = state;
        match tombstoned_at {
            Some(tombstoned_at) => already_tombstoned_receipt(
                key,
                version,
                tombstoned_at,
                tombstone_actor,
                tombstone_reason,
            ),
            None => self.tombstone_live_version(key, version, actor, reason),
        }
    }

    fn tombstone_state(
        &self,
        key: &ArtifactKey,
        version: u64,
    ) -> Result<TombstoneState, StoreError> {
        let state = self
            .conn
            .query_row(
                "SELECT tombstoned_at, tombstone_actor, tombstone_reason
                 FROM artifact_versions
                 WHERE workflow_run_id = ?1 AND artifact_name = ?2 AND version = ?3",
                params![key.workflow_run_id, key.artifact_name, u64_to_i64(version)?],
                tombstone_state_from_row,
            )
            .optional()?;

        state.ok_or(StoreError::NotFound)
    }

    fn tombstone_live_version(
        &self,
        key: &ArtifactKey,
        version: u64,
        actor: &str,
        reason: &str,
    ) -> Result<TombstoneReceipt, StoreError> {
        let tombstone = new_tombstone(actor, reason)?;
        update_tombstone(&self.conn, key, version, &tombstone)?;
        Ok(tombstoned_receipt(key, version, tombstone.meta))
    }
}

fn require_existing_db(db_path: &Path) -> Result<(), StoreError> {
    if db_path.exists() {
        return Ok(());
    }
    Err(StoreError::MigrationRequired)
}

fn open_verified_connection(db_path: &Path) -> Result<Store, StoreError> {
    let conn = Connection::open(db_path)?;
    configure_connection(&conn)?;
    verify_schema(&conn)?;
    Ok(Store { conn })
}

fn open_store_connection(db_path: &Path) -> Result<Connection, StoreError> {
    Connection::open(db_path).map_err(StoreError::from)
}

fn install_schema(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS artifact_versions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                workflow_run_id TEXT NOT NULL CHECK(length(workflow_run_id) > 0),
                artifact_name TEXT NOT NULL CHECK(length(artifact_name) > 0),
                version INTEGER NOT NULL CHECK(version > 0),
                producer_invocation_uuid TEXT NULL,
                sha256 TEXT NOT NULL CHECK(length(sha256) = 64),
                content_len INTEGER NOT NULL CHECK(content_len >= 0),
                format_hint TEXT NULL,
                verdict_line TEXT NULL,
                predecessor_version INTEGER NULL CHECK(predecessor_version IS NULL OR predecessor_version > 0),
                created_at TEXT NOT NULL,
                tombstoned_at TEXT NULL,
                tombstone_actor TEXT NULL,
                tombstone_reason TEXT NULL,
                content BLOB NOT NULL,
                UNIQUE (workflow_run_id, artifact_name, version)
            );

            CREATE INDEX IF NOT EXISTS idx_artifact_versions_latest
                ON artifact_versions (workflow_run_id, artifact_name, tombstoned_at, version);

            CREATE INDEX IF NOT EXISTS idx_artifact_versions_workflow
                ON artifact_versions (workflow_run_id, artifact_name, version);

            CREATE TABLE IF NOT EXISTS schema_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
    )?;
    Ok(())
}

fn initialize_schema_version(conn: &Connection) -> Result<(), StoreError> {
    conn.execute(
        "INSERT OR IGNORE INTO schema_meta(key, value) VALUES ('schema_version', '1')",
        [],
    )?;
    Ok(())
}

fn prepare_put(req: &PutRequest) -> Result<PreparedPut, StoreError> {
    let created_at_text = format_timestamp(Utc::now());
    Ok(PreparedPut {
        sha256: sha256_hex(&req.content),
        content_len: content_len(&req.content)?,
        created_at: parse_timestamp_store(created_at_text.clone(), 9)?,
        created_at_text,
        producer: req.producer_invocation_uuid.map(|uuid| uuid.to_string()),
        predecessor: optional_u64_to_i64(req.predecessor_version)?,
    })
}

fn sha256_hex(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    format!("{:x}", hasher.finalize())
}

fn content_len(content: &[u8]) -> Result<u64, StoreError> {
    u64::try_from(content.len())
        .map_err(|_| StoreError::InvalidInput("content length overflows u64".to_string()))
}

fn next_artifact_version(conn: &Connection, key: &ArtifactKey) -> Result<i64, StoreError> {
    let max_version: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM artifact_versions
         WHERE workflow_run_id = ?1 AND artifact_name = ?2",
        params![key.workflow_run_id, key.artifact_name],
        |row| row.get(0),
    )?;
    increment_version(max_version)
}

fn increment_version(max_version: i64) -> Result<i64, StoreError> {
    max_version
        .checked_add(1)
        .ok_or_else(|| StoreError::InvalidInput("version limit reached".to_string()))
}

fn insert_artifact_version(
    conn: &Connection,
    req: &PutRequest,
    prepared: &PreparedPut,
    next_version: i64,
) -> Result<(), StoreError> {
    match conn.execute(
        "INSERT INTO artifact_versions (
            workflow_run_id, artifact_name, version, producer_invocation_uuid,
            sha256, content_len, format_hint, verdict_line, predecessor_version,
            created_at, content
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            req.key.workflow_run_id,
            req.key.artifact_name,
            next_version,
            prepared.producer,
            prepared.sha256,
            u64_to_i64(prepared.content_len)?,
            req.format_hint,
            req.verdict_line,
            prepared.predecessor,
            prepared.created_at_text,
            req.content.as_slice(),
        ],
    ) {
        Ok(_) => Ok(()),
        Err(err) if is_constraint_violation(&err) => Err(StoreError::Collision),
        Err(err) => Err(StoreError::Database(err)),
    }
}

fn put_receipt(
    req: PutRequest,
    prepared: PreparedPut,
    next_version: i64,
) -> Result<PutReceipt, StoreError> {
    Ok(PutReceipt {
        key: req.key,
        version: i64_to_u64(next_version, 2)?,
        producer_invocation_uuid: req.producer_invocation_uuid,
        sha256: prepared.sha256,
        content_len: prepared.content_len,
        format_hint: req.format_hint,
        verdict_line: req.verdict_line,
        predecessor_version: req.predecessor_version,
        created_at: prepared.created_at,
    })
}

fn finish_transaction<T>(
    conn: &Connection,
    result: Result<T, StoreError>,
) -> Result<T, StoreError> {
    match result {
        Ok(value) => {
            conn.execute_batch("COMMIT")?;
            Ok(value)
        }
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(err)
        }
    }
}

fn validate_tombstone_request(
    key: &ArtifactKey,
    actor: &str,
    reason: &str,
) -> Result<(), StoreError> {
    validate_key(key)?;
    validate_non_empty("actor", actor)?;
    validate_non_empty("reason", reason)
}

fn validate_non_empty(field: &str, value: &str) -> Result<(), StoreError> {
    if value.is_empty() {
        return Err(StoreError::InvalidInput(format!(
            "{field} must not be empty"
        )));
    }
    Ok(())
}

fn tombstone_state_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TombstoneState> {
    Ok(TombstoneState {
        tombstoned_at: row.get(0)?,
        actor: row.get(1)?,
        reason: row.get(2)?,
    })
}

fn already_tombstoned_receipt(
    key: &ArtifactKey,
    version: u64,
    tombstoned_at: String,
    actor: Option<String>,
    reason: Option<String>,
) -> Result<TombstoneReceipt, StoreError> {
    Ok(TombstoneReceipt {
        key: key.clone(),
        version,
        tombstone: tombstone_from_parts(
            tombstoned_at,
            actor.unwrap_or_default(),
            reason.unwrap_or_default(),
            0,
        )?,
        status: TombstoneStatus::AlreadyTombstoned,
    })
}

fn new_tombstone(actor: &str, reason: &str) -> Result<NewTombstone, StoreError> {
    let timestamp_text = format_timestamp(Utc::now());
    let tombstoned_at = parse_timestamp_store(timestamp_text.clone(), 10)?;
    Ok(NewTombstone {
        timestamp_text,
        meta: TombstoneMeta {
            tombstoned_at,
            actor: actor.to_string(),
            reason: reason.to_string(),
        },
    })
}

fn update_tombstone(
    conn: &Connection,
    key: &ArtifactKey,
    version: u64,
    tombstone: &NewTombstone,
) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE artifact_versions
         SET tombstoned_at = ?1, tombstone_actor = ?2, tombstone_reason = ?3
         WHERE workflow_run_id = ?4 AND artifact_name = ?5 AND version = ?6",
        params![
            tombstone.timestamp_text,
            tombstone.meta.actor,
            tombstone.meta.reason,
            key.workflow_run_id,
            key.artifact_name,
            u64_to_i64(version)?,
        ],
    )?;
    Ok(())
}

fn tombstoned_receipt(
    key: &ArtifactKey,
    version: u64,
    tombstone: TombstoneMeta,
) -> TombstoneReceipt {
    TombstoneReceipt {
        key: key.clone(),
        version,
        tombstone,
        status: TombstoneStatus::Tombstoned,
    }
}

fn configure_connection(conn: &Connection) -> Result<(), StoreError> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.busy_timeout(Duration::from_millis(5000))?;
    Ok(())
}

fn verify_schema(conn: &Connection) -> Result<(), StoreError> {
    validate_schema_version(read_schema_version(conn)?)
}

fn read_schema_version(conn: &Connection) -> Result<Option<String>, StoreError> {
    conn.query_row(
        "SELECT value FROM schema_meta WHERE key = 'schema_version'",
        [],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(StoreError::from)
}

fn validate_schema_version(version: Option<String>) -> Result<(), StoreError> {
    match version {
        Some(version) if version == "1" => Ok(()),
        Some(version) => Err(StoreError::IncompatibleSchema(version)),
        None => Err(StoreError::MigrationRequired),
    }
}

fn validate_key(key: &ArtifactKey) -> Result<(), StoreError> {
    if key.workflow_run_id.is_empty() {
        return Err(StoreError::InvalidInput(
            "workflow_run_id must not be empty".to_string(),
        ));
    }
    if key.artifact_name.is_empty() {
        return Err(StoreError::InvalidInput(
            "artifact_name must not be empty".to_string(),
        ));
    }
    Ok(())
}

fn meta_projection(where_clause: &str) -> String {
    format!(
        "SELECT workflow_run_id, artifact_name, version, producer_invocation_uuid,
            sha256, content_len, format_hint, verdict_line, predecessor_version,
            created_at, tombstoned_at, tombstone_actor, tombstone_reason
         FROM artifact_versions {where_clause}"
    )
}

fn record_projection(where_clause: &str) -> String {
    format!(
        "SELECT workflow_run_id, artifact_name, version, producer_invocation_uuid,
            sha256, content_len, format_hint, verdict_line, predecessor_version,
            created_at, tombstoned_at, tombstone_actor, tombstone_reason, content
         FROM artifact_versions {where_clause}"
    )
}

fn artifact_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArtifactRecord> {
    let meta = artifact_meta_from_row(row)?;
    let content = row.get(13)?;
    Ok(ArtifactRecord { meta, content })
}

fn artifact_meta_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArtifactMeta> {
    let workflow_run_id = row.get(0)?;
    let artifact_name = row.get(1)?;
    let version = i64_to_u64_sql(row.get(2)?, 2)?;
    let producer_invocation_uuid = optional_uuid(row.get(3)?, 3)?;
    let sha256 = row.get(4)?;
    let content_len = i64_to_u64_sql(row.get(5)?, 5)?;
    let format_hint = row.get(6)?;
    let verdict_line = row.get(7)?;
    let predecessor_version = optional_i64_to_u64_sql(row.get(8)?, 8)?;
    let created_at = parse_timestamp(row.get(9)?, 9)?;
    let tombstoned_at = row.get(10)?;
    let tombstone_actor = row.get(11)?;
    let tombstone_reason = row.get(12)?;
    let tombstone = match tombstoned_at {
        Some(value) => Some(tombstone_from_parts_sql(
            value,
            tombstone_actor,
            tombstone_reason,
            10,
        )?),
        None => None,
    };

    Ok(ArtifactMeta {
        key: ArtifactKey {
            workflow_run_id,
            artifact_name,
        },
        version,
        producer_invocation_uuid,
        sha256,
        content_len,
        format_hint,
        verdict_line,
        predecessor_version,
        created_at,
        tombstone,
    })
}

fn tombstone_from_parts(
    tombstoned_at: String,
    actor: String,
    reason: String,
    column: usize,
) -> Result<TombstoneMeta, StoreError> {
    Ok(TombstoneMeta {
        tombstoned_at: parse_timestamp_store(tombstoned_at, column)?,
        actor,
        reason,
    })
}

fn tombstone_from_parts_sql(
    tombstoned_at: String,
    actor: Option<String>,
    reason: Option<String>,
    column: usize,
) -> rusqlite::Result<TombstoneMeta> {
    Ok(TombstoneMeta {
        tombstoned_at: parse_timestamp(tombstoned_at, column)?,
        actor: actor.unwrap_or_default(),
        reason: reason.unwrap_or_default(),
    })
}

fn format_timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn parse_timestamp(value: String, column: usize) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|err| rusqlite::Error::FromSqlConversionFailure(column, Type::Text, Box::new(err)))
}

fn parse_timestamp_store(value: String, column: usize) -> Result<DateTime<Utc>, StoreError> {
    parse_timestamp(value, column).map_err(StoreError::Database)
}

fn optional_uuid(value: Option<String>, column: usize) -> rusqlite::Result<Option<Uuid>> {
    value
        .map(|value| {
            Uuid::parse_str(&value).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(column, Type::Text, Box::new(err))
            })
        })
        .transpose()
}

fn i64_to_u64(value: i64, column: usize) -> Result<u64, StoreError> {
    i64_to_u64_sql(value, column).map_err(StoreError::Database)
}

fn i64_to_u64_sql(value: i64, column: usize) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(column, Type::Integer, Box::new(err))
    })
}

fn optional_i64_to_u64_sql(value: Option<i64>, column: usize) -> rusqlite::Result<Option<u64>> {
    value.map(|value| i64_to_u64_sql(value, column)).transpose()
}

fn u64_to_i64(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value)
        .map_err(|_| StoreError::InvalidInput("integer value overflows SQLite INTEGER".to_string()))
}

fn optional_u64_to_i64(value: Option<u64>) -> Result<Option<i64>, StoreError> {
    value.map(u64_to_i64).transpose()
}

fn is_constraint_violation(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.code == ErrorCode::ConstraintViolation
    )
}
