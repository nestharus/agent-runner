//! Additive ownership-authority persistence and projection vocabulary.
//!
//! ## Declared roles
//!
//! `accessor`, `formatter`, `mapper`, `orchestration`, `predicate`, `validator`

use super::{InvocationStatus, RusqliteOptionalExtension, StateDb, sqlite};
use crate::completion_continuation::AdmittedSourceBinding;
use crate::diagnostic_producer::{
    TransactionAttempt, TransactionPhaseGuard, record_sqlite_failure, record_unacquired_release,
};
use crate::diagnostic_recorder::{
    DiagnosticPhase, PhaseObservation, SpanStart, SqliteDatabaseRole, SqliteEventIdentity,
    SqlitePathClass, SqliteTransactionMode, process_recorder,
};
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
const COMPLETION_SOURCE_CONFLICT_SQL: &str = "SELECT EXISTS(
    SELECT 1 FROM (
        SELECT 1
        FROM invocation_completion_v2_identity
             INDEXED BY idx_invocation_completion_v2_registration
        WHERE registration_id=?1 AND registration_digest<>?5
        UNION ALL
        SELECT 1
        FROM invocation_completion_v2_identity
             INDEXED BY idx_invocation_completion_v2_source
        WHERE domain_id=?2 AND source_id=?3 AND registration_digest<>?5
        UNION ALL
        SELECT 1
        FROM invocation_completion_v2_identity
             INDEXED BY idx_invocation_completion_v2_handle
        WHERE domain_id=?2 AND handle=?4 AND registration_digest<>?5
    ) LIMIT 1
)";
const LEGACY_COMPLETION_ADMISSION_SQL: &str = "SELECT EXISTS(
    SELECT 1 FROM invocation_completion_obligations
         INDEXED BY idx_invocation_completion_obligations_legacy
    WHERE completion_v2_binding IS NULL
)";
const COMPLETION_CONTINUITY_SUFFIX_SQL: &str = "SELECT
    o.completion_v2_binding,o.admission_id,o.event_id,
    o.owner_invocation_uuid,o.owner_session_id
FROM invocation_completion_continuity c
JOIN invocation_completion_obligations o ON o.admission_id=c.admission_id
WHERE c.authority_ordinal>?1 AND o.completion_v2_binding IS NOT NULL
ORDER BY c.authority_ordinal LIMIT ?2";

