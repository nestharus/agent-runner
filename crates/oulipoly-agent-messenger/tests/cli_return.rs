mod common;

use uuid::Uuid;

// proposal § Test-Intent Track rows: CLI return JSON envelope, inline body/file/stdin
// contract § CLI subcommand contract and JSON envelope shape
// named risk: Messenger CLI HIGH - return could mix diagnostics with receipts or mutate inline/file/stdin bytes
// selected level: cli_integration
#[test]
fn return_body_json_receipt_has_stable_fields_and_channel_line() {
    let (db, _store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    let channel = db.output_path("returns.jsonl");

    let output = common::run_agent_messenger(&[
        "return",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "proposal.md",
        "--body",
        "hello",
        "--return-channel",
        channel.to_str().expect("utf8 channel"),
        "--format",
        "text/markdown",
        "--verdict-line",
        "APPROVED: ready",
        "--json",
    ]);

    let json = common::stdout_json(&output);
    for field in [
        "schema_version",
        "version_id",
        "name",
        "store_address",
        "sha256",
        "content_len",
        "format_hint",
        "verdict_line",
        "source",
        "producer_invocation_uuid",
        "returned_at",
    ] {
        assert!(
            json.get(field).is_some(),
            "missing JSON field {field}: {json}"
        );
    }
    let channel_body = std::fs::read_to_string(channel).expect("channel body");
    assert_eq!(channel_body.lines().count(), 1);
}

// proposal § Test-Intent Track row: content source exclusivity
// contract § Expected observable signals row mutually exclusive content flags
// named risk: Messenger CLI HIGH - ambiguous content selectors could combine or discard caller bytes
// selected level: cli_integration
#[test]
fn return_rejects_mutually_exclusive_inline_content_sources() {
    let (db, _store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    let file = db.output_path("input.bin");
    common::write_file(&file, &common::binary_bytes());

    let output = common::run_agent_messenger(&[
        "return",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "bad.bin",
        "--body",
        "hello",
        "--content-file",
        file.to_str().expect("utf8 file"),
    ]);

    common::assert_exit_code(&output, 64);
}
