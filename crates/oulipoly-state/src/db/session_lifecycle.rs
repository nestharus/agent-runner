//! Typed repository for durable, session-scoped supervisor lifecycle state.

use super::StateDb;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::error::Error;
use std::fmt;

#[derive(Debug)]
pub enum SessionLifecycleError {
    Sql(rusqlite::Error),
    Invalid(&'static str),
    LeaseHeld,
    FenceMismatch,
    TurnAlreadyActive,
    Missing(&'static str),
    InvalidTransition,
    Conflict(&'static str),
}

impl fmt::Display for SessionLifecycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sql(error) => write!(f, "SQLite error: {error}"),
            Self::Invalid(field) => write!(f, "invalid durable lifecycle field: {field}"),
            Self::LeaseHeld => write!(f, "the session already has another supervisor lease"),
            Self::FenceMismatch => write!(f, "the exact session and process fence did not match"),
            Self::TurnAlreadyActive => {
                write!(f, "the session already has a nonterminal provider turn")
            }
            Self::Missing(kind) => write!(f, "missing {kind}"),
            Self::InvalidTransition => write!(f, "invalid lifecycle transition"),
            Self::Conflict(kind) => write!(f, "conflicting replay for {kind}"),
        }
    }
}

impl Error for SessionLifecycleError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Sql(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for SessionLifecycleError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sql(value)
    }
}

pub type SessionLifecycleResult<T> = Result<T, SessionLifecycleError>;

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
pub enum LeaseReplace {
    Replaced,
    AlreadyReplaced,
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
    pub active_turn: Option<ProviderTurnGeneration>,
    pub ingress_cursor: i64,
    pub acknowledgements: Vec<DeliveryAcknowledgement>,
    pub undisposed_events: Vec<LifecycleEvent>,
}

/// SQL-free runtime port for one session's durable lifecycle authority.
pub trait SessionLifecycleRepository {
    fn acquire_supervisor_lease(
        &mut self,
        session_id: &str,
        fence: &SupervisorFence,
        acquired_at: i64,
    ) -> SessionLifecycleResult<LeaseAcquire>;
    fn replace_supervisor_lease(
        &mut self,
        session_id: &str,
        expected: &SupervisorFence,
        replacement: &SupervisorFence,
        acquired_at: i64,
    ) -> SessionLifecycleResult<LeaseReplace>;
    fn release_supervisor_lease(
        &mut self,
        session_id: &str,
        fence: &SupervisorFence,
    ) -> SessionLifecycleResult<()>;
    fn supervisor_lease(&self, session_id: &str)
    -> SessionLifecycleResult<Option<SupervisorLease>>;
    fn start_provider_turn(&mut self, turn: &ProviderTurnGeneration) -> SessionLifecycleResult<()>;
    fn attach_provider_turn_session(
        &mut self,
        generation_id: &str,
        spawn_invocation_id: &str,
        session_id: &str,
    ) -> SessionLifecycleResult<()>;
    fn provider_turn(
        &self,
        generation_id: &str,
    ) -> SessionLifecycleResult<Option<ProviderTurnGeneration>>;
    fn append_lifecycle_event(
        &mut self,
        session_id: &str,
        event: &NewLifecycleEvent,
    ) -> SessionLifecycleResult<LifecycleEvent>;
    fn transition_turn_and_append_event(
        &mut self,
        fence: &TurnFence,
        from: TurnState,
        to: TurnState,
        event: &NewLifecycleEvent,
    ) -> SessionLifecycleResult<LifecycleEvent>;
    fn record_event_disposition(
        &mut self,
        event_id: &str,
        consumer_id: &str,
        disposition: EventDisposition,
        disposed_at: i64,
    ) -> SessionLifecycleResult<DispositionWrite>;
    fn append_external_ingress(&mut self, ingress: &ExternalIngress) -> SessionLifecycleResult<()>;
    fn read_external_ingress(
        &mut self,
        session_id: &str,
        limit: usize,
    ) -> SessionLifecycleResult<Vec<ExternalIngress>>;
    fn accept_pending(
        &mut self,
        delivery_id: &str,
        session_id: &str,
        turn_generation_id: &str,
        accepted_at: i64,
    ) -> SessionLifecycleResult<AcknowledgementWrite>;
    fn mark_submitted(
        &mut self,
        delivery_id: &str,
        session_id: &str,
        turn_generation_id: &str,
        evidence: &str,
        submitted_at: i64,
    ) -> SessionLifecycleResult<AcknowledgementWrite>;
    fn mark_confirmed(
        &mut self,
        delivery_id: &str,
        session_id: &str,
        turn_generation_id: &str,
        evidence: &str,
        confirmed_at: i64,
    ) -> SessionLifecycleResult<AcknowledgementWrite>;
    fn acknowledgement(
        &self,
        delivery_id: &str,
    ) -> SessionLifecycleResult<Option<DeliveryAcknowledgement>>;
    fn reconstruct_session(
        &self,
        session_id: &str,
        consumer_id: &str,
        limit: usize,
    ) -> SessionLifecycleResult<SessionReconstruction>;
}

