mod common;

// proposal-test-intent-row 16: CLI init JSON idempotency
// assumption-register: A4, A9
// named risk: CLI initialization could be non-idempotent or emit an undocumented envelope that future scripts cannot parse
// selected level: component
#[test]
fn init_json_emits_documented_shape_and_is_idempotent() {
    let db = common::TempDb::new();
    let db_path = db.path().to_str().expect("utf8 db path");

    let first = common::run_agent_store(&["init", "--db", db_path, "--json"]);
    let second = common::run_agent_store(&["init", "--db", db_path, "--json"]);
    common::assert_success(&first);
    common::assert_success(&second);
    let first_json = common::stdout_json(&first);
    let second_json = common::stdout_json(&second);

    assert_eq!(first_json["db_path"].as_str(), Some(db_path));
    assert_eq!(first_json["schema_version"].as_u64(), Some(1));
    assert_eq!(first_json["status"].as_str(), Some("initialized"));
    assert_eq!(second_json["db_path"].as_str(), Some(db_path));
    assert_eq!(second_json["schema_version"].as_u64(), Some(1));
    assert_eq!(second_json["status"].as_str(), Some("already_current"));
}
