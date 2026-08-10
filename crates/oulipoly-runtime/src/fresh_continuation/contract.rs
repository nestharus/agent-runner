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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvocationAction {
    Run,
    Observe,
}

pub trait ContinuationEvidenceValidator {
    fn validate(
        &mut self,
        request: &FreshContinuationRequest,
    ) -> Result<ValidatedContinuation, ContinuationBlock>;
}

pub trait ContinuationArtifactSource {
    fn read(&mut self, artifact: &ArtifactIdentity) -> Result<Vec<u8>, ContinuationBlock>;
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
        action: InvocationAction,
        reservation: &ReservedInvocation,
        context: &ValidatedContinuation,
    ) -> InvocationOutcome;
}

pub trait FreshRunner {
    fn run_or_observe(
        &mut self,
        action: InvocationAction,
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
