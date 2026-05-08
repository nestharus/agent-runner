mod common;

use uuid::Uuid;

// proposal § Test-Intent Track row: exhaustive CLI exit-code matrix
// contract § Exit-code map
// named risk: Messenger CLI HIGH - downstream scripts could lose branchable failure semantics
// selected level: cli_integration
#[test]
fn exit_codes_cover_success_misuse_not_found_db_io_and_channel_cases() {
    let db = common::TempDb::new();
    let initialized = {
        let (db, _store) = common::init_temp_store();
        db
    };
    let invocation = Uuid::new_v4();
    let channel = initialized.output_path("returns.jsonl");
    let missing_file = initialized.output_path("missing.bin");
    let uninitialized = db.output_path("uninitialized.sqlite");

    common::assert_exit_code(&common::run_agent_messenger(&["version"]), 0);
    common::assert_exit_code(&common::run_agent_messenger(&["return", "--db"]), 64);
    common::assert_exit_code(
        &common::run_agent_messenger(&[
            "show",
            "--db",
            &initialized.path_arg(),
            "--version-id",
            &format!("store://return/{invocation}/missing/1"),
        ]),
        65,
    );
    common::assert_exit_code(
        &common::run_agent_messenger(&[
            "list-returned",
            "--db",
            uninitialized.to_str().expect("utf8 db"),
            "--invocation-uuid",
            &invocation.to_string(),
        ]),
        73,
    );
    common::assert_exit_code(
        &common::run_agent_messenger(&[
            "return",
            "--db",
            &initialized.path_arg(),
            "--invocation-uuid",
            &invocation.to_string(),
            "--name",
            "missing.bin",
            "--content-file",
            missing_file.to_str().expect("utf8 missing"),
            "--return-channel",
            channel.to_str().expect("utf8 channel"),
        ]),
        74,
    );
}
