mod common;

use uuid::Uuid;

// proposal § Test-Intent Track row: list-returned CLI
// contract § Expected observable signals row list-returned invocation isolation
// named risk: Messenger CLI HIGH - list-returned could expose another invocation's returned artifacts
// selected level: cli_integration
#[test]
fn list_returned_json_filters_by_invocation_and_name() {
    let (db, _store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    let other_invocation = Uuid::new_v4();
    let channel = db.output_path("returns.jsonl");

    common::assert_success(&common::run_agent_messenger(&[
        "return",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "proposal.md",
        "--body",
        "one",
        "--return-channel",
        channel.to_str().expect("utf8 channel"),
    ]));
    common::assert_success(&common::run_agent_messenger(&[
        "return",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &other_invocation.to_string(),
        "--name",
        "proposal.md",
        "--body",
        "other invocation",
        "--return-channel",
        channel.to_str().expect("utf8 channel"),
    ]));
    common::assert_success(&common::run_agent_messenger(&[
        "return",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "notes.md",
        "--body",
        "other name",
        "--return-channel",
        channel.to_str().expect("utf8 channel"),
    ]));

    let output = common::run_agent_messenger(&[
        "list-returned",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "proposal.md",
        "--json",
    ]);
    let json = common::stdout_json(&output);
    let rows = json.as_array().expect("list array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["producer_invocation_uuid"], invocation.to_string());
    assert_eq!(rows[0]["name"], "proposal.md");
}
