//! ## Declared roles
//!
//! `orchestration`, `validator`, `accessor`, `mapper`, `formatter`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/tests/age290_continuation_evidence_contract.rs
//!     role: adapter
//!     Translates:
//!       - fresh-continuation-evidence-behavior-contract
//!       - Rust-test-fixture-contract
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use oulipoly_runtime::fresh_continuation::{
    ArtifactIdentity, ContinuationArtifactSource, ContinuationBlock, ContinuationBlockKind,
    ContinuationEvidence, ContinuationEvidenceValidator, DefaultContinuationEvidenceValidator,
    FreshContinuationRequest,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const QUESTION: &str = "question";
const ANSWER: &str = "answer";
const SESSION_GRAPH: &str = "session graph";
const ORIGIN_TRACE: &str = "origin trace";
const TICKET_SNAPSHOT: &str = "ticket snapshot";

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
    let first = fixture.validated();
    let second = fixture.validated();

    assert_eq!(first.request, fixture.request);
    assert_eq!(first.fingerprint, second.fingerprint);
    assert!(!first.fingerprint.is_empty());
}

#[test]
fn uppercase_artifact_hash_is_accepted() {
    let mut fixture = EvidenceFixture::valid();
    fixture.request.evidence.question.sha256 =
        fixture.request.evidence.question.sha256.to_ascii_uppercase();

    fixture.validated();
}

#[test]
fn changed_question_id_changes_the_validated_fingerprint() {
    let mut fixture = EvidenceFixture::valid();
    let original = fixture.fingerprint();

    fixture.rebind_question_id("question-2");

    assert_fingerprint_changed(&fixture, &original, "question id");
}

#[test]
fn changed_origin_invocation_changes_the_validated_fingerprint() {
    let mut fixture = EvidenceFixture::valid();
    let original = fixture.fingerprint();

    fixture.rebind_origin_invocation("other-origin-invocation");

    assert_fingerprint_changed(&fixture, &original, "origin invocation");
}

#[test]
fn changed_origin_session_changes_the_validated_fingerprint() {
    let mut fixture = EvidenceFixture::valid();
    let original = fixture.fingerprint();

    fixture.rebind_origin_session("other-origin-session");

    assert_fingerprint_changed(&fixture, &original, "origin session");
}

#[test]
fn changed_planning_root_changes_the_validated_fingerprint() {
    let mut fixture = EvidenceFixture::valid();
    let original = fixture.fingerprint();

    fixture.relocate_planning_root("/other-planning");

    assert_fingerprint_changed(&fixture, &original, "planning root");
}

#[test]
fn changed_worktree_changes_the_validated_fingerprint() {
    let mut fixture = EvidenceFixture::valid();
    let original = fixture.fingerprint();

    fixture.rebind_worktree("/other-worktree");

    assert_fingerprint_changed(&fixture, &original, "worktree");
}

#[test]
fn changed_last_successful_boundary_changes_the_validated_fingerprint() {
    let mut fixture = EvidenceFixture::valid();
    let original = fixture.fingerprint();
    fixture.request.last_successful_boundary = "other-verified-boundary".to_string();

    assert_fingerprint_changed(&fixture, &original, "last successful boundary");
}

#[test]
fn changed_active_blocked_boundary_changes_the_validated_fingerprint() {
    let mut fixture = EvidenceFixture::valid();
    let original = fixture.fingerprint();
    fixture.request.active_blocked_boundary = "other-blocked-boundary".to_string();

    assert_fingerprint_changed(&fixture, &original, "active blocked boundary");
}

#[test]
fn changed_target_model_changes_the_validated_fingerprint() {
    let mut fixture = EvidenceFixture::valid();
    let original = fixture.fingerprint();
    fixture.request.target_model = "other-fresh-model".to_string();

    assert_fingerprint_changed(&fixture, &original, "target model");
}

#[test]
fn changed_question_bytes_and_hash_change_the_validated_fingerprint() {
    assert_artifact_content_changes_fingerprint(QUESTION);
}

