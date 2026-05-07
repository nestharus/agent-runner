mod common;

use serde_json::Value;
use uuid::Uuid;

// proposal § Assumption Register A1
// named risk: Scratchpad CLI HIGH - env-derived invocation scope could override explicit caller scope
// selected level: cli_integration
#[test]
fn explicit_invocation_uuid_takes_precedence_over_parent_env() {
    let (db, store) = common::init_temp_store();
    let explicit = Uuid::new_v4();
    let env_scope = Uuid::new_v4();
    let content_path = db.output_path("input.md");
    common::write_file(&content_path, b"explicit");

    let output = common::run_agent_scratchpad_with_env(
        &[
            "write",
            "--db",
            &db.path_arg(),
            "--invocation-uuid",
            &explicit.to_string(),
            "--name",
            "scope.md",
            "--content-file",
            content_path.to_str().expect("utf8 input path"),
            "--json",
        ],
        &[(
            "OULIPOLY_PARENT_INVOCATION",
            &common::parent_invocation_env(env_scope),
        )],
    );

    let json = common::stdout_json(&output);
    assert_eq!(
        json.get("producer_invocation_uuid").and_then(Value::as_str),
        Some(explicit.to_string().as_str())
    );
    store
        .get(
            &common::store_key(&common::scratchpad_workflow(explicit), "scope.md"),
            None,
        )
        .expect("explicit scope row");
    assert!(
        store
            .get(
                &common::store_key(&common::scratchpad_workflow(env_scope), "scope.md"),
                None,
            )
            .is_err(),
        "env scope must not receive the explicit write"
    );
}

// proposal § Assumption Register A1
// named risk: Scratchpad CLI HIGH - missing explicit scope could fail to parse inherited parent invocation JSON
// selected level: cli_integration
#[test]
fn missing_invocation_uuid_uses_parent_env_id_field() {
    let (db, store) = common::init_temp_store();
    let env_scope = Uuid::new_v4();
    let content_path = db.output_path("input.md");
    common::write_file(&content_path, b"env");

    let output = common::run_agent_scratchpad_with_env(
        &[
            "write",
            "--db",
            &db.path_arg(),
            "--name",
            "scope.md",
            "--content-file",
            content_path.to_str().expect("utf8 input path"),
            "--json",
        ],
        &[(
            "OULIPOLY_PARENT_INVOCATION",
            &common::parent_invocation_env(env_scope),
        )],
    );

    common::assert_success(&output);
    let stored = store
        .get(
            &common::store_key(&common::scratchpad_workflow(env_scope), "scope.md"),
            None,
        )
        .expect("env scope row");
    assert_eq!(stored.content, b"env".to_vec());
}

// proposal § Assumption Register A1
// contract § Expected observable signals row missing-invocation-scope
// named risk: Scratchpad CLI HIGH - missing scope could fall back to unscoped behavior
// selected level: cli_integration
#[test]
fn missing_invocation_scope_exits_64_and_names_scope_sources() {
    let (db, _store) = common::init_temp_store();

    let output = common::run_agent_scratchpad_with_env(
        &["list", "--db", &db.path_arg()],
        &[("OULIPOLY_PARENT_INVOCATION", "")],
    );

    common::assert_exit_code(&output, 64);
    let stderr = common::stderr_text(&output);
    assert!(stderr.contains("OULIPOLY_PARENT_INVOCATION") || stderr.contains("--invocation-uuid"));
}

// proposal § Assumption Register A1
// contract § Expected observable signals row malformed-uuid
// named risk: Scratchpad CLI HIGH - malformed UUIDs could be treated as artifact names or other scope
// selected level: cli_integration
#[test]
fn malformed_explicit_uuid_exits_64_and_names_invalid_argument() {
    let (db, _store) = common::init_temp_store();

    let output = common::run_agent_scratchpad(&[
        "list",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        "not-a-uuid",
    ]);

    common::assert_exit_code(&output, 64);
    assert!(common::stderr_text(&output).contains("not-a-uuid"));
}

// proposal § CLI subcommand contract
// named risk: Scratchpad Documentation HIGH - scope diagnostics could diverge from invocation UUID resolution rules
// selected level: cli_integration
#[test]
fn scope_command_validates_and_prints_resolved_scope() {
    let invocation = Uuid::new_v4();
    let output = common::run_agent_scratchpad(&[
        "scope",
        "--invocation-uuid",
        &invocation.to_string(),
        "--json",
    ]);

    let json = common::stdout_json(&output);
    assert_eq!(
        json.get("invocation_uuid").and_then(Value::as_str),
        Some(invocation.to_string().as_str())
    );
}
