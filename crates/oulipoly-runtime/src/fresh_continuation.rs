use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

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

pub trait ContinuationArtifactSource {
    fn read(&mut self, artifact: &ArtifactIdentity) -> Result<Vec<u8>, ContinuationBlock>;
}

pub struct DefaultContinuationEvidenceValidator<Source> {
    source: Source,
}

impl<Source> DefaultContinuationEvidenceValidator<Source> {
    pub fn new(source: Source) -> Self {
        Self { source }
    }
}

#[derive(Deserialize)]
struct QuestionArtifact {
    schema_version: u64,
    kind: String,
    question_id: String,
    origin: QuestionOrigin,
    state_refs: QuestionStateRefs,
}

#[derive(Deserialize)]
struct QuestionOrigin {
    invocation_uuid: String,
    session_id: String,
    worktree_path: PathBuf,
}

#[derive(Deserialize)]
struct QuestionStateRefs {
    session_graph_manifest: PathBuf,
}

#[derive(Deserialize)]
struct AnswerArtifact {
    schema_version: u64,
    kind: String,
    question_id: String,
    answered_by: Option<String>,
    continuation_plan: Option<AnswerContinuationPlan>,
}

#[derive(Deserialize)]
struct AnswerContinuationPlan {
    session_graph_manifest: PathBuf,
}

#[derive(Deserialize)]
struct SessionGraphArtifact {
    root_invocation_uuid: String,
    invocation_ids: Vec<String>,
    session_ids: Vec<String>,
    question_ids: Vec<String>,
}

#[derive(Deserialize)]
struct OriginTraceArtifact {
    root: TraceRoot,
}

#[derive(Deserialize)]
struct TraceRoot {
    invocation: TraceInvocation,
    session: TraceSession,
}

#[derive(Deserialize)]
struct TraceInvocation {
    id: String,
}

#[derive(Deserialize)]
struct TraceSession {
    provider_session_id: String,
}

impl<Source> ContinuationEvidenceValidator for DefaultContinuationEvidenceValidator<Source>
where
    Source: ContinuationArtifactSource,
{
    fn validate(
        &mut self,
        request: &FreshContinuationRequest,
    ) -> Result<ValidatedContinuation, ContinuationBlock> {
        for artifact in evidence_artifacts(request) {
            require_identity(
                path_is_within(&artifact.path, &request.planning_root),
                "evidence artifact path is outside the declared planning root",
            )?;
        }

        // Perform every declared source read before interpreting any artifact so the
        // validated identity set is complete and independent of artifact ordering.
        let question_bytes = self.read_and_verify("question", &request.evidence.question);
        let answer_bytes = self.read_and_verify("answer", &request.evidence.answer);
        let graph_bytes = self.read_and_verify("session graph", &request.evidence.session_graph);
        let trace_bytes = self.read_and_verify("origin trace", &request.evidence.origin_trace);
        let ticket_bytes =
            self.read_and_verify("ticket snapshot", &request.evidence.ticket_snapshot);

        let question: QuestionArtifact = parse_artifact("question", &question_bytes?)?;
        let answer: AnswerArtifact = parse_artifact("answer", &answer_bytes?)?;
        let graph: SessionGraphArtifact = parse_artifact("session graph", &graph_bytes?)?;
        let trace: OriginTraceArtifact = parse_artifact("origin trace", &trace_bytes?)?;
        ticket_bytes?;

        require_identity(
            question.schema_version == 1,
            "question artifact has an unsupported schema identity",
        )?;
        require_identity(
            question.kind == "agent_question",
            "question artifact kind identity is not agent_question",
        )?;
        require_identity(
            answer.schema_version == 1,
            "answer artifact has an unsupported schema identity",
        )?;
        require_identity(
            answer.kind == "agent_answer",
            "answer artifact kind identity is not agent_answer",
        )?;
        require_identity(
            question.question_id == request.question_id,
            "question artifact does not identify the requested question",
        )?;
        require_identity(
            answer.question_id == request.question_id,
            "answer artifact does not identify the requested question",
        )?;
        require_identity(
            answer.answered_by.as_deref() == Some("user-via-root-orchestrator"),
            "answer artifact is not authorized by the root orchestrator",
        )?;
        require_identity(
            answer.continuation_plan.as_ref().is_some_and(|plan| {
                plan.session_graph_manifest == request.evidence.session_graph.path
            }),
            "answer artifact continuation plan does not identify the requested session graph",
        )?;
        require_identity(
            question.origin.invocation_uuid == request.origin_invocation_id,
            "question artifact origin invocation identity does not match the request",
        )?;
        require_identity(
            question.origin.session_id == "unknown"
                || question.origin.session_id == request.origin_session_id,
            "question artifact origin session identity does not match the request",
        )?;
        require_identity(
            question.origin.worktree_path == request.worktree,
            "question artifact origin worktree identity does not match the request",
        )?;
        require_identity(
            question.state_refs.session_graph_manifest == Path::new("unknown")
                || question.state_refs.session_graph_manifest
                    == request.evidence.session_graph.path,
            "question artifact session graph manifest reference does not match the request",
        )?;
        require_identity(
            graph.root_invocation_uuid == request.origin_invocation_id
                && graph
                    .invocation_ids
                    .iter()
                    .any(|id| id == &request.origin_invocation_id),
            "session graph artifact does not bind the origin invocation identity",
        )?;
        require_identity(
            graph
                .session_ids
                .iter()
                .any(|id| id == &request.origin_session_id),
            "session graph artifact does not bind the origin session identity",
        )?;
        require_identity(
            graph
                .question_ids
                .iter()
                .any(|id| id == &request.question_id),
            "session graph artifact does not bind the question identity",
        )?;
        require_identity(
            trace.root.invocation.id == request.origin_invocation_id,
            "origin trace artifact invocation identity does not match the request origin",
        )?;
        require_identity(
            trace.root.session.provider_session_id == request.origin_session_id,
            "origin trace artifact session identity does not match the request origin",
        )?;

        Ok(ValidatedContinuation {
            request: request.clone(),
            fingerprint: continuation_fingerprint(request),
        })
    }
}

