mod common;

// proposal-test-intent-row 22: CLI exit-code mapping
// assumption-register: A9
// named risk: CLI failures could map to generic or unstable process statuses that downstream scripts cannot branch on
// selected level: component
#[test]
fn exit_codes_cover_success_misuse_not_found_io_and_db_migration_cases() {
    let db = common::TempDb::new();
    let db_path = db.path().to_str().expect("utf8 db path");
    let uninitialized = db.output_path("uninitialized.sqlite");
    let uninitialized_path = uninitialized.to_str().expect("utf8 uninitialized path");
    let missing_content = db.output_path("missing.md");
    let missing_content_path = missing_content.to_str().expect("utf8 missing content path");

    common::assert_exit_code(&common::run_agent_store(&["init", "--json"]), 64);
    common::assert_exit_code(
        &common::run_agent_store(&["init", "--db", db_path, "--json"]),
        0,
    );
    common::assert_exit_code(
        &common::run_agent_store(&[
            "get-meta",
            "--db",
            db_path,
            "--workflow-run-id",
            "missing-run",
            "--artifact-name",
            "missing.md",
            "--json",
        ]),
        65,
    );
    common::assert_exit_code(
        &common::run_agent_store(&[
            "put",
            "--db",
            db_path,
            "--workflow-run-id",
            "R-exit",
            "--artifact-name",
            "missing-source.md",
            "--content-file",
            missing_content_path,
            "--json",
        ]),
        74,
    );
    common::assert_exit_code(
        &common::run_agent_store(&[
            "get-meta",
            "--db",
            uninitialized_path,
            "--workflow-run-id",
            "R-exit",
            "--artifact-name",
            "artifact.md",
            "--json",
        ]),
        73,
    );
}
