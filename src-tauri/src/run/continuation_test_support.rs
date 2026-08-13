use std::collections::HashMap;
use std::path::PathBuf;

use oulipoly_runtime::fresh_continuation::{
    ArtifactIdentity, ContinuationArtifactSource, ContinuationBlock, ContinuationBlockKind,
    ContinuationEvidenceValidator, DefaultContinuationEvidenceValidator, FreshContinuationRequest,
    ValidatedContinuation,
};
use serde_json::json;
use sha2::{Digest, Sha256};

struct ArtifactSourceFake {
    files: HashMap<PathBuf, Vec<u8>>,
}

impl ContinuationArtifactSource for ArtifactSourceFake {
    fn read(&mut self, artifact: &ArtifactIdentity) -> Result<Vec<u8>, ContinuationBlock> {
        self.files
            .get(&artifact.path)
            .cloned()
            .ok_or_else(|| ContinuationBlock {
                kind: ContinuationBlockKind::InvalidEvidence,
                message: "artifact is missing".to_string(),
            })
    }
}

pub(super) fn validated_continuation(
    mut request: FreshContinuationRequest,
) -> ValidatedContinuation {
    let graph_path = request.evidence.session_graph.path.clone();
    let files = HashMap::from([
        (
            request.evidence.question.path.clone(),
            serde_json::to_vec(&json!({
                "schema_version": 1,
                "kind": "agent_question",
                "question_id": request.question_id,
                "origin": {
                    "invocation_uuid": request.origin_invocation_id,
                    "session_id": request.origin_session_id,
                    "worktree_path": request.worktree,
                },
                "state_refs": {"session_graph_manifest": graph_path},
            }))
            .unwrap(),
        ),
        (
            request.evidence.answer.path.clone(),
            serde_json::to_vec(&json!({
                "schema_version": 1,
                "kind": "agent_answer",
                "question_id": request.question_id,
                "answered_by": "user-via-root-orchestrator",
                "continuation_plan": {"session_graph_manifest": graph_path},
            }))
            .unwrap(),
        ),
        (
            request.evidence.session_graph.path.clone(),
            serde_json::to_vec(&json!({
                "root_invocation_uuid": request.origin_invocation_id,
                "invocation_ids": [request.origin_invocation_id],
                "session_ids": [request.origin_session_id],
                "question_ids": [request.question_id],
            }))
            .unwrap(),
        ),
        (
            request.evidence.origin_trace.path.clone(),
            serde_json::to_vec(&json!({
                "root": {
                    "invocation": {"id": request.origin_invocation_id},
                    "session": {"provider_session_id": request.origin_session_id},
                },
            }))
            .unwrap(),
        ),
        (
            request.evidence.ticket_snapshot.path.clone(),
            b"SECRET_TICKET_ARTIFACT_BODY".to_vec(),
        ),
    ]);
    for artifact in [
        &mut request.evidence.question,
        &mut request.evidence.answer,
        &mut request.evidence.session_graph,
        &mut request.evidence.origin_trace,
        &mut request.evidence.ticket_snapshot,
    ] {
        artifact.sha256 = format!("{:x}", Sha256::digest(files.get(&artifact.path).unwrap()));
    }
    DefaultContinuationEvidenceValidator::new(ArtifactSourceFake { files })
        .validate(&request)
        .unwrap()
}
