mod common;

// proposal-test-intent-row 18: CLI tombstone JSON receipt
// assumption-register: A6, A9, A10
// named risk: CLI tombstone output could omit retry status or tombstone fields needed by non-Rust consumers
// selected level: component
#[test]
fn tombstone_json_emits_status_and_audit_fields() {
    let db = common::TempDb::new();
    let db_path = db.path().to_str().expect("utf8 db path");
    let fixture = common::fixture_path("markdown-a.md");
    let fixture_path = fixture.to_str().expect("utf8 fixture path");

    common::assert_success(&common::run_agent_store(&[
        "init", "--db", db_path, "--json",
    ]));
    common::assert_success(&common::run_agent_store(&[
        "put",
        "--db",
        db_path,
        "--workflow-run-id",
        "R-cli-tomb",
        "--artifact-name",
        "artifact.md",
        "--content-file",
        fixture_path,
        "--json",
    ]));

    let first_output = common::run_agent_store(&[
        "tombstone",
        "--db",
        db_path,
        "--workflow-run-id",
        "R-cli-tomb",
        "--artifact-name",
        "artifact.md",
        "--version",
        "1",
        "--actor",
        "operator",
        "--reason",
        "cleanup",
        "--json",
    ]);
    common::assert_success(&first_output);
    let first = common::stdout_json(&first_output);
    let second_output = common::run_agent_store(&[
        "tombstone",
        "--db",
        db_path,
        "--workflow-run-id",
        "R-cli-tomb",
        "--artifact-name",
        "artifact.md",
        "--version",
        "1",
        "--actor",
        "retry-operator",
        "--reason",
        "retry",
        "--json",
    ]);
    common::assert_success(&second_output);
    let second = common::stdout_json(&second_output);

    assert_eq!(first["workflow_run_id"].as_str(), Some("R-cli-tomb"));
    assert_eq!(first["artifact_name"].as_str(), Some("artifact.md"));
    assert_eq!(first["version"].as_u64(), Some(1));
    assert_eq!(first["actor"].as_str(), Some("operator"));
    assert_eq!(first["reason"].as_str(), Some("cleanup"));
    assert_eq!(first["status"].as_str(), Some("tombstoned"));
    assert!(
        first["tombstoned_at"]
            .as_str()
            .is_some_and(common::is_rfc3339)
    );
    assert_eq!(second["status"].as_str(), Some("already_tombstoned"));
    assert_eq!(second["actor"], first["actor"]);
    assert_eq!(second["reason"], first["reason"]);
    assert_eq!(second["tombstoned_at"], first["tombstoned_at"]);
}