impl SessionLifecycleRepository for StateDb {
    fn acquire_supervisor_lease(
        &mut self,
        session_id: &str,
        fence: &SupervisorFence,
        acquired_at: i64,
    ) -> SessionLifecycleResult<LeaseAcquire> {
        validate_session_id(session_id)?;
        validate_supervisor_fence(fence)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(current) = read_lease(&tx, session_id)? {
            return if current.fence == *fence {
                Ok(LeaseAcquire::AlreadyOwned)
            } else {
                Err(SessionLifecycleError::LeaseHeld)
            };
        }
        tx.execute(
            "INSERT INTO session_supervisor_leases (
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

    fn replace_supervisor_lease(
        &mut self,
        session_id: &str,
        expected: &SupervisorFence,
        replacement: &SupervisorFence,
        acquired_at: i64,
    ) -> SessionLifecycleResult<LeaseReplace> {
        validate_session_id(session_id)?;
        validate_supervisor_fence(expected)?;
        validate_supervisor_fence(replacement)?;
        if expected.generation.checked_add(1) != Some(replacement.generation) {
            return Err(SessionLifecycleError::InvalidTransition);
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE session_supervisor_leases
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
        let result = if changed == 1 {
            Ok(LeaseReplace::Replaced)
        } else if read_lease(&tx, session_id)?.is_some_and(|lease| lease.fence == *replacement) {
            Ok(LeaseReplace::AlreadyReplaced)
        } else {
            Err(SessionLifecycleError::FenceMismatch)
        };
        tx.commit()?;
        result
    }

    fn release_supervisor_lease(
        &mut self,
        session_id: &str,
        fence: &SupervisorFence,
    ) -> SessionLifecycleResult<()> {
        validate_session_id(session_id)?;
        validate_supervisor_fence(fence)?;
        let changed = self.conn.execute(
            "DELETE FROM session_supervisor_leases
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
            Err(SessionLifecycleError::FenceMismatch)
        }
    }

    fn supervisor_lease(
        &self,
        session_id: &str,
    ) -> SessionLifecycleResult<Option<SupervisorLease>> {
        validate_session_id(session_id)?;
        read_lease(&self.conn, session_id).map_err(Into::into)
    }

    fn start_provider_turn(&mut self, turn: &ProviderTurnGeneration) -> SessionLifecycleResult<()> {
        validate_turn(turn)?;
        let result = self.conn.execute(
            "INSERT INTO provider_turn_generations (
                generation_id, spawn_invocation_id, session_id, lifecycle_state,
                child_pid, child_boot_id, child_start_time_ticks
             ) VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                turn.generation_id,
                turn.spawn_invocation_id,
                turn.session_id,
                turn.state.as_str(),
                turn.child.pid,
                turn.child.boot_id,
                turn.child.start_time_ticks,
            ],
        );
        map_turn_insert_result(
            &self.conn,
            result,
            turn.session_id.as_deref(),
            &turn.generation_id,
        )
        .map(|_| ())
    }

    fn attach_provider_turn_session(
        &mut self,
        generation_id: &str,
        spawn_invocation_id: &str,
        session_id: &str,
    ) -> SessionLifecycleResult<()> {
        validate_nonempty(generation_id, "generation_id")?;
        validate_nonempty(spawn_invocation_id, "spawn_invocation_id")?;
        validate_session_id(session_id)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = read_turn_by_generation(&tx, generation_id)?
            .ok_or(SessionLifecycleError::Missing("provider turn generation"))?;
        if current.spawn_invocation_id != spawn_invocation_id {
            return Err(SessionLifecycleError::FenceMismatch);
        }
        if let Some(attached) = current.session_id {
            return if attached == session_id {
                Ok(())
            } else {
                Err(SessionLifecycleError::FenceMismatch)
            };
        }
        let result = tx.execute(
            "UPDATE provider_turn_generations SET session_id = ?
             WHERE generation_id = ? AND spawn_invocation_id = ? AND session_id IS NULL",
            params![session_id, generation_id, spawn_invocation_id],
        );
        let result = map_turn_insert_result(&tx, result, Some(session_id), generation_id).and_then(
            |changed| {
                if changed == 1 {
                    Ok(())
                } else {
                    Err(SessionLifecycleError::FenceMismatch)
                }
            },
        );
        tx.commit()?;
        result
    }

    fn provider_turn(
        &self,
        generation_id: &str,
    ) -> SessionLifecycleResult<Option<ProviderTurnGeneration>> {
        validate_nonempty(generation_id, "generation_id")?;
        read_turn_by_generation(&self.conn, generation_id).map_err(Into::into)
    }

    fn append_lifecycle_event(
        &mut self,
        session_id: &str,
        event: &NewLifecycleEvent,
    ) -> SessionLifecycleResult<LifecycleEvent> {
        validate_session_id(session_id)?;
        validate_event(event)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row = append_event(&tx, session_id, event)?;
        tx.commit()?;
        Ok(row)
    }

    fn transition_turn_and_append_event(
        &mut self,
        fence: &TurnFence,
        from: TurnState,
        to: TurnState,
        event: &NewLifecycleEvent,
    ) -> SessionLifecycleResult<LifecycleEvent> {
        validate_turn_fence(fence)?;
        validate_event(event)?;
        if !from.can_transition_to(to) {
            return Err(SessionLifecycleError::InvalidTransition);
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE provider_turn_generations SET lifecycle_state = ?
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
            return Err(SessionLifecycleError::FenceMismatch);
        }
        let row = append_event(&tx, &fence.session_id, event)?;
        tx.commit()?;
        Ok(row)
    }

    fn record_event_disposition(
        &mut self,
        event_id: &str,
        consumer_id: &str,
        disposition: EventDisposition,
        disposed_at: i64,
    ) -> SessionLifecycleResult<DispositionWrite> {
        validate_nonempty(event_id, "event_id")?;
        validate_nonempty(consumer_id, "consumer_id")?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !event_exists(&tx, event_id)? {
            return Err(SessionLifecycleError::Missing("lifecycle event"));
        }
        let existing = tx
            .query_row(
                "SELECT disposition FROM session_lifecycle_event_dispositions
                 WHERE event_id = ? AND consumer_id = ?",
                params![event_id, consumer_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if let Some(existing) = existing {
            return if existing == disposition.as_str() {
                Ok(DispositionWrite::AlreadyRecorded)
            } else {
                Err(SessionLifecycleError::Conflict("event disposition"))
            };
        }
        tx.execute(
            "INSERT INTO session_lifecycle_event_dispositions (
                event_id, consumer_id, disposition, disposed_at
             ) VALUES (?, ?, ?, ?)",
            params![event_id, consumer_id, disposition.as_str(), disposed_at],
        )?;
        tx.commit()?;
        Ok(DispositionWrite::Recorded)
    }

    fn append_external_ingress(&mut self, ingress: &ExternalIngress) -> SessionLifecycleResult<()> {
        validate_ingress(ingress)?;
        let inserted = self.conn.execute(
            "INSERT OR IGNORE INTO session_external_ingress (
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
                 FROM session_external_ingress
                 WHERE ingress_id = ? OR (session_id = ? AND ingress_sequence = ?)",
                params![ingress.ingress_id, ingress.session_id, ingress.sequence],
                map_ingress,
            )
            .optional()?;
        if existing.as_ref() == Some(ingress) {
            Ok(())
        } else {
            Err(SessionLifecycleError::Conflict("external ingress"))
        }
    }

    fn read_external_ingress(
        &mut self,
        session_id: &str,
        limit: usize,
    ) -> SessionLifecycleResult<Vec<ExternalIngress>> {
        validate_session_id(session_id)?;
        let limit = bounded_limit(limit)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let cursor = read_cursor(&tx, session_id)?;
        let rows = {
            let mut statement = tx.prepare(
                "SELECT session_id, ingress_sequence, ingress_id, payload
                 FROM session_external_ingress
                 WHERE session_id = ? AND ingress_sequence > ?
                 ORDER BY ingress_sequence
                 LIMIT ?",
            )?;
            statement
                .query_map(params![session_id, cursor, limit], map_ingress)?
                .collect::<Result<Vec<_>, _>>()?
        };
        if let Some(last) = rows.last() {
            tx.execute(
                "INSERT INTO session_external_ingress_cursors (session_id, last_sequence)
                 VALUES (?, ?)
                 ON CONFLICT(session_id) DO UPDATE SET last_sequence = excluded.last_sequence
                 WHERE excluded.last_sequence > last_sequence",
                params![session_id, last.sequence],
            )?;
        }
        tx.commit()?;
        Ok(rows)
    }

    fn accept_pending(
        &mut self,
        delivery_id: &str,
        session_id: &str,
        turn_generation_id: &str,
        accepted_at: i64,
    ) -> SessionLifecycleResult<AcknowledgementWrite> {
        validate_acknowledgement_fence(delivery_id, session_id, turn_generation_id)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = read_acknowledgement(&tx, delivery_id)? {
            return if existing.session_id == session_id
                && existing.turn_generation_id == turn_generation_id
            {
                Ok(AcknowledgementWrite::AlreadyRecorded)
            } else {
                Err(SessionLifecycleError::FenceMismatch)
            };
        }
        tx.execute(
            "INSERT INTO session_delivery_acknowledgements (
                delivery_id, session_id, turn_generation_id, accepted_at
             ) VALUES (?, ?, ?, ?)",
            params![delivery_id, session_id, turn_generation_id, accepted_at],
        )?;
        tx.commit()?;
        Ok(AcknowledgementWrite::Advanced)
    }

    fn mark_submitted(
        &mut self,
        delivery_id: &str,
        session_id: &str,
        turn_generation_id: &str,
        evidence: &str,
        submitted_at: i64,
    ) -> SessionLifecycleResult<AcknowledgementWrite> {
        validate_acknowledgement_fence(delivery_id, session_id, turn_generation_id)?;
        validate_nonempty(evidence, "submission evidence")?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing =
            acknowledgement_with_fence(&tx, delivery_id, session_id, turn_generation_id)?;
        if let Some(recorded) = existing.submitted_evidence {
            return if recorded == evidence {
                Ok(AcknowledgementWrite::AlreadyRecorded)
            } else {
                Err(SessionLifecycleError::Conflict("submission evidence"))
            };
        }
        tx.execute(
            "UPDATE session_delivery_acknowledgements
             SET submitted_at = ?, submitted_evidence = ?
             WHERE delivery_id = ? AND session_id = ? AND turn_generation_id = ?
               AND submitted_at IS NULL",
            params![
                submitted_at,
                evidence,
                delivery_id,
                session_id,
                turn_generation_id,
            ],
        )?;
        tx.commit()?;
        Ok(AcknowledgementWrite::Advanced)
    }

    fn mark_confirmed(
        &mut self,
        delivery_id: &str,
        session_id: &str,
        turn_generation_id: &str,
        evidence: &str,
        confirmed_at: i64,
    ) -> SessionLifecycleResult<AcknowledgementWrite> {
        validate_acknowledgement_fence(delivery_id, session_id, turn_generation_id)?;
        validate_nonempty(evidence, "confirmation evidence")?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing =
            acknowledgement_with_fence(&tx, delivery_id, session_id, turn_generation_id)?;
        if existing.submitted_at.is_none() {
            return Err(SessionLifecycleError::InvalidTransition);
        }
        if let Some(recorded) = existing.confirmed_evidence {
            return if recorded == evidence {
                Ok(AcknowledgementWrite::AlreadyRecorded)
            } else {
                Err(SessionLifecycleError::Conflict("confirmation evidence"))
            };
        }
        tx.execute(
            "UPDATE session_delivery_acknowledgements
             SET confirmed_at = ?, confirmed_evidence = ?
             WHERE delivery_id = ? AND session_id = ? AND turn_generation_id = ?
               AND confirmed_at IS NULL",
            params![
                confirmed_at,
                evidence,
                delivery_id,
                session_id,
                turn_generation_id,
            ],
        )?;
        tx.commit()?;
        Ok(AcknowledgementWrite::Advanced)
    }

    fn acknowledgement(
        &self,
        delivery_id: &str,
    ) -> SessionLifecycleResult<Option<DeliveryAcknowledgement>> {
        validate_nonempty(delivery_id, "delivery_id")?;
        read_acknowledgement(&self.conn, delivery_id).map_err(Into::into)
    }

    fn reconstruct_session(
        &self,
        session_id: &str,
        consumer_id: &str,
        limit: usize,
    ) -> SessionLifecycleResult<SessionReconstruction> {
        validate_session_id(session_id)?;
        validate_nonempty(consumer_id, "consumer_id")?;
        let limit = bounded_limit(limit)?;
        let tx = self.conn.unchecked_transaction()?;
        let lease = read_lease(&tx, session_id)?;
        let active_turn = tx
            .query_row(
                "SELECT generation_id, spawn_invocation_id, session_id, lifecycle_state,
                        child_pid, child_boot_id, child_start_time_ticks
                 FROM provider_turn_generations
                 WHERE session_id = ? AND lifecycle_state <> 'exited'",
                params![session_id],
                map_turn,
            )
            .optional()?;
        let ingress_cursor = read_cursor(&tx, session_id)?;
        let acknowledgements = {
            let mut statement = tx.prepare(
                "SELECT delivery_id, session_id, turn_generation_id, accepted_at,
                        submitted_at, submitted_evidence, confirmed_at, confirmed_evidence
                 FROM session_delivery_acknowledgements
                 WHERE session_id = ?
                 ORDER BY accepted_at, delivery_id
                 LIMIT ?",
            )?;
            statement
                .query_map(params![session_id, limit], map_acknowledgement)?
                .collect::<Result<Vec<_>, _>>()?
        };
        let undisposed_events = {
            let mut statement = tx.prepare(
                "SELECT e.event_id, e.session_id, e.sequence, e.event_type, e.cause_event_id,
                        e.correlation_id, e.payload, e.created_at
                 FROM session_lifecycle_events e
                 LEFT JOIN session_lifecycle_event_dispositions d
                   ON d.event_id = e.event_id AND d.consumer_id = ?
                 WHERE e.session_id = ? AND d.event_id IS NULL
                 ORDER BY e.sequence
                 LIMIT ?",
            )?;
            statement
                .query_map(params![consumer_id, session_id, limit], map_event)?
                .collect::<Result<Vec<_>, _>>()?
        };
        tx.commit()?;
        Ok(SessionReconstruction {
            session_id: session_id.to_owned(),
            lease,
            active_turn,
            ingress_cursor,
            acknowledgements,
            undisposed_events,
        })
    }
}

fn validate_nonempty(value: &str, field: &'static str) -> SessionLifecycleResult<()> {
    if value.is_empty() {
        Err(SessionLifecycleError::Invalid(field))
    } else {
        Ok(())
    }
}

fn validate_session_id(session_id: &str) -> SessionLifecycleResult<()> {
    validate_nonempty(session_id, "session_id")
}

fn validate_process(process: &ExactProcessIdentity) -> SessionLifecycleResult<()> {
    if process.pid <= 0 {
        return Err(SessionLifecycleError::Invalid("pid"));
    }
    validate_nonempty(&process.boot_id, "boot_id")?;
    if process.start_time_ticks <= 0 {
        return Err(SessionLifecycleError::Invalid("start_time_ticks"));
    }
    Ok(())
}

fn validate_supervisor_fence(fence: &SupervisorFence) -> SessionLifecycleResult<()> {
    if fence.generation <= 0 {
        return Err(SessionLifecycleError::Invalid("supervisor_generation"));
    }
    validate_nonempty(&fence.token, "lease_token")?;
    validate_process(&fence.process)
}

fn validate_turn(turn: &ProviderTurnGeneration) -> SessionLifecycleResult<()> {
    validate_nonempty(&turn.generation_id, "generation_id")?;
    validate_nonempty(&turn.spawn_invocation_id, "spawn_invocation_id")?;
    if let Some(session_id) = turn.session_id.as_deref() {
        validate_session_id(session_id)?;
    }
    validate_process(&turn.child)
}

fn validate_turn_fence(fence: &TurnFence) -> SessionLifecycleResult<()> {
    validate_session_id(&fence.session_id)?;
    validate_nonempty(&fence.generation_id, "generation_id")?;
    validate_nonempty(&fence.spawn_invocation_id, "spawn_invocation_id")
}

fn validate_event(event: &NewLifecycleEvent) -> SessionLifecycleResult<()> {
    validate_nonempty(&event.event_id, "event_id")?;
    validate_nonempty(&event.event_type, "event_type")?;
    if let Some(cause_event_id) = event.cause_event_id.as_deref() {
        validate_nonempty(cause_event_id, "cause_event_id")?;
    }
    validate_nonempty(&event.correlation_id, "correlation_id")
}

fn validate_ingress(ingress: &ExternalIngress) -> SessionLifecycleResult<()> {
    validate_session_id(&ingress.session_id)?;
    if ingress.sequence <= 0 {
        return Err(SessionLifecycleError::Invalid("ingress_sequence"));
    }
    validate_nonempty(&ingress.ingress_id, "ingress_id")
}

fn validate_acknowledgement_fence(
    delivery_id: &str,
    session_id: &str,
    turn_generation_id: &str,
) -> SessionLifecycleResult<()> {
    validate_nonempty(delivery_id, "delivery_id")?;
    validate_session_id(session_id)?;
    validate_nonempty(turn_generation_id, "turn_generation_id")
}

fn bounded_limit(limit: usize) -> SessionLifecycleResult<i64> {
    i64::try_from(limit).map_err(|_| SessionLifecycleError::Invalid("limit"))
}

fn map_turn_insert_result(
    conn: &Connection,
    result: rusqlite::Result<usize>,
    session_id: Option<&str>,
    generation_id: &str,
) -> SessionLifecycleResult<usize> {
    match result {
        Ok(changed) => Ok(changed),
        Err(error)
            if error.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation) =>
        {
            if let Some(session_id) = session_id
                && active_turn_generation(conn, session_id)?.as_deref() != Some(generation_id)
            {
                return Err(SessionLifecycleError::TurnAlreadyActive);
            }
            Err(SessionLifecycleError::Conflict("provider turn identity"))
        }
        Err(error) => Err(error.into()),
    }
}

