//! ## Declared roles
//!
//! `orchestration`, `mapper`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/fresh_continuation/state_db_store.rs
//!     role: adapter
//!     Translates:
//!       - runtime-fresh-continuation-store-contract
//!       - state-continuation-repository-contract
//! ```

use super::contract::{
    AcceptDecision, AcceptedContinuation, ContinuationBlock, ContinuationBlockKind,
    ContinuationStore, FreshContinuationOutcome, HistoricalParentAdmission,
    HistoricalParentAuthorityClaim, InvocationDisposition, InvocationOutcome, PublishedHandoff,
    ReservedInvocation, ResumeAcceptance, RunDecision, ValidatedContinuation,
};
use oulipoly_state::StateDb;
use oulipoly_state::continuation::{
    ContinuationAcceptInput, ContinuationAcceptResult, ContinuationInvocationDisposition,
    ContinuationInvocationOutcome, ContinuationPublishedHandoff, ContinuationRecord,
    ContinuationRepositoryError, ContinuationReservation, ContinuationResumeAcceptance,
    ContinuationRunDecision, ContinuationTerminalOutcome,
};
use oulipoly_state::repositories::ContinuationRepository;
use sha2::{Digest, Sha256};

pub struct StateDbContinuationStore {
    state: StateDb,
}

impl StateDbContinuationStore {
    pub fn new(state: StateDb) -> Self {
        Self { state }
    }

    pub fn historical_parent_admission(
        &self,
        continuation: &AcceptedContinuation,
        claim: HistoricalParentAuthorityClaim<'_>,
    ) -> Result<Option<HistoricalParentAdmission>, ContinuationBlock> {
        let Some(provenance) = continuation.historical_provenance() else {
            return Ok(None);
        };
        if continuation.continuation_id != provenance.continuation_id
            || claim.continuation_id != provenance.continuation_id
            || continuation.context.fingerprint() != provenance.validated_fingerprint
            || continuation.context.request().origin_invocation_id
                != provenance.origin_invocation_id
            || continuation.resume != provenance.resume
            || continuation.fresh != provenance.fresh
        {
            return Ok(None);
        }
        let authorized = self
            .state
            .connection()
            .query_row(
                "SELECT EXISTS (
                    SELECT 1
                    FROM fresh_continuations AS durable
                    JOIN invocations AS origin
                      ON origin.invocation_uuid = ?4
                    JOIN invocations AS parent
                      ON parent.invocation_uuid = ?9
                    WHERE durable.logical_request_key = ?1
                      AND durable.validated_fingerprint = ?2
                      AND durable.continuation_id = ?3
                      AND durable.resume_invocation_id = ?5
                      AND durable.resume_parent_invocation_id = ?6
                      AND durable.fresh_invocation_id = ?7
                      AND durable.fresh_parent_invocation_id = ?8
                      AND (
                        (durable.resume_parent_invocation_id = ?9
                         AND durable.resume_invocation_id = ?10)
                        OR
                        (durable.fresh_parent_invocation_id = ?9
                         AND durable.fresh_invocation_id = ?10)
                      )
                )",
                rusqlite::params![
                    provenance.logical_request_key,
                    provenance.validated_fingerprint,
                    provenance.continuation_id,
                    provenance.origin_invocation_id,
                    provenance.resume.invocation_id,
                    provenance.resume.parent_invocation_id,
                    provenance.fresh.invocation_id,
                    provenance.fresh.parent_invocation_id,
                    claim.parent_invocation_uuid,
                    claim.child_invocation_uuid,
                ],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|error| ContinuationBlock {
                kind: ContinuationBlockKind::Persistence,
                message: format!("validate historical parent authority: {error}"),
            })?;
        Ok(authorized.then(|| {
            HistoricalParentAdmission::new(
                claim.parent_invocation_uuid.to_string(),
                claim.child_invocation_uuid.to_string(),
                provenance.continuation_id.clone(),
            )
        }))
    }
}

impl ContinuationStore for StateDbContinuationStore {
    fn accept(
        &mut self,
        context: &ValidatedContinuation,
    ) -> Result<AcceptDecision, ContinuationBlock> {
        let input = continuation_accept_input(context);
        let result = self
            .state
            .accept_continuation(&input)
            .map_err(map_repository_error)?;
        Ok(runtime_accept_decision(result, context))
    }

