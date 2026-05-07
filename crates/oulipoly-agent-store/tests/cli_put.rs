mod common;

// proposal-test-intent-row 17: CLI put JSON receipt
// assumption-register: A9
// named risk: CLI mutation success output could drift from README JSON names or mix diagnostics into the parseable channel
// selected level: component
#[test]
fn put_json_emits_documented_receipt_shape_for_markdown_fixture() {
    let db = common::TempDb::new();
    let db_path = db.path().to_str().expect("utf8 db path");
    let fixture = common::fixture_path("markdown-a.md");
    let fixture_path = fixture.to_str().expect("utf8 fixture path");
    let fixture_bytes = common::fixture_bytes("markdown-a.md");

    common::assert_success(&common::run_agent_store(&[
        "init", "--db", db_path, "--json",
    ]));
    let output = common::run_agent_store(&[
        "put",
        "--db",
        db_path,
        "--workflow-run-id",
        "R-cli-put",
        "--artifact-name",
        "markdown-a.md",
        "--producer-uuid",
        "550e8400-e29b-41d4-a716-446655440000",
        "--format",
        "text/markdown",
        "--verdict-line",
        "APPROVED: markdown fixture",
        "--content-file",
        fixture_path,
        "--json",
    ]);
    common::assert_success(&output);
    let json = common::stdout_json(&output);

    assert_eq!(json["workflow_run_id"].as_str(), Some("R-cli-put"));
    assert_eq!(json["artifact_name"].as_str(), Some("markdown-a.md"));
    assert_eq!(json["version"].as_u64(), Some(1));
    assert_eq!(
        json["producer_invocation_uuid"].as_str(),
        Some("550e8400-e29b-41d4-a716-446655440000")
    );
    assert_eq!(
        json["sha256"].as_str(),
        Some(common::sha256_hex(&fixture_bytes).as_str())
    );
    assert_eq!(
        json["content_len"].as_u64(),
        Some(fixture_bytes.len() as u64)
    );
    assert_eq!(json["format_hint"].as_str(), Some("text/markdown"));
    assert_eq!(
        json["verdict_line"].as_str(),
        Some("APPROVED: markdown fixture")
    );
    assert!(
        json.as_object()
            .expect("receipt is object")
            .contains_key("predecessor_version"),
        "receipt must include predecessor_version field"
    );
    assert!(json["predecessor_version"].is_null());
    assert!(json["created_at"].as_str().is_some_and(common::is_rfc3339));
}
