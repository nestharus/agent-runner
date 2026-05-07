mod common;

use uuid::Uuid;

// proposal § Test-Intent Track row 14 and Assumption Register A6
// contract § Expected observable signals rows missing-scope, malformed-uuid, tombstoned-read, uninitialized-db, missing-content-file
// named risk: Scratchpad CLI HIGH - shell-facing failures could map to unstable process statuses
// selected level: cli_integration
#[test]
fn exit_codes_cover_success_misuse_not_found_db_and_io_cases() {
    let (db, store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    let uninitialized = db.output_path("uninitialized.sqlite");
    let missing_content = db.output_path("missing.md");
    let receipt = common::put_scratchpad_row(&store, invocation, "hidden.md", b"hidden".to_vec());
    store
        .tombstone(&receipt.key, receipt.version, "tester", "hide")
        .expect("tombstone");

    common::assert_exit_code(
        &common::run_agent_scratchpad(&["scope", "--invocation-uuid", &invocation.to_string()]),
        0,
    );
    common::assert_exit_code(
        &common::run_agent_scratchpad(&["list", "--db", &db.path_arg()]),
        64,
    );
    common::assert_exit_code(
        &common::run_agent_scratchpad(&[
            "list",
            "--db",
            &db.path_arg(),
            "--invocation-uuid",
            "not-a-uuid",
        ]),
        64,
    );
    common::assert_exit_code(
        &common::run_agent_scratchpad(&[
            "read",
            "--db",
            &db.path_arg(),
            "--invocation-uuid",
            &invocation.to_string(),
            "--name",
            "hidden.md",
            "--version",
            &receipt.version.to_string(),
        ]),
        65,
    );
    common::assert_exit_code(
        &common::run_agent_scratchpad(&[
            "list",
            "--db",
            uninitialized.to_str().expect("utf8 uninitialized path"),
            "--invocation-uuid",
            &invocation.to_string(),
        ]),
        73,
    );
    common::assert_exit_code(
        &common::run_agent_scratchpad(&[
            "write",
            "--db",
            &db.path_arg(),
            "--invocation-uuid",
            &invocation.to_string(),
            "--name",
            "missing-source.md",
            "--content-file",
            missing_content.to_str().expect("utf8 missing content path"),
        ]),
        74,
    );
}

// proposal § Test-Intent Track row 14
// named risk: Scratchpad CLI HIGH - rarely reachable collision/serialization statuses could disappear from the public contract
// selected level: documentation
#[test]
fn readme_documents_collision_66_and_serialization_70_exit_rows() {
    let readme =
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"))
            .expect("README.md exists");

    for exit_code_row in ["| 66 |", "| 70 |"] {
        assert!(
            readme.contains(exit_code_row),
            "README missing exit code row {exit_code_row}"
        );
    }
}