#[test]
fn changed_answer_bytes_and_hash_change_the_validated_fingerprint() {
    assert_artifact_content_changes_fingerprint(ANSWER);
}

#[test]
fn changed_session_graph_bytes_and_hash_change_the_validated_fingerprint() {
    assert_artifact_content_changes_fingerprint(SESSION_GRAPH);
}

#[test]
fn changed_origin_trace_bytes_and_hash_change_the_validated_fingerprint() {
    assert_artifact_content_changes_fingerprint(ORIGIN_TRACE);
}

#[test]
fn changed_ticket_snapshot_bytes_and_hash_change_the_validated_fingerprint() {
    assert_artifact_content_changes_fingerprint(TICKET_SNAPSHOT);
}

#[test]
fn changed_question_canonical_path_changes_the_validated_fingerprint() {
    assert_artifact_path_changes_fingerprint(QUESTION, "/planning/question-renamed.json");
}

#[test]
fn changed_answer_canonical_path_changes_the_validated_fingerprint() {
    assert_artifact_path_changes_fingerprint(ANSWER, "/planning/answer-renamed.json");
}

#[test]
fn changed_session_graph_canonical_path_changes_the_validated_fingerprint() {
    assert_artifact_path_changes_fingerprint(SESSION_GRAPH, "/planning/graph-renamed.json");
}

#[test]
fn changed_origin_trace_canonical_path_changes_the_validated_fingerprint() {
    assert_artifact_path_changes_fingerprint(ORIGIN_TRACE, "/planning/trace-renamed.json");
}

#[test]
fn changed_ticket_snapshot_canonical_path_changes_the_validated_fingerprint() {
    assert_artifact_path_changes_fingerprint(TICKET_SNAPSHOT, "/planning/ticket-renamed.md");
}

#[test]
fn changed_question_manifest_hash_rejects_unchanged_bytes() {
    assert_changed_manifest_hash_rejected(QUESTION);
}

#[test]
fn changed_answer_manifest_hash_rejects_unchanged_bytes() {
    assert_changed_manifest_hash_rejected(ANSWER);
}

#[test]
fn changed_session_graph_manifest_hash_rejects_unchanged_bytes() {
    assert_changed_manifest_hash_rejected(SESSION_GRAPH);
}

#[test]
fn changed_origin_trace_manifest_hash_rejects_unchanged_bytes() {
    assert_changed_manifest_hash_rejected(ORIGIN_TRACE);
}

#[test]
fn changed_ticket_snapshot_manifest_hash_rejects_unchanged_bytes() {
    assert_changed_manifest_hash_rejected(TICKET_SNAPSHOT);
}

#[test]
fn changed_question_manifest_path_rejects_bytes_at_the_original_path() {
    assert_changed_manifest_path_rejected(QUESTION, "/planning/question-other.json");
}

#[test]
fn changed_answer_manifest_path_rejects_bytes_at_the_original_path() {
    assert_changed_manifest_path_rejected(ANSWER, "/planning/answer-other.json");
}

#[test]
fn changed_session_graph_manifest_path_rejects_bytes_at_the_original_path() {
    assert_changed_manifest_path_rejected(SESSION_GRAPH, "/planning/graph-other.json");
}

#[test]
fn changed_origin_trace_manifest_path_rejects_bytes_at_the_original_path() {
    assert_changed_manifest_path_rejected(ORIGIN_TRACE, "/planning/trace-other.json");
}

#[test]
fn changed_ticket_snapshot_manifest_path_rejects_bytes_at_the_original_path() {
    assert_changed_manifest_path_rejected(TICKET_SNAPSHOT, "/planning/ticket-other.md");
}

#[test]
fn noncanonical_question_path_identity_is_rejected() {
    assert_noncanonical_artifact_path_rejected(QUESTION, "/outside/question.json");
}

#[test]
fn noncanonical_answer_path_identity_is_rejected() {
    assert_noncanonical_artifact_path_rejected(ANSWER, "/outside/answer.json");
}

