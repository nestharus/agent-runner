//! Prototype-only durable storage contract for a session-scoped supervisor.
//!
//! This module deliberately has no production call sites and does not participate in the
//! `state.db` migration chain. It exists to make AGE-274's storage boundaries executable before
//! choosing the production process-loop and schema integration points.

use std::error::Error;
use std::fmt;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS supervisor_lease (
    session_id TEXT PRIMARY KEY,
    supervisor_generation INTEGER NOT NULL CHECK (supervisor_generation > 0),
    lease_token TEXT NOT NULL CHECK (lease_token <> ''),
    supervisor_pid INTEGER NOT NULL CHECK (supervisor_pid > 0),
    boot_id TEXT NOT NULL CHECK (boot_id <> ''),
    start_time_ticks INTEGER NOT NULL CHECK (start_time_ticks > 0),
    acquired_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS provider_turn_generation (
    generation_id TEXT PRIMARY KEY,
    spawn_invocation_id TEXT NOT NULL UNIQUE,
    session_id TEXT,
    lifecycle_state TEXT NOT NULL CHECK (
        lifecycle_state IN ('starting', 'running', 'draining', 'exited')
    ),
    child_pid INTEGER NOT NULL CHECK (child_pid > 0),
    child_boot_id TEXT NOT NULL CHECK (child_boot_id <> ''),
    child_start_time_ticks INTEGER NOT NULL CHECK (child_start_time_ticks > 0)
);

CREATE UNIQUE INDEX IF NOT EXISTS one_nonterminal_turn_per_session
ON provider_turn_generation(session_id)
WHERE session_id IS NOT NULL AND lifecycle_state <> 'exited';

CREATE TABLE IF NOT EXISTS lifecycle_sequence (
    session_id TEXT PRIMARY KEY,
    last_sequence INTEGER NOT NULL CHECK (last_sequence >= 0)
);

CREATE TABLE IF NOT EXISTS lifecycle_event (
    event_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    event_type TEXT NOT NULL CHECK (event_type <> ''),
    cause_event_id TEXT,
    correlation_id TEXT NOT NULL CHECK (correlation_id <> ''),
    payload TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(session_id, sequence),
    FOREIGN KEY(cause_event_id) REFERENCES lifecycle_event(event_id)
);

CREATE TABLE IF NOT EXISTS lifecycle_event_disposition (
    event_id TEXT NOT NULL,
    consumer_id TEXT NOT NULL,
    disposition TEXT NOT NULL CHECK (disposition IN ('applied', 'ignored')),
    disposed_at INTEGER NOT NULL,
    PRIMARY KEY(event_id, consumer_id),
    FOREIGN KEY(event_id) REFERENCES lifecycle_event(event_id)
);

CREATE TABLE IF NOT EXISTS external_ingress (
    session_id TEXT NOT NULL,
    ingress_sequence INTEGER NOT NULL CHECK (ingress_sequence > 0),
    ingress_id TEXT NOT NULL UNIQUE,
    payload TEXT NOT NULL,
    PRIMARY KEY(session_id, ingress_sequence)
);

CREATE TABLE IF NOT EXISTS external_ingress_cursor (
    session_id TEXT PRIMARY KEY,
    last_sequence INTEGER NOT NULL CHECK (last_sequence >= 0)
);

CREATE TABLE IF NOT EXISTS delivery_acknowledgement (
    delivery_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    turn_generation_id TEXT NOT NULL,
    accepted_at INTEGER NOT NULL,
    submitted_at INTEGER,
    submitted_evidence TEXT,
    confirmed_at INTEGER,
    confirmed_evidence TEXT,
    CHECK ((submitted_at IS NULL) = (submitted_evidence IS NULL)),
    CHECK ((confirmed_at IS NULL) = (confirmed_evidence IS NULL)),
    CHECK (confirmed_at IS NULL OR submitted_at IS NOT NULL)
);

CREATE INDEX IF NOT EXISTS lifecycle_event_by_session
ON lifecycle_event(session_id, sequence);

CREATE INDEX IF NOT EXISTS delivery_acknowledgement_by_session
ON delivery_acknowledgement(session_id, accepted_at, delivery_id);
"#;

#[derive(Debug)]
pub enum PrototypeError {
    Sql(rusqlite::Error),
    LeaseHeld,
    FenceMismatch,
    TurnAlreadyActive,
    Missing(&'static str),
    InvalidTransition,
    Conflict(&'static str),
}

impl fmt::Display for PrototypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sql(error) => write!(f, "SQLite error: {error}"),
            Self::LeaseHeld => write!(f, "the session already has another supervisor lease"),
            Self::FenceMismatch => write!(
                f,
                "the exact session/generation/process fence did not match"
            ),
            Self::TurnAlreadyActive => {
                write!(f, "the session already has a nonterminal provider turn")
            }
            Self::Missing(kind) => write!(f, "missing {kind}"),
            Self::InvalidTransition => write!(f, "invalid lifecycle transition"),
            Self::Conflict(kind) => write!(f, "conflicting replay for {kind}"),
        }
    }
}