fn completion_admission_sql(predicate: &str) -> String {
    format!(
        "SELECT o.completion_v2_binding,o.admission_id,o.event_id,
                o.owner_invocation_uuid,o.owner_session_id
         FROM invocation_completion_obligations o
         JOIN invocation_completion_continuity c ON c.admission_id=o.admission_id
         WHERE ({predicate}) AND o.completion_v2_binding IS NOT NULL
         ORDER BY c.authority_ordinal LIMIT 1"
    )
}

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
    /// Admission still uses the original actor capability and continuity ledger.
    /// Source bytes are inserted atomically with that authority, before sidecar IO.
    pub fn register_completion_continuation_with_authority(
        &mut self,
        mutation_authority: crate::InvocationMutationAuthority<'_>,
        authority: &super::CompletionRegistrationAuthority,
        binding: &AdmittedSourceBinding,
    ) -> Result<CompletionEventRegistrationResult, String> {
        self.register_bound_completion(mutation_authority, Some(authority), false, binding)
    }

    /// Recovery uses only committed State bindings, and the existing exact
    /// admitted-repair checks. It cannot obtain fresh registration authority.
    pub fn repair_admitted_completion_continuation(
        &mut self,
        mutation_authority: crate::InvocationMutationAuthority<'_>,
        binding: &AdmittedSourceBinding,
    ) -> Result<CompletionEventRegistrationResult, String> {
        self.register_bound_completion(mutation_authority, None, true, binding)
    }

    /// A single authority scan per driver pass also supplies original-source
    /// membership for late listeners. The full immutable ledger is still decoded
    /// and validated; no accepted/terminal shortcut grants repair authority.
    /// Each successful materialization retains its usual current writer fences.
    pub fn repair_domain_completion_continuations(
        &mut self,
        domain: &str,
    ) -> Result<Vec<AdmittedSourceBinding>, String> {
        let bindings = self.admitted_completion_continuations()?;
        let originals: std::collections::BTreeSet<Vec<u8>> = bindings
            .iter()
            .filter(|binding| !binding.is_late_listener())
            .map(|binding| binding.registration_bytes().to_vec())
            .collect();
        // One live projection connection; no State writer is retained by these
        // observations. Existing exact repair remains the fallback for any gap.
        let projection = self.completion_authority_state_path().and_then(|path| {
            MailboxDb::open_existing_native_authority(&MailboxDb::path_for_state_db(path)).ok()
        });
        // As in the driver, a failed exact repair stays owed on the next pass.
        Ok(bindings
            .into_iter()
            .filter_map(|binding| {
                self.repair_domain_binding(domain, &originals, projection.as_ref(), binding)
                    .ok()
                    .flatten()
            })
            .collect())
    }

    /// Repair only the append-only State suffix not yet projected to the
    /// sidecar, then return only source registrations that still lack accepted
    /// completion evidence. This is the owner driver's bounded hot path.
    ///
    /// The full ledger remains available to explicit audit paths; successful
    /// history is not decoded on owner recovery or retirement passes.
    pub fn repair_pending_domain_completion_continuations(
        &mut self,
        domain: &str,
        supervisor_authority_id: &str,
        suffix_limit: usize,
        source_limit: usize,
    ) -> Result<Vec<AdmittedSourceBinding>, String> {
        if suffix_limit == 0 || source_limit == 0 {
            return Err("completion repair bounds must be positive".into());
        }
        let path = self
            .completion_authority_state_path()
            .ok_or("completion repair requires stable State identity")?;
        let sidecar_path = MailboxDb::path_for_state_db(path);
        let projection = MailboxDb::open_existing_native_authority(&sidecar_path)?;
        let ordinal = projection.completion_continuity_repair_ordinal()?;
        let suffix = self.admitted_completion_continuations_after(ordinal, suffix_limit)?;
        let originals: std::collections::BTreeSet<Vec<u8>> = suffix
            .iter()
            .filter(|binding| !binding.is_late_listener())
            .map(|binding| binding.registration_bytes().to_vec())
            .collect();
        for binding in suffix {
            self.repair_domain_binding(domain, &originals, Some(&projection), binding)?;
        }
        projection.unaccepted_completion_continuations(supervisor_authority_id, source_limit)
    }

    fn repair_domain_binding(
        &mut self,
        domain: &str,
        originals: &std::collections::BTreeSet<Vec<u8>>,
        projection: Option<&MailboxDb>,
        binding: AdmittedSourceBinding,
    ) -> Result<Option<AdmittedSourceBinding>, String> {
        if binding.registration()?.domain_id != domain {
            return Ok(None);
        }
        if binding.is_late_listener()
            && !originals.contains(binding.registration_bytes())
            && !projection
                .map(|sidecar| sidecar.has_original_completion_source(&binding))
                .transpose()?
                .unwrap_or(false)
        {
            return Err("late listener requires original committed v2 source admission".into());
        }
        // Neither terminal nor accepted status suffices. Skip only when the
        // exact current continuity, source, listener and policy projection agree.
        if let (Some(projection), Some(head)) = (
            projection,
            completion_continuity_head_on(&self.conn).map_err(|e| e.to_string())?,
        ) && projection.continuation_projection_matches(&binding, &head)?
        {
            return Ok(Some(binding));
        }
        self.materialize_bound_completion(
            crate::InvocationMutationAuthority::Standalone,
            None,
            true,
            &binding,
        )?;
        Ok(Some(binding))
    }

    fn register_bound_completion(
        &mut self,
        mutation_authority: crate::InvocationMutationAuthority<'_>,
        authority: Option<&super::CompletionRegistrationAuthority>,
        repair: bool,
        binding: &AdmittedSourceBinding,
    ) -> Result<CompletionEventRegistrationResult, String> {
        if binding.is_late_listener() && !self.has_original_admitted_completion_source(binding)? {
            return Err("late listener requires original committed v2 source admission".into());
        }
        self.materialize_bound_completion(mutation_authority, authority, repair, binding)
    }

    fn materialize_bound_completion(
        &mut self,
        mutation_authority: crate::InvocationMutationAuthority<'_>,
        authority: Option<&super::CompletionRegistrationAuthority>,
        repair: bool,
        binding: &AdmittedSourceBinding,
    ) -> Result<CompletionEventRegistrationResult, String> {
        let source = binding.registration()?;
        let paths = source.paths();
        let listener = binding.admission_listener()?;
        self.register_completion_event_with_binding_on(
            mutation_authority,
            authority,
            repair,
            binding.caller_admission_id(),
            CompletionEventRegistrationInput {
                event_id: &source.handle,
                delivery_mode: &source.delivery_mode,
                owner_session_id: Some(&listener.session_id),
                owner_invocation_uuid: Some(&listener.owner_invocation_uuid),
                state_dir: &source.handle_dir,
                meta_path: &paths[0],
                log_path: &paths[1],
                rc_path: &paths[2],
            },
            Some(binding),
            || {},
            || {},
        )
    }

    /// Historical admissions without v2 bindings cannot authorize source-image
    /// recovery. This says nothing about pending delivery or whether a legacy
    /// supervisor can still submit its original completion through notify.
    pub fn has_legacy_completion_admissions(&self) -> Result<bool, String> {
        self.conn
            .query_row(LEGACY_COMPLETION_ADMISSION_SQL, [], |row| row.get(0))
            .map_err(|error| error.to_string())
    }

    /// Enumerate authority, not sidecar projections or uncommitted source files.
    /// Continuity ordinal is the required sidecar repair order after rollback.
    pub fn admitted_completion_continuations(&self) -> Result<Vec<AdmittedSourceBinding>, String> {
        self.access_scope
            .authorize(crate::live_history::STATE_FULL_ADMISSION_LEDGER, None)?;
        let mut statement = self.conn.prepare(
            "SELECT o.completion_v2_binding,o.admission_id,o.event_id,o.owner_invocation_uuid,o.owner_session_id
             FROM invocation_completion_obligations o
             JOIN invocation_completion_continuity c ON c.admission_id=o.admission_id
             WHERE o.completion_v2_binding IS NOT NULL ORDER BY c.authority_ordinal"
        ).map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        rows.map(|row| decode_admitted_completion_row(row.map_err(|e| e.to_string())?))
            .collect()
    }

    fn admitted_completion_continuations_after(
        &self,
        authority_ordinal: i64,
        limit: usize,
    ) -> Result<Vec<AdmittedSourceBinding>, String> {
        let limit = i64::try_from(limit).map_err(|_| "completion repair limit overflow")?;
        let mut statement = self
            .conn
            .prepare(COMPLETION_CONTINUITY_SUFFIX_SQL)
            .map_err(|error| error.to_string())?;
        statement
            .query_map(sqlite::params![authority_ordinal, limit], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(|error| error.to_string())?
            .map(|row| decode_admitted_completion_row(row.map_err(|error| error.to_string())?))
            .collect()
    }

    /// Atomically retire an idle notification generation against State admission.
    /// The caller separately owns native-context and actual child custody checks.
    /// Lock order matches admission: State first, then sidecar. Missing sidecar
    /// projection after a committed admission is pending, never empty inventory.
    pub fn close_idle_completion_continuation_owner(
        &self,
        owner: &crate::mailbox::CompletionDomainOwner,
    ) -> Result<bool, String> {
        let path = self
            .completion_authority_state_path()
            .ok_or("completion retirement requires stable State identity")?;
        // Open the existing projection before taking State: schema/open work must
        // not prolong the State writer reservation. All mutable duties are still
        // read below under State, then revalidated under the sidecar writer.
        let mut mailbox =
            MailboxDb::open_existing_native_authority(&MailboxDb::path_for_state_db(path))?;
        let cancelling = self.cancelling_native_attempts()?;
        for (generation, invocation) in &cancelling {
            if mailbox.native_runtime_in_domain(
                &owner.domain_id,
                &generation.to_string(),
                &invocation.to_string(),
            )? {
                return Ok(false);
            }
        }
        if self.has_pending_native_channel_duty_for_domain(&owner.domain_id)? {
            return Ok(false);
        }
        let tx =
            sqlite::Transaction::new_unchecked(&self.conn, sqlite::TransactionBehavior::Immediate)
                .map_err(|e| e.to_string())?;
        let state_head = completion_continuity_head_on(&tx).map_err(|error| error.to_string())?;
        // Do not perform sidecar work for a changed cancellation set while the
        // State writer is held. The guardian retries from a fresh preflight.
        if self.cancelling_native_attempts()? != cancelling {
            return Ok(false);
        }
        if self.has_pending_native_channel_duty_for_domain(&owner.domain_id)? {
            return Ok(false);
        }
        let closed = mailbox.close_idle_continuation_generation(owner, state_head.as_ref())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(closed)
    }

    /// Read-only exact confirmation. Missing authority is absence, not permission
    /// to replay registration. Identity conflicts never become exact readback.
    pub fn admitted_completion_continuation(
        &self,
        requested: &AdmittedSourceBinding,
    ) -> Result<Option<AdmittedSourceBinding>, String> {
        let source = requested.registration()?;
        if self
            .has_conflicting_admitted_completion_source(&source, requested.registration_digest())?
        {
            return Err("completion continuation immutable identity conflict".into());
        }
        let paths = source.paths();
        let listener = requested.admission_listener()?;
        let registration = CompletionEventRegistrationInput {
            event_id: &source.handle,
            delivery_mode: &source.delivery_mode,
            owner_session_id: Some(&listener.session_id),
            owner_invocation_uuid: Some(&listener.owner_invocation_uuid),
            state_dir: &source.handle_dir,
            meta_path: &paths[0],
            log_path: &paths[1],
            rc_path: &paths[2],
        };
        let admission_id = completion_bound_admission_id(
            requested.caller_admission_id(),
            &registration,
            Some(requested),
        );
        if let Some(admitted) =
            self.admitted_completion_continuation_by_admission_id(&admission_id)?
        {
            return if admitted == *requested {
                Ok(Some(admitted))
            } else {
                Err("completion continuation immutable identity conflict".into())
            };
        }
        Ok(None)
    }

    fn admitted_completion_continuation_by_admission_id(
        &self,
        admission_id: &str,
    ) -> Result<Option<AdmittedSourceBinding>, String> {
        self.admitted_completion_continuation_row("o.admission_id=?1", [admission_id])
    }

    fn has_conflicting_admitted_completion_source(
        &self,
        source: &crate::completion_continuation::SourceRegistration,
        registration_digest: &str,
    ) -> Result<bool, String> {
        self.conn
            .query_row(
                COMPLETION_SOURCE_CONFLICT_SQL,
                sqlite::params![
                    source.registration_id,
                    source.domain_id,
                    source.source_id,
                    source.handle,
                    registration_digest
                ],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())
    }

    fn admitted_completion_continuation_row<P: rusqlite::Params>(
        &self,
        predicate: &str,
        parameters: P,
    ) -> Result<Option<AdmittedSourceBinding>, String> {
        let sql = completion_admission_sql(predicate);
        self.conn
            .query_row(&sql, parameters, |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .optional()
            .map_err(|error| error.to_string())?
            .map(decode_admitted_completion_row)
            .transpose()
    }

    fn has_original_admitted_completion_source(
        &self,
        requested: &AdmittedSourceBinding,
    ) -> Result<bool, String> {
        let source = requested.registration()?;
        let original = source
            .listeners
            .first()
            .ok_or("completion source has no original listener")?;
        let row = self
            .conn
            .query_row(
                "SELECT o.completion_v2_binding,o.admission_id,o.event_id,
                        o.owner_invocation_uuid,o.owner_session_id
                 FROM invocation_completion_obligations o
                 JOIN invocation_completion_continuity c ON c.admission_id=o.admission_id
                 WHERE o.event_id=?1 AND o.owner_invocation_uuid=?2
                   AND o.completion_v2_binding IS NOT NULL",
                sqlite::params![source.handle, original.owner_invocation_uuid],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let Some(row) = row else {
            return Ok(false);
        };
        let admitted = decode_admitted_completion_row(row)?;
        Ok(!admitted.is_late_listener() && admitted.same_source(requested))
    }

    pub fn register_completion_event_with_authority(
        &mut self,
        mutation_authority: crate::InvocationMutationAuthority<'_>,
        authority: &super::CompletionRegistrationAuthority,
        admission_id: &str,
        registration: CompletionEventRegistrationInput<'_>,
    ) -> Result<CompletionEventRegistrationResult, String> {
        self.register_completion_event_with_obligation_on(
            mutation_authority,
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
        mutation_authority: crate::InvocationMutationAuthority<'_>,
        admission_id: &str,
        registration: CompletionEventRegistrationInput<'_>,
    ) -> Result<CompletionEventRegistrationResult, String> {
        self.register_completion_event_with_obligation_on(
            mutation_authority,
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
        mutation_authority: crate::InvocationMutationAuthority<'_>,
        admission_id: &str,
        registration: CompletionEventRegistrationInput<'_>,
    ) -> Result<CompletionEventRegistrationResult, String> {
        self.register_completion_event_with_obligation_on(
            mutation_authority,
            None,
            false,
            admission_id,
            registration,
            || {},
            || {},
        )
    }

    // Mandatory authority accompanies the existing terminal/admission transaction inputs.
    #[allow(clippy::too_many_arguments)]
    fn register_completion_event_with_obligation_on<BeforeCommit, AfterCommit>(
        &mut self,
        mutation_authority: crate::InvocationMutationAuthority<'_>,
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
        self.register_completion_event_with_binding_on(
            mutation_authority,
            authority,
            admitted_replay_only,
            admission_id,
            registration,
            None,
            before_state_commit,
            after_state_commit,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn register_completion_event_with_binding_on<BeforeCommit, AfterCommit>(
        &mut self,
        mutation_authority: crate::InvocationMutationAuthority<'_>,
        authority: Option<&super::CompletionRegistrationAuthority>,
        admitted_replay_only: bool,
        admission_id: &str,
        registration: CompletionEventRegistrationInput<'_>,
        binding: Option<&AdmittedSourceBinding>,
        before_state_commit: BeforeCommit,
        after_state_commit: AfterCommit,
    ) -> Result<CompletionEventRegistrationResult, String>
    where
        BeforeCommit: FnOnce(),
        AfterCommit: FnOnce(),
    {
        if let Some(binding) = binding {
            binding.validate_input(admission_id, &registration)?;
        }
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
        let admission_id = completion_bound_admission_id(admission_id, &registration, binding);
        let binding_bytes = binding.map(AdmittedSourceBinding::encoded).transpose()?;
        let authority_basis = if admitted_replay_only {
            "admitted_repair"
        } else {
            "live_registration"
        };
        let sidecar_path = MailboxDb::path_for_state_db(completion_authority_state_path);
        let state_start = SpanStart::new("completion_registration", "state_sqlite")
            .with_lifecycle_phase("completion_registration")
            .with_sqlite_identity(
                SqliteEventIdentity::new(
                    SqliteDatabaseRole::State,
                    SqlitePathClass::ManagedFile,
                    "completion.registration.state",
                )
                .with_transaction_mode(SqliteTransactionMode::Immediate),
            )
            .with_busy_timeout(super::opening_write::state_writer_busy_timeout())
            .with_identifier("completion_event_id", registration.event_id)
            .with_identifier("authority_basis", authority_basis)
            .with_identifier("owner_invocation_uuid", owner_invocation_uuid)
            .with_identifier("owner_session_id", owner_session_id)
            .with_hashed_correlation("admission_id", &admission_id);
        process_recorder().with_requested_span(state_start, |state_span| {
        let state_attempt = TransactionAttempt::start();
        let tx = match self
            .conn
            .transaction_with_behavior(sqlite::TransactionBehavior::Immediate)
        {
            Ok(tx) => tx,
            Err(error) => {
                record_sqlite_failure(state_span, &error, state_attempt);
                record_unacquired_release(state_span);
                return Err(format!(
                    "Failed to begin completion admission transaction: {error}"
                ));
            }
        };
        let mut state_phases = TransactionPhaseGuard::acquired(state_span, state_attempt);
        let state_committed = std::cell::Cell::new(false);
        let sidecar_failure_recorded = std::cell::Cell::new(false);
        let state_result = (|| {
        let invocation_row_id: i64 = tx
            .query_row(
                "SELECT id FROM invocations WHERE invocation_uuid=?1",
                [owner_invocation_uuid],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        // Keep the established legacy fence failure ordering. Only exact v2
        // repair is exempt from reacquiring provider-launch mutation authority.
        if binding.is_none() {
            super::provider_launch_lifecycle::validate_invocation_mutation_authority(
                &tx,
                invocation_row_id,
                mutation_authority,
            )?;
        }
        require_completion_continuity_registration_ready(&tx)?;
        if admitted_replay_only {
            require_exact_admitted_completion_replay(
                &tx,
                &admission_id,
                owner_invocation_uuid,
                owner_session_id,
                registration.event_id,
            )?;
        }
        if !admitted_replay_only && let Some(authority) = authority {
            validate_completion_registration_actor(
                &tx,
                authority,
                owner_invocation_uuid,
                owner_session_id,
            )?;
        }
        // A live v2 registration actor presents the original private completion
        // capability. Resolve only its own current provider effect fence inside
        // this transaction; the helper need not inherit generic launch mutation
        // authority, and no new lease or registration secret is reconstructed.
        let linked_registration_fence = if binding.is_some()
            && authority.is_some()
            && !admitted_replay_only
            && matches!(
                mutation_authority,
                crate::InvocationMutationAuthority::Standalone
            ) {
            current_registration_effect_fence(&tx, invocation_row_id)?
        } else {
            None
        };
        let effect_authority = linked_registration_fence
            .as_ref()
            .map(crate::InvocationMutationAuthority::ProviderLaunch)
            .unwrap_or(mutation_authority);
        // Exact admitted repair materializes an existing effect, not a new one.
        // It cannot reacquire or mutate an old provider-launch owner after loss.
        if !(admitted_replay_only && binding.is_some()) {
            if binding.is_some() {
                super::provider_launch_lifecycle::validate_invocation_mutation_authority(
                    &tx,
                    invocation_row_id,
                    effect_authority,
                )?;
            }
            super::provider_launch_lifecycle::promote_invocation_effect(
                &tx,
                effect_authority,
                crate::ProviderLaunchPromotion::MailboxSubmissionAccepted,
                1,
            )?;
        }
        let owner_authorization = completion_owner_authorization(
            &tx,
            owner_invocation_uuid,
            owner_session_id,
            registration.event_id,
            &admission_id,
        )?;
        let state_head = completion_continuity_head_on(&tx).map_err(|error| error.to_string())?;
        let sidecar_start = SpanStart::new(
            "completion_authority_registration",
            "pid_mailbox_sqlite",
        )
        .with_lifecycle_phase("completion_authority")
        .with_sqlite_identity(
            SqliteEventIdentity::new(
                SqliteDatabaseRole::PidMailbox,
                SqlitePathClass::ManagedFile,
                "completion.registration.sidecar",
            )
            .with_transaction_mode(SqliteTransactionMode::Immediate),
        )
        .with_diagnostic_id(state_span.diagnostic_id().clone())
        .with_parent_span_id(state_span.span_id().clone())
        .with_identifier("completion_event_id", registration.event_id)
        .with_identifier("authority_basis", authority_basis)
        .with_identifier("owner_invocation_uuid", owner_invocation_uuid)
        .with_identifier("owner_session_id", owner_session_id)
        .with_hashed_correlation("admission_id", &admission_id)
        .with_busy_timeout(std::time::Duration::ZERO);
        state_span.with_deferred_requested_span(sidecar_start, |sidecar_span| {
        let sidecar_authority = match crate::mailbox::MailboxAuthorityFence::try_acquire(&sidecar_path) {
            Ok(authority) => authority,
            Err(error) => {
                sidecar_failure_recorded.set(true);
                let _ = sidecar_span.record(
                    DiagnosticPhase::Failed,
                    PhaseObservation::not_started()
                        .with_cause("completion_authority_namespace_failed"),
                );
                record_unacquired_release(sidecar_span);
                return Err(error.to_string());
            }
        };
        let mut mailbox = match if state_head.is_none() {
            MailboxDb::open_for_state_authority_instrumented(
                &sidecar_authority,
                sidecar_span,
            )
        } else {
            MailboxDb::open_existing_for_completion_authority_instrumented(
                &sidecar_authority,
                sidecar_span,
            )
            .map_err(|error| {
                    format!(
                        "process_integrity: invocation {owner_invocation_uuid} has admitted completion authority but the sidecar is unavailable: {error}"
                    )
                })
        } {
            Ok(mailbox) => mailbox,
            Err(error) => {
                sidecar_failure_recorded.set(true);
                let _ = sidecar_span.record(
                    DiagnosticPhase::Failed,
                    PhaseObservation::not_started()
                        .with_cause("completion_authority_open_failed"),
                );
                record_unacquired_release(sidecar_span);
                return Err(error);
            }
        };
        let (sidecar_fence, sidecar_attempt) = mailbox
            .begin_completion_authority_fence_instrumented(sidecar_span)
            .map_err(|error| {
                sidecar_failure_recorded.set(true);
                if state_head.is_none() {
                    error
                } else {
                    format!(
                        "process_integrity: invocation {owner_invocation_uuid} cannot fence admitted completion authority: {error}"
                    )
                }
            })?;
        let mut sidecar_fence = Some(sidecar_fence);
        let mut sidecar_phases =
            TransactionPhaseGuard::acquired(sidecar_span, sidecar_attempt);
        let sidecar_result = (|| {
        let sidecar_generation = match sidecar_fence
            .as_ref()
            .expect("sidecar fence remains owned until registration")
            .sidecar_generation()
        {
            Ok(generation) => generation,
            Err(error) => {
                sidecar_failure_recorded.set(true);
                sidecar_phases.failed("completion_authority_generation_failed");
                return Err(if state_head.is_none() {
                    error
                } else {
                    format!(
                        "process_integrity: invocation {owner_invocation_uuid} has invalid admitted completion authority: {error}"
                    )
                });
            }
        };
        let sidecar_head = match sidecar_fence
            .as_ref()
            .expect("sidecar fence remains owned until registration")
            .completion_continuity_head()
        {
            Ok(head) => head,
            Err(error) => {
                sidecar_failure_recorded.set(true);
                sidecar_phases.failed("completion_authority_continuity_read_failed");
                return Err(format!(
                    "process_integrity: invocation {owner_invocation_uuid} cannot read completion continuity authority: {error}"
                ));
            }
        };
        let obligation = CompletionObligationAdmission {
            admission_id: &admission_id,
            invocation_uuid: owner_invocation_uuid,
            event_id: registration.event_id,
            owner_invocation_uuid,
            owner_session_id,
            expected_sidecar_generation: &sidecar_generation,
        };
        if let Err(error) = owner_authorization.validate_observed_generation(&obligation) {
            sidecar_failure_recorded.set(true);
            sidecar_phases.failed("completion_authority_generation_mismatch");
            return Err(error);
        }
        let replay_continuity = if state_head != sidecar_head {
            completion_continuity_by_admission_on(&tx, &admission_id)
                .map_err(|error| error.to_string())?
        } else {
            None
        };
        if let Err(error) = validate_completion_continuity_alignment(
            state_head.as_ref(),
            sidecar_head.as_ref(),
            replay_continuity.as_ref(),
            &obligation,
        ) {
            sidecar_failure_recorded.set(true);
            sidecar_phases.failed("completion_authority_continuity_mismatch");
            return Err(error);
        }
        if let Some(binding) = binding
            && let Err(error) = sidecar_fence
                .as_ref()
                .expect("sidecar fence remains owned until registration")
                .preflight_continuation_binding(binding, admitted_replay_only)
        {
            sidecar_failure_recorded.set(true);
            sidecar_phases.failed("completion_authority_binding_preflight_failed");
            return Err(error);
        }
        if let Err(error) = sidecar_fence
            .as_ref()
            .expect("sidecar fence remains owned until registration")
            .require_continuation_binding(registration.event_id, binding.is_some())
        {
            sidecar_failure_recorded.set(true);
            sidecar_phases.failed("completion_authority_binding_failed");
            return Err(error);
        }
        if let Err(error) = sidecar_fence
            .as_ref()
            .expect("sidecar fence remains owned until registration")
            .preflight_completion_event_registration(&registration)
        {
            sidecar_failure_recorded.set(true);
            sidecar_phases.failed("completion_authority_registration_preflight_failed");
            return Err(error);
        }
        let (_, continuity) = record_completion_obligation_with_continuity_on(
            &tx,
            obligation,
            state_head.as_ref(),
            binding_bytes.as_deref(),
        )
        .map_err(|error| error.to_string())?;
        before_state_commit();
        state_phases.commit_started();
        match tx.commit() {
            Ok(()) => state_phases.committed(),
            Err(error) => {
                state_phases.sqlite_failure(&error);
                return Err(format!("Failed to commit completion admission: {error}"));
            }
        }
        after_state_commit();
        state_committed.set(true);
        state_phases.release_after_owner();
        let registration_result = sidecar_fence
            .take()
            .expect("sidecar fence remains owned until registration")
            .register_completion_event_instrumented(
                registration,
                &continuity,
                binding,
                &mut sidecar_phases,
            );
        if registration_result.is_err() {
            sidecar_failure_recorded.set(true);
            sidecar_phases.failed("completion_authority_registration_failed");
        }
        registration_result
        })();
        // Validation and replay failures deliberately roll back. End that
        // owner before emitting terminal sidecar evidence.
        drop(sidecar_fence.take());
        sidecar_phases.release_after_owner();
        sidecar_result
        })
        })();
        if state_result.is_err()
            && !state_committed.get()
            && !sidecar_failure_recorded.get()
        {
            state_phases.failed("completion_registration_state_transaction_failed");
        }
        state_phases.release_after_owner();
        state_result
        })
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

fn decode_admitted_completion_row(
    row: (Vec<u8>, String, String, String, String),
) -> Result<AdmittedSourceBinding, String> {
    let (bytes, admission_id, event_id, invocation, session) = row;
    let binding = AdmittedSourceBinding::decode(&bytes)?;
    let source = binding.registration()?;
    let paths = source.paths();
    let listener = binding.admission_listener()?;
    let input = CompletionEventRegistrationInput {
        event_id: &source.handle,
        delivery_mode: &source.delivery_mode,
        owner_session_id: Some(&listener.session_id),
        owner_invocation_uuid: Some(&listener.owner_invocation_uuid),
        state_dir: &source.handle_dir,
        meta_path: &paths[0],
        log_path: &paths[1],
        rc_path: &paths[2],
    };
    if completion_bound_admission_id(binding.caller_admission_id(), &input, Some(&binding))
        != admission_id
        || event_id != source.handle
        || invocation != listener.owner_invocation_uuid
        || session != listener.session_id
    {
        return Err(
            "process_integrity: completion source does not match admitted authority".into(),
        );
    }
    Ok(binding)
}

fn current_registration_effect_fence(
    conn: &sqlite::Connection,
    row_id: i64,
) -> Result<Option<crate::ProviderLaunchOwnerFence>, String> {
    let row:Option<(String,String,i64,String)>=conn.query_row(
        "SELECT a.logical_launch_id,a.attempt_id,a.owner_epoch,a.invocation_uuid FROM provider_launch_attempts a WHERE a.invocation_id=?1",
        [row_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
    ).optional().map_err(|e|e.to_string())?;
    row.map(|(launch, attempt, epoch, invocation)| {
        Ok(crate::ProviderLaunchOwnerFence {
            logical_launch_id: uuid::Uuid::parse_str(&launch).map_err(|e| e.to_string())?,
            attempt_id: uuid::Uuid::parse_str(&attempt).map_err(|e| e.to_string())?,
            owner_epoch: u64::try_from(epoch).map_err(|e| e.to_string())?,
            invocation_row_id: row_id,
            invocation_uuid: uuid::Uuid::parse_str(&invocation).map_err(|e| e.to_string())?,
        })
    })
    .transpose()
}

fn completion_bound_admission_id(
    caller_admission_id: &str,
    registration: &CompletionEventRegistrationInput<'_>,
    binding: Option<&AdmittedSourceBinding>,
) -> String {
    let bound = binding.map(|binding| {
        format!(
            "{caller_admission_id}:completion-v2:{}",
            binding.registration_digest()
        )
    });
    completion_registration_admission_id(
        bound.as_deref().unwrap_or(caller_admission_id),
        registration,
    )
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
    binding: Option<&[u8]>,
) -> Result<CompletionObligationAdmissionResult, OwnershipAuthorityError> {
    validate_completion_obligation(&input)?;
    let binding_identity = binding
        .map(AdmittedSourceBinding::decode)
        .transpose()
        .map_err(persistence_message)?;
    if let Some(existing) = completion_obligation_by_admission_id(tx, input.admission_id)? {
        let retained: Option<Vec<u8>> = tx.query_row(
            "SELECT completion_v2_binding FROM invocation_completion_obligations WHERE admission_id=?1",
            [input.admission_id], |row| row.get(0),
        ).map_err(persistence("read exact completion source binding"))?;
        if retained.as_deref() != binding {
            return Err(persistence_message(
                "completion continuation binding conflict",
            ));
        }
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
                owner_session_id, expected_sidecar_generation, admitted_at, completion_v2_binding
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        sqlite::params![
            input.admission_id,
            input.invocation_uuid,
            input.event_id,
            input.owner_invocation_uuid,
            input.owner_session_id,
            input.expected_sidecar_generation,
            &admitted_at,
            binding,
        ],
    )
    .map_err(persistence("insert completion obligation"))?;
    if let Some(binding) = binding_identity {
        let source = binding.registration().map_err(persistence_message)?;
        tx.execute(
            "INSERT INTO invocation_completion_v2_identity (
                admission_id,domain_id,source_id,registration_id,handle,
                registration_digest
             ) VALUES (?1,?2,?3,?4,?5,?6)",
            sqlite::params![
                input.admission_id,
                source.domain_id,
                source.source_id,
                source.registration_id,
                source.handle,
                binding.registration_digest()
            ],
        )
        .map_err(persistence("insert completion v2 identity"))?;
    }
    completion_obligation_by_admission_id(tx, input.admission_id)?
        .map(CompletionObligationAdmissionResult::Recorded)
        .ok_or_else(|| persistence_message("completion obligation disappeared after insert"))
}

fn record_completion_obligation_with_continuity_on(
    tx: &rusqlite::Transaction<'_>,
    input: CompletionObligationAdmission<'_>,
    state_head: Option<&CompletionContinuityHead>,
    binding: Option<&[u8]>,
) -> Result<
    (
        CompletionObligationAdmissionResult,
        CompletionContinuityHead,
    ),
    OwnershipAuthorityError,
> {
    let result = record_completion_obligation_on(tx, input, binding)?;
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
    let existing = exact_admitted_completion_expectation(
        conn,
        admission_id,
        invocation_uuid,
        owner_session_id,
        event_id,
    )?
    .ok_or_else(|| terminal_owner_new_admission_error(invocation_uuid))?;
    if !exact_completion_continuity_exists(conn, admission_id, &existing)? {
        return Err(terminal_owner_new_admission_error(invocation_uuid));
    }
    Ok(CompletionOwnerAuthorization::TerminalExactReplay(existing))
}

fn require_exact_admitted_completion_replay(
    conn: &sqlite::Connection,
    admission_id: &str,
    owner_invocation_uuid: &str,
    owner_session_id: &str,
    event_id: &str,
) -> Result<(), String> {
    let expectation = exact_admitted_completion_expectation(
        conn,
        admission_id,
        owner_invocation_uuid,
        owner_session_id,
        event_id,
    )?
        .ok_or_else(|| {
            format!(
                "process_integrity: completion repair requires an exact admitted replay for event {event_id}"
            )
        })?;
    if exact_completion_continuity_exists(conn, admission_id, &expectation)? {
        return Ok(());
    }
    Err(format!(
        "process_integrity: completion repair requires exact State continuity for event {event_id}"
    ))
}

fn exact_admitted_completion_expectation(
    conn: &sqlite::Connection,
    admission_id: &str,
    owner_invocation_uuid: &str,
    owner_session_id: &str,
    event_id: &str,
) -> Result<Option<CompletionObligationExpectation>, String> {
    completion_obligation_by_admission_id(conn, admission_id)
        .map_err(|error| error.to_string())
        .map(|expectation| {
            expectation.filter(|expectation| {
                expectation.invocation_uuid == owner_invocation_uuid
                    && expectation.event_id == event_id
                    && expectation.owner_invocation_uuid == owner_invocation_uuid
                    && expectation.owner_session_id == owner_session_id
            })
        })
}

fn exact_completion_continuity_exists(
    conn: &sqlite::Connection,
    admission_id: &str,
    expectation: &CompletionObligationExpectation,
) -> Result<bool, String> {
    completion_continuity_by_admission_on(conn, admission_id)
        .map_err(|error| error.to_string())
        .map(|continuity| {
            continuity
                .as_ref()
                .is_some_and(|row| completion_continuity_matches_expectation(row, expectation))
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
    use crate::diagnostic_recorder::{
        FlightRecorder, FlightRecorderReader, RecorderConfig, SqliteMeasurementGap,
        with_test_process_recorder,
    };
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
            .register_completion_event_with_obligation(
                crate::InvocationMutationAuthority::Standalone,
                "age299-s2-memory-admission",
                registration(),
            )
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
                crate::InvocationMutationAuthority::Standalone,
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
                crate::InvocationMutationAuthority::Standalone,
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
                crate::InvocationMutationAuthority::Standalone,
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
                crate::InvocationMutationAuthority::Standalone,
                "age299-s2-retarget-admission",
                registration(),
            )
            .unwrap();
        std::fs::remove_file(&alias_directory).unwrap();
        symlink(&second_directory, &alias_directory).unwrap();

        let error = state
            .finalize_invocation(
                crate::InvocationMutationAuthority::Standalone,
                invocation_row_id,
                true,
                0,
                None,
                None,
            )
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
                crate::InvocationMutationAuthority::Standalone,
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
                crate::InvocationMutationAuthority::Standalone,
                "age299-s2-state-replacement-admission",
                registration(),
            )
            .unwrap();
        std::fs::rename(&state_path, &displaced_path).unwrap();
        std::fs::File::create(&state_path).unwrap();

        let error = state
            .finalize_invocation(
                crate::InvocationMutationAuthority::Standalone,
                invocation_row_id,
                true,
                0,
                None,
                None,
            )
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
                crate::InvocationMutationAuthority::Standalone,
                invocation_row_id,
                false,
                1,
                Some("test_failure"),
                Some("terminal before completion admission"),
            )
            .unwrap();

        let error = state
            .register_completion_event_with_obligation(
                crate::InvocationMutationAuthority::Standalone,
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
                    crate::InvocationMutationAuthority::Standalone,
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
            .register_completion_event_with_obligation(
                crate::InvocationMutationAuthority::Standalone,
                "age299-s2-invalid-registration",
                valid,
            )
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
    fn completion_registration_sidecar_contention_is_parented_and_not_state_failure() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let sidecar_path = MailboxDb::path_for_state_db(&state_path);
        let mut state = StateDb::open(&state_path).unwrap();
        state
            .start_invocation(&InvocationStart {
                invocation_uuid: INVOCATION_UUID.to_string(),
                model_name: "age319".to_string(),
                provider_name: "test-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        drop(MailboxDb::open(&sidecar_path).unwrap());
        let holder = sqlite::Connection::open(&sidecar_path).unwrap();
        holder.execute_batch("BEGIN IMMEDIATE").unwrap();
        let recorder_root = directory.path().join("recorder");
        let recorder = FlightRecorder::open(&recorder_root, RecorderConfig::default()).unwrap();

        let error = with_test_process_recorder(recorder.clone(), || {
            state
                .register_completion_event_with_obligation(
                    crate::InvocationMutationAuthority::Standalone,
                    "age319-completion-contention-admission",
                    registration(),
                )
                .unwrap_err()
        });
        recorder.drain_deferred_for_test().unwrap();
        assert!(error.contains("completion_authority_contention"), "{error}");
        holder.execute_batch("ROLLBACK").unwrap();
        assert!(
            state
                .completion_obligations_for_invocation(INVOCATION_UUID)
                .unwrap()
                .is_empty()
        );

        let report = FlightRecorderReader::new(&recorder_root).inspect();
        let events = report
            .events
            .iter()
            .filter(|record| {
                matches!(
                    record.event.operation.as_str(),
                    "completion_registration" | "completion_authority_registration"
                )
            })
            .map(|record| &record.event)
            .collect::<Vec<_>>();
        assert_eq!(
            events
                .iter()
                .map(|event| (event.operation.as_str(), event.phase))
                .collect::<Vec<_>>(),
            vec![
                ("completion_registration", DiagnosticPhase::Requested),
                ("completion_registration", DiagnosticPhase::Acquired),
                (
                    "completion_authority_registration",
                    DiagnosticPhase::Requested,
                ),
                (
                    "completion_authority_registration",
                    DiagnosticPhase::Contention,
                ),
                (
                    "completion_authority_registration",
                    DiagnosticPhase::Released,
                ),
                ("completion_registration", DiagnosticPhase::Released),
            ]
        );
        let state_requested = events[0];
        for event in &events[2..5] {
            assert_eq!(event.diagnostic_id, state_requested.diagnostic_id);
            assert_eq!(
                event.parent_span_id.as_ref(),
                Some(&state_requested.span_id)
            );
            assert_eq!(event.resource, "pid_mailbox_sqlite");
            assert_eq!(event.observation.busy_timeout_millis, Some(0));
            assert_eq!(
                event
                    .correlations
                    .get("completion_event_id")
                    .map(String::as_str),
                Some(EVENT_ID)
            );
            assert_eq!(
                event
                    .correlations
                    .get("authority_basis")
                    .map(String::as_str),
                Some("live_registration")
            );
        }
        let contention = events[3].observation.sqlite_failure.as_ref().unwrap();
        assert!(contention.contention);
        assert_eq!(events[3].observation.wait_micros, None);
        let evidence = events[3].observation.sqlite.as_ref().unwrap();
        assert!(evidence.writer_authority_acquisition_micros.is_some());
        assert_eq!(evidence.writer_authority_wait_micros, None);
        assert!(
            evidence
                .measurement_gaps
                .contains(&SqliteMeasurementGap::WriterWaitNotExposedByApi)
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
                .register_completion_event_with_obligation(
                    crate::InvocationMutationAuthority::Standalone,
                    admission_id,
                    registration,
                )
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
        let sidecar_path = MailboxDb::path_for_state_db(&state_path);
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
                    crate::InvocationMutationAuthority::Standalone,
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
                .send(finalizer_state.finalize_invocation_typed(
                    crate::InvocationMutationAuthority::Standalone,
                    invocation_row_id,
                    true,
                    0,
                    None,
                    Some("completed"),
                ))
                .unwrap();
        });

        // Release the State writer but deliberately retain the independent
        // sidecar namespace/fence. The finalizer must fail closed promptly,
        // not hold a State writer while waiting through sidecar publication.
        admission_release_tx.send(()).unwrap();
        state_committed_rx.recv().unwrap();
        let first = finalize_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let crate::InvocationFinalizeError::Contention { message } = first.unwrap_err() else {
            panic!("sidecar contention must remain a typed retryable result");
        };
        assert!(
            message.contains("completion_authority_contention"),
            "{message}"
        );
        finalizer.join().unwrap();

        let observed = StateDb::open(&state_path).unwrap();
        let invocation = observed
            .get_invocation_by_uuid(INVOCATION_UUID)
            .unwrap()
            .unwrap();
        assert_eq!(invocation.status, InvocationStatus::Running);
        assert_eq!(invocation.success, None);
        assert_eq!(invocation.exit_code, None);
        assert_eq!(invocation.error_category, None);
        assert_eq!(invocation.terminal_reason, None);
        assert!(
            observed
                .get_provider("age299-s2", "test-provider")
                .unwrap()
                .is_none()
        );
        let sidecar_before = MailboxDb::open(&sidecar_path).unwrap();
        assert!(sidecar_before.completion_event(EVENT_ID).unwrap().is_none());
        assert!(
            sidecar_before
                .completion_event_listeners(EVENT_ID)
                .unwrap()
                .is_empty()
        );
        drop(sidecar_before);

        // A disjoint State writer proves the failed finalizer returned its
        // SQLite transaction while sidecar materialization is still paused.
        let disjoint_row = observed
            .start_invocation(&InvocationStart {
                invocation_uuid: SECOND_INVOCATION_UUID.to_string(),
                model_name: "disjoint".to_string(),
                provider_name: "disjoint-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        assert!(disjoint_row > invocation_row_id);
        drop(observed);

        sidecar_release_tx.send(()).unwrap();
        writer.join().unwrap();
        let materialized = MailboxDb::open(&sidecar_path).unwrap();
        assert!(materialized.completion_event(EVENT_ID).unwrap().is_some());
        assert_eq!(
            materialized
                .completion_event_listeners(EVENT_ID)
                .unwrap()
                .len(),
            1
        );
        drop(materialized);

        let retry = StateDb::open(&state_path).unwrap();
        retry
            .finalize_invocation(
                crate::InvocationMutationAuthority::Standalone,
                invocation_row_id,
                true,
                0,
                None,
                Some("completed"),
            )
            .unwrap();
        let invocation = retry
            .get_invocation_by_uuid(INVOCATION_UUID)
            .unwrap()
            .unwrap();
        assert_eq!(invocation.status, InvocationStatus::Succeeded);
        assert_eq!(invocation.success, Some(true));
        assert_eq!(invocation.exit_code, Some(0));
        assert_eq!(invocation.terminal_reason.as_deref(), Some("completed"));
        assert_eq!(
            retry
                .get_provider("age299-s2", "test-provider")
                .unwrap()
                .unwrap()
                .invocation_count,
            1
        );
        assert!(
            retry
                .finalize_invocation(
                    crate::InvocationMutationAuthority::Standalone,
                    invocation_row_id,
                    true,
                    0,
                    None,
                    Some("completed"),
                )
                .unwrap_err()
                .contains("already finalized")
        );
        assert_eq!(
            retry
                .get_provider("age299-s2", "test-provider")
                .unwrap()
                .unwrap()
                .invocation_count,
            1
        );
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
            .register_completion_event_with_obligation(
                crate::InvocationMutationAuthority::Standalone,
                "age299-s2-first-admission",
                registration(),
            )
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
                crate::InvocationMutationAuthority::Standalone,
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
                crate::InvocationMutationAuthority::Standalone,
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
            .register_completion_event_with_obligation(
                crate::InvocationMutationAuthority::Standalone,
                "age299-s2-first-admission",
                registration(),
            )
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
                crate::InvocationMutationAuthority::Standalone,
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
                crate::InvocationMutationAuthority::Standalone,
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
                crate::InvocationMutationAuthority::Standalone,
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
                crate::InvocationMutationAuthority::Standalone,
                "age299-s2-other-invocation-admission",
                second_registration,
            )
            .unwrap();
        assert!(state.has_legacy_completion_admissions().unwrap());
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
                    crate::InvocationMutationAuthority::Standalone,
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
                .finalize_invocation(
                    crate::InvocationMutationAuthority::Standalone,
                    invocation_id,
                    true,
                    0,
                    None,
                    None,
                )
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
                crate::InvocationMutationAuthority::Standalone,
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
                crate::InvocationMutationAuthority::Standalone,
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
                crate::InvocationMutationAuthority::Standalone,
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
            .register_completion_event_with_obligation(
                crate::InvocationMutationAuthority::Standalone,
                "age299-s2-first-admission",
                registration(),
            )
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
                crate::InvocationMutationAuthority::Standalone,
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
                crate::InvocationMutationAuthority::Standalone,
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
                crate::InvocationMutationAuthority::Standalone,
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
                crate::InvocationMutationAuthority::Standalone,
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
                .register_completion_event_with_obligation(
                    crate::InvocationMutationAuthority::Standalone,
                    "age299-s2-partial-admission",
                    changed,
                )
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
            .register_completion_event_with_obligation(
                crate::InvocationMutationAuthority::Standalone,
                "age299-s2-partial-admission",
                original,
            )
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
            .register_completion_event_with_obligation(
                crate::InvocationMutationAuthority::Standalone,
                "age299-s2-rollback-first",
                first,
            )
            .unwrap();
        state
            .register_completion_event_with_obligation(
                crate::InvocationMutationAuthority::Standalone,
                "age299-s2-rollback-second",
                second,
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
            .register_completion_event_with_obligation(
                crate::InvocationMutationAuthority::Standalone,
                "age299-s2-rollback-second",
                second,
            )
            .unwrap_err();
        assert!(
            out_of_order_error.contains("continuity heads do not match"),
            "{out_of_order_error}"
        );
        let new_admission_error = state
            .repair_admitted_completion_event(
                crate::InvocationMutationAuthority::Standalone,
                "age299-s2-rollback-third",
                registration_for("age299-s2-rollback-third", SESSION_ID, INVOCATION_UUID),
            )
            .unwrap_err();
        assert!(
            new_admission_error.contains("requires an exact admitted replay"),
            "{new_admission_error}"
        );

        state
            .repair_admitted_completion_event(
                crate::InvocationMutationAuthority::Standalone,
                "age299-s2-rollback-first",
                first,
            )
            .unwrap();
        state
            .repair_admitted_completion_event(
                crate::InvocationMutationAuthority::Standalone,
                "age299-s2-rollback-second",
                second,
            )
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
            .register_completion_event_with_obligation(
                crate::InvocationMutationAuthority::Standalone,
                "age299-s2-terminal-partial",
                registration(),
            )
            .unwrap_err();
        assert!(
            first_error.contains("forced terminal repair interruption"),
            "{first_error}"
        );
        state
            .finalize_invocation(
                crate::InvocationMutationAuthority::Standalone,
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
            .register_completion_event_with_obligation(
                crate::InvocationMutationAuthority::Standalone,
                "age299-s2-terminal-partial",
                registration(),
            )
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
                .register_completion_event_with_obligation(
                    crate::InvocationMutationAuthority::Standalone,
                    caller_admission_id,
                    changed,
                )
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
            .register_completion_event_with_obligation(
                crate::InvocationMutationAuthority::Standalone,
                "age299-s2-terminal-partial",
                registration(),
            )
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
            .register_completion_event_with_obligation(
                crate::InvocationMutationAuthority::Standalone,
                "age299-s2-terminal-partial",
                registration(),
            )
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
                crate::InvocationMutationAuthority::Standalone,
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
                crate::InvocationMutationAuthority::Standalone,
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

#[cfg(test)]
mod completion_continuation_tests {
    use super::*;
    use crate::InvocationStart;
    use crate::diagnostic_recorder::{
        DiagnosticPhase, FlightRecorder, FlightRecorderReader, RecorderConfig,
        with_test_process_recorder,
    };
    use std::panic::{AssertUnwindSafe, catch_unwind};

    fn binding() -> AdmittedSourceBinding {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/age360-paired-wire.json"))
                .unwrap();
        AdmittedSourceBinding::new(
            "fixture-admission",
            fixture["registration_bytes_utf8"]
                .as_str()
                .unwrap()
                .as_bytes(),
        )
        .unwrap()
    }

    fn seed_domain(path: &std::path::Path) {
        let mut mailbox =
            MailboxDb::open_completion_continuation_domain(&MailboxDb::path_for_state_db(path))
                .unwrap();
        let source = binding().registration().unwrap();
        mailbox
            .connection()
            .execute(
                "UPDATE completion_continuation_domain SET domain_id=?1",
                [&source.domain_id],
            )
            .unwrap();
        let process =
            crate::pid_identity::read_live_process_identity(i64::from(std::process::id()))
                .unwrap()
                .unwrap();
        let identity = crate::completion_continuation::SourceProcessIdentity {
            pid: process.os_pid,
            boot_id: process.os_boot_id,
            starttime_ticks: process.os_pid_starttime_ticks,
        };
        mailbox
            .publish_completion_continuation_owner(&crate::mailbox::CompletionDomainOwner {
                protocol: source.protocol,
                domain_id: source.domain_id,
                supervisor_authority_id: uuid::Uuid::new_v4().to_string(),
                owner_generation: uuid::Uuid::new_v4().to_string(),
                guardian_identity: identity.clone(),
                driver_identity: identity,
                endpoint: "/fixture/owner.sock".into(),
            })
            .unwrap();
    }

    fn seed(path: &std::path::Path, binding: &AdmittedSourceBinding) -> StateDb {
        let state = StateDb::open(path).unwrap();
        seed_domain(path);
        state
            .start_invocation(&InvocationStart {
                invocation_uuid: binding.registration().unwrap().owner_invocation_uuid,
                model_name: "fixture".into(),
                provider_name: "fixture".into(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        state
    }

    fn admit(state: &mut StateDb, binding: &AdmittedSourceBinding, before: bool, after: bool) {
        let source = binding.registration().unwrap();
        let paths = source.paths();
        let listener = binding.admission_listener().unwrap();
        state
            .register_completion_event_with_binding_on(
                crate::InvocationMutationAuthority::Standalone,
                None,
                false,
                binding.caller_admission_id(),
                CompletionEventRegistrationInput {
                    event_id: &source.handle,
                    delivery_mode: &source.delivery_mode,
                    owner_session_id: Some(&listener.session_id),
                    owner_invocation_uuid: Some(&listener.owner_invocation_uuid),
                    state_dir: &source.handle_dir,
                    meta_path: &paths[0],
                    log_path: &paths[1],
                    rc_path: &paths[2],
                },
                Some(binding),
                || assert!(!before, "before State commit fault"),
                || assert!(!after, "after State commit fault"),
            )
            .unwrap();
    }

    #[test]
    fn exact_continuation_conflict_uses_the_indexed_identity_projection() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.db");
        let binding = binding();
        let mut state = seed(&path, &binding);
        admit(&mut state, &binding, false, false);

        let mut changed = serde_json::to_value(binding.registration().unwrap()).unwrap();
        changed["source_id"] = uuid::Uuid::new_v4().to_string().into();
        changed["handle"] = "different-handle".into();
        changed["handle_dir"] = format!(
            "{}/different-handle",
            changed["spool_root"].as_str().unwrap()
        )
        .into();
        changed["helper"]["path"] =
            format!("{}/runner", changed["handle_dir"].as_str().unwrap()).into();
        changed["recovery"]["path"] =
            format!("{}/agent-bash", changed["handle_dir"].as_str().unwrap()).into();
        let conflict = AdmittedSourceBinding::new(
            "different-caller-admission",
            &serde_json::to_vec(&changed).unwrap(),
        )
        .unwrap();
        assert!(
            state
                .admitted_completion_continuation(&conflict)
                .unwrap_err()
                .contains("immutable identity conflict")
        );

        let source = conflict.registration().unwrap();
        let explain = format!("EXPLAIN QUERY PLAN {COMPLETION_SOURCE_CONFLICT_SQL}");
        let details = state
            .conn
            .prepare(&explain)
            .unwrap()
            .query_map(
                sqlite::params![
                    source.registration_id,
                    source.domain_id,
                    source.source_id,
                    source.handle,
                    conflict.registration_digest()
                ],
                |row| row.get::<_, String>(3),
            )
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        for index in [
            "idx_invocation_completion_v2_registration",
            "idx_invocation_completion_v2_source",
            "idx_invocation_completion_v2_handle",
        ] {
            assert!(
                details.iter().any(|detail| detail.contains(index)),
                "missing {index} from exact conflict plan: {details:?}"
            );
        }

        let exact = format!(
            "EXPLAIN QUERY PLAN {}",
            completion_admission_sql("o.admission_id=?1")
        );
        let exact_details = state
            .conn
            .prepare(&exact)
            .unwrap()
            .query_map(["fixture-admission"], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            exact_details
                .iter()
                .all(|detail| !detail.contains("SCAN invocation_completion")),
            "exact admission readback scanned a ledger: {exact_details:?}"
        );
        assert!(
            exact_details
                .iter()
                .any(|detail| detail.contains("admission_id")),
            "exact admission readback omitted its identity index: {exact_details:?}"
        );

        let suffix = format!("EXPLAIN QUERY PLAN {COMPLETION_CONTINUITY_SUFFIX_SQL}");
        let suffix_details = state
            .conn
            .prepare(&suffix)
            .unwrap()
            .query_map(sqlite::params![0_i64, 1_i64], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            suffix_details
                .iter()
                .any(|detail| detail.contains("INTEGER PRIMARY KEY") && detail.contains(">?")),
            "bounded continuity suffix omitted the ordinal range seek: {suffix_details:?}"
        );
        assert!(
            suffix_details
                .iter()
                .any(|detail| detail.contains("admission_id")),
            "bounded continuity suffix omitted its exact obligation join: {suffix_details:?}"
        );
    }

    #[test]
    fn legacy_admission_startup_probe_uses_only_the_null_binding_projection() {
        let directory = tempfile::tempdir().unwrap();
        let state = StateDb::open(&directory.path().join("state.db")).unwrap();
        assert!(!state.has_legacy_completion_admissions().unwrap());

        let explain = format!("EXPLAIN QUERY PLAN {LEGACY_COMPLETION_ADMISSION_SQL}");
        let details = state
            .conn
            .prepare(&explain)
            .unwrap()
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            details
                .iter()
                .any(|detail| { detail.contains("idx_invocation_completion_obligations_legacy") }),
            "legacy startup probe escaped its partial projection: {details:?}"
        );
    }

    #[test]
    fn live_state_rejects_full_admission_history_and_records_state_evidence() {
        let directory = tempfile::tempdir().unwrap();
        let state = StateDb::open(&directory.path().join("state.db")).unwrap();
        let recorder_root = directory.path().join("recorder");
        let recorder = FlightRecorder::open(&recorder_root, RecorderConfig::default()).unwrap();

        let error = with_test_process_recorder(recorder.clone(), || {
            state.admitted_completion_continuations().unwrap_err()
        });
        recorder.drain_deferred_for_test().unwrap();
        assert!(error.contains("live_history_barrier"), "{error}");

        let report = FlightRecorderReader::new(&recorder_root).inspect();
        let blocked = report
            .events
            .iter()
            .find(|record| {
                record.event.operation == "live_history_barrier"
                    && record.event.phase == DiagnosticPhase::Failed
            })
            .expect("blocked historical access retained outside the live database");
        assert_eq!(
            blocked.event.sqlite.as_ref().unwrap().database_role,
            SqliteDatabaseRole::State
        );
        let evidence = blocked
            .event
            .observation
            .live_history_barrier
            .as_ref()
            .unwrap();
        assert_eq!(evidence.live_trace, "state.live");
        assert_eq!(
            evidence.attempted_access,
            crate::diagnostic_recorder::SqliteAccessClass::HistoricalDiagnostic
        );
        assert_eq!(
            evidence.query_family,
            "completion_continuation.full_admission_ledger"
        );
        assert_eq!(evidence.decision, "blocked");
    }

    #[test]
    fn completion_continuation_sync_acceptance_suppresses_without_ack_and_replay_is_exact() {
        acceptance_presentation_and_replay(false, "sync", false, false, false);
    }

    #[test]
    fn completion_continuation_missing_output_retains_late_listener_and_exact_ack() {
        acceptance_presentation_and_replay(true, "sync", false, false, false);
    }

    #[test]
    fn completion_continuation_explicit_async_acceptance_and_repair_deliver() {
        acceptance_presentation_and_replay(false, "async", false, false, false);
    }

    #[test]
    fn completion_continuation_detach_before_acceptance_survives_repair() {
        acceptance_presentation_and_replay(false, "sync", true, false, false);
    }

    #[test]
    fn completion_continuation_response_only_cannot_settle_physical_custody() {
        acceptance_presentation_and_replay(false, "sync", false, true, false);
    }

    #[test]
    fn storage_batch_repair_retains_postcommit_late_listener_delivery_and_exact_ack() {
        acceptance_presentation_and_replay(true, "sync", false, false, true);
    }

    fn acceptance_presentation_and_replay(
        missing: bool,
        mode: &str,
        detach_before: bool,
        physical_debt: bool,
        batch_repair: bool,
    ) {
        use crate::completion_continuation::VerifiedCompletion;
        use crate::mailbox::CompletionEventTriggerInput;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.db");
        let binding = if mode == "sync" {
            binding()
        } else {
            let mut registration = serde_json::to_value(binding().registration().unwrap()).unwrap();
            registration["delivery_mode"] = mode.into();
            AdmittedSourceBinding::new(
                "fixture-admission",
                &serde_json::to_vec(&registration).unwrap(),
            )
            .unwrap()
        };
        let initially_active = mode == "async" || detach_before;
        let mut state = seed(&path, &binding);
        state.access_scope = crate::live_history::AccessScope::historical();
        admit(&mut state, &binding, false, false);
        // Final-system diagnostic distinguishes exact v2 admissions from legacy
        // authority; no migration-only fixture is needed for this predicate.
        assert!(!state.has_legacy_completion_admissions().unwrap());
        let source = binding.registration().unwrap();
        let paths = source.paths();
        let f: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/age360-missing-output-wire.json"
        ))
        .unwrap();
        let snapshot_bytes = f[if missing {
            "missing_output_snapshot_bytes_utf8"
        } else {
            "snapshot_bytes_utf8"
        }]
        .as_str()
        .unwrap()
        .as_bytes();
        let outcome_bytes = f["outcome_bytes_utf8"].as_str().unwrap().as_bytes();
        let evidence = if mode == "sync" {
            VerifiedCompletion::from_bytes(&binding, snapshot_bytes, outcome_bytes)
        } else {
            // A mode variant has its own immutable registration digest. Keep
            // original missing-output attribution bytes intact in sync cases.
            let mut outcome: serde_json::Value = serde_json::from_slice(outcome_bytes).unwrap();
            outcome["registration_digest"] = binding.registration_digest().into();
            let outcome = serde_json::to_vec(&outcome).unwrap();
            let mut snapshot: serde_json::Value = serde_json::from_slice(snapshot_bytes).unwrap();
            snapshot["registration_digest"] = binding.registration_digest().into();
            snapshot["outcome_sha256"] = crate::completion_continuation::sha256(&outcome).into();
            snapshot["outcome_byte_len"] = outcome.len().into();
            VerifiedCompletion::from_bytes(
                &binding,
                &serde_json::to_vec(&snapshot).unwrap(),
                &outcome,
            )
        }
        .unwrap();
        assert_eq!(evidence.original_output_missing(), missing);
        let payload = serde_json::to_string(&serde_json::json!({
            "kind":"agent_bash_complete", "rc":0,
            "completion_protocol":crate::completion_continuation::PROTOCOL,
            "snapshot":evidence.snapshot, "outcome":evidence.outcome, "output_artifact":null,
        }))
        .unwrap();
        let input = CompletionEventTriggerInput {
            event_id: &source.handle,
            payload_json: &payload,
            state_dir: &source.handle_dir,
            meta_path: &paths[0],
            log_path: &paths[1],
            rc_path: &paths[2],
            rc: 0,
        };
        let mut mailbox = MailboxDb::open(&MailboxDb::path_for_state_db(&path)).unwrap();
        let mut missing_protocol: serde_json::Value = serde_json::from_str(&payload).unwrap();
        missing_protocol
            .as_object_mut()
            .unwrap()
            .remove("completion_protocol");
        let missing_protocol = missing_protocol.to_string();
        assert!(
            mailbox
                .trigger_completion_continuation(
                    CompletionEventTriggerInput {
                        payload_json: &missing_protocol,
                        ..input
                    },
                    &binding,
                    &evidence,
                )
                .unwrap_err()
                .contains("lacks its protocol discriminator")
        );
        assert!(
            mailbox.trigger_completion_event(input).is_err(),
            "legacy trigger must not bypass v2 source evidence"
        );
        if missing {
            let mut wrong: serde_json::Value = serde_json::from_str(&payload).unwrap();
            wrong["snapshot"]["output"] = "".into();
            let wrong = wrong.to_string();
            assert!(
                mailbox
                    .trigger_completion_continuation(
                        CompletionEventTriggerInput {
                            payload_json: &wrong,
                            ..input
                        },
                        &binding,
                        &evidence
                    )
                    .is_err()
            );
            assert!(
                mailbox
                    .trigger_completion_continuation(
                        CompletionEventTriggerInput { rc: 70, ..input },
                        &binding,
                        &evidence
                    )
                    .is_err()
            );
            assert_ne!(
                mailbox
                    .completion_continuation_acceptance(&source.registration_id)
                    .unwrap()
                    .unwrap()["phase"],
                "accepted"
            );
        }
        if detach_before {
            let detached = mailbox
                .request_original_completion_notification(&source.handle)
                .unwrap();
            assert!(detached.mailbox_rows.is_empty());
        }
        let first = mailbox
            .trigger_completion_continuation(input, &binding, &evidence)
            .unwrap();
        assert!(first.triggered);
        assert_eq!(first.mailbox_rows.len(), usize::from(initially_active));
        for row in &first.mailbox_rows {
            let provenance: String = mailbox
                .connection()
                .query_row(
                    "SELECT completion_provenance FROM mailbox WHERE seq=?1",
                    [row.seq],
                    |record| record.get(0),
                )
                .unwrap();
            assert_eq!(
                provenance, "v2",
                "acceptance must stamp the mailbox atomically"
            );
        }
        assert_eq!(first.listeners[0].active, initially_active);
        assert!(first.listeners[0].acknowledged_at.is_none());
        let replay = mailbox
            .trigger_completion_continuation(input, &binding, &evidence)
            .unwrap();
        assert!(!replay.triggered);
        assert_eq!(
            first.listeners[0].mailbox_seq,
            replay.listeners[0].mailbox_seq
        );
        // Host loss has no ACK bridge: reopening and repairing without any
        // in-call receipt must retain the selected presentation policy.
        drop(mailbox);
        let mut mailbox = MailboxDb::open(&MailboxDb::path_for_state_db(&path)).unwrap();
        drop(state);
        let mut state = StateDb::open(&path).unwrap();
        state.access_scope = crate::live_history::AccessScope::historical();
        // Repeated admission and driver repair must not undo sync suppression.
        admit(&mut state, &binding, false, false);
        state
            .repair_admitted_completion_continuation(
                crate::InvocationMutationAuthority::Standalone,
                &binding,
            )
            .unwrap();
        let listeners = mailbox.completion_event_listeners(&source.handle).unwrap();
        assert_eq!(listeners[0].active, initially_active);
        assert!(listeners[0].acknowledged_at.is_none());
        assert!(listeners[0].acknowledgement_reason.is_none());
        assert_eq!(
            mailbox
                .list_pending(&source.owner_session_id)
                .unwrap()
                .len(),
            usize::from(initially_active)
        );
        assert_eq!(
            mailbox
                .completion_continuation_acceptance(&source.registration_id)
                .unwrap()
                .unwrap()["phase"],
            "accepted"
        );
        if missing || initially_active {
            // Reconstruct a synthetic pre-policy sidecar from this verified
            // accepted fixture. No production data or old running writer.
            crate::mailbox::remove_completion_recovery_working_set_for_legacy_fixture(
                mailbox.connection(),
            );
            mailbox
                .connection()
                .execute_batch(
                    "DROP VIEW mailbox_retained_delivery_finalizers;
                DROP INDEX mailbox_completed_turn_pins_attempt;
                DROP TABLE mailbox_completed_turn_pins;
                DROP TABLE mailbox_completed_turn_tails;
                DROP TRIGGER completion_continuation_notification_ack;
                DROP TABLE completion_continuation_notification; PRAGMA user_version=18;",
                )
                .unwrap();
            mailbox = MailboxDb::open(&MailboxDb::path_for_state_db(&path)).unwrap();
            assert_eq!(
                mailbox
                    .completion_notification_diagnostics(&source.handle)
                    .unwrap()[0]["policy"],
                "unknown"
            );
            state
                .repair_admitted_completion_continuation(
                    crate::InvocationMutationAuthority::Standalone,
                    &binding,
                )
                .unwrap();
            let historical = mailbox
                .completion_notification_diagnostics(&source.handle)
                .unwrap();
            assert_eq!(
                historical[0]["policy"],
                if initially_active {
                    "notify"
                } else {
                    "response_only"
                }
            );
            assert!(
                historical[0]["requested_at"].is_null(),
                "historical active is not invented detach"
            );
            assert_eq!(
                mailbox.completion_event_listeners(&source.handle).unwrap(),
                listeners
            );
        }
        if mode == "sync" && !initially_active {
            // Response-only is settled notification policy, never an ACK.
            assert!(mailbox.pending_continuation_attempts().unwrap().is_empty());
            let mut owner = mailbox.completion_continuation_owner().unwrap().unwrap();
            let facts = mailbox
                .completion_notification_diagnostics(&source.handle)
                .unwrap();
            assert_eq!(facts[0]["disposition"], "no_notification_required");
            assert_eq!(facts[0]["policy"], "response_only");
            if physical_debt {
                let attempt = crate::mailbox::ContinuationAttempt {
                    attempt_id: uuid::Uuid::new_v4().to_string(),
                    owner_generation: owner.owner_generation.clone(),
                    operation: "transport".into(),
                    request_sha256: "a".repeat(64),
                    source_registration_id: None,
                    source_listener_revision: None,
                    session_id: None,
                    claim_token: None,
                    result_path: directory
                        .path()
                        .join("unproduced-receipt.json")
                        .to_str()
                        .unwrap()
                        .into(),
                };
                mailbox.reserve_continuation_attempt(&attempt).unwrap();
                mailbox.accept_continuation_attempt(&attempt).unwrap();
                mailbox
                    .attach_original_continuation_custody(
                        &attempt,
                        &owner.driver_identity,
                        &owner.driver_identity,
                        &owner.driver_identity,
                    )
                    .unwrap();
                assert!(
                    !state
                        .close_idle_completion_continuation_owner(&owner)
                        .unwrap()
                );
                assert!(
                    !state
                        .close_idle_completion_continuation_owner(&owner)
                        .unwrap()
                );
                assert_eq!(
                    mailbox.pending_continuation_attempts().unwrap(),
                    vec![attempt]
                );
                assert!(
                    mailbox.completion_event_listeners(&source.handle).unwrap()[0]
                        .acknowledged_at
                        .is_none()
                );
                return; // Never manufacture a physical receipt to finish this fixture.
            }
            assert!(
                state
                    .close_idle_completion_continuation_owner(&owner)
                    .unwrap()
            );
            assert!(
                !state
                    .close_idle_completion_continuation_owner(&owner)
                    .unwrap()
            );
            assert!(
                mailbox
                    .request_original_completion_notification(&source.handle)
                    .unwrap_err()
                    .contains("completion_owner_closed_retryable")
            );
            assert!(
                mailbox
                    .completion_notification_diagnostics(&source.handle)
                    .unwrap()[0]["requested_at"]
                    .is_null()
            );
            let retained = mailbox.completion_event_listeners(&source.handle).unwrap();
            assert!(!retained[0].active);
            assert!(retained[0].mailbox_seq.is_none());
            assert!(retained[0].acknowledged_at.is_none());
            assert!(retained[0].acknowledgement_reason.is_none());
            assert!(mailbox.completion_continuation_owner().unwrap().is_none());
            // Synthetic successor for the remaining late-listener/detach checks.
            // This is not an operational old-owner cutover test.
            owner.owner_generation = uuid::Uuid::new_v4().to_string();
            mailbox
                .publish_completion_continuation_owner(&owner)
                .unwrap();
        }
        let changed = CompletionEventTriggerInput {
            payload_json: r#"{"kind":"agent_bash_complete","rc":70}"#,
            ..input
        };
        assert!(
            mailbox
                .trigger_completion_continuation(changed, &binding, &evidence)
                .is_err()
        );
        let late_owner = "99999999-9999-4999-8999-999999999999";
        state
            .start_invocation(&InvocationStart {
                invocation_uuid: late_owner.into(),
                model_name: "fixture".into(),
                provider_name: "fixture".into(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        let late = binding
            .for_listener(
                "late-listener-admission",
                crate::completion_continuation::ListenerIdentity {
                    listener_id: late_owner.into(),
                    session_id: "late-session".into(),
                    owner_invocation_uuid: late_owner.into(),
                },
            )
            .unwrap();
        mailbox
            .set_notifications_paused("late-session", true)
            .unwrap();
        assert!(catch_unwind(AssertUnwindSafe(|| admit(&mut state, &late, false, true))).is_err());
        assert_eq!(
            state.admitted_completion_continuations().unwrap(),
            vec![binding.clone(), late.clone()]
        );
        assert_eq!(
            mailbox
                .completion_event_listeners(&source.handle)
                .unwrap()
                .len(),
            1
        );
        if batch_repair {
            // Root Act2: with contexts absent and original accepted, the
            // independent State-committed listener still forbids retirement.
            let owner = mailbox.completion_continuation_owner().unwrap().unwrap();
            assert!(
                !state
                    .close_idle_completion_continuation_owner(&owner)
                    .unwrap()
            );
            assert!(mailbox.completion_contexts().unwrap().is_empty());
            assert!(mailbox.completion_continuation_owner().unwrap().is_some());
            assert_eq!(
                state
                    .repair_domain_completion_continuations(&source.domain_id)
                    .unwrap(),
                // The outstanding late State commit is the next continuity
                // ordinal. Earlier repairs cannot jump that projection debt.
                vec![late.clone()]
            );
            // Once the exact committed debt is materialized, the original is
            // still owed: a later ordinary pass must retain both bindings.
            assert_eq!(
                state
                    .repair_domain_completion_continuations(&source.domain_id)
                    .unwrap(),
                vec![binding.clone(), late.clone()]
            );
            assert!(
                state
                    .repair_domain_completion_continuations("not-this-domain")
                    .unwrap()
                    .is_empty()
            );
        } else {
            state
                .repair_admitted_completion_continuation(
                    crate::InvocationMutationAuthority::Standalone,
                    &late,
                )
                .unwrap();
        }
        let listeners = mailbox.completion_event_listeners(&source.handle).unwrap();
        assert_eq!(listeners.len(), 2);
        let late_listener = listeners
            .iter()
            .find(|l| l.session_id == "late-session")
            .unwrap();
        assert!(late_listener.active && late_listener.acknowledged_at.is_none());
        let pending = mailbox.list_pending("late-session").unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].payload_sha256, first.event.payload_sha256);
        let provenance: String = mailbox
            .connection()
            .query_row(
                "SELECT completion_provenance FROM mailbox WHERE seq=?1",
                [pending[0].seq],
                |record| record.get(0),
            )
            .unwrap();
        assert_eq!(
            provenance, "v2",
            "repaired listener materialization retains v2 provenance"
        );
        assert!(std::path::Path::new(pending[0].payload_file_path.as_deref().unwrap()).is_file());
        assert!(mailbox.notifications_paused("late-session").unwrap());
        assert!(
            listeners
                .iter()
                .find(|l| l.session_id == source.owner_session_id)
                .unwrap()
                .acknowledged_at
                .is_none()
        );
        // Late-listener repair uses original source ownership, not the repairing caller.
        admit(&mut state, &late, false, false);
        let replay = mailbox
            .trigger_completion_continuation(input, &late, &evidence)
            .unwrap();
        let owner = replay
            .listeners
            .iter()
            .find(|l| l.session_id == source.owner_session_id)
            .unwrap();
        assert_eq!(owner.active, initially_active);
        assert!(owner.acknowledged_at.is_none());
        assert_eq!(mailbox.list_pending("late-session").unwrap().len(), 1);
        // Explicit post-completion detach selects notification, not ACK.
        let detached = mailbox
            .request_original_completion_notification(&source.handle)
            .unwrap();
        assert_eq!(detached.mailbox_rows.len(), 2);
        admit(&mut state, &binding, false, false);
        let replay = mailbox
            .trigger_completion_continuation(input, &binding, &evidence)
            .unwrap();
        assert_eq!(
            detached
                .listeners
                .iter()
                .find(|l| l.session_id == source.owner_session_id)
                .unwrap()
                .mailbox_seq,
            replay
                .listeners
                .iter()
                .find(|l| l.session_id == source.owner_session_id)
                .unwrap()
                .mailbox_seq
        );
        assert!(replay.listeners[0].acknowledged_at.is_none());
        let seq = detached
            .listeners
            .iter()
            .find(|l| l.session_id == source.owner_session_id)
            .unwrap()
            .mailbox_seq
            .unwrap();
        mailbox
            .acknowledge_range(
                &source.owner_session_id,
                seq,
                seq,
                &source.owner_invocation_uuid,
            )
            .unwrap();
        let provenance_after_ack: String = mailbox
            .connection()
            .query_row(
                "SELECT completion_provenance FROM mailbox WHERE seq=?1",
                [seq],
                |record| record.get(0),
            )
            .unwrap();
        assert_eq!(
            provenance_after_ack, "v2",
            "recipient ACK cannot erase source provenance"
        );
        let ack = mailbox.completion_event_listeners(&source.handle).unwrap();
        assert!(
            ack.iter()
                .find(|l| l.session_id == source.owner_session_id)
                .unwrap()
                .acknowledged_at
                .is_some()
        );
        assert!(
            ack.iter()
                .find(|l| l.session_id == "late-session")
                .unwrap()
                .acknowledged_at
                .is_none()
        );
        assert_ne!(
            ack[0].acknowledgement_reason.as_deref(),
            Some("consumed_in_call")
        );
        let facts = mailbox
            .completion_notification_diagnostics(&source.handle)
            .unwrap();
        let original = facts
            .iter()
            .find(|f| f["listener_id"] == source.owner_invocation_uuid)
            .unwrap();
        assert_eq!(original["disposition"], "handled");
        assert_eq!(original["delivery_evidence"], "manual_assertion");
        assert_eq!(original["ack_actor_label"], source.owner_invocation_uuid);
        assert_eq!(original["ack_original_mailbox_seq"], seq);
        assert!(original["native_receipt_evidence"].is_null());
        let late = facts
            .iter()
            .find(|f| f["listener_id"] == late_owner)
            .unwrap();
        assert_eq!(late["policy"], "notify");
        assert!(
            late["requested_at"].is_null(),
            "original detach must not request independent listeners"
        );
        assert_eq!(late["disposition"], "pending");
        let owner = mailbox.completion_continuation_owner().unwrap().unwrap();
        assert!(
            !state
                .close_idle_completion_continuation_owner(&owner)
                .unwrap(),
            "independent notification debt remains"
        );
        let before = original.clone();
        mailbox
            .request_original_completion_notification(&source.handle)
            .unwrap();
        let after = mailbox
            .completion_notification_diagnostics(&source.handle)
            .unwrap();
        assert_eq!(
            &before,
            after
                .iter()
                .find(|f| f["listener_id"] == source.owner_invocation_uuid)
                .unwrap()
        );
        if batch_repair {
            let late_seq = mailbox.list_pending("late-session").unwrap()[0].seq;
            mailbox
                .acknowledge_range("late-session", late_seq, late_seq, late_owner)
                .unwrap();
            assert!(
                state
                    .close_idle_completion_continuation_owner(&owner)
                    .unwrap()
            );
            assert!(mailbox.completion_continuation_owner().unwrap().is_none());
            assert_eq!(state.admitted_completion_continuations().unwrap().len(), 2);
        }
    }

    #[test]
    fn storage_projected_history_requires_no_state_or_sidecar_writer() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.db");
        let binding = binding();
        let mut state = seed(&path, &binding);
        state.access_scope = crate::live_history::AccessScope::historical();
        admit(&mut state, &binding, false, false);
        let source = binding.registration().unwrap();
        let state_writer = sqlite::Connection::open(&path).unwrap();
        let sidecar_writer = sqlite::Connection::open(MailboxDb::path_for_state_db(&path)).unwrap();
        state_writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        sidecar_writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        assert_eq!(
            state
                .repair_domain_completion_continuations(&source.domain_id)
                .unwrap(),
            vec![binding.clone()]
        );
        // Same unaccepted source and no new useful demand: repeat, with both
        // real writers still obstructing any attempted repair reservation.
        assert_eq!(
            state
                .repair_domain_completion_continuations(&source.domain_id)
                .unwrap(),
            vec![binding]
        );
        sidecar_writer.execute_batch("ROLLBACK").unwrap();
        state_writer.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn completion_continuation_exact_repair_does_not_reacquire_provider_launch_authority() {
        use crate::{
            BeginProviderLaunchRequest, ProviderLaunchAttemptAllocation, ProviderLaunchCandidate,
            ProviderLaunchStartMode,
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.db");
        let mut state = StateDb::open(&path).unwrap();
        seed_domain(&path);
        let request = BeginProviderLaunchRequest {
            logical_launch_id: uuid::Uuid::new_v4(),
            request_identity_sha256: "a".repeat(64),
            model_name: "fixture".into(),
            start_mode: ProviderLaunchStartMode::Create,
            expected_provider_session_id: None,
            candidates: vec![ProviderLaunchCandidate {
                provider_index: 0,
                account_name: "fixture".into(),
            }],
            parent_invocation_id: None,
            allocation: ProviderLaunchAttemptAllocation::allocate().unwrap(),
        };
        let lease = state.begin_launch(&request).unwrap();
        state
            .activate_attempt(&lease, &request.allocation.completion_authority)
            .unwrap();
        state
            .bind_invocation_provider_session_start(
                crate::InvocationMutationAuthority::ProviderLaunch(&lease.owner),
                lease.owner.invocation_row_id,
                &crate::ProviderSessionBinding {
                    provider_session_id: "fixture-session".into(),
                    capture_method: "provider_live_report",
                    resume_input_id: None,
                    provider_session_resolved_account: None,
                },
            )
            .unwrap();
        let original = binding();
        let bytes = std::str::from_utf8(original.registration_bytes())
            .unwrap()
            .replace(
                &original.registration().unwrap().owner_invocation_uuid,
                &lease.owner.invocation_uuid.to_string(),
            );
        let binding =
            AdmittedSourceBinding::new(original.caller_admission_id(), bytes.as_bytes()).unwrap();
        let source = binding.registration().unwrap();
        let paths = source.paths();
        let registration = CompletionEventRegistrationInput {
            event_id: &source.handle,
            delivery_mode: &source.delivery_mode,
            owner_session_id: Some(&source.owner_session_id),
            owner_invocation_uuid: Some(&source.owner_invocation_uuid),
            state_dir: &source.handle_dir,
            meta_path: &paths[0],
            log_path: &paths[1],
            rc_path: &paths[2],
        };
        assert!(
            catch_unwind(AssertUnwindSafe(|| {
                state
                    .register_completion_event_with_binding_on(
                        crate::InvocationMutationAuthority::Standalone,
                        Some(&request.allocation.completion_authority),
                        false,
                        binding.caller_admission_id(),
                        registration,
                        Some(&binding),
                        || {},
                        || panic!("lost registration after State commit"),
                    )
                    .unwrap();
            }))
            .is_err()
        );
        assert_eq!(
            state.admitted_completion_continuation(&binding).unwrap(),
            Some(binding.clone())
        );
        assert!(
            state
                .repair_admitted_completion_continuation(
                    crate::InvocationMutationAuthority::Standalone,
                    &binding,
                )
                .unwrap()
                .inserted
        );
        // This exception is exact materialization, not a generic owner bypass.
        let changed = AdmittedSourceBinding::new(
            binding.caller_admission_id(),
            bytes.replace("ab_fixture", "ab_other").as_bytes(),
        )
        .unwrap();
        assert!(
            state
                .repair_admitted_completion_continuation(
                    crate::InvocationMutationAuthority::Standalone,
                    &changed,
                )
                .is_err()
        );
    }

    #[test]
    fn completion_retirement_yields_busy_sidecar_without_blocking_cancellation() {
        use crate::{
            BeginProviderLaunchRequest, ProviderLaunchAttemptAllocation, ProviderLaunchCandidate,
            ProviderLaunchStartMode,
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.db");
        let state = StateDb::open(&path).unwrap();
        seed_domain(&path);
        let request = BeginProviderLaunchRequest {
            logical_launch_id: uuid::Uuid::new_v4(),
            request_identity_sha256: "a".repeat(64),
            model_name: "fixture".into(),
            start_mode: ProviderLaunchStartMode::Create,
            expected_provider_session_id: None,
            candidates: vec![ProviderLaunchCandidate {
                provider_index: 0,
                account_name: "fixture".into(),
            }],
            parent_invocation_id: None,
            allocation: ProviderLaunchAttemptAllocation::allocate().unwrap(),
        };
        state.begin_launch(&request).unwrap();
        let mailbox = MailboxDb::open(&MailboxDb::path_for_state_db(&path)).unwrap();
        let owner = mailbox.completion_continuation_owner().unwrap().unwrap();
        // Known holder and actual successful statement, not inferred proc wchan.
        mailbox
            .connection()
            .execute_batch("BEGIN IMMEDIATE")
            .unwrap();
        let (send, recv) = std::sync::mpsc::channel();
        let worker_path = path.clone();
        let worker_owner = owner.clone();
        let worker = std::thread::spawn(move || {
            let state = StateDb::open(&worker_path).unwrap();
            send.send(state.close_idle_completion_continuation_owner(&worker_owner))
                .unwrap();
        });
        // The control intentionally keeps the sidecar writer until observation.
        // This is an experiment bound below SQLite's existing five-second wait,
        // not a change to the product cancellation budget.
        let retirement = recv.recv_timeout(std::time::Duration::from_secs(2));
        let cancellation = if retirement.is_ok() {
            Some(state.request_cancel(request.logical_launch_id))
        } else {
            None
        };
        mailbox.connection().execute_batch("ROLLBACK").unwrap();
        worker.join().unwrap();
        assert!(!retirement.unwrap().unwrap());
        assert!(cancellation.unwrap().is_ok());
        assert_eq!(
            serde_json::to_value(mailbox.completion_continuation_owner().unwrap()).unwrap(),
            serde_json::to_value(Some(owner.clone())).unwrap()
        );
        assert!(
            state
                .close_idle_completion_continuation_owner(&owner)
                .unwrap()
        );
        println!(
            "known_sidecar_holder=fixture BEGIN_IMMEDIATE=ok retirement=deferred cancellation=accepted ROLLBACK=ok retry=closed"
        );
    }

    #[test]
    fn completion_continuation_state_commit_before_sidecar_is_recoverable_without_source_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.db");
        let binding = binding();
        let mut state = seed(&path, &binding);
        assert!(
            catch_unwind(AssertUnwindSafe(|| admit(
                &mut state, &binding, false, true
            )))
            .is_err()
        );
        drop(state);
        let mut reopened = StateDb::open(&path).unwrap();
        reopened.access_scope = crate::live_history::AccessScope::historical();
        let admitted = reopened.admitted_completion_continuations().unwrap();
        assert_eq!(admitted, vec![binding.clone()]);
        let owner = MailboxDb::open(&MailboxDb::path_for_state_db(&path))
            .unwrap()
            .completion_continuation_owner()
            .unwrap()
            .unwrap();
        assert!(
            !reopened
                .close_idle_completion_continuation_owner(&owner)
                .unwrap(),
            "State-committed / sidecar-absent source prevents idle retirement"
        );
        let sidecar = MailboxDb::path_for_state_db(&path);
        assert!(
            MailboxDb::open(&sidecar)
                .unwrap()
                .completion_event("ab_fixture")
                .unwrap()
                .is_none()
        );
        assert!(
            reopened
                .repair_admitted_completion_continuation(
                    crate::InvocationMutationAuthority::Standalone,
                    &admitted[0],
                )
                .unwrap()
                .inserted
        );
        assert!(
            MailboxDb::open(&sidecar)
                .unwrap()
                .completion_event("ab_fixture")
                .unwrap()
                .is_some()
        );
        assert_eq!(
            reopened.admitted_completion_continuations().unwrap(),
            vec![binding]
        );
    }

    #[test]
    fn completion_continuation_sidecar_commit_lost_response_repairs_idempotently() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.db");
        let binding = binding();
        let mut state = seed(&path, &binding);
        // Throw away the operation response after both commits, then reopen.
        admit(&mut state, &binding, false, false);
        drop(state);
        let mut reopened = StateDb::open(&path).unwrap();
        reopened.access_scope = crate::live_history::AccessScope::historical();
        assert_eq!(
            reopened.admitted_completion_continuations().unwrap(),
            vec![binding.clone()]
        );
        assert!(
            !reopened
                .repair_admitted_completion_continuation(
                    crate::InvocationMutationAuthority::Standalone,
                    &binding,
                )
                .unwrap()
                .inserted
        );
        let count: i64 = reopened
            .conn
            .query_row(
                "SELECT COUNT(*) FROM invocation_completion_continuity",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn completion_continuation_uncommitted_intent_is_not_recovery_authority() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.db");
        let binding = binding();
        let mut state = seed(&path, &binding);
        state.access_scope = crate::live_history::AccessScope::historical();
        assert!(
            catch_unwind(AssertUnwindSafe(|| admit(
                &mut state, &binding, true, false
            )))
            .is_err()
        );
        assert!(
            state
                .admitted_completion_continuations()
                .unwrap()
                .is_empty()
        );
        assert!(
            state
                .repair_admitted_completion_continuation(
                    crate::InvocationMutationAuthority::Standalone,
                    &binding,
                )
                .unwrap_err()
                .contains("exact admitted replay")
        );
    }

    #[test]
    fn completion_continuation_binding_cannot_be_changed_or_rebound() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.db");
        let binding = binding();
        let mut state = seed(&path, &binding);
        state.access_scope = crate::live_history::AccessScope::historical();
        admit(&mut state, &binding, false, false);
        assert!(
            state
                .conn
                .execute(
                    "UPDATE invocation_completion_obligations SET completion_v2_binding=NULL",
                    []
                )
                .unwrap_err()
                .to_string()
                .contains("append-only")
        );
        let bytes = String::from_utf8(binding.registration_bytes().to_vec())
            .unwrap()
            .replace(
                "22222222-2222-4222-8222-222222222222",
                "77777777-7777-4777-8777-777777777777",
            );
        let changed =
            AdmittedSourceBinding::new(binding.caller_admission_id(), bytes.as_bytes()).unwrap();
        assert!(
            state
                .repair_admitted_completion_continuation(
                    crate::InvocationMutationAuthority::Standalone,
                    &changed,
                )
                .is_err()
        );
        assert_eq!(
            state.admitted_completion_continuations().unwrap(),
            vec![binding]
        );
    }
}