#[test]
fn noncanonical_session_graph_path_identity_is_rejected() {
    assert_noncanonical_artifact_path_rejected(SESSION_GRAPH, "/outside/graph.json");
}

#[test]
fn noncanonical_origin_trace_path_identity_is_rejected() {
    assert_noncanonical_artifact_path_rejected(ORIGIN_TRACE, "/outside/trace.json");
}

#[test]
fn noncanonical_ticket_snapshot_path_identity_is_rejected() {
    assert_noncanonical_artifact_path_rejected(TICKET_SNAPSHOT, "/outside/ticket.md");
}

#[test]
fn question_artifact_must_name_the_requested_question() {
    let mut fixture = EvidenceFixture::valid();
    fixture.rewrite_json(QUESTION, |question| {
        question["question_id"] = json!("another-question");
    });

    assert_identity_error(fixture.validation_error(), QUESTION, "requested question");
}

#[test]
fn answer_artifact_must_name_the_requested_question() {
    let mut fixture = EvidenceFixture::valid();
    fixture.rewrite_json(ANSWER, |answer| {
        answer["question_id"] = json!("another-question");
    });

    assert_identity_error(fixture.validation_error(), ANSWER, "requested question");
}

#[test]
fn question_origin_invocation_must_match_the_request() {
    let mut fixture = EvidenceFixture::valid();
    fixture.rewrite_json(QUESTION, |question| {
        question["origin"]["invocation_uuid"] = json!("other-invocation");
    });

    assert_identity_error(fixture.validation_error(), QUESTION, "origin invocation");
}

#[test]
fn question_origin_session_must_match_the_request() {
    let mut fixture = EvidenceFixture::valid();
    fixture.rewrite_json(QUESTION, |question| {
        question["origin"]["session_id"] = json!("other-session");
    });

    assert_identity_error(fixture.validation_error(), QUESTION, "origin session");
}

#[test]
fn question_origin_worktree_must_match_the_request() {
    let mut fixture = EvidenceFixture::valid();
    fixture.rewrite_json(QUESTION, |question| {
        question["origin"]["worktree_path"] = json!("/other-worktree");
    });

    assert_identity_error(fixture.validation_error(), QUESTION, "origin worktree");
}

#[test]
fn supported_unknown_question_origin_placeholders_are_accepted() {
    let mut fixture = EvidenceFixture::valid();
    fixture.rewrite_json(QUESTION, |question| {
        question["origin"]["session_id"] = json!("unknown");
        question["state_refs"]["session_graph_manifest"] = json!("unknown");
    });

    fixture.validated();
}

#[test]
fn unsupported_question_graph_reference_is_rejected() {
    let mut fixture = EvidenceFixture::valid();
    fixture.rewrite_json(QUESTION, |question| {
        question["state_refs"]["session_graph_manifest"] = json!("unsupported");
    });

    assert_identity_error(
        fixture.validation_error(),
        QUESTION,
        "session graph manifest",
    );
}

#[test]
fn noncanonical_question_graph_reference_is_rejected() {
    let mut fixture = EvidenceFixture::valid();
    fixture.rewrite_json(QUESTION, |question| {
        question["state_refs"]["session_graph_manifest"] = json!("/planning/./graph.json");
    });

    assert_identity_error(
        fixture.validation_error(),
        QUESTION,
        "session graph manifest",
    );
}

#[test]
fn unsupported_answer_graph_reference_is_rejected() {
    let mut fixture = EvidenceFixture::valid();
    fixture.rewrite_json(ANSWER, |answer| {
        answer["continuation_plan"]["session_graph_manifest"] = json!("/planning/other-graph.json");
    });

    assert_identity_error(fixture.validation_error(), ANSWER, "session graph");
}

