mod common;

// proposal-test-intent-row 21: CLI get-meta JSON without content
// assumption-register: A9
// named risk: metadata reads could accidentally include content bytes or omit required metadata for binary artifacts
// selected level: component
#[test]
fn get_meta_json_returns_metadata_shape_without_content_payload() {
    let db = common::TempDb::new();
    let db_path = db.path().to_str().expect("utf8 db path");
    let fixture = common::fixture_path("blob.bin");
    let fixture_path = fixture.to_str().expect("utf8 fixture path");

    common::assert_success(&common::run_agent_store(&[
        "init", "--db", db_path, "--json",
    ]));
    common::assert_success(&common::run_agent_store(&[
        "put",
        "--db",
        db_path,
        "--workflow-run-id",
        "R-cli-meta",
        "--artifact-name",
        "blob.bin",
        "--content-file",
        fixture_path,
        "--json",
    ]));

    let output = common::run_agent_store(&[
        "get-meta",
        "--db",
        db_path,
        "--workflow-run-id",
        "R-cli-meta",
        "--artifact-name",
        "blob.bin",
        "--json",
    ]);
    common::assert_success(&output);
    let json = common::stdout_json(&output);

    common::expect_meta_json_shape(&json);
    assert_eq!(json["workflow_run_id"].as_str(), Some("R-cli-meta"));
    assert_eq!(json["artifact_name"].as_str(), Some("blob.bin"));
    assert_eq!(
        json["content_len"].as_u64(),
        Some(common::fixture_bytes("blob.bin").len() as u64)
    );
    assert!(json["tombstone"].is_null());
}
