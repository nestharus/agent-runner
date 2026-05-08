mod common;

use uuid::Uuid;

// proposal § Assumption Register A1
// contract § Invocation-UUID resolution
// named risk: Messenger Scope HIGH - explicit invocation UUID could be overridden by inherited parent env
// selected level: library_integration
#[test]
fn explicit_invocation_uuid_takes_precedence_over_parent_env_for_return() {
    let (db, _store) = common::init_temp_store();
    let explicit = Uuid::new_v4();
    let inherited = Uuid::new_v4();
    let channel = db.output_path("returns.jsonl");

    let output = common::run_agent_messenger_with_env(
        &[
            "return",
            "--db",
            &db.path_arg(),
            "--invocation-uuid",
            &explicit.to_string(),
            "--name",
            "result.md",
            "--body",
            "hello",
            "--return-channel",
            channel.to_str().expect("utf8 channel"),
            "--json",
        ],
        &[(
            "OULIPOLY_PARENT_INVOCATION",
            &common::parent_invocation_env(inherited),
        )],
    );

    let json = common::stdout_json(&output);
    assert_eq!(
        json.get("producer_invocation_uuid")
            .and_then(serde_json::Value::as_str),
        Some(explicit.to_string().as_str())
    );
}

// proposal § Assumption Register A1
// contract § Expected observable signals rows missing scope/malformed UUID
// named risk: Messenger Scope HIGH - malformed or missing parent scope could silently create root-scoped returns
// selected level: library_integration
#[test]
fn missing_or_malformed_invocation_scope_fails_closed() {
    let (db, _store) = common::init_temp_store();

    let missing = common::run_agent_messenger(&["list-returned", "--db", &db.path_arg(), "--json"]);
    common::assert_exit_code(&missing, 64);
    let stderr = common::stderr_text(&missing);
    let stderr_lower = stderr.to_ascii_lowercase();
    assert!(
        stderr_lower.contains("missing") || stderr_lower.contains("required"),
        "stderr should indicate missing invocation scope: {stderr}"
    );
    assert!(
        stderr.contains("OULIPOLY_PARENT_INVOCATION") || stderr.contains("--invocation-uuid"),
        "stderr should mention the scope source: {stderr}"
    );

    let malformed = common::run_agent_messenger_with_env(
        &["list-returned", "--db", &db.path_arg(), "--json"],
        &[("OULIPOLY_PARENT_INVOCATION", r#"{"id":"not-a-uuid"}"#)],
    );
    common::assert_exit_code(&malformed, 64);
}
