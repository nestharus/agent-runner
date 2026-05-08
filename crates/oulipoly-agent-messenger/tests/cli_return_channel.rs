mod common;

use uuid::Uuid;

// proposal § Assumption Register A8
// contract § Return-channel resolution
// named risk: Messenger Receive Transport HIGH - explicit helper channel could be ignored in favor of inherited stale env
// selected level: cli_integration
#[test]
fn explicit_return_channel_wins_over_env_channel() {
    let (db, _store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    let explicit = db.output_path("explicit.jsonl");
    let inherited = db.output_path("inherited.jsonl");

    let output = common::run_agent_messenger_with_env(
        &[
            "return",
            "--db",
            &db.path_arg(),
            "--invocation-uuid",
            &invocation.to_string(),
            "--name",
            "result.md",
            "--body",
            "hello",
            "--return-channel",
            explicit.to_str().expect("utf8 explicit"),
        ],
        &[(
            "OULIPOLY_RETURN_CHANNEL",
            inherited.to_str().expect("utf8 inherited"),
        )],
    );

    common::assert_success(&output);
    assert!(explicit.exists(), "explicit channel should be written");
    assert!(
        !inherited.exists(),
        "inherited channel should not be written"
    );
}

// proposal § Assumption Register A8
// contract § Expected observable signals row missing return channel
// named risk: Messenger Receive Transport HIGH - dispatched return without a channel could create unbound orphan returns
// selected level: cli_integration
#[test]
fn missing_return_channel_exits_64_and_names_channel() {
    let (db, _store) = common::init_temp_store();
    let invocation = Uuid::new_v4();

    let output = common::run_agent_messenger(&[
        "return",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "result.md",
        "--body",
        "hello",
    ]);

    common::assert_exit_code(&output, 64);
    assert!(common::stderr_text(&output).contains("OULIPOLY_RETURN_CHANNEL"));
}
