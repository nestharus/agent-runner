mod common;

use uuid::Uuid;

// proposal § Test-Intent Track row: show raw-byte stdout and --out
// contract § Expected observable signals row show --version-id raw bytes
// named risk: Messenger CLI HIGH - show could corrupt binary bytes by printing JSON or trailing newlines
// selected level: cli_integration
#[test]
fn show_writes_raw_bytes_to_stdout_and_out_file_without_json() {
    let (db, _store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    let channel = db.output_path("returns.jsonl");
    let content = db.output_path("blob.bin");
    common::write_file(&content, &common::binary_bytes());

    let returned = common::stdout_json(&common::run_agent_messenger(&[
        "return",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "blob.bin",
        "--content-file",
        content.to_str().expect("utf8 content"),
        "--return-channel",
        channel.to_str().expect("utf8 channel"),
        "--json",
    ]));
    let version_id = returned
        .get("version_id")
        .and_then(serde_json::Value::as_str)
        .expect("version_id");

    let stdout_output =
        common::run_agent_messenger(&["show", "--db", &db.path_arg(), "--version-id", version_id]);
    common::assert_success(&stdout_output);
    assert_eq!(stdout_output.stdout, common::binary_bytes());

    let out_path = db.output_path("out.bin");
    let out_output = common::run_agent_messenger(&[
        "show",
        "--db",
        &db.path_arg(),
        "--version-id",
        version_id,
        "--out",
        out_path.to_str().expect("utf8 out"),
    ]);
    common::assert_success(&out_output);
    assert!(out_output.stdout.is_empty());
    assert_eq!(
        std::fs::read(out_path).expect("out bytes"),
        common::binary_bytes()
    );
}

// proposal § Test-Intent Track row: show missing exit 65
// contract § Expected observable signals row show tombstoned/missing
// named risk: Messenger CLI HIGH - missing returned artifacts could produce success with empty stdout
// selected level: cli_integration
#[test]
fn show_missing_version_exits_65_and_keeps_stdout_empty() {
    let (db, _store) = common::init_temp_store();
    let missing = format!("store://return/{}/missing.md/1", Uuid::new_v4());

    let output =
        common::run_agent_messenger(&["show", "--db", &db.path_arg(), "--version-id", &missing]);

    common::assert_exit_code(&output, 65);
    assert!(output.stdout.is_empty());
}