#[test]
fn noncanonical_answer_graph_reference_is_rejected() {
    let mut fixture = EvidenceFixture::valid();
    fixture.rewrite_json(ANSWER, |answer| {
        answer["continuation_plan"]["session_graph_manifest"] = json!("/planning/./graph.json");
    });

    assert_identity_error(fixture.validation_error(), ANSWER, "session graph");
}

#[test]
fn session_graph_root_must_bind_the_origin_invocation() {
    let mut fixture = EvidenceFixture::valid();
    fixture.rewrite_json(SESSION_GRAPH, |graph| {
        graph["root_invocation_uuid"] = json!("other-invocation");
    });

    assert_identity_error(
        fixture.validation_error(),
        SESSION_GRAPH,
        "origin invocation",
    );
}

#[test]
fn session_graph_membership_must_bind_the_origin_invocation() {
    let mut fixture = EvidenceFixture::valid();
    fixture.rewrite_json(SESSION_GRAPH, |graph| {
        graph["invocation_ids"] = json!(["other-invocation"]);
    });

    assert_identity_error(
        fixture.validation_error(),
        SESSION_GRAPH,
        "origin invocation",
    );
}

#[test]
fn session_graph_must_bind_the_origin_session() {
    let mut fixture = EvidenceFixture::valid();
    fixture.rewrite_json(SESSION_GRAPH, |graph| {
        graph["session_ids"] = json!(["other-session"]);
    });

    assert_identity_error(fixture.validation_error(), SESSION_GRAPH, "origin session");
}

#[test]
fn session_graph_must_bind_the_question() {
    let mut fixture = EvidenceFixture::valid();
    fixture.rewrite_json(SESSION_GRAPH, |graph| {
        graph["question_ids"] = json!(["another-question"]);
    });

    assert_identity_error(
        fixture.validation_error(),
        SESSION_GRAPH,
        "question identity",
    );
}

#[test]
fn origin_trace_must_bind_the_origin_invocation() {
    let mut fixture = EvidenceFixture::valid();
    fixture.rewrite_json(ORIGIN_TRACE, |trace| {
        trace["root"]["invocation"]["id"] = json!("other-invocation");
    });

    assert_identity_error(
        fixture.validation_error(),
        ORIGIN_TRACE,
        "invocation identity",
    );
}

#[test]
fn origin_trace_must_bind_the_origin_session() {
    let mut fixture = EvidenceFixture::valid();
    fixture.rewrite_json(ORIGIN_TRACE, |trace| {
        trace["root"]["session"]["provider_session_id"] = json!("other-session");
    });

    assert_identity_error(fixture.validation_error(), ORIGIN_TRACE, "session identity");
}

#[test]
fn answer_must_be_authorized_by_the_root() {
    let mut fixture = EvidenceFixture::valid();
    fixture.rewrite_json(ANSWER, |answer| {
        answer["answered_by"] = json!("model");
    });

    assert_identity_error(fixture.validation_error(), ANSWER, "root orchestrator");
}

