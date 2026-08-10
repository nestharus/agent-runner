//! ## Declared roles
//!
//! `orchestration`, `validator`, `mapper`, `formatter`, `accessor`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/run/continuation_handoff_tests.rs
//!     role: adapter
//!     Translates:
//!       - create-once-handoff-publication-behavior-contract
//!       - filesystem-test-fixture-contract
//! ```

use std::fs;
use std::path::PathBuf;

use oulipoly_runtime::fresh_continuation::{
    ArtifactIdentity, ContinuationBlockKind, ContinuationEvidence, ContinuationHandoff,
    FreshContinuationRequest, HandoffPublisher, InvocationDisposition, InvocationOutcome,
    ResumeAcceptance,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[test]
fn publisher_writes_exact_authoritative_handoff_body_and_final_bytes_hash() {
    let fixture = PublisherFixture::new();
    let mut publisher = fixture.publisher();

    let published = publisher.publish(handoff()).unwrap();

    assert!(published.path.starts_with(fixture.root()));
    let bytes = fs::read(&published.path).unwrap();
    assert_eq!(bytes, expected_bytes(&expected_handoff_json()));
    assert_eq!(published.sha256, sha256(&bytes));
    assert_eq!(
        fs::read_dir(published.path.parent().unwrap())
            .unwrap()
            .count(),
        1,
        "temporary publication files must be removed"
    );
}

#[test]
fn every_authoritative_handoff_field_independently_changes_exact_bytes_and_hash() {
    let baseline_bytes = expected_bytes(&expected_handoff_json());

    for case in mutation_cases() {
        let fixture = PublisherFixture::new();
        let mut publisher = fixture.publisher();
        let mut mutated = handoff();
        mutate(case.field, &mut mutated);

        let published = publisher.publish(mutated).unwrap();
        let bytes = fs::read(&published.path).unwrap();
        let actual: Value = serde_json::from_slice(&bytes).unwrap();
        let mut expected = expected_handoff_json();
        mutate_expected(case.field, &mut expected);

        assert_eq!(actual, expected, "wrong body for {}", case.name);
        assert_ne!(bytes, baseline_bytes, "{} was omitted", case.name);
        assert_eq!(
            published.sha256,
            sha256(&bytes),
            "{} was not hashed from final bytes",
            case.name
        );
    }
}

#[test]
fn exact_handoff_replay_returns_stable_create_once_identity() {
    let fixture = PublisherFixture::new();
    let mut publisher = fixture.publisher();
    let first = publisher.publish(handoff()).unwrap();
    let original = fs::read(&first.path).unwrap();

    let replay = publisher.publish(handoff()).unwrap();

    assert_eq!(replay, first);
    assert_eq!(fs::read(&first.path).unwrap(), original);
    assert_eq!(replay.sha256, sha256(&original));
}

#[test]
fn terminal_replay_verifies_the_recorded_create_once_handoff() {
    let fixture = PublisherFixture::new();
    let mut publisher = fixture.publisher();
    let published = publisher.publish(handoff()).unwrap();

    publisher.verify("continuation-1", &published).unwrap();
}

#[test]
fn terminal_replay_rejects_a_missing_recorded_handoff() {
    let fixture = PublisherFixture::new();
    let mut publisher = fixture.publisher();
    let published = publisher.publish(handoff()).unwrap();
    fs::remove_file(&published.path).unwrap();

    let error = publisher.verify("continuation-1", &published).unwrap_err();

    assert_eq!(error.kind, ContinuationBlockKind::Persistence);
}

#[test]
fn terminal_replay_rejects_replaced_recorded_handoff_bytes() {
    let fixture = PublisherFixture::new();
    let mut publisher = fixture.publisher();
    let published = publisher.publish(handoff()).unwrap();
    fs::write(&published.path, b"replacement").unwrap();

    let error = publisher.verify("continuation-1", &published).unwrap_err();

    assert_eq!(error.kind, ContinuationBlockKind::Conflict);
}

#[cfg(unix)]
#[test]
fn terminal_replay_rejects_a_symlinked_recorded_handoff() {
    use std::os::unix::fs::symlink;

    let fixture = PublisherFixture::new();
    let mut publisher = fixture.publisher();
    let published = publisher.publish(handoff()).unwrap();
    let replacement = fixture.root().join("replacement.json");
    fs::write(&replacement, fs::read(&published.path).unwrap()).unwrap();
    fs::remove_file(&published.path).unwrap();
    symlink(replacement, &published.path).unwrap();

    let error = publisher.verify("continuation-1", &published).unwrap_err();

    assert_eq!(error.kind, ContinuationBlockKind::Persistence);
}

#[test]
fn conflicting_handoff_body_is_rejected_without_changing_published_bytes() {
    let fixture = PublisherFixture::new();
    let mut publisher = fixture.publisher();
    let first = publisher.publish(handoff()).unwrap();
    let original = fs::read(&first.path).unwrap();
    let mut conflict = handoff();
    conflict.fresh_prompt = "conflicting prompt".to_string();

    let error = publisher.publish(conflict).unwrap_err();

    assert_eq!(error.kind, ContinuationBlockKind::Conflict);
    assert_eq!(fs::read(&first.path).unwrap(), original);
}

#[test]
fn invalid_continuation_id_cannot_escape_planning_root() {
    let fixture = PublisherFixture::new();
    let mut handoff = handoff();
    handoff.continuation_id = "../escape".to_string();
    let mut publisher = fixture.publisher();

    let error = publisher.publish(handoff).unwrap_err();

    assert_eq!(error.kind, ContinuationBlockKind::InvalidEvidence);
    assert!(!fixture.root().join("escape.json").exists());
}

struct PublisherFixture {
    directory: tempfile::TempDir,
}

impl PublisherFixture {
    fn new() -> Self {
        Self {
            directory: tempfile::tempdir().unwrap(),
        }
    }

    fn root(&self) -> &std::path::Path {
        self.directory.path()
    }

    fn publisher(&self) -> super::continuation_handoff::FilesystemHandoffPublisher {
        super::continuation_handoff::FilesystemHandoffPublisher::new(self.root().to_path_buf())
    }
}

#[derive(Clone, Copy)]
enum AuthoritativeField {
    FreshPrompt,
    QuestionId,
    OriginInvocationId,
    OriginSessionId,
    PlanningRoot,
    Worktree,
    LastSuccessfulBoundary,
    ActiveBlockedBoundary,
    TargetModel,
    QuestionPath,
    QuestionSha256,
    AnswerPath,
    AnswerSha256,
    SessionGraphPath,
    SessionGraphSha256,
    OriginTracePath,
    OriginTraceSha256,
    TicketSnapshotPath,
    TicketSnapshotSha256,
    ResumeInvocationId,
    ResumeSessionId,
    ResumePhysicalExitCode,
    ResumeAcceptanceField,
    ResumeDisposition,
    ResumeErrorCategory,
    ResumeTerminalReason,
    FreshInvocationId,
    FreshSessionId,
    FreshPhysicalExitCode,
    FreshAcceptanceField,
    FreshDisposition,
    FreshErrorCategory,
    FreshTerminalReason,
}

struct MutationCase {
    name: &'static str,
    field: AuthoritativeField,
}

fn mutation_cases() -> Vec<MutationCase> {
    use AuthoritativeField::*;
    [
        ("fresh prompt", FreshPrompt),
        ("question ID", QuestionId),
        ("origin invocation ID", OriginInvocationId),
        ("origin session ID", OriginSessionId),
        ("planning root", PlanningRoot),
        ("worktree", Worktree),
        ("last successful boundary", LastSuccessfulBoundary),
        ("active blocked boundary", ActiveBlockedBoundary),
        ("target model", TargetModel),
        ("question path", QuestionPath),
        ("question SHA-256", QuestionSha256),
        ("answer path", AnswerPath),
        ("answer SHA-256", AnswerSha256),
        ("session graph path", SessionGraphPath),
        ("session graph SHA-256", SessionGraphSha256),
        ("origin trace path", OriginTracePath),
        ("origin trace SHA-256", OriginTraceSha256),
        ("ticket snapshot path", TicketSnapshotPath),
        ("ticket snapshot SHA-256", TicketSnapshotSha256),
        ("resume invocation ID", ResumeInvocationId),
        ("resume session ID", ResumeSessionId),
        ("resume physical exit code", ResumePhysicalExitCode),
        ("resume acceptance", ResumeAcceptanceField),
        ("resume disposition", ResumeDisposition),
        ("resume error category", ResumeErrorCategory),
        ("resume terminal reason", ResumeTerminalReason),
        ("fresh invocation ID", FreshInvocationId),
        ("fresh session ID", FreshSessionId),
        ("fresh physical exit code", FreshPhysicalExitCode),
        ("fresh acceptance", FreshAcceptanceField),
        ("fresh disposition", FreshDisposition),
        ("fresh error category", FreshErrorCategory),
        ("fresh terminal reason", FreshTerminalReason),
    ]
    .into_iter()
    .map(|(name, field)| MutationCase { name, field })
    .collect()
}

fn mutate(field: AuthoritativeField, handoff: &mut ContinuationHandoff) {
    use AuthoritativeField::*;
    match field {
        FreshPrompt => handoff.fresh_prompt = "mutated prompt".to_string(),
        QuestionId => handoff.request.question_id = "mutated-question".to_string(),
        OriginInvocationId => {
            handoff.request.origin_invocation_id = "mutated-origin-invocation".to_string()
        }
        OriginSessionId => handoff.request.origin_session_id = "mutated-origin-session".to_string(),
        PlanningRoot => handoff.request.planning_root = PathBuf::from("/mutated/planning"),
        Worktree => handoff.request.worktree = PathBuf::from("/mutated/worktree"),
        LastSuccessfulBoundary => {
            handoff.request.last_successful_boundary = "mutated-success-boundary".to_string()
        }
        ActiveBlockedBoundary => {
            handoff.request.active_blocked_boundary = "mutated-blocked-boundary".to_string()
        }
        TargetModel => handoff.request.target_model = "mutated-model".to_string(),
        QuestionPath => {
            handoff.request.evidence.question.path = PathBuf::from("/planning/mutated-question")
        }
        QuestionSha256 => {
            handoff.request.evidence.question.sha256 = "mutated-question-sha".to_string()
        }
        AnswerPath => {
            handoff.request.evidence.answer.path = PathBuf::from("/planning/mutated-answer")
        }
        AnswerSha256 => handoff.request.evidence.answer.sha256 = "mutated-answer-sha".to_string(),
        SessionGraphPath => {
            handoff.request.evidence.session_graph.path =
                PathBuf::from("/planning/mutated-session-graph")
        }
        SessionGraphSha256 => {
            handoff.request.evidence.session_graph.sha256 = "mutated-session-graph-sha".to_string()
        }
        OriginTracePath => {
            handoff.request.evidence.origin_trace.path =
                PathBuf::from("/planning/mutated-origin-trace")
        }
        OriginTraceSha256 => {
            handoff.request.evidence.origin_trace.sha256 = "mutated-origin-trace-sha".to_string()
        }
        TicketSnapshotPath => {
            handoff.request.evidence.ticket_snapshot.path =
                PathBuf::from("/planning/mutated-ticket-snapshot")
        }
        TicketSnapshotSha256 => {
            handoff.request.evidence.ticket_snapshot.sha256 =
                "mutated-ticket-snapshot-sha".to_string()
        }
        ResumeInvocationId => handoff.resume.invocation_id = "mutated-resume".to_string(),
        ResumeSessionId => handoff.resume.session_id = Some("mutated-resume-session".to_string()),
        ResumePhysicalExitCode => handoff.resume.physical_exit_code = 17,
        ResumeAcceptanceField => handoff.resume.acceptance = ResumeAcceptance::Rejected,
        ResumeDisposition => handoff.resume.disposition = InvocationDisposition::Succeeded,
        ResumeErrorCategory => {
            let InvocationDisposition::Failed { error_category, .. } =
                &mut handoff.resume.disposition
            else {
                unreachable!()
            };
            *error_category = "mutated-error-category".to_string();
        }
        ResumeTerminalReason => {
            let InvocationDisposition::Failed {
                terminal_reason, ..
            } = &mut handoff.resume.disposition
            else {
                unreachable!()
            };
            *terminal_reason = "mutated-terminal-reason".to_string();
        }
        FreshInvocationId => fresh_mut(handoff).invocation_id = "mutated-fresh".to_string(),
        FreshSessionId => fresh_mut(handoff).session_id = Some("mutated-fresh-session".to_string()),
        FreshPhysicalExitCode => fresh_mut(handoff).physical_exit_code = 23,
        FreshAcceptanceField => fresh_mut(handoff).acceptance = ResumeAcceptance::Unconfirmed,
        FreshDisposition => fresh_mut(handoff).disposition = InvocationDisposition::Succeeded,
        FreshErrorCategory => {
            let InvocationDisposition::Failed { error_category, .. } =
                &mut fresh_mut(handoff).disposition
            else {
                unreachable!()
            };
            *error_category = "mutated-fresh-error".to_string();
        }
        FreshTerminalReason => {
            let InvocationDisposition::Failed {
                terminal_reason, ..
            } = &mut fresh_mut(handoff).disposition
            else {
                unreachable!()
            };
            *terminal_reason = "mutated-fresh-reason".to_string();
        }
    }
}

fn mutate_expected(field: AuthoritativeField, expected: &mut Value) {
    use AuthoritativeField::*;
    let (pointer, value) = match field {
        FreshPrompt => ("/fresh_prompt", json!("mutated prompt")),
        QuestionId => ("/request/question_id", json!("mutated-question")),
        OriginInvocationId => (
            "/request/origin_invocation_id",
            json!("mutated-origin-invocation"),
        ),
        OriginSessionId => (
            "/request/origin_session_id",
            json!("mutated-origin-session"),
        ),
        PlanningRoot => ("/request/planning_root", json!("/mutated/planning")),
        Worktree => ("/request/worktree", json!("/mutated/worktree")),
        LastSuccessfulBoundary => (
            "/request/last_successful_boundary",
            json!("mutated-success-boundary"),
        ),
        ActiveBlockedBoundary => (
            "/request/active_blocked_boundary",
            json!("mutated-blocked-boundary"),
        ),
        TargetModel => ("/request/target_model", json!("mutated-model")),
        QuestionPath => (
            "/request/evidence/question/path",
            json!("/planning/mutated-question"),
        ),
        QuestionSha256 => (
            "/request/evidence/question/sha256",
            json!("mutated-question-sha"),
        ),
        AnswerPath => (
            "/request/evidence/answer/path",
            json!("/planning/mutated-answer"),
        ),
        AnswerSha256 => (
            "/request/evidence/answer/sha256",
            json!("mutated-answer-sha"),
        ),
        SessionGraphPath => (
            "/request/evidence/session_graph/path",
            json!("/planning/mutated-session-graph"),
        ),
        SessionGraphSha256 => (
            "/request/evidence/session_graph/sha256",
            json!("mutated-session-graph-sha"),
        ),
        OriginTracePath => (
            "/request/evidence/origin_trace/path",
            json!("/planning/mutated-origin-trace"),
        ),
        OriginTraceSha256 => (
            "/request/evidence/origin_trace/sha256",
            json!("mutated-origin-trace-sha"),
        ),
        TicketSnapshotPath => (
            "/request/evidence/ticket_snapshot/path",
            json!("/planning/mutated-ticket-snapshot"),
        ),
        TicketSnapshotSha256 => (
            "/request/evidence/ticket_snapshot/sha256",
            json!("mutated-ticket-snapshot-sha"),
        ),
        ResumeInvocationId => ("/resume/invocation_id", json!("mutated-resume")),
        ResumeSessionId => ("/resume/session_id", json!("mutated-resume-session")),
        ResumePhysicalExitCode => ("/resume/physical_exit_code", json!(17)),
        ResumeAcceptanceField => ("/resume/acceptance", json!("rejected")),
        ResumeDisposition => {
            *expected.pointer_mut("/resume/disposition").unwrap() = json!({"status": "succeeded"});
            return;
        }
        ResumeErrorCategory => (
            "/resume/disposition/error_category",
            json!("mutated-error-category"),
        ),
        ResumeTerminalReason => (
            "/resume/disposition/terminal_reason",
            json!("mutated-terminal-reason"),
        ),
        FreshInvocationId => ("/fresh/invocation_id", json!("mutated-fresh")),
        FreshSessionId => ("/fresh/session_id", json!("mutated-fresh-session")),
        FreshPhysicalExitCode => ("/fresh/physical_exit_code", json!(23)),
        FreshAcceptanceField => ("/fresh/acceptance", json!("unconfirmed")),
        FreshDisposition => {
            *expected.pointer_mut("/fresh/disposition").unwrap() = json!({"status": "succeeded"});
            return;
        }
        FreshErrorCategory => (
            "/fresh/disposition/error_category",
            json!("mutated-fresh-error"),
        ),
        FreshTerminalReason => (
            "/fresh/disposition/terminal_reason",
            json!("mutated-fresh-reason"),
        ),
    };
    *expected.pointer_mut(pointer).unwrap() = value;
}

fn fresh_mut(handoff: &mut ContinuationHandoff) -> &mut InvocationOutcome {
    handoff.fresh.as_mut().unwrap()
}

fn handoff() -> ContinuationHandoff {
    ContinuationHandoff {
        continuation_id: "continuation-1".to_string(),
        fresh_prompt: "exact fresh prompt\nwith a final line\n".to_string(),
        request: FreshContinuationRequest {
            question_id: "question-1".to_string(),
            origin_invocation_id: "origin-invocation-2".to_string(),
            origin_session_id: "origin-session-3".to_string(),
            planning_root: PathBuf::from("/planning/root-4"),
            worktree: PathBuf::from("/worktree/root-5"),
            last_successful_boundary: "successful-boundary-6".to_string(),
            active_blocked_boundary: "blocked-boundary-7".to_string(),
            target_model: "target-model-8".to_string(),
            evidence: ContinuationEvidence {
                question: artifact("question-9.json", "question-sha-10"),
                answer: artifact("answer-11.json", "answer-sha-12"),
                session_graph: artifact("session-graph-13.json", "session-graph-sha-14"),
                origin_trace: artifact("origin-trace-15.json", "origin-trace-sha-16"),
                ticket_snapshot: artifact("ticket-17.md", "ticket-sha-18"),
            },
        },
        resume: InvocationOutcome {
            invocation_id: "resume-invocation-19".to_string(),
            session_id: Some("resume-session-20".to_string()),
            physical_exit_code: 21,
            acceptance: ResumeAcceptance::Accepted,
            disposition: InvocationDisposition::Failed {
                error_category: "resume-error-22".to_string(),
                terminal_reason: "resume-reason-23".to_string(),
            },
        },
        fresh: Some(InvocationOutcome {
            invocation_id: "fresh-invocation-24".to_string(),
            session_id: Some("fresh-session-25".to_string()),
            physical_exit_code: 26,
            acceptance: ResumeAcceptance::NotApplicable,
            disposition: InvocationDisposition::Failed {
                error_category: "fresh-error-27".to_string(),
                terminal_reason: "fresh-reason-28".to_string(),
            },
        }),
    }
}

fn artifact(name: &str, sha256: &str) -> ArtifactIdentity {
    ArtifactIdentity {
        path: PathBuf::from(format!("/planning/{name}")),
        sha256: sha256.to_string(),
    }
}

fn expected_handoff_json() -> Value {
    json!({
        "schema_version": 1,
        "kind": "fresh_continuation_handoff",
        "continuation_id": "continuation-1",
        "fresh_prompt": "exact fresh prompt\nwith a final line\n",
        "request": {
            "schema_version": 1,
            "kind": "fresh_continuation_request",
            "question_id": "question-1",
            "origin_invocation_id": "origin-invocation-2",
            "origin_session_id": "origin-session-3",
            "planning_root": "/planning/root-4",
            "worktree": "/worktree/root-5",
            "last_successful_boundary": "successful-boundary-6",
            "active_blocked_boundary": "blocked-boundary-7",
            "target_model": "target-model-8",
            "evidence": {
                "question": {"path": "/planning/question-9.json", "sha256": "question-sha-10"},
                "answer": {"path": "/planning/answer-11.json", "sha256": "answer-sha-12"},
                "session_graph": {"path": "/planning/session-graph-13.json", "sha256": "session-graph-sha-14"},
                "origin_trace": {"path": "/planning/origin-trace-15.json", "sha256": "origin-trace-sha-16"},
                "ticket_snapshot": {"path": "/planning/ticket-17.md", "sha256": "ticket-sha-18"},
            },
        },
        "resume": {
            "invocation_id": "resume-invocation-19",
            "session_id": "resume-session-20",
            "physical_exit_code": 21,
            "acceptance": "accepted",
            "disposition": {
                "status": "failed",
                "error_category": "resume-error-22",
                "terminal_reason": "resume-reason-23",
            },
        },
        "fresh": {
            "invocation_id": "fresh-invocation-24",
            "session_id": "fresh-session-25",
            "physical_exit_code": 26,
            "acceptance": "not_applicable",
            "disposition": {
                "status": "failed",
                "error_category": "fresh-error-27",
                "terminal_reason": "fresh-reason-28",
            },
        },
    })
}

fn expected_bytes(value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(value).unwrap();
    bytes.push(b'\n');
    bytes
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
