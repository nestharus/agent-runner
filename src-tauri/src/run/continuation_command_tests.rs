//! ## Declared roles
//!
//! `orchestration`, `mapper`, `validator`, `accessor`, `formatter`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/run/continuation_command_tests.rs
//!     role: adapter
//!     Translates:
//!       - fresh-continuation-command-composition-contract
//!       - filesystem-and-SQLite-test-fixture-contract
//! ```

use std::cell::Cell;
use std::fs;
use std::path::Path;

use oulipoly_runtime::fresh_continuation::{
    ArtifactIdentity, ContinuationArtifactSource, ContinuationBlockKind, ContinuationEvidence,
    FreshContinuationOutcome, FreshContinuationRequest, InvocationOutcome, ValidatedContinuation,
};
use oulipoly_state::{InvocationStart, ProviderSessionBinding, StateDb};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::reservation::ReservedRun;

const ORIGIN_INVOCATION_ID: &str = "11111111-1111-4111-8111-111111111111";
const ORIGIN_SESSION_ID: &str = "origin-session";

#[cfg(unix)]
#[test]
fn filesystem_evidence_reader_rejects_a_symlink_outside_the_planning_root() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let planning_root = temp.path().join("planning");
    let outside = temp.path().join("outside.json");
    fs::create_dir(&planning_root).unwrap();
    fs::write(&outside, b"outside").unwrap();
    let linked = planning_root.join("linked.json");
    symlink(&outside, &linked).unwrap();
    let mut source =
        super::continuation_artifact::FilesystemContinuationArtifactSource::new(&planning_root)
            .unwrap();

    let error = source
        .read(&ArtifactIdentity {
            path: linked,
            sha256: format!("{:x}", Sha256::digest(b"outside")),
        })
        .unwrap_err();

    assert_eq!(error.kind, ContinuationBlockKind::InvalidEvidence);
}

#[cfg(unix)]
#[test]
fn filesystem_evidence_reader_rejects_symlinked_parent_traversal() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let planning_root = temp.path().join("planning");
    let outside = temp.path().join("outside");
    fs::create_dir(&planning_root).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("artifact.json"), b"outside").unwrap();
    symlink(&outside, planning_root.join("linked")).unwrap();
    let mut source =
        super::continuation_artifact::FilesystemContinuationArtifactSource::new(&planning_root)
            .unwrap();

    let error = source
        .read(&ArtifactIdentity {
            path: planning_root.join("linked/artifact.json"),
            sha256: format!("{:x}", Sha256::digest(b"outside")),
        })
        .unwrap_err();

    assert_eq!(error.kind, ContinuationBlockKind::InvalidEvidence);
}

#[cfg(unix)]
#[test]
fn filesystem_evidence_reader_uses_the_retained_planning_root_handle() {
    let temp = tempfile::tempdir().unwrap();
    let planning_root = temp.path().join("planning");
    let moved_root = temp.path().join("moved-planning");
    let artifact_path = planning_root.join("artifact.json");
    fs::create_dir(&planning_root).unwrap();
    fs::write(&artifact_path, b"original").unwrap();
    let mut source =
        super::continuation_artifact::FilesystemContinuationArtifactSource::new(&planning_root)
            .unwrap();

    fs::rename(&planning_root, &moved_root).unwrap();
    fs::create_dir(&planning_root).unwrap();
    fs::write(&artifact_path, b"replacement").unwrap();

    let bytes = source
        .read(&ArtifactIdentity {
            path: artifact_path,
            sha256: format!("{:x}", Sha256::digest(b"original")),
        })
        .unwrap();

    assert_eq!(bytes, b"original");
}

#[test]
fn composed_continuation_runs_each_reserved_stage_once_and_replays_without_relaunch() {
    let temp = tempfile::tempdir().unwrap();
    let request = request_fixture(temp.path());
    let db_path = temp.path().join("state.db");
    let seed = StateDb::open(&db_path).unwrap();
    start_invocation(&seed, ORIGIN_INVOCATION_ID, None);
    drop(seed);

    let resume_calls = Cell::new(0);
    let fresh_calls = Cell::new(0);
    let first = execute(&db_path, request.clone(), &resume_calls, &fresh_calls);

    let FreshContinuationOutcome::Continued { handoff, .. } = &first else {
        panic!("expected a completed continuation, observed {first:?}");
    };
    assert_eq!(resume_calls.get(), 1);
    assert_eq!(fresh_calls.get(), 1);
    assert_eq!(
        handoff.sha256,
        format!("{:x}", Sha256::digest(fs::read(&handoff.path).unwrap()))
    );

    let replay = execute(&db_path, request, &resume_calls, &fresh_calls);

    assert_eq!(replay, first);
    assert_eq!(resume_calls.get(), 1);
    assert_eq!(fresh_calls.get(), 1);
}

