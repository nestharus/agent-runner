mod common;

// proposal § CLI subcommand contract row version
// named risk: Messenger CLI HIGH - version output could require DB/scope or omit schema/version facts needed by scripts
// selected level: cli_integration
#[test]
fn version_json_is_scope_free_and_names_package_and_schema_version() {
    let output = common::run_agent_messenger(&["version", "--json"]);
    let json = common::stdout_json(&output);

    assert_eq!(
        json.get("package")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default(),
        "oulipoly-agent-messenger"
    );
    assert_eq!(
        json.get("receipt_schema_version")
            .and_then(serde_json::Value::as_u64),
        Some(1)
    );
}
