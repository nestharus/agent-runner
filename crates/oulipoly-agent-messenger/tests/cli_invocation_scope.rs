mod common;

use uuid::Uuid;

// proposal § Assumption Register A1
// contract § Invocation-UUID resolution
// named risk: Messenger CLI HIGH - env-derived scope could override explicit caller scope
// selected level: cli_integration
#[test]
fn explicit_invocation_uuid_wins_over_parent_env() {
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
            "scope.md",
            "--body",
            "explicit",
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
    let expected_uuid = explicit.to_string();
    assert_eq!(
        json.get("producer_invocation_uuid")
            .and_then(serde_json::Value::as_str),
        Some(expected_uuid.as_str())
    );
}

// proposal § Assumption Register A1
// contract § Expected observable signals rows missing/malformed scope
// named risk: Messenger CLI HIGH - missing or malformed scope could fall back to root behavior
// selected level: cli_integration
#[test]
fn missing_and_malformed_scope_exit_64() {
    let (db, _store) = common::init_temp_store();
    common::assert_exit_code(
        &common::run_agent_messenger(&["list-returned", "--db", &db.path_arg()]),
        64,
    );
    common::assert_exit_code(
        &common::run_agent_messenger(&[
            "list-returned",
            "--db",
            &db.path_arg(),
            "--invocation-uuid",
            "not-a-uuid",
        ]),
        64,
    );
    common::assert_exit_code(
        &common::run_agent_messenger_with_env(
            &["list-returned", "--db", &db.path_arg()],
            &[("OULIPOLY_PARENT_INVOCATION", "not-json")],
        ),
        64,
    );
}