impl<Source> DefaultContinuationEvidenceValidator<Source>
where
    Source: ContinuationArtifactSource,
{
    fn read_and_verify(
        &mut self,
        name: &str,
        artifact: &ArtifactIdentity,
    ) -> Result<Vec<u8>, ContinuationBlock> {
        let bytes = self.source.read(artifact).map_err(|error| {
            invalid_evidence(format!(
                "{name} artifact could not be read for identity validation: {}",
                error.message
            ))
        })?;
        let actual = format!("{:x}", Sha256::digest(&bytes));
        if actual != artifact.sha256 {
            return Err(invalid_evidence(format!(
                "{name} artifact hash identity does not match its declared sha256"
            )));
        }
        Ok(bytes)
    }
}

fn parse_artifact<'a, Artifact>(name: &str, bytes: &'a [u8]) -> Result<Artifact, ContinuationBlock>
where
    Artifact: Deserialize<'a>,
{
    serde_json::from_slice(bytes).map_err(|error| {
        invalid_evidence(format!(
            "{name} artifact identity document is invalid: {error}"
        ))
    })
}

fn require_identity(valid: bool, message: &str) -> Result<(), ContinuationBlock> {
    valid
        .then_some(())
        .ok_or_else(|| invalid_evidence(message.to_string()))
}

fn invalid_evidence(message: String) -> ContinuationBlock {
    ContinuationBlock {
        kind: ContinuationBlockKind::InvalidEvidence,
        message,
    }
}

fn evidence_artifacts(request: &FreshContinuationRequest) -> [&ArtifactIdentity; 5] {
    [
        &request.evidence.question,
        &request.evidence.answer,
        &request.evidence.session_graph,
        &request.evidence.origin_trace,
        &request.evidence.ticket_snapshot,
    ]
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root).is_ok_and(|relative| {
        relative.components().next().is_some()
            && relative
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
    })
}

fn continuation_fingerprint(request: &FreshContinuationRequest) -> String {
    let mut digest = Sha256::new();
    fingerprint_part(&mut digest, b"fresh-continuation-evidence-v1");
    fingerprint_part(&mut digest, request.question_id.as_bytes());
    fingerprint_part(&mut digest, request.origin_invocation_id.as_bytes());
    fingerprint_part(&mut digest, request.origin_session_id.as_bytes());
    fingerprint_part(
        &mut digest,
        request.planning_root.as_os_str().as_encoded_bytes(),
    );
    fingerprint_part(&mut digest, request.worktree.as_os_str().as_encoded_bytes());
    fingerprint_part(&mut digest, request.last_successful_boundary.as_bytes());
    fingerprint_part(&mut digest, request.active_blocked_boundary.as_bytes());
    fingerprint_part(&mut digest, request.target_model.as_bytes());
    for artifact in evidence_artifacts(request) {
        fingerprint_part(&mut digest, artifact.path.as_os_str().as_encoded_bytes());
        fingerprint_part(&mut digest, artifact.sha256.as_bytes());
    }
    format!("{:x}", digest.finalize())
}

fn fingerprint_part(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
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
