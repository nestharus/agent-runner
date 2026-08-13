//! ## Declared roles
//!
//! `accessor`, `validator`, `orchestration`, `formatter`

use std::fmt::Write;
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

/// Complete continuation evidence validated by the production evidence validator.
/// The request and fingerprint are read-only so callers cannot manufacture or
/// rebind validation provenance.
///
/// ```compile_fail
/// use oulipoly_runtime::fresh_continuation::{FreshContinuationRequest, ValidatedContinuation};
///
/// fn forge(request: FreshContinuationRequest) {
///     let _ = ValidatedContinuation {
///         request,
///         fingerprint: "caller-chosen".to_string(),
///     };
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedContinuation {
    request: FreshContinuationRequest,
    fingerprint: String,
}

impl ValidatedContinuation {
    pub(super) fn from_validated_evidence(
        request: FreshContinuationRequest,
        fingerprint: String,
    ) -> Self {
        Self {
            request,
            fingerprint,
        }
    }

    pub fn request(&self) -> &FreshContinuationRequest {
        &self.request
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
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

/// A continuation and its two reservations. Only the production store can
/// attach the private validation-and-acceptance provenance required for
/// historical authority.
///
/// ```compile_fail
/// use oulipoly_runtime::fresh_continuation::{
///     AcceptedContinuation, ReservedInvocation, ValidatedContinuation,
/// };
///
/// fn forge(context: ValidatedContinuation, reservation: ReservedInvocation) {
///     let _ = AcceptedContinuation {
///         continuation_id: "caller-chosen".to_string(),
///         context,
///         resume: reservation.clone(),
///         fresh: reservation,
///         historical_provenance: None,
///     };
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedContinuation {
    pub continuation_id: String,
    pub context: ValidatedContinuation,
    pub resume: ReservedInvocation,
    pub fresh: ReservedInvocation,
    historical_provenance: Option<ValidatedHistoricalParentProvenance>,
}

impl AcceptedContinuation {
    /// Constructs a continuation value for non-persistence store implementations.
    /// This value can exercise the continuation orchestration contract but can
    /// never produce historical-parent authority.
    pub fn without_historical_authority(
        continuation_id: String,
        context: ValidatedContinuation,
        resume: ReservedInvocation,
        fresh: ReservedInvocation,
    ) -> Self {
        Self {
            continuation_id,
            context,
            resume,
            fresh,
            historical_provenance: None,
        }
    }

    pub(super) fn from_validated_store(
        logical_request_key: String,
        continuation_id: String,
        context: ValidatedContinuation,
        resume: ReservedInvocation,
        fresh: ReservedInvocation,
    ) -> Self {
        let historical_provenance = Some(ValidatedHistoricalParentProvenance {
            logical_request_key,
            validated_fingerprint: context.fingerprint().to_string(),
            origin_invocation_id: context.request().origin_invocation_id.clone(),
            continuation_id: continuation_id.clone(),
            resume: resume.clone(),
            fresh: fresh.clone(),
        });
        Self {
            continuation_id,
            context,
            resume,
            fresh,
            historical_provenance,
        }
    }

    pub(super) fn historical_provenance(&self) -> Option<&ValidatedHistoricalParentProvenance> {
        self.historical_provenance.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ValidatedHistoricalParentProvenance {
    pub(super) logical_request_key: String,
    pub(super) validated_fingerprint: String,
    pub(super) origin_invocation_id: String,
    pub(super) continuation_id: String,
    pub(super) resume: ReservedInvocation,
    pub(super) fresh: ReservedInvocation,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunningParentAdmission;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvocationParentAdmission {
    RequireRunning(RunningParentAdmission),
    Historical(HistoricalParentAdmission),
}

impl Default for InvocationParentAdmission {
    fn default() -> Self {
        Self::RequireRunning(RunningParentAdmission)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct HistoricalParentAuthorityClaim<'a> {
    pub continuation_id: &'a str,
    pub parent_invocation_uuid: &'a str,
    pub child_invocation_uuid: &'a str,
}

/// Historical association authority produced only after an opaque validated
/// continuation is accepted and joined to its exact durable reservations.
///
/// ```compile_fail
/// use oulipoly_runtime::fresh_continuation::HistoricalParentAdmission;
///
/// let _ = HistoricalParentAdmission {
///     parent_invocation_uuid: "parent".to_string(),
///     child_invocation_uuid: "child".to_string(),
///     continuation_id: "continuation".to_string(),
/// };
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoricalParentAdmission {
    parent_invocation_uuid: String,
    child_invocation_uuid: String,
    continuation_id: String,
}

impl HistoricalParentAdmission {
    pub(super) fn new(
        parent_invocation_uuid: String,
        child_invocation_uuid: String,
        continuation_id: String,
    ) -> Self {
        Self {
            parent_invocation_uuid,
            child_invocation_uuid,
            continuation_id,
        }
    }

    pub fn parent_invocation_uuid(&self) -> &str {
        &self.parent_invocation_uuid
    }

    pub fn child_invocation_uuid(&self) -> &str {
        &self.child_invocation_uuid
    }

    pub fn continuation_id(&self) -> &str {
        &self.continuation_id
    }
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
    pub fresh_prompt: String,
    pub request: FreshContinuationRequest,
    pub resume: InvocationOutcome,
    pub fresh: Option<InvocationOutcome>,
}

pub fn fresh_prompt(context: &ValidatedContinuation, resume: &InvocationOutcome) -> String {
    let request = context.request();
    let mut prompt = String::new();
    writeln!(
        prompt,
        "Continue the blocked workflow in this fresh provider session."
    )
    .unwrap();
    writeln!(prompt, "Do not retry or mutate the origin session.").unwrap();
    writeln!(
        prompt,
        "Origin invocation: {}",
        request.origin_invocation_id
    )
    .unwrap();
    writeln!(prompt, "Origin session: {}", request.origin_session_id).unwrap();
    writeln!(prompt, "Failed resume invocation: {}", resume.invocation_id).unwrap();
    writeln!(prompt, "Worktree: {}", request.worktree.display()).unwrap();
    writeln!(
        prompt,
        "Last successful boundary: {}",
        request.last_successful_boundary
    )
    .unwrap();
    writeln!(
        prompt,
        "Active blocked boundary: {}",
        request.active_blocked_boundary
    )
    .unwrap();
    writeln!(prompt, "Read these exact artifacts before continuing:").unwrap();
    for (name, artifact) in [
        ("question", &request.evidence.question),
        ("answer", &request.evidence.answer),
        ("session graph", &request.evidence.session_graph),
        ("origin trace", &request.evidence.origin_trace),
        ("ticket snapshot", &request.evidence.ticket_snapshot),
    ] {
        writeln!(
            prompt,
            "- {name}: {} (sha256 {})",
            artifact.path.display(),
            artifact.sha256
        )
        .unwrap();
    }
    prompt
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
    ) -> Result<InvocationOutcome, ContinuationBlock>;
}

pub trait FreshRunner {
    fn run_or_observe(
        &mut self,
        action: InvocationAction,
        reservation: &ReservedInvocation,
        context: &ValidatedContinuation,
        resume: &InvocationOutcome,
    ) -> Result<InvocationOutcome, ContinuationBlock>;
}

pub trait HandoffPublisher {
    fn publish(
        &mut self,
        handoff: ContinuationHandoff,
    ) -> Result<PublishedHandoff, ContinuationBlock>;

    fn verify(
        &mut self,
        continuation_id: &str,
        handoff: &PublishedHandoff,
    ) -> Result<(), ContinuationBlock>;
}

pub trait FreshContinuation {
    fn execute(&mut self, request: FreshContinuationRequest) -> FreshContinuationOutcome;
}
