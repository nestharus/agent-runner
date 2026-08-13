//! ## Declared roles
//!
//! `validator`, `parser`, `predicate`, `mapper`, `accessor`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/fresh_continuation/evidence.rs
//!     role: adapter
//!     Translates:
//!       - fresh-continuation-evidence-port-contract
//!       - versioned-question-answer-graph-and-trace-artifact-contracts
//! ```

use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::contract::{
    ArtifactIdentity, ContinuationArtifactSource, ContinuationBlock, ContinuationBlockKind,
    ContinuationEvidenceValidator, FreshContinuationRequest, ValidatedContinuation,
};

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
    session_graph_manifest: String,
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
    session_graph_manifest: String,
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
        for (name, artifact) in evidence_artifacts(request) {
            require_identity(
                path_is_within(&artifact.path, &request.planning_root),
                &format!("{name} artifact path identity is outside the declared planning root"),
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
                graph_reference_matches(
                    &plan.session_graph_manifest,
                    &request.evidence.session_graph.path,
                )
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
            question.state_refs.session_graph_manifest == "unknown"
                || graph_reference_matches(
                    &question.state_refs.session_graph_manifest,
                    &request.evidence.session_graph.path,
                ),
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

        Ok(ValidatedContinuation::from_validated_evidence(
            request.clone(),
            continuation_fingerprint(request),
        ))
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
        let bytes = self.read_artifact(name, artifact)?;
        verify_artifact_identity(name, artifact, bytes)
    }

    fn read_artifact(
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
        Ok(bytes)
    }
}

fn verify_artifact_identity(
    name: &str,
    artifact: &ArtifactIdentity,
    bytes: Vec<u8>,
) -> Result<Vec<u8>, ContinuationBlock> {
    let actual = format!("{:x}", Sha256::digest(&bytes));
    require_identity(
        actual.eq_ignore_ascii_case(&artifact.sha256),
        &format!("{name} artifact hash identity does not match its declared sha256"),
    )?;
    Ok(bytes)
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

fn evidence_artifacts(request: &FreshContinuationRequest) -> [(&str, &ArtifactIdentity); 5] {
    [
        ("question", &request.evidence.question),
        ("answer", &request.evidence.answer),
        ("session graph", &request.evidence.session_graph),
        ("origin trace", &request.evidence.origin_trace),
        ("ticket snapshot", &request.evidence.ticket_snapshot),
    ]
}

fn graph_reference_matches(reference: &str, expected: &Path) -> bool {
    expected.to_str() == Some(reference)
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
    for (_, artifact) in evidence_artifacts(request) {
        fingerprint_part(&mut digest, artifact.path.as_os_str().as_encoded_bytes());
        fingerprint_part(&mut digest, artifact.sha256.to_ascii_lowercase().as_bytes());
    }
    format!("{:x}", digest.finalize())
}

fn fingerprint_part(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}
