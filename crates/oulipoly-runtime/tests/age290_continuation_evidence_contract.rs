use std::collections::HashMap;
use std::path::{Path, PathBuf};

use oulipoly_runtime::fresh_continuation::{
    ArtifactIdentity, ContinuationArtifactSource, ContinuationBlock, ContinuationBlockKind,
    ContinuationEvidence, ContinuationEvidenceValidator, DefaultContinuationEvidenceValidator,
    FreshContinuationRequest,
};
use sha2::{Digest, Sha256};

struct ArtifactSourceFake {
    files: HashMap<PathBuf, Vec<u8>>,
}

impl ContinuationArtifactSource for ArtifactSourceFake {
    fn read(&mut self, artifact: &ArtifactIdentity) -> Result<Vec<u8>, ContinuationBlock> {
        self.files.get(&artifact.path).cloned().ok_or_else(|| {
            block(
                ContinuationBlockKind::InvalidEvidence,
                "artifact is missing",
            )
        })
    }
}

#[test]
fn matching_artifacts_produce_a_stable_validated_context() {
    let fixture = EvidenceFixture::valid();
    let mut first = fixture.validator();
    let mut second = fixture.validator();

    let first = first.validate(&fixture.request).unwrap();
    let second = second.validate(&fixture.request).unwrap();

    assert_eq!(first.request, fixture.request);
    assert_eq!(first.fingerprint, second.fingerprint);
    assert!(!first.fingerprint.is_empty());
}

#[test]
fn hash_mismatch_returns_typed_invalid_evidence() {
    let mut fixture = EvidenceFixture::valid();
    fixture.request.evidence.answer.sha256 = "wrong-hash".to_string();
    let mut validator = fixture.validator();

    let error = validator.validate(&fixture.request).unwrap_err();

    assert_eq!(error.kind, ContinuationBlockKind::InvalidEvidence);
    assert!(error.message.contains("answer"), "{error:?}");
}

#[test]
fn question_and_answer_must_name_the_requested_question() {
    let mut fixture = EvidenceFixture::valid();
    fixture.replace(
        "/planning/answer.json",
        br#"{"schema_version":1,"kind":"agent_answer","question_id":"another-question"}"#,
    );
    let mut validator = fixture.validator();

    let error = validator.validate(&fixture.request).unwrap_err();

    assert_eq!(error.kind, ContinuationBlockKind::InvalidEvidence);
    assert!(error.message.contains("question"), "{error:?}");
}

#[test]
fn graph_and_trace_must_bind_the_origin_invocation_and_session() {
    let mut fixture = EvidenceFixture::valid();
    fixture.replace(
        "/planning/trace.json",
        br#"{"root":{"invocation":{"id":"other"},"session":{"provider_session_id":"origin-session"}}}"#,
    );
    let mut validator = fixture.validator();

    let error = validator.validate(&fixture.request).unwrap_err();

    assert_eq!(error.kind, ContinuationBlockKind::InvalidEvidence);
    assert!(error.message.contains("origin"), "{error:?}");
}

#[test]
fn answer_must_be_authorized_by_the_root() {
    let mut fixture = EvidenceFixture::valid();
    fixture.replace(
        "/planning/answer.json",
        br#"{"schema_version":1,"kind":"agent_answer","question_id":"question-1","answered_by":"model","continuation_plan":{"session_graph_manifest":"/planning/graph.json"}}"#,
    );
    let mut validator = fixture.validator();

    let error = validator.validate(&fixture.request).unwrap_err();

    assert_eq!(error.kind, ContinuationBlockKind::InvalidEvidence);
    assert!(error.message.contains("answer"), "{error:?}");
}

#[test]
fn planning_root_must_contain_every_bound_artifact() {
    let mut fixture = EvidenceFixture::valid();
    fixture.request.planning_root = PathBuf::from("/other-planning-root");
    let mut validator = fixture.validator();

    let error = validator.validate(&fixture.request).unwrap_err();

    assert_eq!(error.kind, ContinuationBlockKind::InvalidEvidence);
    assert!(error.message.contains("planning"), "{error:?}");
}

