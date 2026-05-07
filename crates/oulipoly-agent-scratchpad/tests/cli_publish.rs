mod common;

use serde_json::Value;
use uuid::Uuid;

// proposal § Test-Intent Track rows 7, 13
// contract § Expected observable signals rows publish-source, publish-producer
// named risk: Scratchpad CLI HIGH - publish JSON could omit lineage or mutate the private source
// selected level: cli_integration
#[test]
fn publish_json_copies_to_canonical_preserves_source_and_reports_lineage() {
    let (db, store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    let content = common::binary_bytes();
    common::put_scratchpad_row(&store, invocation, "draft.bin", content.clone());

    let output = common::run_agent_scratchpad(&[
        "publish",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "draft.bin",
        "--workflow-run-id",
        "canonical-run",
        "--artifact-name",
        "artifact.bin",
        "--json",
    ]);

    let json = common::stdout_json(&output);
    assert_eq!(json.get("source_version").and_then(Value::as_u64), Some(1));
    assert_eq!(
        json.get("destination_version").and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        json.get("producer_invocation_uuid").and_then(Value::as_str),
        Some(invocation.to_string().as_str())
    );
    assert_eq!(
        common::expect_json_string(&json, "destination_sha256"),
        common::sha256_hex(&content)
    );

    let source = store
        .get(
            &common::store_key(&common::scratchpad_workflow(invocation), "draft.bin"),
            None,
        )
        .expect("source preserved");
    let canonical = store
        .get(&common::store_key("canonical-run", "artifact.bin"), None)
        .expect("canonical written");
    assert_eq!(source.content, content);
    assert_eq!(canonical.content, source.content);
    assert_eq!(canonical.meta.producer_invocation_uuid, Some(invocation));
}

// proposal § Test-Intent Track row 8
// named risk: Scratchpad CLI HIGH - publish could read source from another invocation's same artifact name
// selected level: cli_integration
#[test]
fn publish_missing_or_cross_scope_source_exits_65() {
    let (db, store) = common::init_temp_store();
    let owner = Uuid::new_v4();
    let stranger = Uuid::new_v4();
    common::put_scratchpad_row(&store, owner, "draft.md", b"owner".to_vec());

    let output = common::run_agent_scratchpad(&[
        "publish",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &stranger.to_string(),
        "--name",
        "draft.md",
        "--workflow-run-id",
        "canonical-run",
        "--artifact-name",
        "artifact.md",
    ]);

    common::assert_exit_code(&output, 65);
    assert!(output.stdout.is_empty());
}

// proposal § Test-Intent Track row 7
// named risk: Scratchpad Domain Layer HIGH - publish could allow private scratchpad-prefixed canonical destinations
// selected level: cli_integration
#[test]
fn publish_rejects_reserved_destination_prefix_as_misuse() {
    let (db, store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    common::put_scratchpad_row(&store, invocation, "draft.md", b"draft".to_vec());

    let output = common::run_agent_scratchpad(&[
        "publish",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "draft.md",
        "--workflow-run-id",
        "scratchpad:canonical-run",
        "--artifact-name",
        "artifact.md",
    ]);

    common::assert_exit_code(&output, 64);
    assert!(output.stdout.is_empty());
    assert!(common::stderr_text(&output).contains("scratchpad:"));
}

// proposal § Test-Intent Track row 7
// named risk: Agent-store Substrate Consumption HIGH - canonical predecessor/version metadata could be assigned to the wrong row
// selected level: cli_integration
#[test]
fn publish_json_accepts_canonical_metadata_overrides_and_predecessor_version() {
    let (db, store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    common::put_canonical_row(&store, "canonical-run", "artifact.md");
    common::put_scratchpad_row(&store, invocation, "draft.md", b"draft".to_vec());

    let output = common::run_agent_scratchpad(&[
        "publish",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "draft.md",
        "--workflow-run-id",
        "canonical-run",
        "--artifact-name",
        "artifact.md",
        "--format",
        "text/markdown",
        "--verdict-line",
        "APPROVED",
        "--predecessor-version",
        "1",
        "--json",
    ]);

    let json = common::stdout_json(&output);
    assert_eq!(
        json.get("destination_version").and_then(Value::as_u64),
        Some(2)
    );
    assert_eq!(
        json.get("predecessor_version").and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        json.get("format_hint").and_then(Value::as_str),
        Some("text/markdown")
    );
    assert_eq!(
        json.get("verdict_line").and_then(Value::as_str),
        Some("APPROVED")
    );
}
