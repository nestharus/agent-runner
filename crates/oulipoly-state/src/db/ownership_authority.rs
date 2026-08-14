//! Additive ownership-authority persistence and projection vocabulary.
//!
//! ## Declared roles
//!
//! `accessor`, `formatter`, `mapper`, `orchestration`, `predicate`, `validator`

use super::{InvocationStatus, RusqliteOptionalExtension, StateDb, sqlite};
use crate::mailbox::{
    CompletionEventRegistrationInput, CompletionEventRegistrationResult, MailboxDb,
    validate_completion_event_registration,
};
use sha2::{Digest, Sha256};
use std::fmt;

const COMPLETION_OBLIGATION_COLUMNS: &str = concat!(
    "admission_id, invocation_uuid, event_id, owner_invocation_uuid, ",
    "owner_session_id, expected_sidecar_generation, admitted_at"
);

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
    #[cfg(test)]
    pub(crate) fn record_completion_obligation(
        &self,
        input: CompletionObligationAdmission<'_>,
    ) -> Result<CompletionObligationAdmissionResult, OwnershipAuthorityError> {
        validate_completion_obligation(&input)?;
        let tx =
            sqlite::Transaction::new_unchecked(&self.conn, sqlite::TransactionBehavior::Immediate)
                .map_err(persistence("begin completion obligation admission"))?;

        let result = record_completion_obligation_on(&tx, input)?;
        tx.commit()
            .map_err(persistence("commit completion obligation admission"))?;
        Ok(result)
    }

    pub fn register_completion_event_with_obligation(
        &mut self,
        admission_id: &str,
        registration: CompletionEventRegistrationInput<'_>,
    ) -> Result<CompletionEventRegistrationResult, String> {
        self.register_completion_event_with_obligation_on(admission_id, registration, || {}, || {})
    }

    fn register_completion_event_with_obligation_on<BeforeCommit, AfterCommit>(
        &mut self,
        admission_id: &str,
        registration: CompletionEventRegistrationInput<'_>,
        before_state_commit: BeforeCommit,
        after_state_commit: AfterCommit,
    ) -> Result<CompletionEventRegistrationResult, String>
    where
        BeforeCommit: FnOnce(),
        AfterCommit: FnOnce(),
    {
        let completion_authority_state_path =
            self.completion_authority_state_path().ok_or_else(|| {
                "Completion event registration requires an absolute, non-symlink, single-link local state database".to_string()
            })?;
        validate_completion_event_registration(&registration)?;
        let owner_invocation_uuid = registration.owner_invocation_uuid.ok_or_else(|| {
            "Completion event owner session and invocation are both required".to_string()
        })?;
        let owner_session_id = registration.owner_session_id.ok_or_else(|| {
            "Completion event owner session and invocation are both required".to_string()
        })?;
        validate_nonempty(admission_id, "admission_id").map_err(|error| error.to_string())?;
        let admission_id = completion_registration_admission_id(admission_id, &registration);
        let sidecar_path = MailboxDb::path_for_state_db(completion_authority_state_path);
        let tx = self
            .conn
            .transaction_with_behavior(sqlite::TransactionBehavior::Immediate)
            .map_err(|error| {
                format!("Failed to begin completion admission transaction: {error}")
            })?;
        require_running_owner(&tx, owner_invocation_uuid)?;
        let sidecar_authority = crate::mailbox::MailboxAuthorityFence::acquire(&sidecar_path)
            .map_err(|error| error.to_string())?;
        let existing_obligations =
            active_completion_obligations_on(&tx).map_err(|error| error.to_string())?;
        let mut mailbox = if existing_obligations.is_empty() {
            MailboxDb::open_with_authority(&sidecar_authority)?
        } else {
            MailboxDb::open_existing_for_completion_authority(&sidecar_authority).map_err(|error| {
                format!(
                    "process_integrity: invocation {owner_invocation_uuid} has admitted completion authority but the sidecar is unavailable: {error}"
                )
            })?
        };
        let sidecar_fence = mailbox
            .begin_completion_authority_fence()
            .map_err(|error| {
                if existing_obligations.is_empty() {
                    error
                } else {
                    format!(
                        "process_integrity: invocation {owner_invocation_uuid} cannot fence admitted completion authority: {error}"
                    )
                }
            })?;
        let sidecar_generation = sidecar_fence.sidecar_generation().map_err(|error| {
            if existing_obligations.is_empty() {
                error
            } else {
                format!(
                    "process_integrity: invocation {owner_invocation_uuid} has invalid admitted completion authority: {error}"
                )
            }
        })?;
        if let Some(mismatch) = existing_obligations
            .iter()
            .find(|obligation| obligation.expected_sidecar_generation != sidecar_generation)
        {
            return Err(format!(
                "process_integrity: invocation {owner_invocation_uuid} completion sidecar generation changed before admission {} for active invocation {}: expected {}, observed {sidecar_generation}",
                mismatch.admission_id,
                mismatch.invocation_uuid,
                mismatch.expected_sidecar_generation
            ));
        }
        validate_existing_completion_listeners(
            &sidecar_fence,
            &existing_obligations,
            &admission_id,
            owner_invocation_uuid,
        )?;
        sidecar_fence.preflight_completion_event_registration(&registration)?;
        record_completion_obligation_on(
            &tx,
            CompletionObligationAdmission {
                admission_id: &admission_id,
                invocation_uuid: owner_invocation_uuid,
                event_id: registration.event_id,
                owner_invocation_uuid,
                owner_session_id,
                expected_sidecar_generation: &sidecar_generation,
            },
        )
        .map_err(|error| error.to_string())?;
        before_state_commit();
        tx.commit()
            .map_err(|error| format!("Failed to commit completion admission: {error}"))?;
        after_state_commit();
        sidecar_fence.register_completion_event(registration)
    }

    pub(super) fn completion_obligations_for_invocation_on(
        conn: &rusqlite::Connection,
        invocation_uuid: &str,
    ) -> Result<Vec<CompletionObligationExpectation>, OwnershipAuthorityError> {
        validate_nonempty(invocation_uuid, "invocation_uuid")?;
        let mut statement = conn
            .prepare(&format!(
                "SELECT {COMPLETION_OBLIGATION_COLUMNS}
                 FROM invocation_completion_obligations
                 WHERE invocation_uuid = ?1
                 ORDER BY admitted_at, admission_id"
            ))
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
        Self::completion_obligations_for_invocation_on(&self.conn, invocation_uuid)
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

fn completion_registration_admission_id(
    caller_admission_id: &str,
    registration: &CompletionEventRegistrationInput<'_>,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"oulipoly-completion-registration-v1");
    for field in [
        caller_admission_id,
        registration.event_id,
        registration.delivery_mode,
        registration.owner_session_id.unwrap_or_default(),
        registration.owner_invocation_uuid.unwrap_or_default(),
        registration.state_dir,
        registration.meta_path,
        registration.log_path,
        registration.rc_path,
    ] {
        let field_length = u64::try_from(field.len()).expect("registration field length fits u64");
        digest.update(field_length.to_be_bytes());
        digest.update(field.as_bytes());
    }
    let hash = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("{caller_admission_id}:registration-v1:{hash}")
}

fn record_completion_obligation_on(
    tx: &rusqlite::Transaction<'_>,
    input: CompletionObligationAdmission<'_>,
) -> Result<CompletionObligationAdmissionResult, OwnershipAuthorityError> {
    validate_completion_obligation(&input)?;
    if let Some(existing) = completion_obligation_by_admission_id(tx, input.admission_id)? {
        return replay_or_conflict(existing, &input);
    }
    if let Some(existing) =
        completion_obligation_by_listener(tx, input.event_id, input.owner_invocation_uuid)?
    {
        return Err(conflicting_identity(existing));
    }
    if let Some(existing) = completion_obligation_by_event_id(tx, input.event_id)?
        && existing.expected_sidecar_generation != input.expected_sidecar_generation
    {
        return Err(conflicting_identity(existing));
    }
    require_invocation(tx, input.invocation_uuid, false)?;
    require_invocation(tx, input.owner_invocation_uuid, true)?;

    let admitted_at = StateDb::current_rfc3339_timestamp();
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
    completion_obligation_by_admission_id(tx, input.admission_id)?
        .map(CompletionObligationAdmissionResult::Recorded)
        .ok_or_else(|| persistence_message("completion obligation disappeared after insert"))
}

#[cfg(test)]
fn all_completion_obligations_on(
    conn: &rusqlite::Connection,
) -> Result<Vec<CompletionObligationExpectation>, OwnershipAuthorityError> {
    let mut statement = conn
        .prepare(
            "SELECT obligation.admission_id, obligation.invocation_uuid,
                    obligation.event_id, obligation.owner_invocation_uuid,
                    obligation.owner_session_id, obligation.expected_sidecar_generation,
                    obligation.admitted_at
             FROM invocation_completion_obligations AS obligation
             ORDER BY obligation.admitted_at, obligation.admission_id",
        )
        .map_err(persistence("prepare completion obligation authority query"))?;
    let rows = statement
        .query_map([], map_completion_obligation_row)
        .map_err(persistence("query completion obligation authority"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(persistence("read completion obligation authority"))
}

fn active_completion_obligations_on(
    conn: &rusqlite::Connection,
) -> Result<Vec<CompletionObligationExpectation>, OwnershipAuthorityError> {
    let mut statement = conn
        .prepare(
            "SELECT obligation.admission_id, obligation.invocation_uuid,
                    obligation.event_id, obligation.owner_invocation_uuid,
                    obligation.owner_session_id, obligation.expected_sidecar_generation,
                    obligation.admitted_at
             FROM invocation_completion_obligations AS obligation
             JOIN invocations AS invocation
               ON invocation.invocation_uuid = obligation.invocation_uuid
             WHERE invocation.status = 'running'
             ORDER BY obligation.admitted_at, obligation.admission_id",
        )
        .map_err(persistence(
            "prepare active completion obligation authority query",
        ))?;
    let rows = statement
        .query_map([], map_completion_obligation_row)
        .map_err(persistence("query active completion obligation authority"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(persistence("read active completion obligation authority"))
}

fn validate_existing_completion_listeners(
    sidecar: &crate::mailbox::CompletionAuthorityFence<'_>,
    obligations: &[CompletionObligationExpectation],
    replay_admission_id: &str,
    owner_invocation_uuid: &str,
) -> Result<(), String> {
    for obligation in obligations {
        if obligation.admission_id == replay_admission_id {
            continue;
        }
        let present = sidecar.contains_completion_obligation(
            &obligation.event_id,
            &obligation.owner_invocation_uuid,
            &obligation.owner_session_id,
        )
        .map_err(|error| {
            format!(
                "process_integrity: invocation {owner_invocation_uuid} cannot validate completion obligation {} before new admission: {error}",
                obligation.admission_id,
            )
        })?;
        if !present {
            return Err(format!(
                "process_integrity: invocation {owner_invocation_uuid} cannot admit new completion authority because obligation {} owned by {} expects event {} in mailbox sidecar generation {}, but the event listener is absent",
                obligation.admission_id,
                obligation.owner_invocation_uuid,
                obligation.event_id,
                obligation.expected_sidecar_generation,
            ));
        }
    }
    Ok(())
}

fn require_running_owner(conn: &sqlite::Connection, invocation_uuid: &str) -> Result<(), String> {
    let status: Option<String> = conn
        .query_row(
            "SELECT status FROM invocations WHERE invocation_uuid = ?1",
            sqlite::params![invocation_uuid],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| format!("Failed to lock completion listener owner: {error}"))?;
    let Some(status) = status else {
        return Err(format!(
            "Completion listener invocation {invocation_uuid} does not exist"
        ));
    };
    let status = status
        .parse::<InvocationStatus>()
        .map_err(|error| format!("Failed to lock completion listener owner: {error}"))?;
    if status == InvocationStatus::Running {
        Ok(())
    } else {
        Err(format!(
            "Completion listener invocation {invocation_uuid} is not running"
        ))
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
    if value.is_empty() || value.trim() != value {
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
        CompletionObligationIdentity::AdmissionId,
        admission_id,
    )
}

fn completion_obligation_by_event_id(
    conn: &sqlite::Connection,
    event_id: &str,
) -> Result<Option<CompletionObligationExpectation>, OwnershipAuthorityError> {
    completion_obligation_by_identity(conn, CompletionObligationIdentity::EventId, event_id)
}

fn completion_obligation_by_listener(
    conn: &sqlite::Connection,
    event_id: &str,
    owner_invocation_uuid: &str,
) -> Result<Option<CompletionObligationExpectation>, OwnershipAuthorityError> {
    let sql = format!(
        "SELECT {COMPLETION_OBLIGATION_COLUMNS}
         FROM invocation_completion_obligations
         WHERE event_id = ?1 AND owner_invocation_uuid = ?2"
    );
    conn.query_row(
        &sql,
        sqlite::params![event_id, owner_invocation_uuid],
        map_completion_obligation_row,
    )
    .optional()
    .map_err(persistence(
        "read completion obligation by listener identity",
    ))
}

enum CompletionObligationIdentity {
    AdmissionId,
    EventId,
}

fn completion_obligation_by_identity(
    conn: &sqlite::Connection,
    identity_kind: CompletionObligationIdentity,
    identity: &str,
) -> Result<Option<CompletionObligationExpectation>, OwnershipAuthorityError> {
    let (predicate, context) = match identity_kind {
        CompletionObligationIdentity::AdmissionId => ("WHERE admission_id = ?1", "admission ID"),
        CompletionObligationIdentity::EventId => (
            "WHERE event_id = ?1 ORDER BY admitted_at, admission_id LIMIT 1",
            "event ID",
        ),
    };
    let sql = format!(
        "SELECT {COMPLETION_OBLIGATION_COLUMNS}
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InvocationStart;
    use crate::mailbox::CompletionEventTriggerInput;
    use std::sync::mpsc;
    use std::time::Duration;
    const INVOCATION_UUID: &str = "11111111-1111-4111-8111-111111111111";
    const SECOND_INVOCATION_UUID: &str = "22222222-2222-4222-8222-222222222222";
    const THIRD_INVOCATION_UUID: &str = "33333333-3333-4333-8333-333333333333";
    const EVENT_ID: &str = "age299-s2-barrier-event";
    const SESSION_ID: &str = "age299-s2-barrier-session";

    #[test]
    fn completion_registration_requires_a_durable_state_database() {
        let mut state = StateDb::open(std::path::Path::new(":memory:")).unwrap();

        let error = state
            .register_completion_event_with_obligation("age299-s2-memory-admission", registration())
            .unwrap_err();

        assert_eq!(
            error,
            "Completion event registration requires an absolute, non-symlink, single-link local state database"
        );
    }

    #[test]
    fn writable_state_open_rejects_sqlite_uri_paths() {
        let directory = tempfile::tempdir().unwrap();
        let uris = [
            "file:age299-s2-memory?mode=memory&cache=shared".to_string(),
            format!(
                "file:{}?mode=rwc",
                directory.path().join("uri-state.db").display()
            ),
        ];

        for uri in uris {
            let error = StateDb::open(std::path::Path::new(&uri))
                .err()
                .expect("SQLite URI writable open must fail");

            assert_eq!(
                error,
                "State DB writable open does not accept SQLite URI paths"
            );
        }
    }

    #[test]
    fn completion_registration_rejects_a_relative_state_path() {
        let current_directory = std::env::current_dir().unwrap();
        let directory = tempfile::tempdir_in(&current_directory).unwrap();
        let relative_path = directory
            .path()
            .strip_prefix(&current_directory)
            .unwrap()
            .join("state.db");

        let mut state = StateDb::open(&relative_path).unwrap();

        assert_eq!(state.path(), std::fs::canonicalize(&relative_path).unwrap());
        assert!(state.path().is_absolute());
        let error = state
            .register_completion_event_with_obligation(
                "age299-s2-relative-path-admission",
                registration(),
            )
            .unwrap_err();
        assert_eq!(
            error,
            "Completion event registration requires an absolute, non-symlink, single-link local state database"
        );
        assert!(
            state
                .completion_obligations_for_invocation(INVOCATION_UUID)
                .unwrap()
                .is_empty()
        );
    }

    #[cfg(unix)]
    #[test]
    fn completion_registration_rejects_a_state_file_symlink_alias() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let alias_path = directory.path().join("state-alias.db");
        let state = StateDb::open(&state_path).unwrap();
        state
            .start_invocation(&InvocationStart {
                invocation_uuid: INVOCATION_UUID.to_string(),
                model_name: "age299-s2".to_string(),
                provider_name: "test-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        drop(state);
        symlink(&state_path, &alias_path).unwrap();

        let mut alias_state = StateDb::open(&alias_path).unwrap();
        assert_eq!(
            alias_state.path(),
            std::fs::canonicalize(&state_path).unwrap()
        );
        let error = alias_state
            .register_completion_event_with_obligation(
                "age299-s2-symlink-admission",
                registration(),
            )
            .unwrap_err();

        assert_eq!(
            error,
            "Completion event registration requires an absolute, non-symlink, single-link local state database"
        );
        assert!(
            alias_state
                .completion_obligations_for_invocation(INVOCATION_UUID)
                .unwrap()
                .is_empty()
        );
        assert!(!MailboxDb::path_for_state_db(&state_path).exists());
        assert!(!MailboxDb::path_for_state_db(&alias_path).exists());
    }

    #[cfg(unix)]
    #[test]
    fn completion_registration_rejects_a_parent_directory_symlink_alias() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let state_directory = directory.path().join("state-directory");
        let alias_directory = directory.path().join("state-directory-alias");
        std::fs::create_dir(&state_directory).unwrap();
        symlink(&state_directory, &alias_directory).unwrap();
        let state_path = alias_directory.join("state.db");
        let mut state = StateDb::open(&state_path).unwrap();

        let error = state
            .register_completion_event_with_obligation(
                "age299-s2-parent-symlink-admission",
                registration(),
            )
            .unwrap_err();

        assert_eq!(
            error,
            "Completion event registration requires an absolute, non-symlink, single-link local state database"
        );
        assert!(
            state
                .completion_obligations_for_invocation(INVOCATION_UUID)
                .unwrap()
                .is_empty()
        );
        assert!(!MailboxDb::path_for_state_db(&state_path).exists());
    }

    #[cfg(unix)]
    #[test]
    fn completion_registration_rejects_a_hard_linked_state_file() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let alias_path = directory.path().join("state-hard-link.db");
        let mut state = StateDb::open(&state_path).unwrap();
        state
            .start_invocation(&InvocationStart {
                invocation_uuid: INVOCATION_UUID.to_string(),
                model_name: "age299-s2".to_string(),
                provider_name: "test-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        std::fs::hard_link(&state_path, &alias_path).unwrap();

        let error = state
            .register_completion_event_with_obligation(
                "age299-s2-hard-link-admission",
                registration(),
            )
            .unwrap_err();

        assert_eq!(
            error,
            "Completion event registration requires an absolute, non-symlink, single-link local state database"
        );
        assert!(
            state
                .completion_obligations_for_invocation(INVOCATION_UUID)
                .unwrap()
                .is_empty()
        );
        assert!(!MailboxDb::path_for_state_db(&state_path).exists());
        assert!(!MailboxDb::path_for_state_db(&alias_path).exists());
    }

    #[cfg(unix)]
    #[test]
    fn obligation_bearing_finalization_rejects_same_path_state_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let displaced_path = directory.path().join("state.displaced.db");
        let mut state = StateDb::open(&state_path).unwrap();
        let invocation_row_id = state
            .start_invocation(&InvocationStart {
                invocation_uuid: INVOCATION_UUID.to_string(),
                model_name: "age299-s2".to_string(),
                provider_name: "test-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        state
            .register_completion_event_with_obligation(
                "age299-s2-state-replacement-admission",
                registration(),
            )
            .unwrap();
        std::fs::rename(&state_path, &displaced_path).unwrap();
        std::fs::File::create(&state_path).unwrap();

        let error = state
            .finalize_invocation(invocation_row_id, true, 0, None, None)
            .unwrap_err();

        assert!(error.contains("process_integrity"), "{error}");
        assert!(error.contains(INVOCATION_UUID), "{error}");
        assert!(error.contains("no longer has"), "{error}");
        assert_eq!(
            state
                .get_invocation_by_uuid(INVOCATION_UUID)
                .unwrap()
                .unwrap()
                .status,
            crate::InvocationStatus::Running
        );
    }

    #[test]
    fn completion_registration_reports_an_unknown_owner_invocation() {
        let state = StateDb::open(std::path::Path::new(":memory:")).unwrap();

        let error = require_running_owner(&state.conn, INVOCATION_UUID).unwrap_err();

        assert_eq!(
            error,
            format!("Completion listener invocation {INVOCATION_UUID} does not exist")
        );
    }

    #[test]
    fn invalid_registration_never_commits_an_unreplayable_obligation() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let sidecar_path = MailboxDb::path_for_state_db(&state_path);
        let mut state = StateDb::open(&state_path).unwrap();
        state
            .start_invocation(&InvocationStart {
                invocation_uuid: INVOCATION_UUID.to_string(),
                model_name: "age299-s2".to_string(),
                provider_name: "test-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        let valid = registration();

        for invalid in [
            CompletionEventRegistrationInput {
                delivery_mode: "invalid",
                ..valid
            },
            CompletionEventRegistrationInput {
                state_dir: "",
                ..valid
            },
            CompletionEventRegistrationInput {
                meta_path: "",
                ..valid
            },
            CompletionEventRegistrationInput {
                log_path: "",
                ..valid
            },
            CompletionEventRegistrationInput {
                rc_path: "",
                ..valid
            },
        ] {
            state
                .register_completion_event_with_obligation(
                    "age299-s2-invalid-registration",
                    invalid,
                )
                .unwrap_err();
            assert!(
                state
                    .completion_obligations_for_invocation(INVOCATION_UUID)
                    .unwrap()
                    .is_empty()
            );
            assert!(!sidecar_path.exists());
        }

        state
            .register_completion_event_with_obligation("age299-s2-invalid-registration", valid)
            .unwrap();
        assert_eq!(
            state
                .completion_obligations_for_invocation(INVOCATION_UUID)
                .unwrap()
                .len(),
            1
        );
        assert!(
            MailboxDb::open(&sidecar_path)
                .unwrap()
                .contains_completion_obligation(EVENT_ID, INVOCATION_UUID, SESSION_ID)
                .unwrap()
        );
    }

    #[test]
    fn retained_sidecar_conflicts_are_rejected_before_state_admission() {
        const EVENT_CONFLICT: &str = "age299-s2-retained-event-conflict";
        const LISTENER_CONFLICT: &str = "age299-s2-retained-listener-conflict";
        const TRIGGERED_CONFLICT: &str = "age299-s2-retained-triggered-conflict";

        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let sidecar_path = MailboxDb::path_for_state_db(&state_path);
        let mut state = StateDb::open(&state_path).unwrap();
        state
            .start_invocation(&InvocationStart {
                invocation_uuid: INVOCATION_UUID.to_string(),
                model_name: "age299-s2".to_string(),
                provider_name: "test-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();

        let mut sidecar = MailboxDb::open(&sidecar_path).unwrap();
        sidecar
            .register_completion_event(CompletionEventRegistrationInput {
                delivery_mode: "sync",
                ..registration_for(EVENT_CONFLICT, SESSION_ID, INVOCATION_UUID)
            })
            .unwrap();
        sidecar
            .register_completion_event(registration_for(
                LISTENER_CONFLICT,
                "age299-s2-conflicting-session",
                INVOCATION_UUID,
            ))
            .unwrap();
        let triggered_registration = registration_for(
            TRIGGERED_CONFLICT,
            "age299-s2-triggered-session",
            SECOND_INVOCATION_UUID,
        );
        sidecar
            .register_completion_event(triggered_registration)
            .unwrap();
        sidecar
            .trigger_completion_event(CompletionEventTriggerInput {
                event_id: TRIGGERED_CONFLICT,
                payload_json: r#"{"schema_version":2,"handle":"age299-s2-retained-triggered-conflict"}"#,
                state_dir: triggered_registration.state_dir,
                meta_path: triggered_registration.meta_path,
                log_path: triggered_registration.log_path,
                rc_path: triggered_registration.rc_path,
                rc: 0,
                consumed: false,
            })
            .unwrap();
        drop(sidecar);

        for (admission_id, registration, expected_error) in [
            (
                "age299-s2-event-conflict-admission",
                registration_for(EVENT_CONFLICT, SESSION_ID, INVOCATION_UUID),
                "conflicts with its durable identity",
            ),
            (
                "age299-s2-listener-conflict-admission",
                registration_for(LISTENER_CONFLICT, SESSION_ID, INVOCATION_UUID),
                "listener registration conflicts",
            ),
            (
                "age299-s2-triggered-conflict-admission",
                registration_for(TRIGGERED_CONFLICT, SESSION_ID, INVOCATION_UUID),
                "after it was triggered",
            ),
        ] {
            let error = state
                .register_completion_event_with_obligation(admission_id, registration)
                .unwrap_err();
            assert!(error.contains(expected_error), "{error}");
            assert!(
                state
                    .completion_obligations_for_invocation(INVOCATION_UUID)
                    .unwrap()
                    .is_empty()
            );
        }

        let retained = MailboxDb::open(&sidecar_path).unwrap();
        assert_eq!(
            retained
                .completion_event(TRIGGERED_CONFLICT)
                .unwrap()
                .unwrap()
                .state,
            "triggered"
        );
        assert_eq!(
            retained
                .completion_event_listeners(LISTENER_CONFLICT)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn completion_admission_serializes_finalization_through_sidecar_materialization() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let state = StateDb::open(&state_path).unwrap();
        let invocation_row_id = state
            .start_invocation(&InvocationStart {
                invocation_uuid: INVOCATION_UUID.to_string(),
                model_name: "age299-s2".to_string(),
                provider_name: "test-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        drop(state);

        let (admission_reached_tx, admission_reached_rx) = mpsc::channel();
        let (admission_release_tx, admission_release_rx) = mpsc::channel();
        let (state_committed_tx, state_committed_rx) = mpsc::channel();
        let (sidecar_release_tx, sidecar_release_rx) = mpsc::channel();
        let writer_state_path = state_path.clone();
        let writer = std::thread::spawn(move || {
            let mut state = StateDb::open(&writer_state_path).unwrap();
            state
                .register_completion_event_with_obligation_on(
                    "age299-s2-barrier-admission",
                    registration(),
                    || {
                        admission_reached_tx.send(()).unwrap();
                        admission_release_rx.recv().unwrap();
                    },
                    || {
                        state_committed_tx.send(()).unwrap();
                        sidecar_release_rx.recv().unwrap();
                    },
                )
                .unwrap();
        });

        admission_reached_rx.recv().unwrap();
        let (finalize_tx, finalize_rx) = mpsc::channel();
        let finalizer_state_path = state_path.clone();
        let finalizer = std::thread::spawn(move || {
            let state = StateDb::open(&finalizer_state_path).unwrap();
            finalize_tx
                .send(state.finalize_invocation(invocation_row_id, true, 0, None, None))
                .unwrap();
        });
        assert!(
            finalize_rx.recv_timeout(Duration::from_millis(25)).is_err(),
            "finalization must wait behind the completion-admission state writer"
        );

        admission_release_tx.send(()).unwrap();
        state_committed_rx.recv().unwrap();
        assert!(
            finalize_rx.recv_timeout(Duration::from_millis(25)).is_err(),
            "finalization must wait behind sidecar materialization"
        );

        sidecar_release_tx.send(()).unwrap();
        writer.join().unwrap();
        finalize_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        finalizer.join().unwrap();
    }

    #[test]
    fn completion_registration_rejects_a_second_generation_until_authority_returns() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let sidecar_path = MailboxDb::path_for_state_db(&state_path);
        let held_sidecar_path = directory.path().join("pid-identity.held");
        let mut state = StateDb::open(&state_path).unwrap();
        state
            .start_invocation(&InvocationStart {
                invocation_uuid: INVOCATION_UUID.to_string(),
                model_name: "age299-s2".to_string(),
                provider_name: "test-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        state
            .register_completion_event_with_obligation("age299-s2-first-admission", registration())
            .unwrap();
        let generation = MailboxDb::open(&sidecar_path)
            .unwrap()
            .sidecar_generation()
            .unwrap();
        std::fs::rename(&sidecar_path, &held_sidecar_path).unwrap();

        let second_registration = CompletionEventRegistrationInput {
            event_id: "age299-s2-second-event",
            delivery_mode: "async",
            owner_session_id: Some(SESSION_ID),
            owner_invocation_uuid: Some(INVOCATION_UUID),
            state_dir: "/tmp/age299-s2-second-state",
            meta_path: "/tmp/age299-s2-second-meta",
            log_path: "/tmp/age299-s2-second-log",
            rc_path: "/tmp/age299-s2-second-rc",
        };
        let error = state
            .register_completion_event_with_obligation(
                "age299-s2-second-admission",
                second_registration,
            )
            .unwrap_err();

        assert!(error.contains("process_integrity"), "{error}");
        assert!(error.contains(INVOCATION_UUID), "{error}");
        assert!(!sidecar_path.exists());
        assert_eq!(
            state
                .completion_obligations_for_invocation(INVOCATION_UUID)
                .unwrap()
                .len(),
            1
        );

        std::fs::rename(&held_sidecar_path, &sidecar_path).unwrap();
        state
            .register_completion_event_with_obligation(
                "age299-s2-second-admission",
                second_registration,
            )
            .unwrap();
        let obligations = state
            .completion_obligations_for_invocation(INVOCATION_UUID)
            .unwrap();
        assert_eq!(obligations.len(), 2);
        assert!(
            obligations
                .iter()
                .all(|obligation| obligation.expected_sidecar_generation == generation)
        );
    }

    #[test]
    fn completion_registration_joins_authority_across_active_invocations() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let sidecar_path = MailboxDb::path_for_state_db(&state_path);
        let held_sidecar_path = directory.path().join("pid-identity.held");
        let mut state = StateDb::open(&state_path).unwrap();
        for invocation_uuid in [INVOCATION_UUID, SECOND_INVOCATION_UUID] {
            state
                .start_invocation(&InvocationStart {
                    invocation_uuid: invocation_uuid.to_string(),
                    model_name: "age299-s2".to_string(),
                    provider_name: "test-provider".to_string(),
                    provider_index: 0,
                    parent_invocation_id: None,
                })
                .unwrap();
        }
        state
            .register_completion_event_with_obligation("age299-s2-first-admission", registration())
            .unwrap();
        let retained_generation = MailboxDb::open(&sidecar_path)
            .unwrap()
            .sidecar_generation()
            .unwrap();
        std::fs::rename(&sidecar_path, &held_sidecar_path).unwrap();
        let second_registration = CompletionEventRegistrationInput {
            event_id: "age299-s2-other-invocation-event",
            delivery_mode: "async",
            owner_session_id: Some("age299-s2-other-invocation-session"),
            owner_invocation_uuid: Some(SECOND_INVOCATION_UUID),
            state_dir: "/tmp/age299-s2-other-invocation-state",
            meta_path: "/tmp/age299-s2-other-invocation-meta",
            log_path: "/tmp/age299-s2-other-invocation-log",
            rc_path: "/tmp/age299-s2-other-invocation-rc",
        };

        let missing_error = state
            .register_completion_event_with_obligation(
                "age299-s2-other-invocation-admission",
                second_registration,
            )
            .unwrap_err();
        assert!(
            missing_error.contains("process_integrity"),
            "{missing_error}"
        );
        assert!(!sidecar_path.exists());
        assert!(
            state
                .completion_obligations_for_invocation(SECOND_INVOCATION_UUID)
                .unwrap()
                .is_empty()
        );

        let replacement_generation = MailboxDb::open(&sidecar_path)
            .unwrap()
            .sidecar_generation()
            .unwrap();
        assert_ne!(replacement_generation, retained_generation);
        let mismatch_error = state
            .register_completion_event_with_obligation(
                "age299-s2-other-invocation-admission",
                second_registration,
            )
            .unwrap_err();
        assert!(
            mismatch_error.contains(&retained_generation),
            "{mismatch_error}"
        );
        assert!(
            mismatch_error.contains(&replacement_generation),
            "{mismatch_error}"
        );
        assert!(
            state
                .completion_obligations_for_invocation(SECOND_INVOCATION_UUID)
                .unwrap()
                .is_empty()
        );

        std::fs::remove_file(&sidecar_path).unwrap();
        std::fs::rename(&held_sidecar_path, &sidecar_path).unwrap();
        state
            .register_completion_event_with_obligation(
                "age299-s2-other-invocation-admission",
                second_registration,
            )
            .unwrap();
        let obligations = all_completion_obligations_on(state.raw_connection()).unwrap();
        assert_eq!(obligations.len(), 2);
        assert!(
            obligations.iter().all(|obligation| {
                obligation.expected_sidecar_generation == retained_generation
            })
        );
    }

    #[test]
    fn terminal_obligations_remain_recorded_without_gating_new_admission() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let sidecar_path = MailboxDb::path_for_state_db(&state_path);
        let mut state = StateDb::open(&state_path).unwrap();
        let mut invocation_ids = Vec::new();
        for invocation_uuid in [
            INVOCATION_UUID,
            SECOND_INVOCATION_UUID,
            THIRD_INVOCATION_UUID,
        ] {
            invocation_ids.push(
                state
                    .start_invocation(&InvocationStart {
                        invocation_uuid: invocation_uuid.to_string(),
                        model_name: "age299-s2".to_string(),
                        provider_name: "test-provider".to_string(),
                        provider_index: 0,
                        parent_invocation_id: None,
                    })
                    .unwrap(),
            );
        }
        state
            .register_completion_event_with_obligation(
                "age299-s2-terminal-success-admission",
                registration_for(
                    "age299-s2-terminal-success-event",
                    "age299-s2-terminal-success-session",
                    INVOCATION_UUID,
                ),
            )
            .unwrap();
        state
            .register_completion_event_with_obligation(
                "age299-s2-terminal-failure-admission",
                registration_for(
                    "age299-s2-terminal-failure-event",
                    "age299-s2-terminal-failure-session",
                    SECOND_INVOCATION_UUID,
                ),
            )
            .unwrap();
        let retained_generation = MailboxDb::open(&sidecar_path)
            .unwrap()
            .sidecar_generation()
            .unwrap();
        state
            .finalize_invocation(invocation_ids[0], true, 0, None, None)
            .unwrap();
        state
            .finalize_invocation(
                invocation_ids[1],
                false,
                1,
                Some("test_failure"),
                Some("test_failure"),
            )
            .unwrap();
        std::fs::remove_file(&sidecar_path).unwrap();
        let third_registration = registration_for(
            "age299-s2-post-terminal-event",
            "age299-s2-post-terminal-session",
            THIRD_INVOCATION_UUID,
        );

        state
            .register_completion_event_with_obligation(
                "age299-s2-post-terminal-admission",
                third_registration,
            )
            .unwrap();
        let replacement_generation = MailboxDb::open(&sidecar_path)
            .unwrap()
            .sidecar_generation()
            .unwrap();
        assert_ne!(replacement_generation, retained_generation);
        let obligations = all_completion_obligations_on(state.raw_connection()).unwrap();
        assert_eq!(obligations.len(), 3);
        assert!(
            obligations[..2].iter().all(|obligation| {
                obligation.expected_sidecar_generation == retained_generation
            })
        );
        assert_eq!(
            obligations[2].expected_sidecar_generation,
            replacement_generation
        );
    }

    #[test]
    fn completion_registration_rejects_a_stale_same_generation_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let sidecar_path = MailboxDb::path_for_state_db(&state_path);
        let stale_sidecar_path = directory.path().join("pid-identity.stale");
        let retained_sidecar_path = directory.path().join("pid-identity.retained");
        let mut state = StateDb::open(&state_path).unwrap();
        for invocation_uuid in [INVOCATION_UUID, SECOND_INVOCATION_UUID] {
            state
                .start_invocation(&InvocationStart {
                    invocation_uuid: invocation_uuid.to_string(),
                    model_name: "age299-s2".to_string(),
                    provider_name: "test-provider".to_string(),
                    provider_index: 0,
                    parent_invocation_id: None,
                })
                .unwrap();
        }
        let retained_generation = MailboxDb::open(&sidecar_path)
            .unwrap()
            .sidecar_generation()
            .unwrap();
        std::fs::copy(&sidecar_path, &stale_sidecar_path).unwrap();
        state
            .register_completion_event_with_obligation("age299-s2-first-admission", registration())
            .unwrap();
        std::fs::rename(&sidecar_path, &retained_sidecar_path).unwrap();
        std::fs::copy(&stale_sidecar_path, &sidecar_path).unwrap();
        let second_registration = registration_for(
            "age299-s2-stale-snapshot-event",
            "age299-s2-stale-snapshot-session",
            SECOND_INVOCATION_UUID,
        );

        let error = state
            .register_completion_event_with_obligation(
                "age299-s2-stale-snapshot-admission",
                second_registration,
            )
            .unwrap_err();
        assert!(error.contains("event listener is absent"), "{error}");
        assert!(
            state
                .completion_obligations_for_invocation(SECOND_INVOCATION_UUID)
                .unwrap()
                .is_empty()
        );
        let stale_sidecar = MailboxDb::open(&sidecar_path).unwrap();
        assert_eq!(
            stale_sidecar.sidecar_generation().unwrap(),
            retained_generation
        );
        assert!(
            stale_sidecar
                .completion_event(second_registration.event_id)
                .unwrap()
                .is_none()
        );
        drop(stale_sidecar);

        std::fs::remove_file(&sidecar_path).unwrap();
        std::fs::rename(&retained_sidecar_path, &sidecar_path).unwrap();
        state
            .register_completion_event_with_obligation(
                "age299-s2-stale-snapshot-admission",
                second_registration,
            )
            .unwrap();
    }

    #[test]
    fn exact_replay_repairs_its_own_missing_listener_before_new_admission() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let sidecar_path = MailboxDb::path_for_state_db(&state_path);
        let mut state = StateDb::open(&state_path).unwrap();
        state
            .start_invocation(&InvocationStart {
                invocation_uuid: INVOCATION_UUID.to_string(),
                model_name: "age299-s2".to_string(),
                provider_name: "test-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        drop(MailboxDb::open(&sidecar_path).unwrap());
        let fault_connection = rusqlite::Connection::open(&sidecar_path).unwrap();
        fault_connection
            .execute_batch(
                "CREATE TRIGGER reject_completion_registration
                 BEFORE INSERT ON completion_event
                 BEGIN
                   SELECT RAISE(ABORT, 'forced completion registration failure');
                 END;",
            )
            .unwrap();
        drop(fault_connection);

        let first_error = state
            .register_completion_event_with_obligation(
                "age299-s2-partial-admission",
                registration(),
            )
            .unwrap_err();
        assert!(
            first_error.contains("forced completion registration failure"),
            "{first_error}"
        );
        assert_eq!(
            state
                .completion_obligations_for_invocation(INVOCATION_UUID)
                .unwrap()
                .len(),
            1
        );
        let distinct_error = state
            .register_completion_event_with_obligation(
                "age299-s2-distinct-admission",
                registration_for(
                    "age299-s2-distinct-event",
                    "age299-s2-distinct-session",
                    INVOCATION_UUID,
                ),
            )
            .unwrap_err();
        assert!(
            distinct_error.contains("event listener is absent"),
            "{distinct_error}"
        );

        rusqlite::Connection::open(&sidecar_path)
            .unwrap()
            .execute_batch("DROP TRIGGER reject_completion_registration;")
            .unwrap();
        let original = registration();
        for changed in [
            CompletionEventRegistrationInput {
                delivery_mode: "sync",
                ..original
            },
            CompletionEventRegistrationInput {
                state_dir: "/tmp/changed-state",
                ..original
            },
            CompletionEventRegistrationInput {
                meta_path: "/tmp/changed-meta",
                ..original
            },
            CompletionEventRegistrationInput {
                log_path: "/tmp/changed-log",
                ..original
            },
            CompletionEventRegistrationInput {
                rc_path: "/tmp/changed-rc",
                ..original
            },
        ] {
            let changed_error = state
                .register_completion_event_with_obligation("age299-s2-partial-admission", changed)
                .unwrap_err();
            assert!(
                changed_error.contains("event listener is absent"),
                "{changed_error}"
            );
            assert!(
                MailboxDb::open(&sidecar_path)
                    .unwrap()
                    .completion_event(EVENT_ID)
                    .unwrap()
                    .is_none()
            );
        }
        state
            .register_completion_event_with_obligation("age299-s2-partial-admission", original)
            .unwrap();
        assert!(
            MailboxDb::open(&sidecar_path)
                .unwrap()
                .contains_completion_obligation(EVENT_ID, INVOCATION_UUID, SESSION_ID)
                .unwrap()
        );
    }

    fn registration() -> CompletionEventRegistrationInput<'static> {
        registration_for(EVENT_ID, SESSION_ID, INVOCATION_UUID)
    }

    fn registration_for(
        event_id: &'static str,
        session_id: &'static str,
        invocation_uuid: &'static str,
    ) -> CompletionEventRegistrationInput<'static> {
        CompletionEventRegistrationInput {
            event_id,
            delivery_mode: "async",
            owner_session_id: Some(session_id),
            owner_invocation_uuid: Some(invocation_uuid),
            state_dir: "/tmp/age299-s2-barrier-state",
            meta_path: "/tmp/age299-s2-barrier-meta",
            log_path: "/tmp/age299-s2-barrier-log",
            rc_path: "/tmp/age299-s2-barrier-rc",
        }
    }
}