#[test]
fn missing_artifact_returns_typed_invalid_evidence() {
    let mut fixture = EvidenceFixture::valid();
    fixture.files.remove(Path::new("/planning/ticket.md"));
    let mut validator = fixture.validator();

    let error = validator.validate(&fixture.request).unwrap_err();

    assert_eq!(error.kind, ContinuationBlockKind::InvalidEvidence);
}

struct EvidenceFixture {
    request: FreshContinuationRequest,
    files: HashMap<PathBuf, Vec<u8>>,
}

impl EvidenceFixture {
    fn valid() -> Self {
        let files = HashMap::from([
            (
                PathBuf::from("/planning/question.json"),
                br#"{"schema_version":1,"kind":"agent_question","question_id":"question-1","origin":{"invocation_uuid":"origin-invocation","session_id":"unknown","worktree_path":"/worktree"},"state_refs":{"session_graph_manifest":"unknown"}}"#.to_vec(),
            ),
            (
                PathBuf::from("/planning/answer.json"),
                br#"{"schema_version":1,"kind":"agent_answer","question_id":"question-1","answered_by":"user-via-root-orchestrator","continuation_plan":{"session_graph_manifest":"/planning/graph.json"}}"#.to_vec(),
            ),
            (
                PathBuf::from("/planning/graph.json"),
                br#"{"root_invocation_uuid":"origin-invocation","invocation_ids":["origin-invocation"],"session_ids":["origin-session"],"question_ids":["question-1"]}"#.to_vec(),
            ),
            (
                PathBuf::from("/planning/trace.json"),
                br#"{"root":{"invocation":{"id":"origin-invocation"},"session":{"provider_session_id":"origin-session"}}}"#.to_vec(),
            ),
            (
                PathBuf::from("/planning/ticket.md"),
                b"AGE-290 ticket snapshot".to_vec(),
            ),
        ]);
        let request = FreshContinuationRequest {
            question_id: "question-1".to_string(),
            origin_invocation_id: "origin-invocation".to_string(),
            origin_session_id: "origin-session".to_string(),
            planning_root: PathBuf::from("/planning"),
            worktree: PathBuf::from("/worktree"),
            last_successful_boundary: "verified".to_string(),
            active_blocked_boundary: "apply".to_string(),
            target_model: "fresh-model".to_string(),
            evidence: ContinuationEvidence {
                question: artifact(&files, "/planning/question.json"),
                answer: artifact(&files, "/planning/answer.json"),
                session_graph: artifact(&files, "/planning/graph.json"),
                origin_trace: artifact(&files, "/planning/trace.json"),
                ticket_snapshot: artifact(&files, "/planning/ticket.md"),
            },
        };
        Self { request, files }
    }

    fn replace(&mut self, path: &str, bytes: &[u8]) {
        self.files.insert(PathBuf::from(path), bytes.to_vec());
        let sha256 = digest(bytes);
        let artifact = match path {
            "/planning/question.json" => &mut self.request.evidence.question,
            "/planning/answer.json" => &mut self.request.evidence.answer,
            "/planning/graph.json" => &mut self.request.evidence.session_graph,
            "/planning/trace.json" => &mut self.request.evidence.origin_trace,
            "/planning/ticket.md" => &mut self.request.evidence.ticket_snapshot,
            _ => panic!("unknown fixture artifact"),
        };
        artifact.sha256 = sha256;
    }

    fn validator(&self) -> DefaultContinuationEvidenceValidator<ArtifactSourceFake> {
        DefaultContinuationEvidenceValidator::new(ArtifactSourceFake {
            files: self.files.clone(),
        })
    }
}

fn artifact(files: &HashMap<PathBuf, Vec<u8>>, path: &str) -> ArtifactIdentity {
    ArtifactIdentity {
        path: PathBuf::from(path),
        sha256: digest(files.get(Path::new(path)).unwrap()),
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn block(kind: ContinuationBlockKind, message: &str) -> ContinuationBlock {
    ContinuationBlock {
        kind,
        message: message.to_string(),
    }
}
