//! ## Declared roles
//!
//! `orchestration`, `validator`, `mapper`, `accessor`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/run/continuation_request_tests.rs
//!     role: adapter
//!     Translates:
//!       - versioned-request-and-prompt-behavior-contract
//!       - filesystem-request-test-fixture-contract
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use oulipoly_runtime::fresh_continuation::{
    ArtifactIdentity, ContinuationEvidence, FreshContinuationRequest, InvocationDisposition,
    InvocationOutcome, ResumeAcceptance, ValidatedContinuation,
};
use serde_json::json;

#[test]
fn versioned_request_maps_exact_internal_contract() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_request(dir.path(), valid_request_json());

    let request = super::continuation_request::read(&path).unwrap();

    assert_eq!(request, request_contract());
}

#[test]
fn unsupported_request_version_and_kind_have_distinct_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let mut wrong_version = valid_request_json();
    wrong_version["schema_version"] = json!(2);
    let version_path = write_named_request(dir.path(), "version.json", wrong_version);
    let mut wrong_kind = valid_request_json();
    wrong_kind["kind"] = json!("other_request");
    let kind_path = write_named_request(dir.path(), "kind.json", wrong_kind);

    let version_error = super::continuation_request::read(&version_path).unwrap_err();
    let kind_error = super::continuation_request::read(&kind_path).unwrap_err();

    assert_eq!(
        version_error.kind,
        oulipoly_runtime::fresh_continuation::ContinuationBlockKind::InvalidEvidence
    );
    assert_eq!(
        version_error.message,
        "Unsupported fresh continuation request schema version 2 (expected 1)"
    );
    assert_eq!(
        kind_error.kind,
        oulipoly_runtime::fresh_continuation::ContinuationBlockKind::InvalidEvidence
    );
    assert_eq!(
        kind_error.message,
        "Unsupported fresh continuation request kind \"other_request\" (expected \"fresh_continuation_request\")"
    );
}

#[test]
fn unknown_request_field_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut value = valid_request_json();
    value["unexpected"] = json!(true);
    let path = write_request(dir.path(), value);

    let error = super::continuation_request::read(&path).unwrap_err();

    assert_eq!(
        error.kind,
        oulipoly_runtime::fresh_continuation::ContinuationBlockKind::InvalidEvidence
    );
}

#[test]
fn unknown_evidence_field_is_rejected_during_request_parsing() {
    let dir = tempfile::tempdir().unwrap();
    let mut value = valid_request_json();
    value["evidence"]["unexpected"] = json!(true);
    let path = write_request(dir.path(), value);

    let error = super::continuation_request::read(&path).unwrap_err();

    assert_eq!(
        error.kind,
        oulipoly_runtime::fresh_continuation::ContinuationBlockKind::InvalidEvidence
    );
}

#[test]
fn unknown_artifact_identity_field_is_rejected_during_request_parsing() {
    let dir = tempfile::tempdir().unwrap();
    let mut value = valid_request_json();
    value["evidence"]["question"]["unexpected"] = json!(true);
    let path = write_request(dir.path(), value);

    let error = super::continuation_request::read(&path).unwrap_err();

    assert_eq!(
        error.kind,
        oulipoly_runtime::fresh_continuation::ContinuationBlockKind::InvalidEvidence
    );
}

#[test]
fn fresh_prompt_names_exact_pull_references_and_boundaries() {
    let context = ValidatedContinuation {
        request: request_contract(),
        fingerprint: "request-fingerprint".to_string(),
    };

    let prompt = super::continuation_request::fresh_prompt(&context, &resume_outcome());

    for expected in [
        "origin-invocation",
        "origin-session",
        "resume-invocation",
        "/planning/question.json",
        "question-sha",
        "/planning/answer.json",
        "answer-sha",
        "/planning/graph.json",
        "graph-sha",
        "/planning/trace.json",
        "trace-sha",
        "/planning/ticket.md",
        "ticket-sha",
        "phase-4-verified",
        "phase-5-apply-answer",
        "/worktree",
    ] {
        assert!(prompt.contains(expected), "missing {expected}: {prompt}");
    }
    assert!(prompt.contains("Do not retry or mutate the origin session"));
}

#[test]
fn fresh_prompt_contains_references_not_artifact_bodies() {
    let dir = tempfile::tempdir().unwrap();
    let question_path = dir.path().join("question.json");
    fs::write(&question_path, "SECRET_ARTIFACT_BODY").unwrap();
    let mut context = ValidatedContinuation {
        request: request_contract(),
        fingerprint: "request-fingerprint".to_string(),
    };
    context.request.evidence.question.path = question_path.clone();

    let prompt = super::continuation_request::fresh_prompt(&context, &resume_outcome());

    assert!(prompt.contains(&question_path.display().to_string()));
    assert!(!prompt.contains("SECRET_ARTIFACT_BODY"));
}

fn write_request(root: &Path, value: serde_json::Value) -> PathBuf {
    write_named_request(root, "request.json", value)
}

fn write_named_request(root: &Path, name: &str, value: serde_json::Value) -> PathBuf {
    let path = root.join(name);
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    path
}

fn valid_request_json() -> serde_json::Value {
    json!({
        "schema_version": 1,
        "kind": "fresh_continuation_request",
        "question_id": "question-1",
        "origin_invocation_id": "origin-invocation",
        "origin_session_id": "origin-session",
        "planning_root": "/planning",
        "worktree": "/worktree",
        "last_successful_boundary": "phase-4-verified",
        "active_blocked_boundary": "phase-5-apply-answer",
        "target_model": "fresh-model",
        "evidence": {
            "question": {"path": "/planning/question.json", "sha256": "question-sha"},
            "answer": {"path": "/planning/answer.json", "sha256": "answer-sha"},
            "session_graph": {"path": "/planning/graph.json", "sha256": "graph-sha"},
            "origin_trace": {"path": "/planning/trace.json", "sha256": "trace-sha"},
            "ticket_snapshot": {"path": "/planning/ticket.md", "sha256": "ticket-sha"}
        }
    })
}

fn request_contract() -> FreshContinuationRequest {
    FreshContinuationRequest {
        question_id: "question-1".to_string(),
        origin_invocation_id: "origin-invocation".to_string(),
        origin_session_id: "origin-session".to_string(),
        planning_root: PathBuf::from("/planning"),
        worktree: PathBuf::from("/worktree"),
        last_successful_boundary: "phase-4-verified".to_string(),
        active_blocked_boundary: "phase-5-apply-answer".to_string(),
        target_model: "fresh-model".to_string(),
        evidence: ContinuationEvidence {
            question: artifact("question.json", "question-sha"),
            answer: artifact("answer.json", "answer-sha"),
            session_graph: artifact("graph.json", "graph-sha"),
            origin_trace: artifact("trace.json", "trace-sha"),
            ticket_snapshot: artifact("ticket.md", "ticket-sha"),
        },
    }
}

fn artifact(name: &str, sha256: &str) -> ArtifactIdentity {
    ArtifactIdentity {
        path: PathBuf::from(format!("/planning/{name}")),
        sha256: sha256.to_string(),
    }
}

fn resume_outcome() -> InvocationOutcome {
    InvocationOutcome {
        invocation_id: "resume-invocation".to_string(),
        session_id: Some("origin-session".to_string()),
        physical_exit_code: 0,
        acceptance: ResumeAcceptance::Accepted,
        disposition: InvocationDisposition::Failed {
            error_category: "resume_completion_unconfirmed".to_string(),
            terminal_reason: "resume_completion_unconfirmed".to_string(),
        },
    }
}
