mod common;

use uuid::Uuid;

// proposal § Test-Intent Track row 12 and Assumption Register A7
// named risk: Scratchpad CLI HIGH - raw read stdout could be polluted by JSON, logs, UTF-8 conversion, or newlines
// selected level: cli_integration
#[test]
fn read_writes_raw_bytes_to_stdout_without_trailing_newline_or_json() {
    let (db, store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    let bytes = common::binary_bytes();
    common::put_scratchpad_row(&store, invocation, "blob.bin", bytes.clone());

    let output = common::run_agent_scratchpad(&[
        "read",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "blob.bin",
    ]);

    common::assert_success(&output);
    assert_eq!(output.stdout, bytes);
    assert!(serde_json::from_slice::<serde_json::Value>(&output.stdout).is_err());
}

// proposal § Test-Intent Track row 12
// contract § Expected observable signals row read-out-separates-stdout
// named risk: Scratchpad CLI HIGH - --out could duplicate bytes to stdout or corrupt the output file
// selected level: cli_integration
#[test]
fn read_out_writes_identical_bytes_to_file_and_leaves_stdout_empty() {
    let (db, store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    let bytes = common::binary_bytes();
    common::put_scratchpad_row(&store, invocation, "blob.bin", bytes.clone());
    let out_path = db.output_path("blob-out.bin");

    let output = common::run_agent_scratchpad(&[
        "read",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "blob.bin",
        "--out",
        out_path.to_str().expect("utf8 out path"),
    ]);

    common::assert_success(&output);
    assert!(output.stdout.is_empty());
    let written = std::fs::read(out_path).expect("out file");
    assert_eq!(written, bytes);
    assert_eq!(common::sha256_hex(&written), common::sha256_hex(&bytes));
}

// proposal § Test-Intent Track rows 4, 12
// named risk: Scratchpad CLI HIGH - not-found diagnostics could leak onto raw stdout
// selected level: cli_integration
#[test]
fn read_missing_artifact_exits_65_with_stderr_only() {
    let (db, _store) = common::init_temp_store();
    let invocation = Uuid::new_v4();

    let output = common::run_agent_scratchpad(&[
        "read",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "missing.md",
    ]);

    common::assert_exit_code(&output, 65);
    assert!(output.stdout.is_empty());
    assert!(common::stderr_text(&output).contains("missing.md"));
}

// proposal § Test-Intent Track rows 5, 12
// contract § Expected observable signals row tombstoned-explicit-read-exits-65
// named risk: Scratchpad CLI HIGH - explicit reads could expose tombstoned content
// selected level: cli_integration
#[test]
fn read_tombstoned_explicit_version_exits_65() {
    let (db, store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    let receipt = common::put_scratchpad_row(&store, invocation, "hidden.md", b"hidden".to_vec());
    store
        .tombstone(&receipt.key, receipt.version, "tester", "hide")
        .expect("tombstone");

    let output = common::run_agent_scratchpad(&[
        "read",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "hidden.md",
        "--version",
        &receipt.version.to_string(),
    ]);

    common::assert_exit_code(&output, 65);
    assert!(output.stdout.is_empty());
}
