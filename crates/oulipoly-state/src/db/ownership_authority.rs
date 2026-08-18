//! Additive ownership-authority persistence and projection vocabulary.
//!
//! ## Declared roles
//!
//! `accessor`, `formatter`, `mapper`, `orchestration`, `predicate`, `validator`

use super::{InvocationStatus, RusqliteOptionalExtension, StateDb, sqlite};
use crate::mailbox::{
    COMPLETION_CONTINUITY_GENESIS_DIGEST, CompletionContinuityHead,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionContinuityRecoveryState {
    Ready,
    OperatorRecoveryRequired { unproven_obligation_count: i64 },
}

enum CompletionOwnerAuthorization {
    Running,
    TerminalExactReplay(CompletionObligationExpectation),
}

pub(super) struct CompletionAuthoritySummary {
    pub(super) obligation_count: i64,
    pub(super) continuity_count: i64,
}

pub(super) struct CompletionMaterializationExpectation {
    pub(super) materialized_count: i64,
    pub(super) authority_ordinal: i64,
    pub(super) sidecar_generation: String,
    pub(super) continuity_digest: String,
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
    pub fn register_completion_event_with_authority(
        &mut self,
        authority: &super::CompletionRegistrationAuthority,
        admission_id: &str,
        registration: CompletionEventRegistrationInput<'_>,
    ) -> Result<CompletionEventRegistrationResult, String> {
        self.register_completion_event_with_obligation_on(
            Some(authority),
            false,
            admission_id,
            registration,
            || {},
            || {},
        )
    }

    /// Materialize a hash-identical State admission without creating new authority.
    pub fn repair_admitted_completion_event(
        &mut self,
        admission_id: &str,
        registration: CompletionEventRegistrationInput<'_>,
    ) -> Result<CompletionEventRegistrationResult, String> {
        self.register_completion_event_with_obligation_on(
            None,
            true,
            admission_id,
            registration,
            || {},
            || {},
        )
    }

    #[cfg(test)]
    pub fn register_completion_event_with_obligation(
        &mut self,
        admission_id: &str,
        registration: CompletionEventRegistrationInput<'_>,
    ) -> Result<CompletionEventRegistrationResult, String> {
        self.register_completion_event_with_obligation_on(
            None,
            false,
            admission_id,
            registration,
            || {},
            || {},
        )
    }

    fn register_completion_event_with_obligation_on<BeforeCommit, AfterCommit>(
        &mut self,
        authority: Option<&super::CompletionRegistrationAuthority>,
        admitted_replay_only: bool,
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
                "Completion event registration requires a stable, single-link local state database identity".to_string()
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
        require_completion_continuity_registration_ready(&tx)?;
        if admitted_replay_only {
            require_exact_admitted_completion_replay(
                &tx,
                &admission_id,
                owner_invocation_uuid,
                owner_session_id,
                registration.event_id,
            )?;
        } else if let Some(authority) = authority {
            validate_completion_registration_actor(
                &tx,
                authority,
                owner_invocation_uuid,
                owner_session_id,
            )?;
        }
        let owner_authorization = completion_owner_authorization(
            &tx,
            owner_invocation_uuid,
            owner_session_id,
            registration.event_id,
            &admission_id,
        )?;
        let sidecar_authority = crate::mailbox::MailboxAuthorityFence::acquire(&sidecar_path)
            .map_err(|error| error.to_string())?;
        let state_head = completion_continuity_head_on(&tx).map_err(|error| error.to_string())?;
        let mut mailbox = if state_head.is_none() {
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
                if state_head.is_none() {
                    error
                } else {
                    format!(
                        "process_integrity: invocation {owner_invocation_uuid} cannot fence admitted completion authority: {error}"
                    )
                }
            })?;
        let sidecar_generation = sidecar_fence.sidecar_generation().map_err(|error| {
            if state_head.is_none() {
                error
            } else {
                format!(
                    "process_integrity: invocation {owner_invocation_uuid} has invalid admitted completion authority: {error}"
                )
            }
        })?;
        let sidecar_head = sidecar_fence.completion_continuity_head().map_err(|error| {
            format!(
                "process_integrity: invocation {owner_invocation_uuid} cannot read completion continuity authority: {error}"
            )
        })?;
        let obligation = CompletionObligationAdmission {
            admission_id: &admission_id,
            invocation_uuid: owner_invocation_uuid,
            event_id: registration.event_id,
            owner_invocation_uuid,
            owner_session_id,
            expected_sidecar_generation: &sidecar_generation,
        };
        owner_authorization.validate_observed_generation(&obligation)?;
        let replay_continuity = if state_head != sidecar_head {
            completion_continuity_by_admission_on(&tx, &admission_id)
                .map_err(|error| error.to_string())?
        } else {
            None
        };
        validate_completion_continuity_alignment(
            state_head.as_ref(),
            sidecar_head.as_ref(),
            replay_continuity.as_ref(),
            &obligation,
        )?;
        sidecar_fence.preflight_completion_event_registration(&registration)?;
        let (_, continuity) =
            record_completion_obligation_with_continuity_on(&tx, obligation, state_head.as_ref())
                .map_err(|error| error.to_string())?;
        before_state_commit();
        tx.commit()
            .map_err(|error| format!("Failed to commit completion admission: {error}"))?;
        after_state_commit();
        sidecar_fence.register_completion_event(registration, &continuity)
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

    pub(super) fn first_completion_obligation_for_invocation_on(
        conn: &rusqlite::Connection,
        invocation_uuid: &str,
    ) -> Result<Option<CompletionObligationExpectation>, OwnershipAuthorityError> {
        validate_nonempty(invocation_uuid, "invocation_uuid")?;
        conn.query_row(
            &format!(
                "SELECT {COMPLETION_OBLIGATION_COLUMNS}
                 FROM invocation_completion_obligations
                 WHERE invocation_uuid = ?1
                 ORDER BY admitted_at, admission_id
                 LIMIT 1"
            ),
            sqlite::params![invocation_uuid],
            map_completion_obligation_row,
        )
        .optional()
        .map_err(persistence("read first completion obligation"))
    }

    pub(super) fn completion_authority_summary_on(
        conn: &rusqlite::Connection,
        invocation_uuid: &str,
    ) -> Result<Option<CompletionAuthoritySummary>, OwnershipAuthorityError> {
        validate_nonempty(invocation_uuid, "invocation_uuid")?;
        conn.query_row(
            "SELECT obligation_count, continuity_count
             FROM invocation_completion_authority_summary
             WHERE invocation_uuid = ?1",
            sqlite::params![invocation_uuid],
            |row| {
                Ok(CompletionAuthoritySummary {
                    obligation_count: row.get(0)?,
                    continuity_count: row.get(1)?,
                })
            },
        )
        .optional()
        .map_err(persistence("read completion authority summary"))
    }

    pub(super) fn completion_materialization_expectation_on(
        conn: &rusqlite::Connection,
        invocation_uuid: &str,
    ) -> Result<Option<CompletionMaterializationExpectation>, OwnershipAuthorityError> {
        validate_nonempty(invocation_uuid, "invocation_uuid")?;
        conn.query_row(
            "SELECT materialized_count, authority_ordinal, sidecar_generation, continuity_digest
             FROM invocation_completion_materialization_summary
             WHERE invocation_uuid = ?1",
            sqlite::params![invocation_uuid],
            |row| {
                Ok(CompletionMaterializationExpectation {
                    materialized_count: row.get(0)?,
                    authority_ordinal: row.get(1)?,
                    sidecar_generation: row.get(2)?,
                    continuity_digest: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(persistence("read completion materialization expectation"))
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

    pub fn completion_continuity_recovery_state(
        &self,
    ) -> Result<CompletionContinuityRecoveryState, OwnershipAuthorityError> {
        completion_continuity_recovery_state_on(&self.conn)
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

fn record_completion_obligation_with_continuity_on(
    tx: &rusqlite::Transaction<'_>,
    input: CompletionObligationAdmission<'_>,
    state_head: Option<&CompletionContinuityHead>,
) -> Result<
    (
        CompletionObligationAdmissionResult,
        CompletionContinuityHead,
    ),
    OwnershipAuthorityError,
> {
    let result = record_completion_obligation_on(tx, input)?;
    let continuity = match &result {
        CompletionObligationAdmissionResult::Recorded(expectation) => {
            let continuity = next_completion_continuity(state_head, expectation);
            append_completion_continuity_on(tx, &continuity)?;
            continuity
        }
        CompletionObligationAdmissionResult::Replay(expectation) => {
            let continuity = completion_continuity_by_admission_on(tx, &expectation.admission_id)?
                .ok_or_else(|| {
                    persistence_message(format!(
                        "completion obligation {} has no continuity admission",
                        expectation.admission_id
                    ))
                })?;
            if completion_continuity_matches_expectation(&continuity, expectation) {
                continuity
            } else {
                return Err(persistence_message(format!(
                    "completion obligation {} continuity identity mismatch",
                    expectation.admission_id
                )));
            }
        }
    };
    Ok((result, continuity))
}

fn next_completion_continuity(
    state_head: Option<&CompletionContinuityHead>,
    expectation: &CompletionObligationExpectation,
) -> CompletionContinuityHead {
    let authority_ordinal = state_head.map_or(1, |head| head.authority_ordinal + 1);
    let previous_continuity_digest = state_head.map_or_else(
        || COMPLETION_CONTINUITY_GENESIS_DIGEST.to_string(),
        |head| head.continuity_digest.clone(),
    );
    let continuity_digest =
        completion_continuity_digest(authority_ordinal, &previous_continuity_digest, expectation);
    CompletionContinuityHead {
        authority_ordinal,
        admission_id: expectation.admission_id.clone(),
        sidecar_generation: expectation.expected_sidecar_generation.clone(),
        invocation_uuid: expectation.invocation_uuid.clone(),
        event_id: expectation.event_id.clone(),
        owner_invocation_uuid: expectation.owner_invocation_uuid.clone(),
        owner_session_id: expectation.owner_session_id.clone(),
        previous_continuity_digest,
        continuity_digest,
    }
}

fn completion_continuity_digest(
    authority_ordinal: i64,
    previous_continuity_digest: &str,
    expectation: &CompletionObligationExpectation,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"oulipoly-completion-continuity-v1");
    digest.update(authority_ordinal.to_be_bytes());
    for field in [
        previous_continuity_digest,
        expectation.expected_sidecar_generation.as_str(),
        expectation.admission_id.as_str(),
        expectation.invocation_uuid.as_str(),
        expectation.event_id.as_str(),
        expectation.owner_invocation_uuid.as_str(),
        expectation.owner_session_id.as_str(),
    ] {
        let field_length = u64::try_from(field.len()).expect("continuity field length fits u64");
        digest.update(field_length.to_be_bytes());
        digest.update(field.as_bytes());
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn append_completion_continuity_on(
    conn: &sqlite::Connection,
    continuity: &CompletionContinuityHead,
) -> Result<(), OwnershipAuthorityError> {
    conn.execute(
        "INSERT INTO invocation_completion_continuity (
            authority_ordinal, admission_id, expected_sidecar_generation,
            invocation_uuid, event_id, owner_invocation_uuid, owner_session_id,
            previous_continuity_digest, continuity_digest
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        sqlite::params![
            continuity.authority_ordinal,
            continuity.admission_id,
            continuity.sidecar_generation,
            continuity.invocation_uuid,
            continuity.event_id,
            continuity.owner_invocation_uuid,
            continuity.owner_session_id,
            continuity.previous_continuity_digest,
            continuity.continuity_digest,
        ],
    )
    .map(|_| ())
    .map_err(persistence("append completion continuity"))
}

pub(super) fn completion_continuity_head_on(
    conn: &sqlite::Connection,
) -> Result<Option<CompletionContinuityHead>, OwnershipAuthorityError> {
    #[cfg(test)]
    COMPLETION_CONTINUITY_HEAD_QUERIES.with(|count| count.set(count.get() + 1));
    conn.query_row(
        "SELECT authority_ordinal, admission_id, expected_sidecar_generation,
                invocation_uuid, event_id, owner_invocation_uuid, owner_session_id,
                previous_continuity_digest, continuity_digest
         FROM invocation_completion_continuity
         ORDER BY authority_ordinal DESC
         LIMIT 1",
        [],
        map_completion_continuity_head,
    )
    .optional()
    .map_err(persistence("read completion continuity head"))
}

fn completion_continuity_by_admission_on(
    conn: &sqlite::Connection,
    admission_id: &str,
) -> Result<Option<CompletionContinuityHead>, OwnershipAuthorityError> {
    conn.query_row(
        "SELECT authority_ordinal, admission_id, expected_sidecar_generation,
                invocation_uuid, event_id, owner_invocation_uuid, owner_session_id,
                previous_continuity_digest, continuity_digest
         FROM invocation_completion_continuity
         WHERE admission_id = ?1",
        sqlite::params![admission_id],
        map_completion_continuity_head,
    )
    .optional()
    .map_err(persistence("read completion continuity admission"))
}

fn map_completion_continuity_head(
    row: &sqlite::Row<'_>,
) -> sqlite::Result<CompletionContinuityHead> {
    Ok(CompletionContinuityHead {
        authority_ordinal: row.get(0)?,
        admission_id: row.get(1)?,
        sidecar_generation: row.get(2)?,
        invocation_uuid: row.get(3)?,
        event_id: row.get(4)?,
        owner_invocation_uuid: row.get(5)?,
        owner_session_id: row.get(6)?,
        previous_continuity_digest: row.get(7)?,
        continuity_digest: row.get(8)?,
    })
}

fn validate_completion_continuity_alignment(
    state_head: Option<&CompletionContinuityHead>,
    sidecar_head: Option<&CompletionContinuityHead>,
    replay_continuity: Option<&CompletionContinuityHead>,
    admission: &CompletionObligationAdmission<'_>,
) -> Result<(), String> {
    let generation_matches = state_head
        .into_iter()
        .chain(sidecar_head)
        .chain(replay_continuity)
        .all(|head| head.sidecar_generation == admission.expected_sidecar_generation);
    if generation_matches && state_head == sidecar_head {
        return Ok(());
    }
    if generation_matches
        && replay_continuity.is_some_and(|head| {
            completion_continuity_is_one_ahead(head, sidecar_head)
                && completion_continuity_matches_admission(head, admission)
        })
    {
        return Ok(());
    }
    Err(format!(
        "process_integrity: invocation {} cannot admit completion authority because state and sidecar continuity heads do not match: observed_generation={}, state={}, sidecar={}",
        admission.owner_invocation_uuid,
        admission.expected_sidecar_generation,
        format_completion_continuity_head(state_head),
        format_completion_continuity_head(sidecar_head),
    ))
}

fn completion_continuity_is_one_ahead(
    state_head: &CompletionContinuityHead,
    sidecar_head: Option<&CompletionContinuityHead>,
) -> bool {
    let sidecar_ordinal = sidecar_head.map_or(0, |head| head.authority_ordinal);
    let sidecar_digest = sidecar_head.map_or(COMPLETION_CONTINUITY_GENESIS_DIGEST, |head| {
        head.continuity_digest.as_str()
    });
    state_head.authority_ordinal == sidecar_ordinal + 1
        && state_head.previous_continuity_digest == sidecar_digest
        && sidecar_head.is_none_or(|head| head.sidecar_generation == state_head.sidecar_generation)
}

fn completion_continuity_matches_admission(
    continuity: &CompletionContinuityHead,
    admission: &CompletionObligationAdmission<'_>,
) -> bool {
    continuity.admission_id == admission.admission_id
        && continuity.sidecar_generation == admission.expected_sidecar_generation
        && continuity.invocation_uuid == admission.invocation_uuid
        && continuity.event_id == admission.event_id
        && continuity.owner_invocation_uuid == admission.owner_invocation_uuid
        && continuity.owner_session_id == admission.owner_session_id
}

fn completion_continuity_matches_expectation(
    continuity: &CompletionContinuityHead,
    expectation: &CompletionObligationExpectation,
) -> bool {
    continuity.admission_id == expectation.admission_id
        && continuity.sidecar_generation == expectation.expected_sidecar_generation
        && continuity.invocation_uuid == expectation.invocation_uuid
        && continuity.event_id == expectation.event_id
        && continuity.owner_invocation_uuid == expectation.owner_invocation_uuid
        && continuity.owner_session_id == expectation.owner_session_id
}

fn format_completion_continuity_head(head: Option<&CompletionContinuityHead>) -> String {
    head.map_or_else(
        || "none".to_string(),
        |head| {
            format!(
                "generation {generation}, ordinal {ordinal}, digest {digest}",
                generation = head.sidecar_generation,
                ordinal = head.authority_ordinal,
                digest = head.continuity_digest,
            )
        },
    )
}

pub(super) fn require_completion_continuity_registration_ready(
    conn: &sqlite::Connection,
) -> Result<(), String> {
    match completion_continuity_recovery_state_on(conn).map_err(|error| error.to_string())? {
        CompletionContinuityRecoveryState::Ready => Ok(()),
        CompletionContinuityRecoveryState::OperatorRecoveryRequired {
            unproven_obligation_count,
        } => Err(format!(
            "process_integrity: completion_continuity_recovery=operator_recovery_required; {unproven_obligation_count} schema-14 completion obligation(s) lack exact continuity proof; run `agents migrate --rebuild`"
        )),
    }
}

fn completion_continuity_recovery_state_on(
    conn: &sqlite::Connection,
) -> Result<CompletionContinuityRecoveryState, OwnershipAuthorityError> {
    let recovery: Option<(String, i64)> = conn
        .query_row(
            "SELECT recovery_state, unproven_obligation_count
             FROM invocation_completion_continuity_recovery
             WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(persistence("read completion continuity recovery state"))?;
    match recovery {
        None => Ok(CompletionContinuityRecoveryState::Ready),
        Some((state, unproven_obligation_count))
            if state == "operator_recovery_required" && unproven_obligation_count > 0 =>
        {
            Ok(
                CompletionContinuityRecoveryState::OperatorRecoveryRequired {
                    unproven_obligation_count,
                },
            )
        }
        Some((state, unproven_obligation_count)) => Err(persistence_message(format!(
            "invalid completion continuity recovery state: state={state}, unproven_obligation_count={unproven_obligation_count}"
        ))),
    }
}

#[cfg(test)]
thread_local! {
    static COMPLETION_CONTINUITY_HEAD_QUERIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(super) fn reset_completion_continuity_head_query_count() {
    COMPLETION_CONTINUITY_HEAD_QUERIES.with(|count| count.set(0));
}

#[cfg(test)]
pub(super) fn completion_continuity_head_query_count() -> usize {
    COMPLETION_CONTINUITY_HEAD_QUERIES.with(std::cell::Cell::get)
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

impl CompletionOwnerAuthorization {
    fn validate_observed_generation(
        &self,
        admission: &CompletionObligationAdmission<'_>,
    ) -> Result<(), String> {
        match self {
            Self::Running => Ok(()),
            Self::TerminalExactReplay(existing)
                if completion_obligation_matches(existing, admission) =>
            {
                Ok(())
            }
            Self::TerminalExactReplay(_) => Err(format!(
                "process_integrity: terminal invocation {} cannot replay completion admission {} because the retained sidecar generation changed",
                admission.owner_invocation_uuid, admission.admission_id
            )),
        }
    }
}

fn completion_owner_authorization(
    conn: &sqlite::Connection,
    invocation_uuid: &str,
    owner_session_id: &str,
    event_id: &str,
    admission_id: &str,
) -> Result<CompletionOwnerAuthorization, String> {
    let status = completion_owner_status(conn, invocation_uuid)?;
    if status == InvocationStatus::Running {
        return Ok(CompletionOwnerAuthorization::Running);
    }
    let existing = completion_obligation_by_admission_id(conn, admission_id)
        .map_err(|error| error.to_string())?
        .filter(|existing| {
            existing.invocation_uuid == invocation_uuid
                && existing.event_id == event_id
                && existing.owner_invocation_uuid == invocation_uuid
                && existing.owner_session_id == owner_session_id
        })
        .ok_or_else(|| terminal_owner_new_admission_error(invocation_uuid))?;
    completion_continuity_by_admission_on(conn, admission_id)
        .map_err(|error| error.to_string())?
        .filter(|continuity| completion_continuity_matches_expectation(continuity, &existing))
        .ok_or_else(|| terminal_owner_new_admission_error(invocation_uuid))?;
    Ok(CompletionOwnerAuthorization::TerminalExactReplay(existing))
}

fn require_exact_admitted_completion_replay(
    conn: &sqlite::Connection,
    admission_id: &str,
    owner_invocation_uuid: &str,
    owner_session_id: &str,
    event_id: &str,
) -> Result<(), String> {
    let expectation = completion_obligation_by_admission_id(conn, admission_id)
        .map_err(|error| error.to_string())?
        .filter(|expectation| {
            expectation.invocation_uuid == owner_invocation_uuid
                && expectation.event_id == event_id
                && expectation.owner_invocation_uuid == owner_invocation_uuid
                && expectation.owner_session_id == owner_session_id
        })
        .ok_or_else(|| {
            format!(
                "process_integrity: completion repair requires an exact admitted replay for event {event_id}"
            )
        })?;
    completion_continuity_by_admission_on(conn, admission_id)
        .map_err(|error| error.to_string())?
        .filter(|continuity| completion_continuity_matches_expectation(continuity, &expectation))
        .map(|_| ())
        .ok_or_else(|| {
            format!(
                "process_integrity: completion repair requires exact State continuity for event {event_id}"
            )
        })
}

fn validate_completion_registration_actor(
    conn: &sqlite::Connection,
    authority: &super::CompletionRegistrationAuthority,
    invocation_uuid: &str,
    owner_session_id: &str,
) -> Result<(), String> {
    let binding: Option<(Option<String>, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT completion_registration_capability_digest,
                    provider_session_id,
                    session_id
             FROM invocations
             WHERE invocation_uuid = ?1",
            sqlite::params![invocation_uuid],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| {
            format!("Failed to resolve completion registration actor authority: {error}")
        })?;
    let Some((Some(expected_digest), provider_session_id, session_id)) = binding else {
        return Err(format!(
            "process_integrity: invocation {invocation_uuid} has no caller-bound completion registration authority"
        ));
    };
    let observed_digest = authority.digest();
    if !constant_time_text_eq(&expected_digest, &observed_digest) {
        return Err(format!(
            "process_integrity: completion registration actor is not authorized for invocation {invocation_uuid}"
        ));
    }
    let authoritative_session = provider_session_id.or(session_id).ok_or_else(|| {
        format!(
            "process_integrity: invocation {invocation_uuid} has no authoritative session binding for completion registration"
        )
    })?;
    if authoritative_session != owner_session_id {
        return Err(format!(
            "process_integrity: completion registration session {owner_session_id} is not the authoritative session for invocation {invocation_uuid}"
        ));
    }
    Ok(())
}

fn constant_time_text_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn completion_owner_status(
    conn: &sqlite::Connection,
    invocation_uuid: &str,
) -> Result<InvocationStatus, String> {
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
    InvocationStatus::from_str(&status).ok_or_else(|| {
        format!("Completion listener invocation {invocation_uuid} has invalid status {status}")
    })
}

fn terminal_owner_new_admission_error(invocation_uuid: &str) -> String {
    format!(
        "Completion listener invocation {invocation_uuid} is not running and registration is not an exact admitted replay"
    )
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
            "Completion event registration requires a stable, single-link local state database identity"
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
    fn completion_registration_accepts_a_stable_relative_state_path() {
        let current_directory = std::env::current_dir().unwrap();
        let repository_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let directory = tempfile::tempdir_in(repository_root.parent().unwrap()).unwrap();
        let mut relative_path = std::path::PathBuf::from("..");
        for _ in current_directory
            .strip_prefix(repository_root)
            .unwrap()
            .components()
        {
            relative_path.push("..");
        }
        relative_path.push(directory.path().file_name().unwrap());
        relative_path.push("state.db");

        let mut state = StateDb::open(&relative_path).unwrap();
        state
            .start_invocation(&InvocationStart {
                invocation_uuid: INVOCATION_UUID.to_string(),
                model_name: "age299-s2".to_string(),
                provider_name: "test-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();

        assert_eq!(state.path(), std::fs::canonicalize(&relative_path).unwrap());
        assert!(state.path().is_absolute());
        state
            .register_completion_event_with_obligation(
                "age299-s2-relative-path-admission",
                registration(),
            )
            .unwrap();
        assert_eq!(
            state
                .completion_obligations_for_invocation(INVOCATION_UUID)
                .unwrap()
                .len(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn completion_registration_accepts_a_stable_state_file_symlink_alias() {
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
        alias_state
            .register_completion_event_with_obligation(
                "age299-s2-symlink-admission",
                registration(),
            )
            .unwrap();

        assert_eq!(
            alias_state
                .completion_obligations_for_invocation(INVOCATION_UUID)
                .unwrap()
                .len(),
            1
        );
        assert!(MailboxDb::path_for_state_db(&state_path).exists());
        assert!(!MailboxDb::path_for_state_db(&alias_path).exists());
    }

    #[cfg(unix)]
    #[test]
    fn completion_registration_accepts_a_stable_parent_directory_symlink_alias() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let state_directory = directory.path().join("state-directory");
        let alias_directory = directory.path().join("state-directory-alias");
        std::fs::create_dir(&state_directory).unwrap();
        symlink(&state_directory, &alias_directory).unwrap();
        let state_path = alias_directory.join("state.db");
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
            .register_completion_event_with_obligation(
                "age299-s2-parent-symlink-admission",
                registration(),
            )
            .unwrap();

        assert_eq!(
            state
                .completion_obligations_for_invocation(INVOCATION_UUID)
                .unwrap()
                .len(),
            1
        );
        assert!(MailboxDb::path_for_state_db(&state_directory.join("state.db")).exists());
        assert_eq!(
            std::fs::canonicalize(MailboxDb::path_for_state_db(&state_path)).unwrap(),
            std::fs::canonicalize(MailboxDb::path_for_state_db(
                &state_directory.join("state.db")
            ))
            .unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn completion_authority_rejects_a_retargeted_parent_directory_alias() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let first_directory = directory.path().join("first");
        let second_directory = directory.path().join("second");
        let alias_directory = directory.path().join("current");
        std::fs::create_dir(&first_directory).unwrap();
        std::fs::create_dir(&second_directory).unwrap();
        symlink(&first_directory, &alias_directory).unwrap();
        let mut state = StateDb::open(&alias_directory.join("state.db")).unwrap();
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
                "age299-s2-retarget-admission",
                registration(),
            )
            .unwrap();
        std::fs::remove_file(&alias_directory).unwrap();
        symlink(&second_directory, &alias_directory).unwrap();

        let error = state
            .finalize_invocation(invocation_row_id, true, 0, None, None)
            .unwrap_err();

        assert!(error.contains("process_integrity"), "{error}");
        assert!(error.contains("retained canonical identity"), "{error}");
        assert_eq!(
            state
                .get_invocation_by_uuid(INVOCATION_UUID)
                .unwrap()
                .unwrap()
                .status,
            InvocationStatus::Running
        );
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
            "Completion event registration requires a stable, single-link local state database identity"
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
        assert!(error.contains("retained canonical identity"), "{error}");
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

        let error = completion_owner_status(&state.conn, INVOCATION_UUID).unwrap_err();

        assert_eq!(
            error,
            format!("Completion listener invocation {INVOCATION_UUID} does not exist")
        );
    }

    #[test]
    fn terminal_owner_cannot_create_a_new_completion_admission_or_sidecar() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let sidecar_path = MailboxDb::path_for_state_db(&state_path);
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
            .finalize_invocation(
                invocation_row_id,
                false,
                1,
                Some("test_failure"),
                Some("terminal before completion admission"),
            )
            .unwrap();

        let error = state
            .register_completion_event_with_obligation(
                "age299-s2-terminal-new-admission",
                registration(),
            )
            .unwrap_err();

        assert!(error.contains("not an exact admitted replay"), "{error}");
        assert!(!sidecar_path.exists());
        assert!(
            state
                .completion_obligations_for_invocation(INVOCATION_UUID)
                .unwrap()
                .is_empty()
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
        let mut writer_state = StateDb::open(&state_path).unwrap();
        let finalizer_state = StateDb::open(&state_path).unwrap();
        let writer = std::thread::spawn(move || {
            writer_state
                .register_completion_event_with_obligation_on(
                    None,
                    false,
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
        let finalizer = std::thread::spawn(move || {
            finalize_tx
                .send(finalizer_state.finalize_invocation(invocation_row_id, true, 0, None, None))
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

        std::fs::File::create(&sidecar_path).unwrap();
        let empty_error = state
            .register_completion_event_with_obligation(
                "age299-s2-other-invocation-admission",
                second_registration,
            )
            .unwrap_err();
        assert!(empty_error.contains("process_integrity"), "{empty_error}");
        assert!(
            empty_error.contains("sidecar is unavailable")
                || empty_error.contains("invalid admitted completion authority")
                || empty_error.contains("completion continuity authority"),
            "{empty_error}"
        );
        std::fs::remove_file(&sidecar_path).unwrap();

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
    fn mature_terminal_and_running_history_retains_authority_with_bounded_head_queries() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let sidecar_path = MailboxDb::path_for_state_db(&state_path);
        let mut state = StateDb::open(&state_path).unwrap();
        let mut invocation_ids = Vec::new();
        for index in 0..64 {
            let invocation_uuid = format!("10000000-0000-4000-8000-{index:012}");
            let event_id = format!("age299-s2-mature-event-{index}");
            let session_id = format!("age299-s2-mature-session-{index}");
            let admission_id = format!("age299-s2-mature-admission-{index}");
            let invocation_id = state
                .start_invocation(&InvocationStart {
                    invocation_uuid: invocation_uuid.clone(),
                    model_name: "age299-s2".to_string(),
                    provider_name: "test-provider".to_string(),
                    provider_index: 0,
                    parent_invocation_id: None,
                })
                .unwrap();
            state
                .register_completion_event_with_obligation(
                    &admission_id,
                    CompletionEventRegistrationInput {
                        event_id: &event_id,
                        delivery_mode: "async",
                        owner_session_id: Some(&session_id),
                        owner_invocation_uuid: Some(&invocation_uuid),
                        state_dir: "/tmp/age299-s2-mature-state",
                        meta_path: "/tmp/age299-s2-mature-meta",
                        log_path: "/tmp/age299-s2-mature-log",
                        rc_path: "/tmp/age299-s2-mature-rc",
                    },
                )
                .unwrap();
            invocation_ids.push(invocation_id);
        }
        for invocation_id in invocation_ids.into_iter().take(32) {
            state
                .finalize_invocation(invocation_id, true, 0, None, None)
                .unwrap();
        }
        COMPLETION_CONTINUITY_HEAD_QUERIES.with(|count| count.set(0));
        crate::mailbox::reset_completion_continuity_head_query_count();
        state
            .start_invocation(&InvocationStart {
                invocation_uuid: THIRD_INVOCATION_UUID.to_string(),
                model_name: "age299-s2".to_string(),
                provider_name: "test-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        state
            .register_completion_event_with_obligation(
                "age299-s2-post-mature-admission",
                registration_for(
                    "age299-s2-post-mature-event",
                    "age299-s2-post-mature-session",
                    THIRD_INVOCATION_UUID,
                ),
            )
            .unwrap();

        assert_eq!(
            COMPLETION_CONTINUITY_HEAD_QUERIES.with(std::cell::Cell::get),
            1
        );
        assert_eq!(crate::mailbox::completion_continuity_head_query_count(), 1);
        let state_plan = state
            .raw_connection()
            .query_row(
                "EXPLAIN QUERY PLAN
                 SELECT authority_ordinal
                 FROM invocation_completion_continuity
                 ORDER BY authority_ordinal DESC
                 LIMIT 1",
                [],
                |row| row.get::<_, String>(3),
            )
            .unwrap();
        assert!(
            state_plan.contains("invocation_completion_continuity"),
            "{state_plan}"
        );
        let sidecar = MailboxDb::open(&sidecar_path).unwrap();
        let sidecar_plan = sidecar
            .connection()
            .query_row(
                "EXPLAIN QUERY PLAN
                 SELECT authority_ordinal
                 FROM completion_authority_continuity
                 ORDER BY authority_ordinal DESC
                 LIMIT 1",
                [],
                |row| row.get::<_, String>(3),
            )
            .unwrap();
        assert!(
            sidecar_plan.contains("completion_authority_continuity"),
            "{sidecar_plan}"
        );
        drop(sidecar);
        let obligations = all_completion_obligations_on(state.raw_connection()).unwrap();
        assert_eq!(obligations.len(), 65);

        let retained_generation = MailboxDb::open(&sidecar_path)
            .unwrap()
            .sidecar_generation()
            .unwrap();
        std::fs::remove_file(&sidecar_path).unwrap();
        let distinct_registration = registration_for(
            "age299-s2-after-terminal-history-event",
            "age299-s2-after-terminal-history-session",
            THIRD_INVOCATION_UUID,
        );
        let missing_error = state
            .register_completion_event_with_obligation(
                "age299-s2-after-terminal-history-admission",
                distinct_registration,
            )
            .unwrap_err();
        assert!(
            missing_error.contains("process_integrity"),
            "{missing_error}"
        );
        assert!(!sidecar_path.exists());
        let replacement_generation = MailboxDb::open(&sidecar_path)
            .unwrap()
            .sidecar_generation()
            .unwrap();
        assert_ne!(replacement_generation, retained_generation);
        let replacement_error = state
            .register_completion_event_with_obligation(
                "age299-s2-after-terminal-history-admission",
                distinct_registration,
            )
            .unwrap_err();
        assert!(
            replacement_error.contains("continuity heads do not match"),
            "{replacement_error}"
        );
        assert_eq!(
            all_completion_obligations_on(state.raw_connection())
                .unwrap()
                .len(),
            65
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
        assert!(error.contains("continuity heads do not match"), "{error}");
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
            distinct_error.contains("continuity heads do not match"),
            "{distinct_error}"
        );
        assert_eq!(
            state
                .completion_obligations_for_invocation(INVOCATION_UUID)
                .unwrap()
                .len(),
            1
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
                changed_error.contains("continuity heads do not match"),
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
        let state_continuity: (i64, String) = state
            .raw_connection()
            .query_row(
                "SELECT authority_ordinal, continuity_digest
                 FROM invocation_completion_continuity",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let sidecar = MailboxDb::open(&sidecar_path).unwrap();
        let sidecar_continuity: (i64, String) = sidecar
            .connection()
            .query_row(
                "SELECT authority_ordinal, continuity_digest
                 FROM completion_authority_continuity",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state_continuity, sidecar_continuity);
    }

    #[test]
    fn exact_ordered_replays_repair_a_multi_row_same_generation_rollback() {
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
        let pre_admission_sidecar = std::fs::read(&sidecar_path).unwrap();
        let first = registration_for("age299-s2-rollback-first", SESSION_ID, INVOCATION_UUID);
        let second = registration_for("age299-s2-rollback-second", SESSION_ID, INVOCATION_UUID);

        state
            .register_completion_event_with_obligation("age299-s2-rollback-first", first)
            .unwrap();
        state
            .register_completion_event_with_obligation("age299-s2-rollback-second", second)
            .unwrap();
        assert_eq!(
            state
                .raw_connection()
                .query_row(
                    "SELECT COUNT(*) FROM invocation_completion_continuity",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );

        std::fs::remove_file(&sidecar_path).unwrap();
        std::fs::write(&sidecar_path, pre_admission_sidecar).unwrap();
        let rolled_back = MailboxDb::open(&sidecar_path).unwrap();
        assert_eq!(
            rolled_back
                .connection()
                .query_row(
                    "SELECT COUNT(*) FROM completion_authority_continuity",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        drop(rolled_back);

        let out_of_order_error = state
            .register_completion_event_with_obligation("age299-s2-rollback-second", second)
            .unwrap_err();
        assert!(
            out_of_order_error.contains("continuity heads do not match"),
            "{out_of_order_error}"
        );
        let new_admission_error = state
            .repair_admitted_completion_event(
                "age299-s2-rollback-third",
                registration_for("age299-s2-rollback-third", SESSION_ID, INVOCATION_UUID),
            )
            .unwrap_err();
        assert!(
            new_admission_error.contains("requires an exact admitted replay"),
            "{new_admission_error}"
        );

        state
            .repair_admitted_completion_event("age299-s2-rollback-first", first)
            .unwrap();
        state
            .repair_admitted_completion_event("age299-s2-rollback-second", second)
            .unwrap();

        let sidecar = MailboxDb::open(&sidecar_path).unwrap();
        let state_head: (i64, String) = state
            .raw_connection()
            .query_row(
                "SELECT authority_ordinal, continuity_digest
                 FROM invocation_completion_continuity
                 ORDER BY authority_ordinal DESC
                 LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let sidecar_head: (i64, String) = sidecar
            .connection()
            .query_row(
                "SELECT authority_ordinal, continuity_digest
                 FROM completion_authority_continuity
                 ORDER BY authority_ordinal DESC
                 LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state_head, sidecar_head);
        assert_eq!(sidecar_head.0, 2);
        assert!(
            sidecar
                .contains_completion_obligation(first.event_id, INVOCATION_UUID, SESSION_ID)
                .unwrap()
        );
        assert!(
            sidecar
                .contains_completion_obligation(second.event_id, INVOCATION_UUID, SESSION_ID)
                .unwrap()
        );
    }

    #[test]
    fn terminal_owner_exact_replay_repairs_only_its_admitted_partial_state() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let sidecar_path = MailboxDb::path_for_state_db(&state_path);
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
        drop(MailboxDb::open(&sidecar_path).unwrap());
        let fault_connection = rusqlite::Connection::open(&sidecar_path).unwrap();
        fault_connection
            .execute_batch(
                "CREATE TRIGGER reject_terminal_repair
                 BEFORE INSERT ON completion_event
                 BEGIN
                   SELECT RAISE(ABORT, 'forced terminal repair interruption');
                 END;",
            )
            .unwrap();
        drop(fault_connection);

        let first_error = state
            .register_completion_event_with_obligation("age299-s2-terminal-partial", registration())
            .unwrap_err();
        assert!(
            first_error.contains("forced terminal repair interruption"),
            "{first_error}"
        );
        state
            .finalize_invocation(
                invocation_row_id,
                false,
                1,
                Some("test_failure"),
                Some("terminalized after state-first partial admission"),
            )
            .unwrap();
        assert_eq!(
            state
                .get_invocation_by_uuid(INVOCATION_UUID)
                .unwrap()
                .unwrap()
                .status,
            InvocationStatus::Failed
        );

        let interrupted_repair = state
            .register_completion_event_with_obligation("age299-s2-terminal-partial", registration())
            .unwrap_err();
        assert!(
            interrupted_repair.contains("forced terminal repair interruption"),
            "{interrupted_repair}"
        );
        assert_eq!(
            state
                .raw_connection()
                .query_row(
                    "SELECT COUNT(*) FROM invocation_completion_continuity",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        let admitted_state_head: (i64, String, String) = state
            .raw_connection()
            .query_row(
                "SELECT authority_ordinal, admission_id, continuity_digest
                 FROM invocation_completion_continuity",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        rusqlite::Connection::open(&sidecar_path)
            .unwrap()
            .execute_batch("DROP TRIGGER reject_terminal_repair;")
            .unwrap();

        let original = registration();
        for (caller_admission_id, changed) in [
            (
                "age299-s2-terminal-partial",
                CompletionEventRegistrationInput {
                    delivery_mode: "sync",
                    ..original
                },
            ),
            (
                "age299-s2-terminal-partial",
                CompletionEventRegistrationInput {
                    state_dir: "/tmp/age299-s2-terminal-changed-state",
                    ..original
                },
            ),
            ("age299-s2-terminal-distinct-admission", original),
            (
                "age299-s2-terminal-distinct-event-admission",
                registration_for(
                    "age299-s2-terminal-distinct-event",
                    SESSION_ID,
                    INVOCATION_UUID,
                ),
            ),
        ] {
            let error = state
                .register_completion_event_with_obligation(caller_admission_id, changed)
                .unwrap_err();
            assert!(error.contains("not an exact admitted replay"), "{error}");
            let sidecar = MailboxDb::open(&sidecar_path).unwrap();
            assert!(sidecar.completion_event(EVENT_ID).unwrap().is_none());
            assert!(
                sidecar
                    .completion_event("age299-s2-terminal-distinct-event")
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                sidecar
                    .connection()
                    .query_row(
                        "SELECT COUNT(*) FROM completion_authority_continuity",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                0
            );
        }

        state
            .register_completion_event_with_obligation("age299-s2-terminal-partial", registration())
            .unwrap();
        let sidecar = MailboxDb::open(&sidecar_path).unwrap();
        let repaired_sidecar_head: (i64, String, String) = sidecar
            .connection()
            .query_row(
                "SELECT authority_ordinal, admission_id, continuity_digest
                 FROM completion_authority_continuity",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(repaired_sidecar_head, admitted_state_head);
        drop(sidecar);

        state
            .register_completion_event_with_obligation("age299-s2-terminal-partial", registration())
            .unwrap();
        assert_eq!(
            MailboxDb::open(&sidecar_path)
                .unwrap()
                .connection()
                .query_row(
                    "SELECT COUNT(*) FROM completion_authority_continuity",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );

        state
            .start_invocation(&InvocationStart {
                invocation_uuid: SECOND_INVOCATION_UUID.to_string(),
                model_name: "age299-s2".to_string(),
                provider_name: "test-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        state
            .register_completion_event_with_obligation(
                "age299-s2-after-terminal-repair",
                registration_for(
                    "age299-s2-after-terminal-repair-event",
                    "age299-s2-after-terminal-repair-session",
                    SECOND_INVOCATION_UUID,
                ),
            )
            .unwrap();
        assert_eq!(
            state
                .raw_connection()
                .query_row(
                    "SELECT COUNT(*) FROM invocation_completion_continuity",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
        assert_eq!(
            MailboxDb::open(&sidecar_path)
                .unwrap()
                .connection()
                .query_row(
                    "SELECT COUNT(*) FROM completion_authority_continuity",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
    }

    #[test]
    fn completion_continuity_and_listener_identity_are_immutable() {
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
        state
            .register_completion_event_with_obligation(
                "age299-s2-immutable-continuity-admission",
                registration(),
            )
            .unwrap();

        for statement in [
            "UPDATE invocation_completion_continuity
             SET continuity_digest = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'",
            "DELETE FROM invocation_completion_continuity",
        ] {
            let error = state.raw_connection().execute(statement, []).unwrap_err();
            assert!(error.to_string().contains("append-only"), "{error}");
        }

        let sidecar = rusqlite::Connection::open(&sidecar_path).unwrap();
        for statement in [
            "UPDATE completion_authority_continuity
             SET continuity_digest = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'",
            "DELETE FROM completion_authority_continuity",
        ] {
            let error = sidecar.execute(statement, []).unwrap_err();
            assert!(error.to_string().contains("append-only"), "{error}");
        }
        let update_error = sidecar
            .execute(
                "UPDATE completion_event_listener
                 SET owner_invocation_uuid = 'changed-owner'",
                [],
            )
            .unwrap_err();
        assert!(
            update_error
                .to_string()
                .contains("listener identity is immutable"),
            "{update_error}"
        );
        let delete_error = sidecar
            .execute("DELETE FROM completion_event_listener", [])
            .unwrap_err();
        assert!(
            delete_error
                .to_string()
                .contains("listener continuity identity is immutable"),
            "{delete_error}"
        );
        assert_eq!(
            sidecar
                .execute("UPDATE completion_event_listener SET active = 0", [])
                .unwrap(),
            1
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