    fn begin_resume(
        &mut self,
        continuation: &AcceptedContinuation,
    ) -> Result<RunDecision, ContinuationBlock> {
        self.state
            .begin_continuation_resume(&state_record(continuation))
            .map(runtime_run_decision)
            .map_err(map_repository_error)
    }

    fn record_resume(
        &mut self,
        continuation: &AcceptedContinuation,
        outcome: &InvocationOutcome,
    ) -> Result<(), ContinuationBlock> {
        self.state
            .record_continuation_resume(&state_record(continuation), &state_outcome(outcome))
            .map_err(map_repository_error)
    }

    fn begin_fresh(
        &mut self,
        continuation: &AcceptedContinuation,
    ) -> Result<RunDecision, ContinuationBlock> {
        self.state
            .begin_continuation_fresh(&state_record(continuation))
            .map(runtime_run_decision)
            .map_err(map_repository_error)
    }

    fn record_fresh(
        &mut self,
        continuation: &AcceptedContinuation,
        outcome: &InvocationOutcome,
    ) -> Result<(), ContinuationBlock> {
        self.state
            .record_continuation_fresh(&state_record(continuation), &state_outcome(outcome))
            .map_err(map_repository_error)
    }

    fn finish(
        &mut self,
        continuation: &AcceptedContinuation,
        handoff: &PublishedHandoff,
    ) -> Result<FreshContinuationOutcome, ContinuationBlock> {
        self.state
            .finish_continuation(&state_record(continuation), &state_handoff(handoff))
            .map(runtime_terminal)
            .map_err(map_repository_error)
    }
}

fn continuation_accept_input(context: &ValidatedContinuation) -> ContinuationAcceptInput {
    ContinuationAcceptInput {
        logical_request_key: logical_request_key(context),
        fingerprint: context.fingerprint().to_string(),
        origin_invocation_id: context.request().origin_invocation_id.clone(),
    }
}

fn runtime_accept_decision(
    result: ContinuationAcceptResult,
    context: &ValidatedContinuation,
) -> AcceptDecision {
    match result {
        ContinuationAcceptResult::Accepted(record) => {
            AcceptDecision::Accepted(Box::new(accepted_continuation(record, context.clone())))
        }
        ContinuationAcceptResult::Replay(terminal) => {
            AcceptDecision::Replay(Box::new(runtime_terminal(terminal)))
        }
    }
}

fn logical_request_key(context: &ValidatedContinuation) -> String {
    let mut digest = Sha256::new();
    logical_key_part(&mut digest, b"fresh-continuation-logical-request-v1");
    logical_key_part(&mut digest, context.request().question_id.as_bytes());
    logical_key_part(
        &mut digest,
        context.request().origin_invocation_id.as_bytes(),
    );
    format!("{:x}", digest.finalize())
}