#[test]
fn missing_ticket_snapshot_returns_typed_invalid_evidence() {
    let mut fixture = EvidenceFixture::valid();
    let ticket_path = fixture.artifact(TICKET_SNAPSHOT).path.clone();
    fixture.files.remove(&ticket_path);

    assert_identity_error(
        fixture.validation_error(),
        TICKET_SNAPSHOT,
        "could not be read",
    );
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
                br#"{"schema_version":1,"kind":"agent_question","question_id":"question-1","origin":{"invocation_uuid":"origin-invocation","session_id":"origin-session","worktree_path":"/worktree"},"state_refs":{"session_graph_manifest":"/planning/graph.json"}}"#.to_vec(),
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

    fn validated(&self) -> oulipoly_runtime::fresh_continuation::ValidatedContinuation {
        self.validator().validate(&self.request).unwrap()
    }

    fn fingerprint(&self) -> String {
        self.validated().fingerprint
    }

    fn validation_error(&self) -> ContinuationBlock {
        self.validator().validate(&self.request).unwrap_err()
    }

    fn validator(&self) -> DefaultContinuationEvidenceValidator<ArtifactSourceFake> {
        DefaultContinuationEvidenceValidator::new(ArtifactSourceFake {
            files: self.files.clone(),
        })
    }

    fn artifact(&self, name: &str) -> &ArtifactIdentity {
        match name {
            QUESTION => &self.request.evidence.question,
            ANSWER => &self.request.evidence.answer,
            SESSION_GRAPH => &self.request.evidence.session_graph,
            ORIGIN_TRACE => &self.request.evidence.origin_trace,
            TICKET_SNAPSHOT => &self.request.evidence.ticket_snapshot,
            _ => panic!("unknown fixture artifact: {name}"),
        }
    }

    fn artifact_mut(&mut self, name: &str) -> &mut ArtifactIdentity {
        match name {
            QUESTION => &mut self.request.evidence.question,
            ANSWER => &mut self.request.evidence.answer,
            SESSION_GRAPH => &mut self.request.evidence.session_graph,
            ORIGIN_TRACE => &mut self.request.evidence.origin_trace,
            TICKET_SNAPSHOT => &mut self.request.evidence.ticket_snapshot,
            _ => panic!("unknown fixture artifact: {name}"),
        }
    }

    fn rewrite_json(&mut self, name: &str, edit: impl FnOnce(&mut Value)) {
        let path = self.artifact_path(name);
        let mut document = parse_json(self.file_bytes(&path));
        edit(&mut document);
        let bytes = format_json(&document);
        self.replace_bytes(name, bytes);
    }

    fn artifact_path(&self, name: &str) -> PathBuf {
        self.artifact(name).path.clone()
    }

    fn file_bytes(&self, path: &Path) -> &[u8] {
        self.files.get(path).unwrap()
    }

    fn replace_bytes(&mut self, name: &str, bytes: Vec<u8>) {
        let path = self.artifact(name).path.clone();
        self.files.insert(path, bytes.clone());
        self.artifact_mut(name).sha256 = digest(&bytes);
    }

    fn add_semantically_ignored_whitespace(&mut self, name: &str) {
        let path = self.artifact(name).path.clone();
        let mut bytes = self.files.get(&path).unwrap().clone();
        bytes.push(b' ');
        self.replace_bytes(name, bytes);
    }

    fn relocate_artifact(&mut self, name: &str, new_path: &str) {
        let old_path = self.artifact(name).path.clone();
        let bytes = self.files.remove(&old_path).unwrap();
        let new_path = PathBuf::from(new_path);
        self.files.insert(new_path.clone(), bytes);
        self.artifact_mut(name).path = new_path.clone();
        if name == SESSION_GRAPH {
            self.rewrite_json(QUESTION, |question| {
                question["state_refs"]["session_graph_manifest"] = json!(new_path);
            });
            self.rewrite_json(ANSWER, |answer| {
                answer["continuation_plan"]["session_graph_manifest"] = json!(new_path);
            });
        }
    }

    fn relocate_planning_root(&mut self, new_root: &str) {
        let new_root = PathBuf::from(new_root);
        for name in [
            QUESTION,
            ANSWER,
            SESSION_GRAPH,
            ORIGIN_TRACE,
            TICKET_SNAPSHOT,
        ] {
            let file_name = self.artifact(name).path.file_name().unwrap().to_owned();
            let old_path = self.artifact(name).path.clone();
            let bytes = self.files.remove(&old_path).unwrap();
            let new_path = new_root.join(file_name);
            self.files.insert(new_path.clone(), bytes);
            self.artifact_mut(name).path = new_path;
        }
        self.request.planning_root = new_root;
        let graph_path = self.request.evidence.session_graph.path.clone();
        self.rewrite_json(QUESTION, |question| {
            question["state_refs"]["session_graph_manifest"] = json!(graph_path);
        });
        self.rewrite_json(ANSWER, |answer| {
            answer["continuation_plan"]["session_graph_manifest"] = json!(graph_path);
        });
    }

    fn rebind_question_id(&mut self, question_id: &str) {
        self.request.question_id = question_id.to_string();
        self.rewrite_json(QUESTION, |question| {
            question["question_id"] = json!(question_id);
        });
        self.rewrite_json(ANSWER, |answer| {
            answer["question_id"] = json!(question_id);
        });
        self.rewrite_json(SESSION_GRAPH, |graph| {
            graph["question_ids"] = json!([question_id]);
        });
    }

    fn rebind_origin_invocation(&mut self, invocation_id: &str) {
        self.request.origin_invocation_id = invocation_id.to_string();
        self.rewrite_json(QUESTION, |question| {
            question["origin"]["invocation_uuid"] = json!(invocation_id);
        });
        self.rewrite_json(SESSION_GRAPH, |graph| {
            graph["root_invocation_uuid"] = json!(invocation_id);
            graph["invocation_ids"] = json!([invocation_id]);
        });
        self.rewrite_json(ORIGIN_TRACE, |trace| {
            trace["root"]["invocation"]["id"] = json!(invocation_id);
        });
    }

    fn rebind_origin_session(&mut self, session_id: &str) {
        self.request.origin_session_id = session_id.to_string();
        self.rewrite_json(QUESTION, |question| {
            question["origin"]["session_id"] = json!(session_id);
        });
        self.rewrite_json(SESSION_GRAPH, |graph| {
            graph["session_ids"] = json!([session_id]);
        });
        self.rewrite_json(ORIGIN_TRACE, |trace| {
            trace["root"]["session"]["provider_session_id"] = json!(session_id);
        });
    }

    fn rebind_worktree(&mut self, worktree: &str) {
        self.request.worktree = PathBuf::from(worktree);
        self.rewrite_json(QUESTION, |question| {
            question["origin"]["worktree_path"] = json!(worktree);
        });
    }
}

