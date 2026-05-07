mod common;

use std::fs;

// proposal-test-intent-row 20: CLI get byte fidelity
// assumption-register: A9
// named risk: binary content output could be corrupted by JSON prefixes, logs, UTF-8 conversion, or divergent --out behavior
// selected level: component
#[test]
fn get_writes_raw_bytes_to_stdout_and_out_file_without_json() {
    let db = common::TempDb::new();
    let db_path = db.path().to_str().expect("utf8 db path");
    let fixture = common::fixture_path("blob.bin");
    let fixture_path = fixture.to_str().expect("utf8 fixture path");
    let fixture_bytes = common::fixture_bytes("blob.bin");
    let out_path = db.output_path("blob-out.bin");
    let out_arg = out_path.to_str().expect("utf8 out path");

    common::assert_success(&common::run_agent_store(&[
        "init", "--db", db_path, "--json",
    ]));
    common::assert_success(&common::run_agent_store(&[
        "put",
        "--db",
        db_path,
        "--workflow-run-id",
        "R-cli-get",
        "--artifact-name",
        "blob.bin",
        "--format",
        "application/octet-stream",
        "--content-file",
        fixture_path,
        "--json",
    ]));

    let stdout_output = common::run_agent_store(&[
        "get",
        "--db",
        db_path,
        "--workflow-run-id",
        "R-cli-get",
        "--artifact-name",
        "blob.bin",
    ]);
    common::assert_success(&stdout_output);
    assert_eq!(stdout_output.stdout, fixture_bytes);
    assert!(
        serde_json::from_slice::<serde_json::Value>(&stdout_output.stdout).is_err(),
        "expected raw binary stdout; blob.bin fixture must not be valid JSON"
    );

    let out_output = common::run_agent_store(&[
        "get",
        "--db",
        db_path,
        "--workflow-run-id",
        "R-cli-get",
        "--artifact-name",
        "blob.bin",
        "--out",
        out_arg,
    ]);
    common::assert_success(&out_output);
    assert!(out_output.stdout.is_empty());
    assert_eq!(fs::read(out_path).expect("out bytes"), fixture_bytes);
}
