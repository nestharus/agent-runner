mod common;

use serde_json::Value;
use uuid::Uuid;

// proposal § Test-Intent Track rows 5, 13
// contract § Expected observable signals row delete-version-idempotent
// named risk: Scratchpad CLI HIGH - delete retries could be non-idempotent or lose audit vectors
// selected level: cli_integration
#[test]
fn delete_version_json_is_idempotent() {
    let (db, store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    let receipt = common::put_scratchpad_row(&store, invocation, "retry.md", b"retry".to_vec());

    let args = [
        "delete",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "retry.md",
        "--version",
        &receipt.version.to_string(),
        "--json",
    ];
    let first = common::stdout_json(&common::run_agent_scratchpad(&args));
    let second = common::stdout_json(&common::run_agent_scratchpad(&args));

    assert_eq!(
        first
            .get("tombstoned_versions")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        second
            .get("tombstoned_versions")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );
    assert_eq!(
        second
            .get("already_tombstoned_versions")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
}

// proposal § Test-Intent Track row 5
// named risk: Scratchpad CLI HIGH - --all-versions could leave active versions readable
// selected level: cli_integration
#[test]
fn delete_all_versions_json_hides_every_active_private_version() {
    let (db, store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    common::put_scratchpad_row(&store, invocation, "history.md", b"v1".to_vec());
    common::put_scratchpad_row(&store, invocation, "history.md", b"v2".to_vec());

    let output = common::run_agent_scratchpad(&[
        "delete",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "history.md",
        "--all-versions",
        "--actor",
        "test-actor",
        "--reason",
        "test cleanup",
        "--json",
    ]);
    let json = common::stdout_json(&output);
    assert_eq!(
        json.get("actor").and_then(Value::as_str),
        Some("test-actor")
    );
    assert_eq!(
        json.get("reason").and_then(Value::as_str),
        Some("test cleanup")
    );
    assert_eq!(
        json.get("tombstoned_versions")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(2)
    );

    let read = common::run_agent_scratchpad(&[
        "read",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "history.md",
    ]);
    common::assert_exit_code(&read, 65);
}

// proposal § Test-Intent Track row 4
// named risk: Scratchpad CLI HIGH - delete by name could tombstone another invocation's artifact
// selected level: cli_integration
#[test]
fn delete_cross_scope_name_exits_65_and_preserves_owner_row() {
    let (db, store) = common::init_temp_store();
    let owner = Uuid::new_v4();
    let stranger = Uuid::new_v4();
    common::put_scratchpad_row(&store, owner, "draft.md", b"owner".to_vec());

    let output = common::run_agent_scratchpad(&[
        "delete",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &stranger.to_string(),
        "--name",
        "draft.md",
    ]);

    common::assert_exit_code(&output, 65);
    let owner_record = store
        .get(
            &common::store_key(&common::scratchpad_workflow(owner), "draft.md"),
            None,
        )
        .expect("owner row remains");
    assert_eq!(owner_record.content, b"owner".to_vec());
}