fn active_turn_generation(conn: &Connection, session_id: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT generation_id FROM provider_turn_generations
         WHERE session_id = ? AND lifecycle_state <> 'exited'",
        params![session_id],
        |row| row.get(0),
    )
    .optional()
}

fn read_lease(conn: &Connection, session_id: &str) -> rusqlite::Result<Option<SupervisorLease>> {
    conn.query_row(
        "SELECT session_id, supervisor_generation, lease_token, supervisor_pid,
                boot_id, start_time_ticks, acquired_at
         FROM session_supervisor_leases WHERE session_id = ?",
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
         FROM provider_turn_generations WHERE generation_id = ?",
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

fn append_event(
    tx: &Transaction<'_>,
    session_id: &str,
    event: &NewLifecycleEvent,
) -> SessionLifecycleResult<LifecycleEvent> {
    if let Some(cause_event_id) = event.cause_event_id.as_deref()
        && !event_exists(tx, cause_event_id)?
    {
        return Err(SessionLifecycleError::Missing("cause lifecycle event"));
    }
    let sequence = next_event_sequence(tx, session_id)?;
    tx.execute(
        "INSERT INTO session_lifecycle_events (
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

fn next_event_sequence(tx: &Transaction<'_>, session_id: &str) -> rusqlite::Result<i64> {
    tx.execute(
        "INSERT OR IGNORE INTO session_lifecycle_sequences (session_id, last_sequence)
         VALUES (?, 0)",
        params![session_id],
    )?;
    tx.execute(
        "UPDATE session_lifecycle_sequences
         SET last_sequence = last_sequence + 1 WHERE session_id = ?",
        params![session_id],
    )?;
    tx.query_row(
        "SELECT last_sequence FROM session_lifecycle_sequences WHERE session_id = ?",
        params![session_id],
        |row| row.get(0),
    )
}

fn event_exists(conn: &Connection, event_id: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM session_lifecycle_events WHERE event_id = ?)",
        params![event_id],
        |row| row.get(0),
    )
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
            "SELECT last_sequence FROM session_external_ingress_cursors WHERE session_id = ?",
            params![session_id],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0))
}

fn acknowledgement_with_fence(
    conn: &Connection,
    delivery_id: &str,
    session_id: &str,
    turn_generation_id: &str,
) -> SessionLifecycleResult<DeliveryAcknowledgement> {
    let existing = read_acknowledgement(conn, delivery_id)?
        .ok_or(SessionLifecycleError::Missing("delivery acknowledgement"))?;
    if existing.session_id != session_id || existing.turn_generation_id != turn_generation_id {
        return Err(SessionLifecycleError::FenceMismatch);
    }
    Ok(existing)
}

fn read_acknowledgement(
    conn: &Connection,
    delivery_id: &str,
) -> rusqlite::Result<Option<DeliveryAcknowledgement>> {
    conn.query_row(
        "SELECT delivery_id, session_id, turn_generation_id, accepted_at,
                submitted_at, submitted_evidence, confirmed_at, confirmed_evidence
         FROM session_delivery_acknowledgements WHERE delivery_id = ?",
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
