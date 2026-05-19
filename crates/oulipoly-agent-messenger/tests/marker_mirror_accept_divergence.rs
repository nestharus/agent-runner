//! AGE-160 risk: PP-004 + A5 helper marker mirror accept-divergence.
//! Selected level: integration.
//! Source: the AGE-160 proposal § Test-intent track.

mod common;

use uuid::Uuid;

/// AGE-160 risk: PP-004 + A5 helper id-only canonical JSON subset.
/// Selected level: integration.
/// Source: the AGE-160 proposal § Test-intent track; validates A5.
#[test]
fn age160_helper_id_only_extraction_does_not_break_with_declared_grammar() {
    let (db, _store) = common::init_temp_store();
    let invocation_uuid = Uuid::parse_str("7ad2916c-38dd-49e6-a1f7-3ef22766ff70").unwrap();
    let channel = db.output_path("age160-return-channel.jsonl");
    let parent_env = r#"{"source":"codex2","id":"7ad2916c-38dd-49e6-a1f7-3ef22766ff70"}"#;

    let output = common::run_agent_messenger_with_env(
        &[
            "return",
            "--db",
            &db.path_arg(),
            "--name",
            "age160.md",
            "--body",
            "age160 messenger",
            "--return-channel",
            channel.to_str().expect("utf8 channel"),
            "--json",
        ],
        &[("OULIPOLY_PARENT_INVOCATION", parent_env)],
    );

    let json = common::stdout_json(&output);
    assert_eq!(
        json.get("producer_invocation_uuid")
            .and_then(serde_json::Value::as_str),
        Some(invocation_uuid.to_string().as_str())
    );
}

/// AGE-160 risk: PP-004 + A5 helper legacy divergence rejection.
/// Selected level: integration.
/// Source: the AGE-160 proposal § Test-intent track; validates A5.
#[test]
fn age160_helper_rejects_legacy_shell_mangled_parent_payload_by_declared_divergence() {
    let (db, _store) = common::init_temp_store();

    let output = common::run_agent_messenger_with_env(
        &["list-returned", "--db", &db.path_arg()],
        &[(
            "OULIPOLY_PARENT_INVOCATION",
            "{source:'codex2',id:'7ad2916c-38dd-49e6-a1f7-3ef22766ff70'}",
        )],
    );

    common::assert_exit_code(&output, 64);
    assert!(
        common::stderr_text(&output).contains("not valid JSON"),
        "messenger helper accepts canonical JSON subset only"
    );
}