fn logical_key_part(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

fn accepted_continuation(
    record: ContinuationRecord,
    context: ValidatedContinuation,
) -> AcceptedContinuation {
    AcceptedContinuation::from_validated_store(
        record.logical_request_key,
        record.continuation_id,
        context,
        runtime_reservation(record.resume),
        runtime_reservation(record.fresh),
    )
}

fn state_record(continuation: &AcceptedContinuation) -> ContinuationRecord {
    ContinuationRecord {
        logical_request_key: logical_request_key(&continuation.context),
        continuation_id: continuation.continuation_id.clone(),
        fingerprint: continuation.context.fingerprint().to_string(),
        resume: state_reservation(&continuation.resume),
        fresh: state_reservation(&continuation.fresh),
    }
}

fn state_reservation(reservation: &ReservedInvocation) -> ContinuationReservation {
    ContinuationReservation {
        invocation_id: reservation.invocation_id.clone(),
        parent_invocation_id: reservation.parent_invocation_id.clone(),
    }
}

fn runtime_reservation(reservation: ContinuationReservation) -> ReservedInvocation {
    ReservedInvocation {
        invocation_id: reservation.invocation_id,
        parent_invocation_id: reservation.parent_invocation_id,
    }
}

fn state_outcome(outcome: &InvocationOutcome) -> ContinuationInvocationOutcome {
    ContinuationInvocationOutcome {
        invocation_id: outcome.invocation_id.clone(),
        session_id: outcome.session_id.clone(),
        physical_exit_code: outcome.physical_exit_code,
        acceptance: match outcome.acceptance {
            ResumeAcceptance::Accepted => ContinuationResumeAcceptance::Accepted,
            ResumeAcceptance::Rejected => ContinuationResumeAcceptance::Rejected,
            ResumeAcceptance::Unconfirmed => ContinuationResumeAcceptance::Unconfirmed,
            ResumeAcceptance::NotApplicable => ContinuationResumeAcceptance::NotApplicable,
        },
        disposition: match &outcome.disposition {
            InvocationDisposition::Succeeded => ContinuationInvocationDisposition::Succeeded,
            InvocationDisposition::Failed {
                error_category,
                terminal_reason,
            } => ContinuationInvocationDisposition::Failed {
                error_category: error_category.clone(),
                terminal_reason: terminal_reason.clone(),
            },
        },
    }
}

fn runtime_outcome(outcome: ContinuationInvocationOutcome) -> InvocationOutcome {
    InvocationOutcome {
        invocation_id: outcome.invocation_id,
        session_id: outcome.session_id,
        physical_exit_code: outcome.physical_exit_code,
        acceptance: match outcome.acceptance {
            ContinuationResumeAcceptance::Accepted => ResumeAcceptance::Accepted,
            ContinuationResumeAcceptance::Rejected => ResumeAcceptance::Rejected,
            ContinuationResumeAcceptance::Unconfirmed => ResumeAcceptance::Unconfirmed,
            ContinuationResumeAcceptance::NotApplicable => ResumeAcceptance::NotApplicable,
        },
        disposition: match outcome.disposition {
            ContinuationInvocationDisposition::Succeeded => InvocationDisposition::Succeeded,
            ContinuationInvocationDisposition::Failed {
                error_category,
                terminal_reason,
            } => InvocationDisposition::Failed {
                error_category,
                terminal_reason,
            },
        },
    }
}

fn state_handoff(handoff: &PublishedHandoff) -> ContinuationPublishedHandoff {
    ContinuationPublishedHandoff {
        path: handoff.path.clone(),
        sha256: handoff.sha256.clone(),
    }
}

fn runtime_handoff(handoff: ContinuationPublishedHandoff) -> PublishedHandoff {
    PublishedHandoff {
        path: handoff.path,
        sha256: handoff.sha256,
    }
}

fn runtime_run_decision(decision: ContinuationRunDecision) -> RunDecision {
    match decision {
        ContinuationRunDecision::Run(reservation) => {
            RunDecision::Run(runtime_reservation(reservation))
        }
        ContinuationRunDecision::Observe(reservation) => {
            RunDecision::Observe(runtime_reservation(reservation))
        }
        ContinuationRunDecision::Terminal(terminal) => {
            RunDecision::Terminal(Box::new(runtime_terminal(*terminal)))
        }
    }
}

fn runtime_terminal(terminal: ContinuationTerminalOutcome) -> FreshContinuationOutcome {
    match terminal {
        ContinuationTerminalOutcome::Continued {
            continuation_id,
            resume,
            fresh,
            handoff,
        } => FreshContinuationOutcome::Continued {
            continuation_id,
            resume: runtime_outcome(resume),
            fresh: runtime_outcome(fresh),
            handoff: runtime_handoff(handoff),
        },
        ContinuationTerminalOutcome::Failed {
            continuation_id,
            resume,
            fresh,
            handoff,
            reason,
        } => FreshContinuationOutcome::Failed {
            continuation_id,
            resume: runtime_outcome(resume),
            fresh: Some(runtime_outcome(fresh)),
            handoff: Some(runtime_handoff(handoff)),
            reason: ContinuationBlock {
                kind: ContinuationBlockKind::InvocationFailed,
                message: reason,
            },
        },
    }
}

fn map_repository_error(error: ContinuationRepositoryError) -> ContinuationBlock {
    match error {
        ContinuationRepositoryError::Conflict(message) => ContinuationBlock {
            kind: ContinuationBlockKind::Conflict,
            message,
        },
        ContinuationRepositoryError::AmbiguousState(message) => ContinuationBlock {
            kind: ContinuationBlockKind::AmbiguousState,
            message,
        },
        ContinuationRepositoryError::Persistence(message) => ContinuationBlock {
            kind: ContinuationBlockKind::Persistence,
            message,
        },
    }
}
