use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactIdentity {
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinuationEvidence {
    pub question: ArtifactIdentity,
    pub answer: ArtifactIdentity,
    pub session_graph: ArtifactIdentity,
    pub origin_trace: ArtifactIdentity,
    pub ticket_snapshot: ArtifactIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreshContinuationRequest {
    pub question_id: String,
    pub origin_invocation_id: String,
    pub origin_session_id: String,
    pub planning_root: PathBuf,
    pub worktree: PathBuf,
    pub last_successful_boundary: String,
    pub active_blocked_boundary: String,
    pub target_model: String,
    pub evidence: ContinuationEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedContinuation {
    pub request: FreshContinuationRequest,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservedInvocation {
    pub invocation_id: String,
    pub parent_invocation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeAcceptance {
    Accepted,
    Rejected,
    Unconfirmed,
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvocationDisposition {
    Succeeded,
    Failed {
        error_category: String,
        terminal_reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvocationOutcome {
    pub invocation_id: String,
    pub session_id: Option<String>,
    pub physical_exit_code: i32,
    pub acceptance: ResumeAcceptance,
    pub disposition: InvocationDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedContinuation {
    pub continuation_id: String,
    pub context: ValidatedContinuation,
    pub resume: ReservedInvocation,
    pub fresh: ReservedInvocation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContinuationBlockKind {
    InvalidEvidence,
    Conflict,
    TriggerNotMet,
    AmbiguousState,
    InvocationFailed,
    Persistence,
    Publication,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinuationBlock {
    pub kind: ContinuationBlockKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinuationHandoff {
    pub continuation_id: String,
    pub resume: InvocationOutcome,
    pub fresh: Option<InvocationOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedHandoff {
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FreshContinuationOutcome {
    Continued {
        continuation_id: String,
        resume: InvocationOutcome,
        fresh: InvocationOutcome,
        handoff: PublishedHandoff,
    },
    Blocked {
        continuation_id: Option<String>,
        resume: Option<InvocationOutcome>,
        fresh: Option<InvocationOutcome>,
        handoff: Option<PublishedHandoff>,
        reason: ContinuationBlock,
    },
    Failed {
        continuation_id: String,
        resume: InvocationOutcome,
        fresh: Option<InvocationOutcome>,
        handoff: Option<PublishedHandoff>,
        reason: ContinuationBlock,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcceptDecision {
    Accepted(Box<AcceptedContinuation>),
    Replay(Box<FreshContinuationOutcome>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunDecision {
    Run(ReservedInvocation),
    Observe(ReservedInvocation),
    Terminal(Box<FreshContinuationOutcome>),
}

pub trait ContinuationEvidenceValidator {
    fn validate(
        &mut self,
        request: &FreshContinuationRequest,
    ) -> Result<ValidatedContinuation, ContinuationBlock>;
}

pub trait ContinuationStore {
    fn accept(
        &mut self,
        context: &ValidatedContinuation,
    ) -> Result<AcceptDecision, ContinuationBlock>;

    fn begin_resume(
        &mut self,
        continuation: &AcceptedContinuation,
    ) -> Result<RunDecision, ContinuationBlock>;

    fn record_resume(
        &mut self,
        continuation: &AcceptedContinuation,
        outcome: &InvocationOutcome,
    ) -> Result<(), ContinuationBlock>;

    fn begin_fresh(
        &mut self,
        continuation: &AcceptedContinuation,
    ) -> Result<RunDecision, ContinuationBlock>;

    fn record_fresh(
        &mut self,
        continuation: &AcceptedContinuation,
        outcome: &InvocationOutcome,
    ) -> Result<(), ContinuationBlock>;

    fn finish(
        &mut self,
        continuation: &AcceptedContinuation,
        handoff: &PublishedHandoff,
    ) -> Result<FreshContinuationOutcome, ContinuationBlock>;
}

pub trait ResumeRunner {
    fn run_or_observe(
        &mut self,
        reservation: &ReservedInvocation,
        context: &ValidatedContinuation,
    ) -> InvocationOutcome;
}

pub trait FreshRunner {
    fn run_or_observe(
        &mut self,
        reservation: &ReservedInvocation,
        context: &ValidatedContinuation,
        resume: &InvocationOutcome,
    ) -> InvocationOutcome;
}

pub trait HandoffPublisher {
    fn publish(
        &mut self,
        handoff: ContinuationHandoff,
    ) -> Result<PublishedHandoff, ContinuationBlock>;
}

pub trait FreshContinuation {
    fn execute(&mut self, request: FreshContinuationRequest) -> FreshContinuationOutcome;
}

pub struct FreshContinuationCoordinator<Validator, Store, Resume, Fresh, Publisher> {
    validator: Validator,
    store: Store,
    resume: Resume,
    fresh: Fresh,
    publisher: Publisher,
}

impl<Validator, Store, Resume, Fresh, Publisher>
    FreshContinuationCoordinator<Validator, Store, Resume, Fresh, Publisher>
{
    pub fn new(
        validator: Validator,
        store: Store,
        resume: Resume,
        fresh: Fresh,
        publisher: Publisher,
    ) -> Self {
        Self {
            validator,
            store,
            resume,
            fresh,
            publisher,
        }
    }
}

impl<Validator, Store, Resume, Fresh, Publisher> FreshContinuation
    for FreshContinuationCoordinator<Validator, Store, Resume, Fresh, Publisher>
where
    Validator: ContinuationEvidenceValidator,
    Store: ContinuationStore,
    Resume: ResumeRunner,
    Fresh: FreshRunner,
    Publisher: HandoffPublisher,
{
    fn execute(&mut self, request: FreshContinuationRequest) -> FreshContinuationOutcome {
        let context = match self.validator.validate(&request) {
            Ok(context) => context,
            Err(reason) => {
                return FreshContinuationOutcome::Blocked {
                    continuation_id: None,
                    resume: None,
                    fresh: None,
                    handoff: None,
                    reason,
                };
            }
        };

        let continuation = match self.store.accept(&context) {
            Ok(AcceptDecision::Accepted(continuation)) => continuation,
            Ok(AcceptDecision::Replay(outcome)) => return *outcome,
            Err(reason) => {
                return FreshContinuationOutcome::Blocked {
                    continuation_id: None,
                    resume: None,
                    fresh: None,
                    handoff: None,
                    reason,
                };
            }
        };

        let resume_reservation = match self.store.begin_resume(&continuation) {
            Ok(RunDecision::Run(reservation) | RunDecision::Observe(reservation)) => reservation,
            Ok(RunDecision::Terminal(outcome)) => return *outcome,
            Err(reason) => {
                return FreshContinuationOutcome::Blocked {
                    continuation_id: Some(continuation.continuation_id.clone()),
                    resume: None,
                    fresh: None,
                    handoff: None,
                    reason,
                };
            }
        };

        let resume = self
            .resume
            .run_or_observe(&resume_reservation, &continuation.context);
        if let Err(reason) = self.store.record_resume(&continuation, &resume) {
            return FreshContinuationOutcome::Failed {
                continuation_id: continuation.continuation_id.clone(),
                resume,
                fresh: None,
                handoff: None,
                reason,
            };
        }

        if !is_fresh_continuation_trigger(&resume) {
            return FreshContinuationOutcome::Blocked {
                continuation_id: Some(continuation.continuation_id.clone()),
                resume: Some(resume),
                fresh: None,
                handoff: None,
                reason: ContinuationBlock {
                    kind: ContinuationBlockKind::TriggerNotMet,
                    message: "resume outcome does not meet the fresh-continuation trigger"
                        .to_string(),
                },
            };
        }

        let fresh_reservation = match self.store.begin_fresh(&continuation) {
            Ok(RunDecision::Run(reservation) | RunDecision::Observe(reservation)) => reservation,
            Ok(RunDecision::Terminal(outcome)) => return *outcome,
            Err(reason) => {
                return FreshContinuationOutcome::Failed {
                    continuation_id: continuation.continuation_id.clone(),
                    resume,
                    fresh: None,
                    handoff: None,
                    reason,
                };
            }
        };

        let fresh = self
            .fresh
            .run_or_observe(&fresh_reservation, &continuation.context, &resume);
        if let Err(reason) = self.store.record_fresh(&continuation, &fresh) {
            return FreshContinuationOutcome::Failed {
                continuation_id: continuation.continuation_id.clone(),
                resume,
                fresh: Some(fresh),
                handoff: None,
                reason,
            };
        }

        let handoff = match self.publisher.publish(ContinuationHandoff {
            continuation_id: continuation.continuation_id.clone(),
            resume: resume.clone(),
            fresh: Some(fresh.clone()),
        }) {
            Ok(handoff) => handoff,
            Err(reason) => {
                return FreshContinuationOutcome::Failed {
                    continuation_id: continuation.continuation_id.clone(),
                    resume,
                    fresh: Some(fresh),
                    handoff: None,
                    reason,
                };
            }
        };

        match self.store.finish(&continuation, &handoff) {
            Ok(outcome) => outcome,
            Err(reason) => FreshContinuationOutcome::Failed {
                continuation_id: continuation.continuation_id,
                resume,
                fresh: Some(fresh),
                handoff: Some(handoff),
                reason,
            },
        }
    }
}

fn is_fresh_continuation_trigger(outcome: &InvocationOutcome) -> bool {
    outcome.physical_exit_code == 0
        && outcome.acceptance == ResumeAcceptance::Accepted
        && matches!(
            &outcome.disposition,
            InvocationDisposition::Failed {
                error_category,
                terminal_reason,
            } if error_category == "resume_completion_unconfirmed"
                && terminal_reason == "resume_completion_unconfirmed"
        )
}