fn parse_json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

fn format_json(document: &Value) -> Vec<u8> {
    serde_json::to_vec(document).unwrap()
}

fn assert_fingerprint_changed(fixture: &EvidenceFixture, original: &str, identity: &str) {
    let changed = fixture.fingerprint();
    assert_ne!(changed, original, "{identity} must bind the fingerprint");
}

fn assert_artifact_content_changes_fingerprint(name: &str) {
    let mut fixture = EvidenceFixture::valid();
    let original = fixture.fingerprint();
    fixture.add_semantically_ignored_whitespace(name);

    assert_fingerprint_changed(&fixture, &original, &format!("{name} hash"));
}

fn assert_artifact_path_changes_fingerprint(name: &str, new_path: &str) {
    let mut fixture = EvidenceFixture::valid();
    let original = fixture.fingerprint();
    fixture.relocate_artifact(name, new_path);

    assert_fingerprint_changed(&fixture, &original, &format!("{name} path"));
}

fn assert_changed_manifest_hash_rejected(name: &str) {
    let mut fixture = EvidenceFixture::valid();
    fixture.artifact_mut(name).sha256 = digest(b"different bytes");

    assert_identity_error(fixture.validation_error(), name, "hash identity");
}

fn assert_changed_manifest_path_rejected(name: &str, path: &str) {
    let mut fixture = EvidenceFixture::valid();
    fixture.artifact_mut(name).path = PathBuf::from(path);

    assert_identity_error(fixture.validation_error(), name, "could not be read");
}

fn assert_noncanonical_artifact_path_rejected(name: &str, path: &str) {
    let mut fixture = EvidenceFixture::valid();
    fixture.artifact_mut(name).path = PathBuf::from(path);

    assert_identity_error(fixture.validation_error(), name, "planning root");
}

fn assert_identity_error(error: ContinuationBlock, artifact: &str, identity: &str) {
    assert_eq!(error.kind, ContinuationBlockKind::InvalidEvidence);
    assert!(error.message.contains(artifact), "{artifact}: {error:?}");
    assert!(error.message.contains(identity), "{artifact}: {error:?}");
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
