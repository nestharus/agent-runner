mod common;

use serde_json::Value;
use uuid::Uuid;

// proposal § Test-Intent Track rows 6, 13
// named risk: Scratchpad CLI HIGH - list JSON could be unscoped, unordered, or omit metadata fields
// selected level: cli_integration
#[test]
fn list_json_returns_only_invocation_scope_in_name_version_order() {
    let (db, store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    let other = Uuid::new_v4();
    common::put_scratchpad_row(&store, invocation, "b.md", b"b".to_vec());
    common::put_scratchpad_row(&store, invocation, "a.md", b"a1".to_vec());
    common::put_scratchpad_row(&store, invocation, "a.md", b"a2".to_vec());
    common::put_scratchpad_row(&store, other, "a.md", b"other".to_vec());

    let output = common::run_agent_scratchpad(&[
        "list",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--json",
    ]);

    let json = common::stdout_json(&output);
    let rows = json.as_array().expect("list JSON is an array");
    let projection: Vec<_> = rows
        .iter()
        .map(|row| {
            (
                row.get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                row.get("version")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                row.get("invocation_uuid")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    assert_eq!(
        projection,
        vec![
            ("a.md".to_string(), 1, invocation.to_string()),
            ("a.md".to_string(), 2, invocation.to_string()),
            ("b.md".to_string(), 1, invocation.to_string()),
        ]
    );
}

// proposal § Test-Intent Track rows 6, 13
// named risk: Scratchpad CLI HIGH - include-tombstoned could leak tombstones from another scope or hide audit metadata
// selected level: cli_integration
#[test]
fn list_json_include_tombstoned_exposes_tombstone_only_for_caller_scope() {
    let (db, store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    let other = Uuid::new_v4();
    let receipt = common::put_scratchpad_row(&store, invocation, "draft.md", b"draft".to_vec());
    store
        .tombstone(&receipt.key, receipt.version, "tester", "done")
        .expect("tombstone");
    common::put_scratchpad_row(&store, other, "draft.md", b"other".to_vec());

    let output = common::run_agent_scratchpad(&[
        "list",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "draft.md",
        "--include-tombstoned",
        "--json",
    ]);

    let json = common::stdout_json(&output);
    let rows = json.as_array().expect("list JSON is an array");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("invocation_uuid").and_then(Value::as_str),
        Some(invocation.to_string().as_str())
    );
    assert!(
        rows[0]
            .get("tombstone")
            .and_then(Value::as_object)
            .is_some(),
        "tombstone field must be a non-null object"
    );
}
