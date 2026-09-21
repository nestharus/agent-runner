use super::EVENT_SCHEMA_VERSION;
use super::envelope::{
    Digest32, EnvelopeError, EventEnvelopeV1, EventId, GenerationId, ProcessInstanceId,
    WriterInstanceId,
};
use rusqlite::{Connection, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::fmt;

pub const MAX_APPEND_BATCH_RECORDS: usize = 32;
pub const MAX_APPEND_BATCH_PAYLOAD_BYTES: usize = 1024 * 1024;

const EVENT_SCHEMA_SQL: &str = r#"
CREATE TABLE generation_metadata (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    generation_id BLOB NOT NULL CHECK (length(generation_id) = 16),
    predecessor_generation_id BLOB CHECK (predecessor_generation_id IS NULL OR length(predecessor_generation_id) = 16),
    writer_instance_id BLOB NOT NULL CHECK (length(writer_instance_id) = 16),
    process_instance_id BLOB NOT NULL CHECK (length(process_instance_id) = 16),
    process_root_id BLOB NOT NULL CHECK (length(process_root_id) = 16),
    parent_process_instance_id BLOB CHECK (parent_process_instance_id IS NULL OR length(parent_process_instance_id) = 16),
    supervisor_authority_id BLOB CHECK (supervisor_authority_id IS NULL OR length(supervisor_authority_id) = 16),
    native_os_pid INTEGER,
    native_os_boot_id_sha256 BLOB CHECK (native_os_boot_id_sha256 IS NULL OR length(native_os_boot_id_sha256) = 32),
    native_os_pid_starttime_ticks INTEGER,
    created_at_unix_micros INTEGER NOT NULL CHECK (created_at_unix_micros >= 0),
    head_epoch INTEGER NOT NULL CHECK (head_epoch >= 0),
    state TEXT NOT NULL CHECK (state IN ('prepared', 'writable', 'closed')),
    closed_at_unix_micros INTEGER CHECK (closed_at_unix_micros IS NULL OR closed_at_unix_micros >= 0),
    successor_generation_id BLOB CHECK (successor_generation_id IS NULL OR length(successor_generation_id) = 16),
    predecessor_status TEXT NOT NULL CHECK (predecessor_status IN ('none', 'healthy', 'corrupt')),
    CHECK ((native_os_pid IS NULL) = (native_os_boot_id_sha256 IS NULL)),
    CHECK ((native_os_pid IS NULL) = (native_os_pid_starttime_ticks IS NULL)),
    CHECK (native_os_pid IS NULL OR native_os_pid > 0),
    CHECK (native_os_pid_starttime_ticks IS NULL OR native_os_pid_starttime_ticks > 0),
    CHECK ((state = 'closed') = (closed_at_unix_micros IS NOT NULL)),
    CHECK ((state = 'closed') = (successor_generation_id IS NOT NULL))
) STRICT;

CREATE TABLE events (
    local_sequence INTEGER PRIMARY KEY,
    event_id BLOB UNIQUE NOT NULL CHECK (length(event_id) = 16),
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    family INTEGER NOT NULL CHECK (family BETWEEN 1 AND 5),
    kind TEXT NOT NULL CHECK (
        length(CAST(kind AS BLOB)) BETWEEN 1 AND 64
        AND kind NOT GLOB '*[^a-z0-9._-]*'
    ),
    recorded_at_unix_micros INTEGER NOT NULL CHECK (recorded_at_unix_micros >= 0),
    ingested_at_unix_micros INTEGER NOT NULL CHECK (ingested_at_unix_micros >= 0),
    producer_sequence INTEGER NOT NULL CHECK (producer_sequence >= 0),
    writer_instance_id BLOB NOT NULL CHECK (length(writer_instance_id) = 16),
    process_instance_id BLOB NOT NULL CHECK (length(process_instance_id) = 16),
    process_root_id BLOB NOT NULL CHECK (length(process_root_id) = 16),
    parent_process_instance_id BLOB CHECK (parent_process_instance_id IS NULL OR length(parent_process_instance_id) = 16),
    supervisor_authority_id BLOB CHECK (supervisor_authority_id IS NULL OR length(supervisor_authority_id) = 16),
    trace_id BLOB CHECK (trace_id IS NULL OR length(trace_id) = 16),
    span_id BLOB CHECK (span_id IS NULL OR length(span_id) = 16),
    parent_span_id BLOB CHECK (parent_span_id IS NULL OR length(parent_span_id) = 16),
    invocation_uuid BLOB CHECK (invocation_uuid IS NULL OR length(invocation_uuid) = 16),
    session_correlation_sha256 BLOB CHECK (session_correlation_sha256 IS NULL OR length(session_correlation_sha256) = 32),
    payload_codec INTEGER NOT NULL CHECK (payload_codec = 1),
    payload TEXT NOT NULL CHECK (json_valid(payload)),
    payload_sha256 BLOB NOT NULL CHECK (length(payload_sha256) = 32),
    payload_bytes INTEGER NOT NULL CHECK (
        payload_bytes >= 0 AND payload_bytes <= 1048576
        AND payload_bytes = length(CAST(payload AS BLOB))
    ),
    immutable_sha256 BLOB NOT NULL CHECK (length(immutable_sha256) = 32),
    legacy_provenance_json TEXT CHECK (legacy_provenance_json IS NULL OR json_valid(legacy_provenance_json)),
    retry_of_generation_id BLOB CHECK (retry_of_generation_id IS NULL OR length(retry_of_generation_id) = 16),
    UNIQUE (writer_instance_id, producer_sequence),
    CHECK ((trace_id IS NULL) = (span_id IS NULL)),
    CHECK (parent_span_id IS NULL OR (trace_id IS NOT NULL AND span_id IS NOT NULL))
) STRICT;

CREATE INDEX events_recorded_idx
    ON events(recorded_at_unix_micros, event_id);
CREATE INDEX events_family_kind_recorded_idx
    ON events(family, kind, recorded_at_unix_micros, event_id);
CREATE INDEX events_trace_recorded_idx
    ON events(trace_id, recorded_at_unix_micros, event_id);
CREATE INDEX events_trace_span_idx
    ON events(trace_id, span_id);
CREATE INDEX events_invocation_recorded_idx
    ON events(invocation_uuid, recorded_at_unix_micros, event_id);
CREATE INDEX events_session_recorded_idx
    ON events(session_correlation_sha256, recorded_at_unix_micros, event_id);
CREATE INDEX events_process_tree_recorded_idx
    ON events(process_root_id, process_instance_id, recorded_at_unix_micros, event_id);
CREATE INDEX events_supervisor_recorded_idx
    ON events(supervisor_authority_id, recorded_at_unix_micros, event_id);
"#;

pub(crate) const REQUIRED_INDEXES: &[&str] = &[
    "sqlite_autoindex_events_1",
    "sqlite_autoindex_events_2",
    "events_recorded_idx",
    "events_family_kind_recorded_idx",
    "events_trace_recorded_idx",
    "events_trace_span_idx",
    "events_invocation_recorded_idx",
    "events_session_recorded_idx",
    "events_process_tree_recorded_idx",
    "events_supervisor_recorded_idx",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationState {
    Prepared,
    Writable,
    Closed,
}

impl GenerationState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Writable => "writable",
            Self::Closed => "closed",
        }
    }

    fn parse(value: &str) -> Result<Self, EventStoreError> {
        match value {
            "prepared" => Ok(Self::Prepared),
            "writable" => Ok(Self::Writable),
            "closed" => Ok(Self::Closed),
            _ => Err(EventStoreError::Schema(format!(
                "unknown generation state {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationMetadata {
    pub schema_version: i64,
    pub generation_id: GenerationId,
    pub predecessor_generation_id: Option<GenerationId>,
    pub writer_instance_id: WriterInstanceId,
    pub process_instance_id: ProcessInstanceId,
    pub process_root_id: ProcessInstanceId,
    pub parent_process_instance_id: Option<ProcessInstanceId>,
    pub supervisor_authority_id: Option<super::SupervisorAuthorityId>,
    pub native_process: Option<super::NativeProcessIdentity>,
    pub created_at_unix_micros: i64,
    pub head_epoch: i64,
    pub state: GenerationState,
    pub closed_at_unix_micros: Option<i64>,
    pub successor_generation_id: Option<GenerationId>,
    /// `none`, `healthy`, or `corrupt`. A corrupt predecessor never becomes an
    /// empty/successful coverage claim.
    pub predecessor_status: String,
}

impl GenerationMetadata {
    pub fn prepared(
        generation_id: GenerationId,
        predecessor_generation_id: Option<GenerationId>,
        producer: &super::ProducerIdentity,
        created_at_unix_micros: i64,
        head_epoch: i64,
    ) -> Self {
        Self {
            schema_version: EVENT_SCHEMA_VERSION,
            generation_id,
            predecessor_generation_id,
            writer_instance_id: producer.writer_instance_id,
            process_instance_id: producer.process_instance_id,
            process_root_id: producer.process_root_id,
            parent_process_instance_id: producer.parent_process_instance_id,
            supervisor_authority_id: producer.supervisor_authority_id,
            native_process: producer.native_process.clone(),
            created_at_unix_micros,
            head_epoch,
            state: GenerationState::Prepared,
            closed_at_unix_micros: None,
            successor_generation_id: None,
            predecessor_status: if predecessor_generation_id.is_some() {
                "healthy".to_string()
            } else {
                "none".to_string()
            },
        }
    }

    pub fn validate(&self) -> Result<(), EventStoreError> {
        if self.schema_version != EVENT_SCHEMA_VERSION {
            return Err(EventStoreError::Schema(format!(
                "unsupported generation schema {}",
                self.schema_version
            )));
        }
        if self.created_at_unix_micros < 0 || self.head_epoch < 0 {
            return Err(EventStoreError::InvalidMetadata(
                "negative generation creation time or head epoch",
            ));
        }
        if !matches!(
            self.predecessor_status.as_str(),
            "none" | "healthy" | "corrupt"
        ) {
            return Err(EventStoreError::InvalidMetadata(
                "invalid predecessor status",
            ));
        }
        if (self.state == GenerationState::Closed)
            != (self.closed_at_unix_micros.is_some() && self.successor_generation_id.is_some())
        {
            return Err(EventStoreError::InvalidMetadata(
                "closed generation requires close time and exact successor",
            ));
        }
        if let Some(native) = &self.native_process {
            native.validate().map_err(EventStoreError::Envelope)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendTicket {
    pub writer_instance_id: WriterInstanceId,
    pub generation_id: GenerationId,
    pub event_id: EventId,
    pub immutable_sha256: Digest32,
}

impl AppendTicket {
    pub fn new(
        generation_id: GenerationId,
        event: &EventEnvelopeV1,
    ) -> Result<Self, EventStoreError> {
        Ok(Self {
            writer_instance_id: event.producer.writer_instance_id,
            generation_id,
            event_id: event.event_id,
            immutable_sha256: event
                .immutable_digest()
                .map_err(EventStoreError::Envelope)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppendDisposition {
    Committed { local_sequence: i64 },
    AlreadyCommitted { local_sequence: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconciliationOutcome {
    AlreadyCommitted { local_sequence: i64 },
    AbsentHealthy,
    IdentityConflict { stored_immutable_sha256: Digest32 },
    Unknown { reason: String },
}

pub fn initialize_generation_schema(
    connection: &mut Connection,
    metadata: &GenerationMetadata,
) -> Result<(), EventStoreError> {
    metadata.validate()?;
    let journal_mode: String =
        connection.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(EventStoreError::Configuration(format!(
            "SQLite refused WAL mode: {journal_mode}"
        )));
    }
    connection.pragma_update(None, "synchronous", "FULL")?;
    let synchronous: i64 = connection.query_row("PRAGMA synchronous", [], |row| row.get(0))?;
    if synchronous != 2 {
        return Err(EventStoreError::Configuration(format!(
            "SQLite synchronous mode is {synchronous}, expected FULL(2)"
        )));
    }

    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(EVENT_SCHEMA_SQL)?;
    let native_pid = metadata.native_process.as_ref().map(|value| value.os_pid);
    let native_boot = metadata
        .native_process
        .as_ref()
        .map(|value| value.os_boot_id_sha256.as_bytes().as_slice());
    let native_start = metadata
        .native_process
        .as_ref()
        .map(|value| value.os_pid_starttime_ticks);
    transaction.execute(
        "INSERT INTO generation_metadata (
            singleton, schema_version, generation_id, predecessor_generation_id,
            writer_instance_id, process_instance_id, process_root_id,
            parent_process_instance_id, supervisor_authority_id,
            native_os_pid, native_os_boot_id_sha256, native_os_pid_starttime_ticks,
            created_at_unix_micros, head_epoch, state, closed_at_unix_micros,
            successor_generation_id, predecessor_status
         ) VALUES (
            1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17
         )",
        params![
            metadata.schema_version,
            metadata.generation_id.as_bytes().as_slice(),
            metadata
                .predecessor_generation_id
                .as_ref()
                .map(|value| value.as_bytes().as_slice()),
            metadata.writer_instance_id.as_bytes().as_slice(),
            metadata.process_instance_id.as_bytes().as_slice(),
            metadata.process_root_id.as_bytes().as_slice(),
            metadata
                .parent_process_instance_id
                .as_ref()
                .map(|value| value.as_bytes().as_slice()),
            metadata
                .supervisor_authority_id
                .as_ref()
                .map(|value| value.as_bytes().as_slice()),
            native_pid,
            native_boot,
            native_start,
            metadata.created_at_unix_micros,
            metadata.head_epoch,
            metadata.state.as_str(),
            metadata.closed_at_unix_micros,
            metadata
                .successor_generation_id
                .as_ref()
                .map(|value| value.as_bytes().as_slice()),
            metadata.predecessor_status,
        ],
    )?;
    transaction.pragma_update(None, "user_version", EVENT_SCHEMA_VERSION)?;
    transaction.commit()?;
    verify_generation_schema(connection, Some(metadata))
}

pub fn verify_generation_schema(
    connection: &Connection,
    expected_metadata: Option<&GenerationMetadata>,
) -> Result<(), EventStoreError> {
    let journal_mode: String = connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(EventStoreError::Configuration(format!(
            "generation is not in WAL mode: {journal_mode}"
        )));
    }
    let synchronous: i64 = connection.query_row("PRAGMA synchronous", [], |row| row.get(0))?;
    if synchronous != 2 {
        return Err(EventStoreError::Configuration(format!(
            "connection is not synchronous FULL: {synchronous}"
        )));
    }
    let strict: i64 = connection.query_row(
        "SELECT strict FROM pragma_table_list WHERE name = 'events' AND type = 'table'",
        [],
        |row| row.get(0),
    )?;
    if strict != 1 {
        return Err(EventStoreError::Schema(
            "events table is not STRICT".to_string(),
        ));
    }
    for index in REQUIRED_INDEXES {
        let present: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_index_list('events') WHERE name = ?1)",
            [index],
            |row| row.get(0),
        )?;
        if !present {
            return Err(EventStoreError::Schema(format!(
                "required event index is missing: {index}"
            )));
        }
    }
    let metadata = read_generation_metadata(connection)?;
    metadata.validate()?;
    if expected_metadata.is_some_and(|expected| expected != &metadata) {
        return Err(EventStoreError::Schema(
            "generation metadata does not match prepared manifest values".to_string(),
        ));
    }
    Ok(())
}

pub fn read_generation_metadata(
    connection: &Connection,
) -> Result<GenerationMetadata, EventStoreError> {
    connection
        .query_row(
            "SELECT schema_version, generation_id, predecessor_generation_id,
                    writer_instance_id, process_instance_id, process_root_id,
                    parent_process_instance_id, supervisor_authority_id,
                    native_os_pid, native_os_boot_id_sha256, native_os_pid_starttime_ticks,
                    created_at_unix_micros, head_epoch, state, closed_at_unix_micros,
                    successor_generation_id, predecessor_status
               FROM generation_metadata WHERE singleton = 1",
            [],
            decode_generation_metadata,
        )
        .map_err(EventStoreError::from)
}

pub fn mark_generation_writable(connection: &Connection) -> Result<(), EventStoreError> {
    let changed = connection.execute(
        "UPDATE generation_metadata SET state = 'writable'
         WHERE singleton = 1 AND state = 'prepared'",
        [],
    )?;
    if changed != 1 {
        return Err(EventStoreError::InvalidGenerationState);
    }
    Ok(())
}

pub fn close_generation(
    connection: &mut Connection,
    closed_at_unix_micros: i64,
    successor_generation_id: GenerationId,
) -> Result<(), EventStoreError> {
    if closed_at_unix_micros < 0 {
        return Err(EventStoreError::InvalidMetadata("negative close time"));
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let changed = transaction.execute(
        "UPDATE generation_metadata
            SET state = 'closed', closed_at_unix_micros = ?1, successor_generation_id = ?2
          WHERE singleton = 1 AND state = 'writable'",
        params![
            closed_at_unix_micros,
            successor_generation_id.as_bytes().as_slice()
        ],
    )?;
    if changed != 1 {
        return Err(EventStoreError::InvalidGenerationState);
    }
    transaction.commit()?;
    Ok(())
}

pub fn append_batch(
    connection: &mut Connection,
    generation_id: GenerationId,
    events: &[EventEnvelopeV1],
) -> Result<Vec<AppendDisposition>, EventStoreError> {
    if events.is_empty() || events.len() > MAX_APPEND_BATCH_RECORDS {
        return Err(EventStoreError::BatchBounds);
    }
    let payload_bytes = events.iter().try_fold(0usize, |total, event| {
        total
            .checked_add(event.payload.len())
            .ok_or(EventStoreError::BatchBounds)
    })?;
    if payload_bytes > MAX_APPEND_BATCH_PAYLOAD_BYTES {
        return Err(EventStoreError::BatchBounds);
    }
    for event in events {
        event.validate().map_err(EventStoreError::Envelope)?;
    }

    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let metadata = read_generation_metadata(&transaction)?;
    if metadata.generation_id != generation_id {
        return Err(EventStoreError::GenerationIdentityMismatch);
    }
    if metadata.state != GenerationState::Writable {
        return Err(EventStoreError::InvalidGenerationState);
    }

    let mut dispositions = Vec::with_capacity(events.len());
    for event in events {
        if event.producer.writer_instance_id != metadata.writer_instance_id
            || event.producer.process_instance_id != metadata.process_instance_id
            || event.producer.process_root_id != metadata.process_root_id
            || event.producer.parent_process_instance_id != metadata.parent_process_instance_id
            || event.producer.supervisor_authority_id != metadata.supervisor_authority_id
            || event.producer.native_process != metadata.native_process
        {
            return Err(EventStoreError::ProducerIdentityMismatch);
        }
        let immutable = event
            .immutable_digest()
            .map_err(EventStoreError::Envelope)?;
        let legacy = event
            .legacy_provenance
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| EventStoreError::Envelope(EnvelopeError::Json(error.to_string())))?;
        let inserted = transaction.execute(
            "INSERT INTO events (
                event_id, schema_version, family, kind,
                recorded_at_unix_micros, ingested_at_unix_micros, producer_sequence,
                writer_instance_id, process_instance_id, process_root_id,
                parent_process_instance_id, supervisor_authority_id,
                trace_id, span_id, parent_span_id, invocation_uuid,
                session_correlation_sha256, payload_codec, payload, payload_sha256,
                payload_bytes, immutable_sha256, legacy_provenance_json,
                retry_of_generation_id
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24
             ) ON CONFLICT(event_id) DO NOTHING",
            params![
                event.event_id.as_bytes().as_slice(),
                event.schema_version,
                event.family as i64,
                event.kind.as_str(),
                event.recorded_at_unix_micros,
                event.ingested_at_unix_micros,
                event.producer_sequence,
                event.producer.writer_instance_id.as_bytes().as_slice(),
                event.producer.process_instance_id.as_bytes().as_slice(),
                event.producer.process_root_id.as_bytes().as_slice(),
                event
                    .producer
                    .parent_process_instance_id
                    .as_ref()
                    .map(|value| value.as_bytes().as_slice()),
                event
                    .producer
                    .supervisor_authority_id
                    .as_ref()
                    .map(|value| value.as_bytes().as_slice()),
                event
                    .correlations
                    .trace_id
                    .as_ref()
                    .map(|value| value.as_bytes().as_slice()),
                event
                    .correlations
                    .span_id
                    .as_ref()
                    .map(|value| value.as_bytes().as_slice()),
                event
                    .correlations
                    .parent_span_id
                    .as_ref()
                    .map(|value| value.as_bytes().as_slice()),
                event
                    .correlations
                    .invocation_uuid
                    .as_ref()
                    .map(|value| value.as_bytes().as_slice()),
                event
                    .correlations
                    .session_correlation_sha256
                    .as_ref()
                    .map(|value| value.as_bytes().as_slice()),
                event.payload_codec as i64,
                event.payload,
                event.payload_sha256.as_bytes().as_slice(),
                event.payload_bytes,
                immutable.as_bytes().as_slice(),
                legacy,
                event
                    .retry_of_generation_id
                    .as_ref()
                    .map(|value| value.as_bytes().as_slice()),
            ],
        )?;
        if inserted == 1 {
            dispositions.push(AppendDisposition::Committed {
                local_sequence: transaction.last_insert_rowid(),
            });
            continue;
        }
        let stored: (i64, Vec<u8>) = transaction.query_row(
            "SELECT local_sequence, immutable_sha256 FROM events WHERE event_id = ?1",
            [event.event_id.as_bytes().as_slice()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let stored_digest = Digest32::from_slice(&stored.1).map_err(EventStoreError::Envelope)?;
        if stored_digest != immutable {
            return Err(EventStoreError::IdentityConflict {
                event_id: event.event_id,
                stored_immutable_sha256: stored_digest,
                attempted_immutable_sha256: immutable,
            });
        }
        dispositions.push(AppendDisposition::AlreadyCommitted {
            local_sequence: stored.0,
        });
    }
    transaction.commit()?;
    Ok(dispositions)
}

fn decode_generation_metadata(row: &rusqlite::Row<'_>) -> rusqlite::Result<GenerationMetadata> {
    let generation_id: Vec<u8> = row.get(1)?;
    let predecessor: Option<Vec<u8>> = row.get(2)?;
    let writer: Vec<u8> = row.get(3)?;
    let process: Vec<u8> = row.get(4)?;
    let root: Vec<u8> = row.get(5)?;
    let parent: Option<Vec<u8>> = row.get(6)?;
    let supervisor: Option<Vec<u8>> = row.get(7)?;
    let native_pid: Option<i64> = row.get(8)?;
    let native_boot: Option<Vec<u8>> = row.get(9)?;
    let native_start: Option<i64> = row.get(10)?;
    let state: String = row.get(13)?;
    let successor: Option<Vec<u8>> = row.get(15)?;
    let native_process = match (native_pid, native_boot, native_start) {
        (Some(os_pid), Some(boot), Some(os_pid_starttime_ticks)) => {
            Some(super::NativeProcessIdentity {
                os_pid,
                os_boot_id_sha256: Digest32::from_slice(&boot).map_err(to_sql_error)?,
                os_pid_starttime_ticks,
            })
        }
        (None, None, None) => None,
        _ => {
            return Err(to_sql_error(EnvelopeError::InvalidField(
                "partial native identity",
            )));
        }
    };
    Ok(GenerationMetadata {
        schema_version: row.get(0)?,
        generation_id: GenerationId::from_slice(&generation_id).map_err(to_sql_error)?,
        predecessor_generation_id: predecessor
            .as_deref()
            .map(GenerationId::from_slice)
            .transpose()
            .map_err(to_sql_error)?,
        writer_instance_id: WriterInstanceId::from_slice(&writer).map_err(to_sql_error)?,
        process_instance_id: ProcessInstanceId::from_slice(&process).map_err(to_sql_error)?,
        process_root_id: ProcessInstanceId::from_slice(&root).map_err(to_sql_error)?,
        parent_process_instance_id: parent
            .as_deref()
            .map(ProcessInstanceId::from_slice)
            .transpose()
            .map_err(to_sql_error)?,
        supervisor_authority_id: supervisor
            .as_deref()
            .map(super::SupervisorAuthorityId::from_slice)
            .transpose()
            .map_err(to_sql_error)?,
        native_process,
        created_at_unix_micros: row.get(11)?,
        head_epoch: row.get(12)?,
        state: GenerationState::parse(&state).map_err(to_sql_error)?,
        closed_at_unix_micros: row.get(14)?,
        successor_generation_id: successor
            .as_deref()
            .map(GenerationId::from_slice)
            .transpose()
            .map_err(to_sql_error)?,
        predecessor_status: row.get(16)?,
    })
}

fn to_sql_error(error: impl fmt::Display) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Blob,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            error.to_string(),
        )),
    )
}

#[derive(Debug)]
pub enum EventStoreError {
    Sqlite(rusqlite::Error),
    Envelope(EnvelopeError),
    Configuration(String),
    Schema(String),
    InvalidMetadata(&'static str),
    InvalidGenerationState,
    GenerationIdentityMismatch,
    ProducerIdentityMismatch,
    BatchBounds,
    IdentityConflict {
        event_id: EventId,
        stored_immutable_sha256: Digest32,
        attempted_immutable_sha256: Digest32,
    },
}

impl fmt::Display for EventStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(formatter, "event SQLite error: {error}"),
            Self::Envelope(error) => error.fmt(formatter),
            Self::Configuration(error) => write!(formatter, "event SQLite configuration: {error}"),
            Self::Schema(error) => write!(formatter, "event schema error: {error}"),
            Self::InvalidMetadata(reason) => {
                write!(formatter, "invalid generation metadata: {reason}")
            }
            Self::InvalidGenerationState => {
                write!(formatter, "generation is not in the required state")
            }
            Self::GenerationIdentityMismatch => {
                write!(formatter, "exact generation identity mismatch")
            }
            Self::ProducerIdentityMismatch => {
                write!(formatter, "event producer does not own this generation")
            }
            Self::BatchBounds => write!(formatter, "event append batch exceeds version-1 bounds"),
            Self::IdentityConflict { event_id, .. } => {
                write!(formatter, "event identity conflict for {event_id}")
            }
        }
    }
}

impl std::error::Error for EventStoreError {}

impl From<rusqlite::Error> for EventStoreError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sqlite(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_store::{
        Digest32, EventCorrelations, EventFamily, EventId, EventKind, NativeProcessIdentity,
        NewEventV1, PayloadNormalizationPolicy, ProcessInstanceId, ProducerIdentity,
        SupervisorAuthorityId,
    };
    use serde_json::json;

    fn producer() -> ProducerIdentity {
        ProducerIdentity {
            writer_instance_id: WriterInstanceId::from_bytes([1; 16]),
            process_instance_id: ProcessInstanceId::from_bytes([2; 16]),
            process_root_id: ProcessInstanceId::from_bytes([3; 16]),
            parent_process_instance_id: Some(ProcessInstanceId::from_bytes([4; 16])),
            supervisor_authority_id: Some(SupervisorAuthorityId::from_bytes([5; 16])),
            native_process: Some(NativeProcessIdentity {
                os_pid: 10,
                os_boot_id_sha256: Digest32::from_bytes([6; 32]),
                os_pid_starttime_ticks: 20,
            }),
        }
    }

    fn event(id: u8, sequence: i64, value: i64) -> EventEnvelopeV1 {
        EventEnvelopeV1::normalize(
            NewEventV1 {
                event_id: EventId::from_bytes([id; 16]),
                family: EventFamily::Diagnostic,
                kind: EventKind::registered("schema.test").unwrap(),
                recorded_at_unix_micros: 100 + sequence,
                producer_sequence: sequence,
                producer: producer(),
                correlations: EventCorrelations::default(),
                payload: json!({"value": value}),
                legacy_provenance: None,
                retry_of_generation_id: None,
            },
            &PayloadNormalizationPolicy::registered(&["value"]).unwrap(),
            200 + sequence,
        )
        .unwrap()
    }

    fn database() -> (tempfile::TempDir, Connection, GenerationId) {
        let directory = tempfile::tempdir().unwrap();
        let mut connection = Connection::open(directory.path().join("events.sqlite3")).unwrap();
        let generation_id = GenerationId::from_bytes([7; 16]);
        let metadata = GenerationMetadata::prepared(generation_id, None, &producer(), 1, 0);
        initialize_generation_schema(&mut connection, &metadata).unwrap();
        mark_generation_writable(&connection).unwrap();
        (directory, connection, generation_id)
    }

    #[test]
    fn strict_schema_has_required_indexes_and_reopens_in_wal_full_mode() {
        let (directory, connection, _) = database();
        verify_generation_schema(&connection, None).unwrap();
        drop(connection);
        let reopened = Connection::open(directory.path().join("events.sqlite3")).unwrap();
        reopened.pragma_update(None, "synchronous", "FULL").unwrap();
        verify_generation_schema(&reopened, None).unwrap();
        let malformed = reopened.execute(
            "INSERT INTO events (
                event_id,schema_version,family,kind,recorded_at_unix_micros,
                ingested_at_unix_micros,producer_sequence,writer_instance_id,
                process_instance_id,process_root_id,payload_codec,payload,
                payload_sha256,payload_bytes,immutable_sha256
             ) VALUES (x'01',1,1,'schema.test',1,1,1,zeroblob(16),zeroblob(16),
                       zeroblob(16),1,'{}',zeroblob(32),2,zeroblob(32))",
            [],
        );
        assert!(
            malformed.is_err(),
            "fixed-width checks are enforced by SQLite"
        );
    }

    #[test]
    fn duplicate_retry_is_idempotent_and_identity_conflict_rolls_back_batch() {
        let (_directory, mut connection, generation_id) = database();
        let original = event(8, 0, 1);
        assert!(matches!(
            append_batch(
                &mut connection,
                generation_id,
                std::slice::from_ref(&original)
            )
            .unwrap()
            .as_slice(),
            [AppendDisposition::Committed { local_sequence: 1 }]
        ));
        assert!(matches!(
            append_batch(
                &mut connection,
                generation_id,
                std::slice::from_ref(&original)
            )
            .unwrap()
            .as_slice(),
            [AppendDisposition::AlreadyCommitted { local_sequence: 1 }]
        ));
        let conflict = event(8, 0, 2);
        assert!(matches!(
            append_batch(&mut connection, generation_id, &[conflict]),
            Err(EventStoreError::IdentityConflict { .. })
        ));
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn producer_sequence_is_unique_within_generation_and_batch_caps_are_honest() {
        let (_directory, mut connection, generation_id) = database();
        append_batch(&mut connection, generation_id, &[event(1, 5, 1)]).unwrap();
        assert!(append_batch(&mut connection, generation_id, &[event(2, 5, 2)]).is_err());
        assert!(matches!(
            append_batch(&mut connection, generation_id, &[]),
            Err(EventStoreError::BatchBounds)
        ));
    }
}
