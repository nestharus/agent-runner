use std::fs;

use oulipoly_runtime::fresh_continuation::{
    ContinuationBlockKind, ContinuationHandoff, HandoffPublisher, InvocationDisposition,
    InvocationOutcome, ResumeAcceptance,
};
use sha2::{Digest, Sha256};

#[test]
fn publisher_writes_exact_versioned_handoff_and_hash() {
    let dir = tempfile::tempdir().unwrap();
    let mut publisher =
        super::continuation_handoff::FilesystemHandoffPublisher::new(dir.path().to_path_buf());

    let published = publisher.publish(handoff()).unwrap();

    assert_eq!(
        published.path,
        dir.path().join("continuations").join("continuation-1.json")
    );
    let bytes = fs::read(&published.path).unwrap();
    assert_eq!(published.sha256, sha256(&bytes));
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["kind"], "fresh_continuation_handoff");
    assert_eq!(value["continuation_id"], "continuation-1");
    assert_eq!(value["resume"]["invocation_id"], "resume-invocation");
    assert_eq!(value["resume"]["acceptance"], "accepted");
    assert_eq!(value["resume"]["disposition"]["status"], "failed");
    assert_eq!(
        value["resume"]["disposition"]["error_category"],
        "resume_completion_unconfirmed"
    );
    assert_eq!(value["fresh"]["invocation_id"], "fresh-invocation");
    assert_eq!(value["fresh"]["session_id"], "fresh-session");
    assert_eq!(value["fresh"]["disposition"]["status"], "succeeded");
    let entries = fs::read_dir(published.path.parent().unwrap())
        .unwrap()
        .count();
    assert_eq!(entries, 1, "temporary publication files must be removed");
}

#[test]
fn exact_handoff_replay_reads_existing_immutable_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut publisher =
        super::continuation_handoff::FilesystemHandoffPublisher::new(dir.path().to_path_buf());
    let first = publisher.publish(handoff()).unwrap();
    let mut permissions = fs::metadata(&first.path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&first.path, permissions).unwrap();

    let replay = publisher.publish(handoff()).unwrap();

    assert_eq!(replay, first);
}

#[test]
fn conflicting_existing_handoff_is_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let output_dir = dir.path().join("continuations");
    fs::create_dir_all(&output_dir).unwrap();
    let path = output_dir.join("continuation-1.json");
    fs::write(&path, b"existing-conflict").unwrap();
    let mut publisher =
        super::continuation_handoff::FilesystemHandoffPublisher::new(dir.path().to_path_buf());

    let error = publisher.publish(handoff()).unwrap_err();

    assert_eq!(error.kind, ContinuationBlockKind::Conflict);
    assert_eq!(fs::read(path).unwrap(), b"existing-conflict");
}

#[test]
fn invalid_continuation_id_cannot_escape_planning_root() {
    let dir = tempfile::tempdir().unwrap();
    let mut handoff = handoff();
    handoff.continuation_id = "../escape".to_string();
    let mut publisher =
        super::continuation_handoff::FilesystemHandoffPublisher::new(dir.path().to_path_buf());

    let error = publisher.publish(handoff).unwrap_err();

    assert_eq!(error.kind, ContinuationBlockKind::InvalidEvidence);
    assert!(!dir.path().join("escape.json").exists());
}

fn handoff() -> ContinuationHandoff {
    ContinuationHandoff {
        continuation_id: "continuation-1".to_string(),
        resume: InvocationOutcome {
            invocation_id: "resume-invocation".to_string(),
            session_id: Some("origin-session".to_string()),
            physical_exit_code: 0,
            acceptance: ResumeAcceptance::Accepted,
            disposition: InvocationDisposition::Failed {
                error_category: "resume_completion_unconfirmed".to_string(),
                terminal_reason: "resume_completion_unconfirmed".to_string(),
            },
        },
        fresh: Some(InvocationOutcome {
            invocation_id: "fresh-invocation".to_string(),
            session_id: Some("fresh-session".to_string()),
            physical_exit_code: 0,
            acceptance: ResumeAcceptance::NotApplicable,
            disposition: InvocationDisposition::Succeeded,
        }),
    }
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
