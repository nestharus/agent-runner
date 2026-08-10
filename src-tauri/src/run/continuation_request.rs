use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};

use oulipoly_runtime::fresh_continuation::{
    ArtifactIdentity, ContinuationBlock, ContinuationBlockKind, ContinuationEvidence,
    FreshContinuationRequest, InvocationOutcome, ValidatedContinuation,
};
use serde::Deserialize;

const SCHEMA_VERSION: u32 = 1;
const REQUEST_KIND: &str = "fresh_continuation_request";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FreshContinuationRequestV1 {
    schema_version: u32,
    kind: String,
    question_id: String,
    origin_invocation_id: String,
    origin_session_id: String,
    planning_root: PathBuf,
    worktree: PathBuf,
    last_successful_boundary: String,
    active_blocked_boundary: String,
    target_model: String,
    evidence: ContinuationEvidenceV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContinuationEvidenceV1 {
    question: ArtifactIdentityV1,
    answer: ArtifactIdentityV1,
    session_graph: ArtifactIdentityV1,
    origin_trace: ArtifactIdentityV1,
    ticket_snapshot: ArtifactIdentityV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactIdentityV1 {
    path: PathBuf,
    sha256: String,
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn read(path: &Path) -> Result<FreshContinuationRequest, ContinuationBlock> {
    let bytes = fs::read(path).map_err(|error| {
        invalid_request(format!(
            "Failed to read fresh continuation request {}: {error}",
            path.display()
        ))
    })?;
    let request: FreshContinuationRequestV1 = serde_json::from_slice(&bytes)
        .map_err(|error| invalid_request(format!("Invalid fresh continuation request: {error}")))?;
    if request.schema_version != SCHEMA_VERSION || request.kind != REQUEST_KIND {
        return Err(invalid_request(
            "Unsupported fresh continuation request schema or kind".to_string(),
        ));
    }
    Ok(request.into())
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn fresh_prompt(context: &ValidatedContinuation, resume: &InvocationOutcome) -> String {
    let request = &context.request;
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

impl From<FreshContinuationRequestV1> for FreshContinuationRequest {
    fn from(request: FreshContinuationRequestV1) -> Self {
        Self {
            question_id: request.question_id,
            origin_invocation_id: request.origin_invocation_id,
            origin_session_id: request.origin_session_id,
            planning_root: request.planning_root,
            worktree: request.worktree,
            last_successful_boundary: request.last_successful_boundary,
            active_blocked_boundary: request.active_blocked_boundary,
            target_model: request.target_model,
            evidence: request.evidence.into(),
        }
    }
}

impl From<ContinuationEvidenceV1> for ContinuationEvidence {
    fn from(evidence: ContinuationEvidenceV1) -> Self {
        Self {
            question: evidence.question.into(),
            answer: evidence.answer.into(),
            session_graph: evidence.session_graph.into(),
            origin_trace: evidence.origin_trace.into(),
            ticket_snapshot: evidence.ticket_snapshot.into(),
        }
    }
}

impl From<ArtifactIdentityV1> for ArtifactIdentity {
    fn from(artifact: ArtifactIdentityV1) -> Self {
        Self {
            path: artifact.path,
            sha256: artifact.sha256,
        }
    }
}

fn invalid_request(message: String) -> ContinuationBlock {
    ContinuationBlock {
        kind: ContinuationBlockKind::InvalidEvidence,
        message,
    }
}
