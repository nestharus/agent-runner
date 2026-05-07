mod common;

use serde_json::Value;
use uuid::Uuid;

// proposal § Test-Intent Track row 11
// named risk: Scratchpad CLI HIGH - write JSON could omit receipt fields or mutate file bytes
// selected level: cli_integration
#[test]
fn write_json_from_content_file_emits_receipt_and_stores_exact_bytes() {
    let (db, store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    let content = common::text_bytes();
    let content_path = db.output_path("input.md");
    common::write_file(&content_path, &content);
    let output = common::run_agent_scratchpad(&[
        "write",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "notes.md",
        "--format",
        "text/markdown",
        "--verdict-line",
        "PRIVATE: ready",
        "--content-file",
        content_path.to_str().expect("utf8 content path"),
        "--json",
    ]);

    let json = common::stdout_json(&output);
    assert_eq!(json.get("version").and_then(Value::as_u64), Some(1));
    assert_eq!(
        common::expect_json_string(&json, "sha256"),
        common::sha256_hex(&content)
    );
    assert_eq!(
        json.get("producer_invocation_uuid").and_then(Value::as_str),
        Some(invocation.to_string().as_str())
    );
    assert_eq!(
        json.get("content_len").and_then(Value::as_u64),
        Some(content.len() as u64)
    );
    assert_eq!(
        json.get("format_hint").and_then(Value::as_str),
        Some("text/markdown")
    );
    assert_eq!(
        json.get("verdict_line").and_then(Value::as_str),
        Some("PRIVATE: ready")
    );

    let backing = store
        .get(
            &common::store_key(&common::scratchpad_workflow(invocation), "notes.md"),
            Some(1),
        )
        .expect("backing row");
    assert_eq!(backing.content, content);
}

// proposal § Test-Intent Track row 11
// named risk: Scratchpad CLI HIGH - stdin content could be ignored or mixed with file mode
// selected level: cli_integration
#[test]
fn write_json_from_content_stdin_accepts_raw_binary_bytes() {
    let (db, store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    let bytes = common::binary_bytes();
    let mut command = common::agent_scratchpad_cmd();
    command.args([
        "write",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "blob.bin",
        "--content-stdin",
        "--json",
    ]);
    command.stdin(std::process::Stdio::piped());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    let mut child = command.spawn().expect("spawn agent-scratchpad");
    {
        let mut stdin = child.stdin.take().expect("stdin pipe");
        std::io::Write::write_all(&mut stdin, &bytes).expect("write stdin");
    }
    let output = child.wait_with_output().expect("wait");

    let json = common::stdout_json(&output);
    assert_eq!(
        common::expect_json_string(&json, "sha256"),
        common::sha256_hex(&bytes)
    );
    let backing = store
        .get(
            &common::store_key(&common::scratchpad_workflow(invocation), "blob.bin"),
            Some(1),
        )
        .expect("backing row");
    assert_eq!(backing.content, bytes);
}

// proposal § Test-Intent Track row 11
// contract § Expected observable signals row missing-content-file-path
// named risk: Scratchpad CLI HIGH - file I/O failures could be reported as caller misuse
// selected level: cli_integration
#[test]
fn write_missing_content_file_exits_74_and_keeps_stdout_empty() {
    let (db, _store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    let missing_path = db.output_path("missing.md");

    let output = common::run_agent_scratchpad(&[
        "write",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "notes.md",
        "--content-file",
        missing_path.to_str().expect("utf8 missing path"),
    ]);

    common::assert_exit_code(&output, 74);
    assert!(output.stdout.is_empty());
    assert!(common::stderr_text(&output).contains("missing.md"));
}

// proposal § Test-Intent Track row 11
// named risk: Scratchpad CLI HIGH - write could accept ambiguous content sources
// selected level: cli_integration
#[test]
fn write_rejects_missing_or_multiple_content_sources_as_misuse() {
    let (db, _store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    let input_path = db.output_path("input.md");
    common::write_file(&input_path, b"content");

    let missing = common::run_agent_scratchpad(&[
        "write",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "notes.md",
    ]);
    common::assert_exit_code(&missing, 64);

    let multiple = common::run_agent_scratchpad(&[
        "write",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "notes.md",
        "--content-file",
        input_path.to_str().expect("utf8 input path"),
        "--content-stdin",
    ]);
    common::assert_exit_code(&multiple, 64);
}
