//! Additive ownership-authority persistence and projection vocabulary.
//!
//! ## Declared roles
//!
//! `accessor`, `formatter`, `mapper`, `predicate`, `validator`

use super::{RusqliteOptionalExtension, StateDb, sqlite};
use std::fmt;

#[derive(Debug, Clone, Copy)]
pub struct CompletionObligationAdmission<'a> {
    pub admission_id: &'a str,
    pub invocation_uuid: &'a str,
    pub event_id: &'a str,
    pub owner_invocation_uuid: &'a str,
    pub owner_session_id: &'a str,
    pub expected_sidecar_generation: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionObligationExpectation {
    pub admission_id: String,
    pub invocation_uuid: String,
    pub event_id: String,
    pub owner_invocation_uuid: String,
    pub owner_session_id: String,
    pub expected_sidecar_generation: String,
    pub admitted_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionObligationAdmissionResult {
    Recorded(CompletionObligationExpectation),
    Replay(CompletionObligationExpectation),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionObligationAuthority {
    NoAdmittedObligation,
    Admitted(CompletionObligationExpectation),
}

impl CompletionObligationAuthority {
    pub fn sidecar_generation_state(
        &self,
        observed_sidecar_generation: Option<&str>,
    ) -> SidecarGenerationState {
        match self {
            Self::NoAdmittedObligation => SidecarGenerationState::NoAdmittedObligation,
            Self::Admitted(expectation) => {
                expectation.sidecar_generation_state(observed_sidecar_generation)
            }
        }
    }
}

impl CompletionObligationExpectation {
    pub fn sidecar_generation_state(
        &self,
        observed_sidecar_generation: Option<&str>,
    ) -> SidecarGenerationState {
        let expected = self.expected_sidecar_generation.clone();
        match observed_sidecar_generation {
            None => SidecarGenerationState::ExpectedButUnobserved { expected },
            Some(observed) if observed == expected => SidecarGenerationState::Matching {
                expected,
                observed: observed.to_string(),
            },
            Some(observed) => SidecarGenerationState::Mismatched {
                expected,
                observed: observed.to_string(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidecarGenerationState {
    NoAdmittedObligation,
    ExpectedButUnobserved { expected: String },
    Matching { expected: String, observed: String },
    Mismatched { expected: String, observed: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedCompletionEventState {
    Pending,
    Triggered {
        terminal_rc: i32,
    },
    UnknownOrInvalid {
        state: String,
        terminal_rc: Option<i32>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerLineageRelationship {
    ExactOwner,
    RecursiveDescendant { depth: u32 },
    OutsideRecursiveLineage,
    UnknownOrInvalidAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SettlementVerifierIdentity(String);

impl SettlementVerifierIdentity {
    pub fn new(value: impl Into<String>) -> Result<Self, OwnershipAuthorityError> {
        let value = value.into();
        validate_nonempty(&value, "settlement verifier identity")?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListenerSettlementClass {
    PendingOrUnsettled,
    VerifiedTransportDelivery {
        verifier: SettlementVerifierIdentity,
    },
    ExactOwnerConsumption {
        verifier: SettlementVerifierIdentity,
    },
    ManualOrAdminAcknowledgement {
        verifier: SettlementVerifierIdentity,
    },
    ExplicitAbandonment {
        verifier: SettlementVerifierIdentity,
    },
    ExplicitWaiver {
        verifier: SettlementVerifierIdentity,
    },
    UnknownOrInvalidAuthority {
        verifier: Option<SettlementVerifierIdentity>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryDisposition {
    NotRecorded,
    Pending,
    Abandoned {
        authority: SettlementVerifierIdentity,
    },
    Waived {
        authority: SettlementVerifierIdentity,
    },
    UnknownOrInvalid {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnershipAuthoritySnapshot {
    pub invocation_uuid: String,
    pub event_id: String,
    pub sidecar_generation: SidecarGenerationState,
    pub event_state: OwnedCompletionEventState,
    pub owner_invocation_uuid: String,
    pub owner_session_id: String,
    pub owner_relationship: OwnerLineageRelationship,
    pub listener_settlement: ListenerSettlementClass,
    pub recovery_disposition: RecoveryDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveTerminalDisposition {
    pub success: bool,
    pub exit_code: i32,
    pub error_category: Option<String>,
    pub terminal_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnershipAuthorityError {
    InvalidIdentity(&'static str),
    InvocationNotFound(String),
    OwnerInvocationNotFound(String),
    ConflictingImmutableIdentity {
        existing: Box<CompletionObligationExpectation>,
    },
    Persistence(String),
}

impl fmt::Display for OwnershipAuthorityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentity(field) => {
                write!(formatter, "invalid ownership identity: {field}")
            }
            Self::InvocationNotFound(invocation_uuid) => {
                write!(formatter, "invocation not found: {invocation_uuid}")
            }
            Self::OwnerInvocationNotFound(invocation_uuid) => {
                write!(formatter, "owner invocation not found: {invocation_uuid}")
            }
            Self::ConflictingImmutableIdentity { existing } => write!(
                formatter,
                "completion obligation conflicts with immutable admission {}",
                existing.admission_id
            ),
            Self::Persistence(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for OwnershipAuthorityError {}

impl StateDb {
    pub fn record_completion_obligation(
        &self,
        input: CompletionObligationAdmission<'_>,
    ) -> Result<CompletionObligationAdmissionResult, OwnershipAuthorityError> {
        validate_completion_obligation(&input)?;
        let tx =
            sqlite::Transaction::new_unchecked(&self.conn, sqlite::TransactionBehavior::Immediate)
                .map_err(persistence("begin completion obligation admission"))?;

        if let Some(existing) = completion_obligation_by_admission_id(&tx, input.admission_id)? {
            return replay_or_conflict(existing, &input);
        }
        if let Some(existing) =
            completion_obligation_by_listener(&tx, input.event_id, input.owner_invocation_uuid)?
        {
            return Err(conflicting_identity(existing));
        }
        if let Some(existing) = completion_obligation_by_event_id(&tx, input.event_id)?
            && existing.expected_sidecar_generation != input.expected_sidecar_generation
        {
            return Err(conflicting_identity(existing));
        }
        require_invocation(&tx, input.invocation_uuid, false)?;
        require_invocation(&tx, input.owner_invocation_uuid, true)?;

        let admitted_at = Self::current_rfc3339_timestamp();
        tx.execute(
            "INSERT INTO invocation_completion_obligations (
                admission_id, invocation_uuid, event_id, owner_invocation_uuid,
                owner_session_id, expected_sidecar_generation, admitted_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            sqlite::params![
                input.admission_id,
                input.invocation_uuid,
                input.event_id,
                input.owner_invocation_uuid,
                input.owner_session_id,
                input.expected_sidecar_generation,
                &admitted_at,
            ],
        )
        .map_err(persistence("insert completion obligation"))?;
        let recorded = completion_obligation_by_admission_id(&tx, input.admission_id)?
            .ok_or_else(|| persistence_message("completion obligation disappeared after insert"))?;
        tx.commit()
            .map_err(persistence("commit completion obligation admission"))?;
        Ok(CompletionObligationAdmissionResult::Recorded(recorded))
    }

    pub fn completion_obligation_authority(
        &self,
        admission_id: &str,
    ) -> Result<CompletionObligationAuthority, OwnershipAuthorityError> {
        validate_nonempty(admission_id, "admission_id")?;
        completion_obligation_by_admission_id(&self.conn, admission_id).map(|row| match row {
            Some(expectation) => CompletionObligationAuthority::Admitted(expectation),
            None => CompletionObligationAuthority::NoAdmittedObligation,
        })
    }

    pub fn completion_obligations_for_invocation(
        &self,
        invocation_uuid: &str,
    ) -> Result<Vec<CompletionObligationExpectation>, OwnershipAuthorityError> {
        validate_nonempty(invocation_uuid, "invocation_uuid")?;
        let mut statement = self
            .conn
            .prepare(
                "SELECT admission_id, invocation_uuid, event_id, owner_invocation_uuid,
                        owner_session_id, expected_sidecar_generation, admitted_at
                 FROM invocation_completion_obligations
                 WHERE invocation_uuid = ?1
                 ORDER BY admitted_at, admission_id",
            )
            .map_err(persistence("prepare completion obligation query"))?;
        let rows = statement
            .query_map(
                sqlite::params![invocation_uuid],
                map_completion_obligation_row,
            )
            .map_err(persistence("query completion obligations"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(persistence("read completion obligations"))
    }

    pub fn owner_lineage_relationship(
        &self,
        invocation_uuid: &str,
        owner_invocation_uuid: &str,
    ) -> Result<OwnerLineageRelationship, OwnershipAuthorityError> {
        validate_nonempty(invocation_uuid, "invocation_uuid")?;
        validate_nonempty(owner_invocation_uuid, "owner_invocation_uuid")?;
        let Some(invocation_id) = invocation_row_id(&self.conn, invocation_uuid)? else {
            return Ok(OwnerLineageRelationship::UnknownOrInvalidAuthority);
        };
        let Some(owner_id) = invocation_row_id(&self.conn, owner_invocation_uuid)? else {
            return Ok(OwnerLineageRelationship::UnknownOrInvalidAuthority);
        };
        if invocation_id == owner_id {
            return Ok(OwnerLineageRelationship::ExactOwner);
        }
        recursive_descendant_depth(&self.conn, invocation_id, owner_id).map(|depth| match depth {
            Some(depth) => OwnerLineageRelationship::RecursiveDescendant { depth },
            None => OwnerLineageRelationship::OutsideRecursiveLineage,
        })
    }
}

fn validate_completion_obligation(
    input: &CompletionObligationAdmission<'_>,
) -> Result<(), OwnershipAuthorityError> {
    validate_nonempty(input.admission_id, "admission_id")?;
    validate_nonempty(input.invocation_uuid, "invocation_uuid")?;
    validate_nonempty(input.event_id, "event_id")?;
    validate_nonempty(input.owner_invocation_uuid, "owner_invocation_uuid")?;
    validate_nonempty(input.owner_session_id, "owner_session_id")?;
    validate_nonempty(
        input.expected_sidecar_generation,
        "expected_sidecar_generation",
    )
}

fn validate_nonempty(value: &str, field: &'static str) -> Result<(), OwnershipAuthorityError> {
    if value.trim().is_empty() {
        Err(OwnershipAuthorityError::InvalidIdentity(field))
    } else {
        Ok(())
    }
}

fn completion_obligation_by_admission_id(
    conn: &sqlite::Connection,
    admission_id: &str,
) -> Result<Option<CompletionObligationExpectation>, OwnershipAuthorityError> {
    completion_obligation_by_identity(
        conn,
        "WHERE admission_id = ?1",
        admission_id,
        "admission ID",
    )
}

fn completion_obligation_by_event_id(
    conn: &sqlite::Connection,
    event_id: &str,
) -> Result<Option<CompletionObligationExpectation>, OwnershipAuthorityError> {
    completion_obligation_by_identity(
        conn,
        "WHERE event_id = ?1 ORDER BY admitted_at, admission_id LIMIT 1",
        event_id,
        "event ID",
    )
}

fn completion_obligation_by_listener(
    conn: &sqlite::Connection,
    event_id: &str,
    owner_invocation_uuid: &str,
) -> Result<Option<CompletionObligationExpectation>, OwnershipAuthorityError> {
    conn.query_row(
        "SELECT admission_id, invocation_uuid, event_id, owner_invocation_uuid,
                owner_session_id, expected_sidecar_generation, admitted_at
         FROM invocation_completion_obligations
         WHERE event_id = ?1 AND owner_invocation_uuid = ?2",
        sqlite::params![event_id, owner_invocation_uuid],
        map_completion_obligation_row,
    )
    .optional()
    .map_err(persistence(
        "read completion obligation by listener identity",
    ))
}

fn completion_obligation_by_identity(
    conn: &sqlite::Connection,
    predicate: &str,
    identity: &str,
    context: &str,
) -> Result<Option<CompletionObligationExpectation>, OwnershipAuthorityError> {
    let sql = format!(
        "SELECT admission_id, invocation_uuid, event_id, owner_invocation_uuid,
                owner_session_id, expected_sidecar_generation, admitted_at
         FROM invocation_completion_obligations {predicate}"
    );
    conn.query_row(
        &sql,
        sqlite::params![identity],
        map_completion_obligation_row,
    )
    .optional()
    .map_err(persistence_owned(format!(
        "read completion obligation by {context}"
    )))
}

fn map_completion_obligation_row(
    row: &sqlite::Row<'_>,
) -> sqlite::Result<CompletionObligationExpectation> {
    Ok(CompletionObligationExpectation {
        admission_id: row.get(0)?,
        invocation_uuid: row.get(1)?,
        event_id: row.get(2)?,
        owner_invocation_uuid: row.get(3)?,
        owner_session_id: row.get(4)?,
        expected_sidecar_generation: row.get(5)?,
        admitted_at: row.get(6)?,
    })
}

fn replay_or_conflict(
    existing: CompletionObligationExpectation,
    input: &CompletionObligationAdmission<'_>,
) -> Result<CompletionObligationAdmissionResult, OwnershipAuthorityError> {
    if completion_obligation_matches(&existing, input) {
        Ok(CompletionObligationAdmissionResult::Replay(existing))
    } else {
        Err(conflicting_identity(existing))
    }
}

fn completion_obligation_matches(
    existing: &CompletionObligationExpectation,
    input: &CompletionObligationAdmission<'_>,
) -> bool {
    existing.admission_id == input.admission_id
        && existing.invocation_uuid == input.invocation_uuid
        && existing.event_id == input.event_id
        && existing.owner_invocation_uuid == input.owner_invocation_uuid
        && existing.owner_session_id == input.owner_session_id
        && existing.expected_sidecar_generation == input.expected_sidecar_generation
}

fn conflicting_identity(existing: CompletionObligationExpectation) -> OwnershipAuthorityError {
    OwnershipAuthorityError::ConflictingImmutableIdentity {
        existing: Box::new(existing),
    }
}

fn require_invocation(
    conn: &sqlite::Connection,
    invocation_uuid: &str,
    owner: bool,
) -> Result<(), OwnershipAuthorityError> {
    if invocation_row_id(conn, invocation_uuid)?.is_some() {
        return Ok(());
    }
    if owner {
        Err(OwnershipAuthorityError::OwnerInvocationNotFound(
            invocation_uuid.to_string(),
        ))
    } else {
        Err(OwnershipAuthorityError::InvocationNotFound(
            invocation_uuid.to_string(),
        ))
    }
}

fn invocation_row_id(
    conn: &sqlite::Connection,
    invocation_uuid: &str,
) -> Result<Option<i64>, OwnershipAuthorityError> {
    conn.query_row(
        "SELECT id FROM invocations WHERE invocation_uuid = ?1",
        sqlite::params![invocation_uuid],
        |row| row.get(0),
    )
    .optional()
    .map_err(persistence("read invocation identity"))
}

fn recursive_descendant_depth(
    conn: &sqlite::Connection,
    invocation_id: i64,
    owner_id: i64,
) -> Result<Option<u32>, OwnershipAuthorityError> {
    conn.query_row(
        "WITH RECURSIVE descendants(id, depth) AS (
             SELECT id, 1
             FROM invocations
             WHERE parent_invocation_id = ?1
             UNION ALL
             SELECT child.id, parent.depth + 1
             FROM invocations AS child
             JOIN descendants AS parent ON child.parent_invocation_id = parent.id
         )
         SELECT depth FROM descendants WHERE id = ?2",
        sqlite::params![invocation_id, owner_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(persistence("read recursive invocation ownership"))
}

fn persistence(context: &'static str) -> impl FnOnce(sqlite::Error) -> OwnershipAuthorityError {
    move |error| persistence_message(format!("{context}: {error}"))
}

fn persistence_owned(context: String) -> impl FnOnce(sqlite::Error) -> OwnershipAuthorityError {
    move |error| persistence_message(format!("{context}: {error}"))
}

fn persistence_message(message: impl Into<String>) -> OwnershipAuthorityError {
    OwnershipAuthorityError::Persistence(message.into())
}