impl Error for PrototypeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Sql(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for PrototypeError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sql(value)
    }
}

pub type PrototypeResult<T> = Result<T, PrototypeError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExactProcessIdentity {
    pub pid: i64,
    pub boot_id: String,
    pub start_time_ticks: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SupervisorFence {
    pub generation: i64,
    pub token: String,
    pub process: ExactProcessIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SupervisorLease {
    pub session_id: String,
    pub fence: SupervisorFence,
    pub acquired_at: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeaseAcquire {
    Acquired,
    AlreadyOwned,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TurnState {
    Starting,
    Running,
    Draining,
    Exited,
}

impl TurnState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Draining => "draining",
            Self::Exited => "exited",
        }
    }

    fn parse(value: &str) -> rusqlite::Result<Self> {
        match value {
            "starting" => Ok(Self::Starting),
            "running" => Ok(Self::Running),
            "draining" => Ok(Self::Draining),
            "exited" => Ok(Self::Exited),
            _ => Err(rusqlite::Error::InvalidQuery),
        }
    }

    fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Starting, Self::Running | Self::Exited)
                | (Self::Running, Self::Draining | Self::Exited)
                | (Self::Draining, Self::Exited)
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnFence {
    pub session_id: String,
    pub generation_id: String,
    pub spawn_invocation_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderTurnGeneration {
    pub generation_id: String,
    pub spawn_invocation_id: String,
    pub session_id: Option<String>,
    pub state: TurnState,
    pub child: ExactProcessIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewLifecycleEvent {
    pub event_id: String,
    pub event_type: String,
    pub cause_event_id: Option<String>,
    pub correlation_id: String,
    pub payload: String,
    pub created_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LifecycleEvent {
    pub event_id: String,
    pub session_id: String,
    pub sequence: i64,
    pub event_type: String,
    pub cause_event_id: Option<String>,
    pub correlation_id: String,
    pub payload: String,
    pub created_at: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventDisposition {
    Applied,
    Ignored,
}

impl EventDisposition {
    fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Ignored => "ignored",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DispositionWrite {
    Recorded,
    AlreadyRecorded,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalIngress {
    pub session_id: String,
    pub sequence: i64,
    pub ingress_id: String,
    pub payload: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcknowledgementStage {
    AcceptedPending,
    Submitted,
    Confirmed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeliveryAcknowledgement {
    pub delivery_id: String,
    pub session_id: String,
    pub turn_generation_id: String,
    pub accepted_at: i64,
    pub submitted_at: Option<i64>,
    pub submitted_evidence: Option<String>,
    pub confirmed_at: Option<i64>,
    pub confirmed_evidence: Option<String>,
}

impl DeliveryAcknowledgement {
    pub fn stage(&self) -> AcknowledgementStage {
        if self.confirmed_at.is_some() {
            AcknowledgementStage::Confirmed
        } else if self.submitted_at.is_some() {
            AcknowledgementStage::Submitted
        } else {
            AcknowledgementStage::AcceptedPending
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcknowledgementWrite {
    Advanced,
    AlreadyRecorded,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionReconstruction {
    pub session_id: String,
    pub lease: Option<SupervisorLease>,
    pub active_child: Option<ProviderTurnGeneration>,
    pub ingress_cursor: i64,
    pub acknowledgements: Vec<DeliveryAcknowledgement>,
    pub unconsumed_events: Vec<LifecycleEvent>,
}

pub struct DurableLifecyclePrototype {
    conn: Connection,
}

impl DurableLifecyclePrototype {
    pub fn open(path: impl AsRef<Path>) -> PrototypeResult<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA synchronous = FULL; PRAGMA foreign_keys = ON;")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    pub fn acquire_supervisor_lease(
        &mut self,
        session_id: &str,
        fence: &SupervisorFence,
        acquired_at: i64,
    ) -> PrototypeResult<LeaseAcquire> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(current) = read_lease(&tx, session_id)? {
            if current.fence == *fence {
                return Ok(LeaseAcquire::AlreadyOwned);
            }
            return Err(PrototypeError::LeaseHeld);
        }
        tx.execute(
            "INSERT INTO supervisor_lease (
                session_id, supervisor_generation, lease_token, supervisor_pid,
                boot_id, start_time_ticks, acquired_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                session_id,
                fence.generation,
                fence.token,
                fence.process.pid,
                fence.process.boot_id,
                fence.process.start_time_ticks,
                acquired_at,
            ],
        )?;
        tx.commit()?;
        Ok(LeaseAcquire::Acquired)
    }

    pub fn replace_supervisor_lease(
        &mut self,
        session_id: &str,
        expected: &SupervisorFence,
        replacement: &SupervisorFence,
        acquired_at: i64,
    ) -> PrototypeResult<()> {
        if replacement.generation <= expected.generation {
            return Err(PrototypeError::InvalidTransition);
        }
        let changed = self.conn.execute(
            "UPDATE supervisor_lease
             SET supervisor_generation = ?, lease_token = ?, supervisor_pid = ?,
                 boot_id = ?, start_time_ticks = ?, acquired_at = ?
             WHERE session_id = ? AND supervisor_generation = ? AND lease_token = ?
               AND supervisor_pid = ? AND boot_id = ? AND start_time_ticks = ?",
            params![
                replacement.generation,
                replacement.token,
                replacement.process.pid,
                replacement.process.boot_id,
                replacement.process.start_time_ticks,
                acquired_at,
                session_id,
                expected.generation,
                expected.token,
                expected.process.pid,
                expected.process.boot_id,
                expected.process.start_time_ticks,
            ],
        )?;
        if changed == 1 {
            Ok(())
        } else {
            Err(PrototypeError::FenceMismatch)
        }
    }

    pub fn release_supervisor_lease(
        &mut self,
        session_id: &str,
        fence: &SupervisorFence,
    ) -> PrototypeResult<()> {
        let changed = self.conn.execute(
            "DELETE FROM supervisor_lease
             WHERE session_id = ? AND supervisor_generation = ? AND lease_token = ?
               AND supervisor_pid = ? AND boot_id = ? AND start_time_ticks = ?",
            params![
                session_id,
                fence.generation,
                fence.token,
                fence.process.pid,
                fence.process.boot_id,
                fence.process.start_time_ticks,
            ],
        )?;
        if changed == 1 {
            Ok(())
        } else {
            Err(PrototypeError::FenceMismatch)
        }
    }

    pub fn supervisor_lease(&self, session_id: &str) -> PrototypeResult<Option<SupervisorLease>> {
        read_lease(&self.conn, session_id).map_err(Into::into)
    }

    pub fn start_provider_turn(
        &mut self,
        generation_id: &str,
        spawn_invocation_id: &str,
        session_id: Option<&str>,
        state: TurnState,
        child: &ExactProcessIdentity,
    ) -> PrototypeResult<()> {
        let result = self.conn.execute(
            "INSERT INTO provider_turn_generation (
                generation_id, spawn_invocation_id, session_id, lifecycle_state,
                child_pid, child_boot_id, child_start_time_ticks
             ) VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                generation_id,
                spawn_invocation_id,
                session_id,
                state.as_str(),
                child.pid,
                child.boot_id,
                child.start_time_ticks,
            ],
        );
        map_turn_uniqueness(result).map(|_| ())
    }

    pub fn attach_provider_turn_session(
        &mut self,
        generation_id: &str,
        spawn_invocation_id: &str,
        session_id: &str,
    ) -> PrototypeResult<()> {
        let current = read_turn_by_generation(&self.conn, generation_id)?
            .ok_or(PrototypeError::Missing("provider turn generation"))?;
        if current.spawn_invocation_id != spawn_invocation_id {
            return Err(PrototypeError::FenceMismatch);
        }
        if let Some(attached) = current.session_id {
            return if attached == session_id {
                Ok(())
            } else {
                Err(PrototypeError::FenceMismatch)
            };
        }
        let result = self.conn.execute(
            "UPDATE provider_turn_generation SET session_id = ?
             WHERE generation_id = ? AND spawn_invocation_id = ? AND session_id IS NULL",
            params![session_id, generation_id, spawn_invocation_id],
        );
        map_turn_uniqueness(result).and_then(|changed| {
            if changed == 1 {
                Ok(())
            } else {
                Err(PrototypeError::FenceMismatch)
            }
        })
    }

    pub fn provider_turn(
        &self,
        generation_id: &str,
    ) -> PrototypeResult<Option<ProviderTurnGeneration>> {
        read_turn_by_generation(&self.conn, generation_id).map_err(Into::into)
    }

    pub fn append_lifecycle_event(
        &mut self,
        session_id: &str,
        event: &NewLifecycleEvent,
    ) -> PrototypeResult<LifecycleEvent> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row = append_event(&tx, session_id, event)?;
        tx.commit()?;
        Ok(row)
    }

    pub fn transition_turn_and_append_event(
        &mut self,
        fence: &TurnFence,
        from: TurnState,
        to: TurnState,
        event: &NewLifecycleEvent,
    ) -> PrototypeResult<LifecycleEvent> {
        if !from.can_transition_to(to) {
            return Err(PrototypeError::InvalidTransition);
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE provider_turn_generation SET lifecycle_state = ?
             WHERE session_id = ? AND generation_id = ? AND spawn_invocation_id = ?
               AND lifecycle_state = ?",
            params![
                to.as_str(),
                fence.session_id,
                fence.generation_id,
                fence.spawn_invocation_id,
                from.as_str(),
            ],
        )?;
        if changed != 1 {
            return Err(PrototypeError::FenceMismatch);
        }
        let row = append_event(&tx, &fence.session_id, event)?;
        tx.commit()?;
        Ok(row)
    }

    pub fn record_event_disposition(
        &mut self,
        event_id: &str,
        consumer_id: &str,
        disposition: EventDisposition,
        disposed_at: i64,
    ) -> PrototypeResult<DispositionWrite> {
        let existing = self
            .conn
            .query_row(
                "SELECT disposition FROM lifecycle_event_disposition
                 WHERE event_id = ? AND consumer_id = ?",
                params![event_id, consumer_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if let Some(existing) = existing {
            return if existing == disposition.as_str() {
                Ok(DispositionWrite::AlreadyRecorded)
            } else {
                Err(PrototypeError::Conflict("event disposition"))
            };
        }
        self.conn.execute(
            "INSERT INTO lifecycle_event_disposition (
                event_id, consumer_id, disposition, disposed_at
             ) VALUES (?, ?, ?, ?)",
            params![event_id, consumer_id, disposition.as_str(), disposed_at],
        )?;
        Ok(DispositionWrite::Recorded)
    }

    pub fn append_external_ingress(&mut self, ingress: &ExternalIngress) -> PrototypeResult<()> {
        let inserted = self.conn.execute(
            "INSERT OR IGNORE INTO external_ingress (
                session_id, ingress_sequence, ingress_id, payload
             ) VALUES (?, ?, ?, ?)",
            params![
                ingress.session_id,
                ingress.sequence,
                ingress.ingress_id,
                ingress.payload,
            ],
        )?;
        if inserted == 1 {
            return Ok(());
        }
        let existing = self
            .conn
            .query_row(
                "SELECT session_id, ingress_sequence, ingress_id, payload
                 FROM external_ingress
                 WHERE ingress_id = ? OR (session_id = ? AND ingress_sequence = ?)",
                params![ingress.ingress_id, ingress.session_id, ingress.sequence],
                map_ingress,
            )
            .optional()?;
        if existing.as_ref() == Some(ingress) {
            Ok(())
        } else {
            Err(PrototypeError::Conflict("external ingress"))
        }
    }

    pub fn advance_external_ingress_cursor(
        &mut self,
        session_id: &str,
        sequence: i64,
    ) -> PrototypeResult<()> {
        self.conn.execute(
            "INSERT INTO external_ingress_cursor (session_id, last_sequence)
             VALUES (?, ?)
             ON CONFLICT(session_id) DO UPDATE SET
                last_sequence = max(last_sequence, excluded.last_sequence)",
            params![session_id, sequence],
        )?;
        Ok(())
    }

    pub fn read_external_ingress(
        &mut self,
        session_id: &str,
        limit: usize,
    ) -> PrototypeResult<Vec<ExternalIngress>> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let cursor = read_cursor(&tx, session_id)?;
        let rows = {
            let mut statement = tx.prepare(
                "SELECT session_id, ingress_sequence, ingress_id, payload
                 FROM external_ingress
                 WHERE session_id = ? AND ingress_sequence > ?
                 ORDER BY ingress_sequence
                 LIMIT ?",
            )?;
            statement
                .query_map(params![session_id, cursor, limit as i64], map_ingress)?
                .collect::<Result<Vec<_>, _>>()?
        };
        if let Some(last) = rows.last() {
            tx.execute(
                "INSERT INTO external_ingress_cursor (session_id, last_sequence)
                 VALUES (?, ?)
                 ON CONFLICT(session_id) DO UPDATE SET last_sequence = excluded.last_sequence
                 WHERE excluded.last_sequence > last_sequence",
                params![session_id, last.sequence],
            )?;
        }
        tx.commit()?;
        Ok(rows)
    }

    pub fn accept_pending(
        &mut self,
        delivery_id: &str,
        session_id: &str,
        turn_generation_id: &str,
        accepted_at: i64,
    ) -> PrototypeResult<AcknowledgementWrite> {
        if let Some(existing) = read_acknowledgement(&self.conn, delivery_id)? {
            return if existing.session_id == session_id
                && existing.turn_generation_id == turn_generation_id
            {
                Ok(AcknowledgementWrite::AlreadyRecorded)
            } else {
                Err(PrototypeError::FenceMismatch)
            };
        }
        self.conn.execute(
            "INSERT INTO delivery_acknowledgement (
                delivery_id, session_id, turn_generation_id, accepted_at
             ) VALUES (?, ?, ?, ?)",
            params![delivery_id, session_id, turn_generation_id, accepted_at],
        )?;
        Ok(AcknowledgementWrite::Advanced)
    }

    pub fn mark_submitted(
        &mut self,
        delivery_id: &str,
        session_id: &str,
        turn_generation_id: &str,
        evidence: &str,
        submitted_at: i64,
    ) -> PrototypeResult<AcknowledgementWrite> {
        let existing =
            self.acknowledgement_with_fence(delivery_id, session_id, turn_generation_id)?;
        if let Some(recorded) = existing.submitted_evidence {
            return if recorded == evidence {
                Ok(AcknowledgementWrite::AlreadyRecorded)
            } else {
                Err(PrototypeError::Conflict("submission evidence"))
            };
        }
        self.conn.execute(
            "UPDATE delivery_acknowledgement
             SET submitted_at = ?, submitted_evidence = ?
             WHERE delivery_id = ? AND session_id = ? AND turn_generation_id = ?
               AND submitted_at IS NULL",
            params![
                submitted_at,
                evidence,
                delivery_id,
                session_id,
                turn_generation_id
            ],
        )?;
        Ok(AcknowledgementWrite::Advanced)
    }

    pub fn mark_confirmed(
        &mut self,
        delivery_id: &str,
        session_id: &str,
        turn_generation_id: &str,
        evidence: &str,
        confirmed_at: i64,
    ) -> PrototypeResult<AcknowledgementWrite> {
        let existing =
            self.acknowledgement_with_fence(delivery_id, session_id, turn_generation_id)?;
        if existing.submitted_at.is_none() {
            return Err(PrototypeError::InvalidTransition);
        }
        if let Some(recorded) = existing.confirmed_evidence {
            return if recorded == evidence {
                Ok(AcknowledgementWrite::AlreadyRecorded)
            } else {
                Err(PrototypeError::Conflict("confirmation evidence"))
            };
        }
        self.conn.execute(
            "UPDATE delivery_acknowledgement
             SET confirmed_at = ?, confirmed_evidence = ?
             WHERE delivery_id = ? AND session_id = ? AND turn_generation_id = ?
               AND confirmed_at IS NULL",
            params![
                confirmed_at,
                evidence,
                delivery_id,
                session_id,
                turn_generation_id
            ],
        )?;
        Ok(AcknowledgementWrite::Advanced)
    }

    pub fn acknowledgement(
        &self,
        delivery_id: &str,
    ) -> PrototypeResult<Option<DeliveryAcknowledgement>> {
        read_acknowledgement(&self.conn, delivery_id).map_err(Into::into)
    }

    pub fn reconstruct_session(
        &self,
        session_id: &str,
        consumer_id: &str,
        limit: usize,
    ) -> PrototypeResult<SessionReconstruction> {
        let lease = read_lease(&self.conn, session_id)?;
        let active_child = self
            .conn
            .query_row(
                "SELECT generation_id, spawn_invocation_id, session_id, lifecycle_state,
                        child_pid, child_boot_id, child_start_time_ticks
                 FROM provider_turn_generation
                 WHERE session_id = ? AND lifecycle_state <> 'exited'",
                params![session_id],
                map_turn,
            )
            .optional()?;
        let ingress_cursor = read_cursor(&self.conn, session_id)?;
        let acknowledgements = {
            let mut statement = self.conn.prepare(
                "SELECT delivery_id, session_id, turn_generation_id, accepted_at,
                        submitted_at, submitted_evidence, confirmed_at, confirmed_evidence
                 FROM delivery_acknowledgement
                 WHERE session_id = ?
                 ORDER BY accepted_at, delivery_id
                 LIMIT ?",
            )?;
            statement
                .query_map(params![session_id, limit as i64], map_acknowledgement)?
                .collect::<Result<Vec<_>, _>>()?
        };
        let unconsumed_events = {
            let mut statement = self.conn.prepare(
                "SELECT e.event_id, e.session_id, e.sequence, e.event_type, e.cause_event_id,
                        e.correlation_id, e.payload, e.created_at
                 FROM lifecycle_event e
                 LEFT JOIN lifecycle_event_disposition d
                   ON d.event_id = e.event_id AND d.consumer_id = ?
                 WHERE e.session_id = ? AND d.event_id IS NULL
                 ORDER BY e.sequence
                 LIMIT ?",
            )?;
            statement
                .query_map(params![consumer_id, session_id, limit as i64], map_event)?
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(SessionReconstruction {
            session_id: session_id.to_owned(),
            lease,
            active_child,
            ingress_cursor,
            acknowledgements,
            unconsumed_events,
        })
    }

    fn acknowledgement_with_fence(
        &self,
        delivery_id: &str,
        session_id: &str,
        turn_generation_id: &str,
    ) -> PrototypeResult<DeliveryAcknowledgement> {
        let existing = read_acknowledgement(&self.conn, delivery_id)?
            .ok_or(PrototypeError::Missing("delivery acknowledgement"))?;
        if existing.session_id != session_id || existing.turn_generation_id != turn_generation_id {
            return Err(PrototypeError::FenceMismatch);
        }
        Ok(existing)
    }
}

fn map_turn_uniqueness(result: rusqlite::Result<usize>) -> PrototypeResult<usize> {
    match result {
        Ok(changed) => Ok(changed),
        Err(error)
            if error.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation) =>
        {
            Err(PrototypeError::TurnAlreadyActive)
        }
        Err(error) => Err(error.into()),
    }
}

fn read_lease(conn: &Connection, session_id: &str) -> rusqlite::Result<Option<SupervisorLease>> {
    conn.query_row(
        "SELECT session_id, supervisor_generation, lease_token, supervisor_pid,
                boot_id, start_time_ticks, acquired_at
         FROM supervisor_lease WHERE session_id = ?",
        params![session_id],
        |row| {
            Ok(SupervisorLease {
                session_id: row.get(0)?,
                fence: SupervisorFence {
                    generation: row.get(1)?,
                    token: row.get(2)?,
                    process: ExactProcessIdentity {
                        pid: row.get(3)?,
                        boot_id: row.get(4)?,
                        start_time_ticks: row.get(5)?,
                    },
                },
                acquired_at: row.get(6)?,
            })
        },
    )
    .optional()
}

fn read_turn_by_generation(
    conn: &Connection,
    generation_id: &str,
) -> rusqlite::Result<Option<ProviderTurnGeneration>> {
    conn.query_row(
        "SELECT generation_id, spawn_invocation_id, session_id, lifecycle_state,
                child_pid, child_boot_id, child_start_time_ticks
         FROM provider_turn_generation WHERE generation_id = ?",
        params![generation_id],
        map_turn,
    )
    .optional()
}

fn map_turn(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProviderTurnGeneration> {
    Ok(ProviderTurnGeneration {
        generation_id: row.get(0)?,
        spawn_invocation_id: row.get(1)?,
        session_id: row.get(2)?,
        state: TurnState::parse(&row.get::<_, String>(3)?)?,
        child: ExactProcessIdentity {
            pid: row.get(4)?,
            boot_id: row.get(5)?,
            start_time_ticks: row.get(6)?,
        },
    })
}

fn next_event_sequence(tx: &Transaction<'_>, session_id: &str) -> rusqlite::Result<i64> {
    tx.execute(
        "INSERT OR IGNORE INTO lifecycle_sequence (session_id, last_sequence) VALUES (?, 0)",
        params![session_id],
    )?;
    tx.execute(
        "UPDATE lifecycle_sequence SET last_sequence = last_sequence + 1 WHERE session_id = ?",
        params![session_id],
    )?;
    tx.query_row(
        "SELECT last_sequence FROM lifecycle_sequence WHERE session_id = ?",
        params![session_id],
        |row| row.get(0),
    )
}

fn append_event(
    tx: &Transaction<'_>,
    session_id: &str,
    event: &NewLifecycleEvent,
) -> rusqlite::Result<LifecycleEvent> {
    let sequence = next_event_sequence(tx, session_id)?;
    tx.execute(
        "INSERT INTO lifecycle_event (
            event_id, session_id, sequence, event_type, cause_event_id,
            correlation_id, payload, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            event.event_id,
            session_id,
            sequence,
            event.event_type,
            event.cause_event_id,
            event.correlation_id,
            event.payload,
            event.created_at,
        ],
    )?;
    Ok(LifecycleEvent {
        event_id: event.event_id.clone(),
        session_id: session_id.to_owned(),
        sequence,
        event_type: event.event_type.clone(),
        cause_event_id: event.cause_event_id.clone(),
        correlation_id: event.correlation_id.clone(),
        payload: event.payload.clone(),
        created_at: event.created_at,
    })
}

fn map_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<LifecycleEvent> {
    Ok(LifecycleEvent {
        event_id: row.get(0)?,
        session_id: row.get(1)?,
        sequence: row.get(2)?,
        event_type: row.get(3)?,
        cause_event_id: row.get(4)?,
        correlation_id: row.get(5)?,
        payload: row.get(6)?,
        created_at: row.get(7)?,
    })
}

fn map_ingress(row: &rusqlite::Row<'_>) -> rusqlite::Result<ExternalIngress> {
    Ok(ExternalIngress {
        session_id: row.get(0)?,
        sequence: row.get(1)?,
        ingress_id: row.get(2)?,
        payload: row.get(3)?,
    })
}

fn read_cursor(conn: &Connection, session_id: &str) -> rusqlite::Result<i64> {
    Ok(conn
        .query_row(
            "SELECT last_sequence FROM external_ingress_cursor WHERE session_id = ?",
            params![session_id],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0))
}

fn read_acknowledgement(
    conn: &Connection,
    delivery_id: &str,
) -> rusqlite::Result<Option<DeliveryAcknowledgement>> {
    conn.query_row(
        "SELECT delivery_id, session_id, turn_generation_id, accepted_at,
                submitted_at, submitted_evidence, confirmed_at, confirmed_evidence
         FROM delivery_acknowledgement WHERE delivery_id = ?",
        params![delivery_id],
        map_acknowledgement,
    )
    .optional()
}

fn map_acknowledgement(row: &rusqlite::Row<'_>) -> rusqlite::Result<DeliveryAcknowledgement> {
    Ok(DeliveryAcknowledgement {
        delivery_id: row.get(0)?,
        session_id: row.get(1)?,
        turn_generation_id: row.get(2)?,
        accepted_at: row.get(3)?,
        submitted_at: row.get(4)?,
        submitted_evidence: row.get(5)?,
        confirmed_at: row.get(6)?,
        confirmed_evidence: row.get(7)?,
    })
}