fn execute(
    db_path: &Path,
    request: FreshContinuationRequest,
    resume_calls: &Cell<usize>,
    fresh_calls: &Cell<usize>,
) -> FreshContinuationOutcome {
    let store_state = StateDb::open(db_path).unwrap();
    let observation_state = StateDb::open(db_path).unwrap();
    super::continuation_command::execute_with_callbacks(
        request,
        store_state,
        &observation_state,
        |reserved: &ReservedRun, _: &ValidatedContinuation| {
            resume_calls.set(resume_calls.get() + 1);
            let row_id = start_invocation(
                &observation_state,
                reserved.invocation_id(),
                Some(reserved.parent_invocation_row_id()),
            );
            bind_session(
                &observation_state,
                row_id,
                ORIGIN_SESSION_ID,
                "resumed",
                Some(ORIGIN_SESSION_ID),
            );
            observation_state
                .update_resume_acceptance(row_id, "accepted", Some("matched origin session"))
                .unwrap();
            observation_state
                .finalize_invocation(
                    row_id,
                    false,
                    0,
                    Some("resume_completion_unconfirmed"),
                    Some("resume_completion_unconfirmed"),
                )
                .unwrap();
            Ok(false)
        },
        |reserved: &ReservedRun, _: &ValidatedContinuation, _: &InvocationOutcome| {
            fresh_calls.set(fresh_calls.get() + 1);
            let row_id = start_invocation(
                &observation_state,
                reserved.invocation_id(),
                Some(reserved.parent_invocation_row_id()),
            );
            bind_session(
                &observation_state,
                row_id,
                "fresh-session",
                "forced_flag_verified",
                None,
            );
            observation_state
                .finalize_invocation(row_id, true, 0, None, None)
                .unwrap();
            Ok(())
        },
    )
}

fn request_fixture(root: &Path) -> FreshContinuationRequest {
    let planning_root = root.join("planning");
    let worktree = root.join("worktree");
    fs::create_dir_all(&planning_root).unwrap();
    fs::create_dir_all(&worktree).unwrap();
    let graph_path = planning_root.join("graph.json");
    let question = write_artifact(
        &planning_root.join("question.json"),
        &serde_json::to_vec(&json!({
            "schema_version": 1,
            "kind": "agent_question",
            "question_id": "question-1",
            "origin": {
                "invocation_uuid": ORIGIN_INVOCATION_ID,
                "session_id": ORIGIN_SESSION_ID,
                "worktree_path": worktree,
            },
            "state_refs": {"session_graph_manifest": graph_path},
        }))
        .unwrap(),
    );
    let answer = write_artifact(
        &planning_root.join("answer.json"),
        &serde_json::to_vec(&json!({
            "schema_version": 1,
            "kind": "agent_answer",
            "question_id": "question-1",
            "answered_by": "user-via-root-orchestrator",
            "continuation_plan": {"session_graph_manifest": graph_path},
        }))
        .unwrap(),
    );
    let session_graph = write_artifact(
        &graph_path,
        &serde_json::to_vec(&json!({
            "root_invocation_uuid": ORIGIN_INVOCATION_ID,
            "invocation_ids": [ORIGIN_INVOCATION_ID],
            "session_ids": [ORIGIN_SESSION_ID],
            "question_ids": ["question-1"],
        }))
        .unwrap(),
    );
    let origin_trace = write_artifact(
        &planning_root.join("trace.json"),
        &serde_json::to_vec(&json!({
            "root": {
                "invocation": {"id": ORIGIN_INVOCATION_ID},
                "session": {"provider_session_id": ORIGIN_SESSION_ID},
            },
        }))
        .unwrap(),
    );
    let ticket_snapshot =
        write_artifact(&planning_root.join("ticket.md"), b"AGE-290 ticket snapshot");

    FreshContinuationRequest {
        question_id: "question-1".to_string(),
        origin_invocation_id: ORIGIN_INVOCATION_ID.to_string(),
        origin_session_id: ORIGIN_SESSION_ID.to_string(),
        planning_root,
        worktree,
        last_successful_boundary: "verified".to_string(),
        active_blocked_boundary: "apply".to_string(),
        target_model: "fresh-model".to_string(),
        evidence: ContinuationEvidence {
            question,
            answer,
            session_graph,
            origin_trace,
            ticket_snapshot,
        },
    }
}

fn write_artifact(path: &Path, bytes: &[u8]) -> ArtifactIdentity {
    fs::write(path, bytes).unwrap();
    ArtifactIdentity {
        path: path.to_path_buf(),
        sha256: format!("{:x}", Sha256::digest(bytes)),
    }
}

fn start_invocation(state: &StateDb, invocation_uuid: &str, parent: Option<i64>) -> i64 {
    state
        .start_invocation(&InvocationStart {
            invocation_uuid: invocation_uuid.to_string(),
            model_name: "fixture-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: parent,
        })
        .unwrap()
}

fn bind_session(
    state: &StateDb,
    row_id: i64,
    session_id: &str,
    capture_method: &'static str,
    resume_input_id: Option<&str>,
) {
    state
        .bind_invocation_provider_session_start(
            row_id,
            &ProviderSessionBinding {
                provider_session_id: session_id.to_string(),
                capture_method,
                resume_input_id: resume_input_id.map(str::to_string),
                provider_session_resolved_account: None,
            },
        )
        .unwrap();
}
