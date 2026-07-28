mod common;

use std::path::PathBuf;
use std::process::{Output, Stdio};

use chrono::{DateTime, Utc};
use oulipoly_agent_scratchpad::ScratchpadMeta;
use oulipoly_agent_store::{PutReceipt, Store};
use serde_json::{Value, json};
use uuid::Uuid;

fn invocation(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

fn run_owned(args: &[String]) -> Output {
    common::agent_scratchpad_cmd()
        .args(args)
        .output()
        .expect("run agent-scratchpad")
}

fn run_owned_with_env(args: &[String], key: &str, value: &str) -> Output {
    common::agent_scratchpad_cmd()
        .args(args)
        .env(key, value)
        .output()
        .expect("run agent-scratchpad with environment")
}

fn assert_exact_text_output(output: &Output, expected: &[u8]) {
    common::assert_success(output);
    assert_eq!(output.stdout, expected);
    assert!(output.stderr.is_empty());
    assert_eq!(output.stdout.last(), Some(&b'\n'));
}

fn assert_json_output(output: &Output, expected: &Value) {
    common::assert_success(output);
    assert!(output.stderr.is_empty());
    assert_eq!(output.stdout.last(), Some(&b'\n'));
    assert_eq!(
        output.stdout.iter().filter(|byte| **byte == b'\n').count(),
        1,
        "JSON output must have exactly one final newline"
    );
    let actual: Value = serde_json::from_slice(&output.stdout).expect("stdout is JSON");
    assert_eq!(&actual, expected);
}

fn setup_write_fixture() -> (common::TempDb, Store, Uuid, PathBuf, Vec<u8>) {
    let (db, store) = common::init_temp_store();
    let invocation = invocation(1);
    let content = b"characterized write bytes".to_vec();
    let path = db.output_path("write-input.bin");
    common::write_file(&path, &content);
    (db, store, invocation, path, content)
}

fn setup_list_text_fixture() -> (common::TempDb, Store, Uuid, Vec<u8>) {
    let (db, store) = common::init_temp_store();
    let invocation = invocation(2);
    let a = common::put_scratchpad_row(&store, invocation, "a.md", b"a".to_vec());
    let b = common::put_scratchpad_row(&store, invocation, "b.md", b"b".to_vec());
    let expected = format!("a.md v1 {}\nb.md v1 {}\n", a.sha256, b.sha256).into_bytes();
    (db, store, invocation, expected)
}

fn setup_single_row_fixture(
    id: u128,
    name: &str,
    content: &[u8],
) -> (common::TempDb, Store, Uuid, PutReceipt) {
    let (db, store) = common::init_temp_store();
    let invocation = invocation(id);
    let receipt = common::put_scratchpad_row(&store, invocation, name, content.to_vec());
    (db, store, invocation, receipt)
}

fn setup_list_json_fixture() -> (common::TempDb, Store, Uuid, Vec<ScratchpadMeta>) {
    let (db, store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let invocation = invocation(7);
    scratchpad
        .write({
            let mut request = common::write_request(invocation, "shape.md", b"shape-v1".to_vec());
            request.format_hint = Some("text/markdown".to_string());
            request.verdict_line = Some("PRIVATE: keep".to_string());
            request
        })
        .expect("write list JSON v1");
    let second = scratchpad
        .write({
            let mut request = common::write_request(invocation, "shape.md", b"shape-v2".to_vec());
            request.predecessor_version = Some(1);
            request
        })
        .expect("write list JSON v2");
    store
        .tombstone(
            &common::store_key(&common::scratchpad_workflow(invocation), "shape.md"),
            second.version,
            "shape-actor",
            "shape-reason",
        )
        .expect("tombstone list JSON v2");
    let rows = scratchpad
        .list(common::list_request(invocation, Some("shape.md"), true))
        .expect("list seeded JSON rows");
    (db, store, invocation, rows)
}

fn setup_publish_json_fixture() -> (common::TempDb, Store, Uuid, PutReceipt, Vec<u8>) {
    let (db, store) = common::init_temp_store();
    let invocation = invocation(9);
    let first_content = b"source-v1".to_vec();
    let first =
        common::put_scratchpad_row(&store, invocation, "versioned.md", first_content.clone());
    common::put_scratchpad_row(&store, invocation, "versioned.md", b"source-v2".to_vec());
    (db, store, invocation, first, first_content)
}

fn setup_gc_json_fixture() -> (common::TempDb, Store, Uuid) {
    let (db, store) = common::init_temp_store();
    let invocation = invocation(10);
    common::put_scratchpad_row(&store, invocation, "b.md", b"b".to_vec());
    common::put_scratchpad_row(&store, invocation, "a.md", b"a".to_vec());
    (db, store, invocation)
}

fn setup_explicit_read_fixture() -> (common::TempDb, Store, Uuid) {
    let (db, store) = common::init_temp_store();
    let invocation = invocation(14);
    common::put_scratchpad_row(&store, invocation, "history.bin", b"v1".to_vec());
    common::put_scratchpad_row(&store, invocation, "history.bin", b"v2".to_vec());
    (db, store, invocation)
}

fn setup_delete_selector_fixture() -> (common::TempDb, Store, Uuid) {
    let (db, store) = common::init_temp_store();
    let invocation = invocation(18);
    for name in ["latest.md", "version.md", "all.md"] {
        common::put_scratchpad_row(&store, invocation, name, name.as_bytes().to_vec());
    }
    (db, store, invocation)
}

fn meta_json(meta: &ScratchpadMeta) -> Value {
    json!({
        "address": {
            "invocation_uuid": meta.address.invocation_uuid.to_string(),
            "name": meta.address.name.as_str(),
        },
        "invocation_uuid": meta.invocation_uuid.to_string(),
        "name": meta.name.as_str(),
        "version": meta.version,
        "sha256": meta.sha256,
        "content_len": meta.content_len,
        "producer_invocation_uuid": meta.producer_invocation_uuid.map(|uuid| uuid.to_string()),
        "format_hint": meta.format_hint,
        "verdict_line": meta.verdict_line,
        "predecessor_version": meta.predecessor_version,
        "created_at": meta.created_at.to_rfc3339(),
        "tombstone": meta.tombstone.as_ref().map(|tombstone| json!({
            "tombstoned_at": tombstone.tombstoned_at.to_rfc3339(),
            "actor": tombstone.actor,
            "reason": tombstone.reason,
        })),
    })
}

// C-GAP-02: successful non-JSON write output is a byte contract.
#[test]
fn write_text_output_is_byte_exact_and_newline_terminated() {
    let (db, _store, invocation, input, content) = setup_write_fixture();

    let output = common::run_agent_scratchpad(&[
        "write",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "notes.md",
        "--content-file",
        input.to_str().expect("UTF-8 input path"),
    ]);

    let expected = format!("notes.md v1 {}\n", common::sha256_hex(&content));
    assert_exact_text_output(&output, expected.as_bytes());
}

// C-GAP-02: successful non-JSON list output preserves row order and bytes.
#[test]
fn list_text_output_is_byte_exact_and_newline_terminated() {
    let (db, _store, invocation, expected) = setup_list_text_fixture();

    let output = common::run_agent_scratchpad(&[
        "list",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
    ]);

    assert_exact_text_output(&output, &expected);
}

// C-GAP-02/C-GAP-04: omitted delete selectors mean latest and have exact text.
#[test]
fn delete_default_latest_text_output_is_byte_exact() {
    let (db, _store, invocation, _receipt) = setup_single_row_fixture(3, "delete.md", b"delete");

    let output = common::run_agent_scratchpad(&[
        "delete",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "delete.md",
    ]);

    assert_exact_text_output(&output, b"delete.md tombstoned=1 already_tombstoned=0\n");
}

// C-GAP-02: successful non-JSON publish output is a byte contract.
#[test]
fn publish_text_output_is_byte_exact_and_newline_terminated() {
    let content = b"publish bytes";
    let (db, _store, invocation, _receipt) = setup_single_row_fixture(4, "draft.md", content);

    let output = common::run_agent_scratchpad(&[
        "publish",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "draft.md",
        "--workflow-run-id",
        "canonical-run",
        "--artifact-name",
        "artifact.md",
    ]);

    let expected = format!(
        "draft.md -> canonical-run artifact.md v1 {}\n",
        common::sha256_hex(content)
    );
    assert_exact_text_output(&output, expected.as_bytes());
}

// C-GAP-02: successful non-JSON GC output is a byte contract.
#[test]
fn gc_text_output_is_byte_exact_and_newline_terminated() {
    let (db, _store, invocation, _receipt) =
        setup_single_row_fixture(5, "candidate.md", b"candidate");

    let output = common::run_agent_scratchpad(&[
        "gc",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--dry-run",
    ]);

    assert_exact_text_output(&output, b"gc dry_run=true tombstoned=1\n");
}

// C-GAP-02/C-GAP-04: scope has an explicit-only, byte-exact text form.
#[test]
fn scope_text_output_is_byte_exact_and_newline_terminated() {
    let invocation = invocation(6);

    let output =
        common::run_agent_scratchpad(&["scope", "--invocation-uuid", &invocation.to_string()]);

    let expected = format!("scratchpad:{invocation}\n");
    assert_exact_text_output(&output, expected.as_bytes());
}

// C-GAP-03: write JSON preserves every key, nested address, null, type,
// timestamp rendering, and its single final newline.
#[test]
fn write_json_shape_is_complete() {
    let (db, store, invocation, input, content) = setup_write_fixture();

    let output = common::run_agent_scratchpad(&[
        "write",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "shape.bin",
        "--content-file",
        input.to_str().expect("UTF-8 input path"),
        "--json",
    ]);
    let meta = store
        .get_meta(
            &common::store_key(&common::scratchpad_workflow(invocation), "shape.bin"),
            Some(1),
        )
        .expect("written backing metadata");

    assert_json_output(
        &output,
        &json!({
            "address": {"invocation_uuid": invocation.to_string(), "name": "shape.bin"},
            "invocation_uuid": invocation.to_string(),
            "name": "shape.bin",
            "version": 1,
            "producer_invocation_uuid": invocation.to_string(),
            "sha256": common::sha256_hex(&content),
            "content_len": content.len() as u64,
            "format_hint": null,
            "verdict_line": null,
            "predecessor_version": null,
            "created_at": meta.created_at.to_rfc3339(),
        }),
    );
}

// C-GAP-03: list JSON preserves complete active and tombstoned row shapes.
#[test]
fn list_json_shape_is_complete() {
    let (db, _store, invocation, rows) = setup_list_json_fixture();

    let output = common::run_agent_scratchpad(&[
        "list",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "shape.md",
        "--include-tombstoned",
        "--json",
    ]);

    let expected = Value::Array(rows.iter().map(meta_json).collect());
    assert_json_output(&output, &expected);
}

// C-GAP-03/C-GAP-04: the default delete selector serializes as `latest`,
// and the complete receipt shape includes its backing tombstone timestamp.
#[test]
fn delete_json_shape_and_default_latest_selector_are_complete() {
    let (db, store, invocation, receipt) =
        setup_single_row_fixture(8, "delete-shape.md", b"delete-shape");

    let output = common::run_agent_scratchpad(&[
        "delete",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "delete-shape.md",
        "--json",
    ]);
    let tombstoned = store
        .get_meta(&receipt.key, Some(receipt.version))
        .expect("tombstoned backing metadata")
        .tombstone
        .expect("tombstone metadata");

    assert_json_output(
        &output,
        &json!({
            "address": {"invocation_uuid": invocation.to_string(), "name": "delete-shape.md"},
            "selector": "latest",
            "tombstoned_versions": [1],
            "already_tombstoned_versions": [],
            "actor": "agent-scratchpad",
            "reason": "scratchpad delete",
            "tombstoned_at": tombstoned.tombstoned_at.to_rfc3339(),
        }),
    );
}

// C-GAP-03/C-GAP-04: publish --version selects that private source and emits
// the complete destination receipt shape without inferring optional metadata.
#[test]
fn publish_json_shape_and_explicit_source_version_are_complete() {
    let (db, store, invocation, first, first_content) = setup_publish_json_fixture();

    let output = common::run_agent_scratchpad(&[
        "publish",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "versioned.md",
        "--version",
        "1",
        "--workflow-run-id",
        "canonical-run",
        "--artifact-name",
        "published.md",
        "--json",
    ]);
    let destination = store
        .get_meta(&common::store_key("canonical-run", "published.md"), Some(1))
        .expect("published backing metadata");

    assert_json_output(
        &output,
        &json!({
            "source": {"invocation_uuid": invocation.to_string(), "name": "versioned.md"},
            "source_version": 1,
            "source_sha256": first.sha256,
            "destination": {"workflow_run_id": "canonical-run", "artifact_name": "published.md"},
            "destination_version": 1,
            "destination_sha256": common::sha256_hex(&first_content),
            "content_len": first_content.len() as u64,
            "producer_invocation_uuid": invocation.to_string(),
            "format_hint": null,
            "verdict_line": null,
            "predecessor_version": null,
            "created_at": destination.created_at.to_rfc3339(),
        }),
    );
}

// C-GAP-03/C-GAP-08: GC JSON preserves selector text, ordered address arrays,
// custom metadata, timestamp type, and the final newline.
#[test]
fn gc_json_shape_is_complete() {
    let (db, _store, invocation) = setup_gc_json_fixture();
    let before = Utc::now();

    let output = common::run_agent_scratchpad(&[
        "gc",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--dry-run",
        "--actor",
        "custom-gc-actor",
        "--reason",
        "custom gc reason",
        "--json",
    ]);
    let after = Utc::now();
    let actual: Value = serde_json::from_slice(&output.stdout).expect("stdout is JSON");
    let evaluated_text = actual
        .get("evaluated_at")
        .and_then(Value::as_str)
        .expect("evaluated_at string");
    let evaluated_at = DateTime::parse_from_rfc3339(evaluated_text)
        .expect("RFC3339 evaluated_at")
        .with_timezone(&Utc);

    assert!(before <= evaluated_at && evaluated_at <= after);
    assert_json_output(
        &output,
        &json!({
            "selector": format!("invocation:{invocation}"),
            "dry_run": true,
            "tombstoned_rows": [
                {"invocation_uuid": invocation.to_string(), "name": "a.md"},
                {"invocation_uuid": invocation.to_string(), "name": "b.md"},
            ],
            "already_tombstoned_rows": [],
            "actor": "custom-gc-actor",
            "reason": "custom gc reason",
            "evaluated_at": evaluated_text,
        }),
    );
}

// C-GAP-03: scope JSON is fully deterministic, including key order and newline.
#[test]
fn scope_json_bytes_are_complete_and_exact() {
    let invocation = invocation(11);

    let output = common::run_agent_scratchpad(&[
        "scope",
        "--invocation-uuid",
        &invocation.to_string(),
        "--json",
    ]);

    let expected = format!(
        "{{\"invocation_uuid\":\"{invocation}\",\"workflow_run_id\":\"scratchpad:{invocation}\"}}\n"
    );
    assert_exact_text_output(&output, expected.as_bytes());
}

// C-GAP-04: each command family retains representative required arguments.
#[test]
fn representative_missing_required_flags_are_clap_misuse() {
    let (db, _store) = common::init_temp_store();
    let invocation = invocation(12).to_string();
    let db_path = db.path_arg();
    let cases = [
        (
            vec!["write", "--name", "notes.md", "--content-stdin"],
            "--db <DB>",
        ),
        (
            vec!["read", "--db", &db_path, "--invocation-uuid", &invocation],
            "--name <NAME>",
        ),
        (vec!["list", "--invocation-uuid", &invocation], "--db <DB>"),
        (
            vec!["delete", "--db", &db_path, "--invocation-uuid", &invocation],
            "--name <NAME>",
        ),
        (
            vec![
                "publish",
                "--db",
                &db_path,
                "--invocation-uuid",
                &invocation,
                "--name",
                "draft.md",
                "--workflow-run-id",
                "canonical-run",
            ],
            "--artifact-name <ARTIFACT_NAME>",
        ),
        (vec!["gc", "--invocation-uuid", &invocation], "--db <DB>"),
        (vec!["scope"], "--invocation-uuid <INVOCATION_UUID>"),
    ];

    for (args, missing_flag) in cases {
        let output = common::run_agent_scratchpad(&args);
        common::assert_exit_code(&output, 64);
        assert!(output.stdout.is_empty());
        assert!(common::stderr_text(&output).contains(missing_flag));
    }
}

// C-GAP-04: delete and GC selector conflicts remain parser misuse.
#[test]
fn selector_conflicts_are_rejected() {
    let (db, _store) = common::init_temp_store();
    let invocation = invocation(13).to_string();
    let db_path = db.path_arg();
    let cutoff = "2026-01-01T00:00:00Z";
    let cases = [
        vec![
            "delete",
            "--db",
            &db_path,
            "--invocation-uuid",
            &invocation,
            "--name",
            "notes.md",
            "--version",
            "1",
            "--all-versions",
        ],
        vec![
            "gc",
            "--db",
            &db_path,
            "--invocation-uuid",
            &invocation,
            "--expired-before",
            cutoff,
        ],
    ];

    for args in cases {
        let output = common::run_agent_scratchpad(&args);
        common::assert_exit_code(&output, 64);
        assert!(output.stdout.is_empty());
    }
}

// C-GAP-04: read --version selects an older active version byte-for-byte.
#[test]
fn read_explicit_active_version_returns_that_version() {
    let (db, _store, invocation) = setup_explicit_read_fixture();

    let output = common::run_agent_scratchpad(&[
        "read",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "history.bin",
        "--version",
        "1",
    ]);

    common::assert_success(&output);
    assert_eq!(output.stdout, b"v1");
    assert!(output.stderr.is_empty());
}

// C-GAP-04: invalid GC UUID and cutoff values are bounded misuse cases.
#[test]
fn invalid_gc_selector_values_are_rejected() {
    let (db, _store) = common::init_temp_store();
    let cases = [
        ("--invocation-uuid", "not-a-uuid"),
        ("--expired-before", "not-a-timestamp"),
    ];

    for (selector, value) in cases {
        let output = common::run_agent_scratchpad(&["gc", "--db", &db.path_arg(), selector, value]);
        common::assert_exit_code(&output, 64);
        assert!(output.stdout.is_empty());
        assert!(common::stderr_text(&output).contains(value));
    }
}

// C-GAP-05: missing and non-string parent id payloads share the declared
// invalid-scope result; an invalid UUID string remains distinguishable.
#[test]
fn invalid_parent_payload_shapes_are_rejected() {
    let (db, _store) = common::init_temp_store();
    let cases = [
        ("{}", "OULIPOLY_PARENT_INVOCATION is missing id"),
        (r#"{"id":null}"#, "OULIPOLY_PARENT_INVOCATION is missing id"),
        (r#"{"id":42}"#, "OULIPOLY_PARENT_INVOCATION is missing id"),
        (r#"{"id":{}}"#, "OULIPOLY_PARENT_INVOCATION is missing id"),
        (r#"{"id":[]}"#, "OULIPOLY_PARENT_INVOCATION is missing id"),
        (r#"{"id":"not-a-uuid"}"#, "not-a-uuid: invalid UUID"),
    ];

    for (payload, expected_stderr) in cases {
        let output = common::run_agent_scratchpad_with_env(
            &["list", "--db", &db.path_arg()],
            &[("OULIPOLY_PARENT_INVOCATION", payload)],
        );
        common::assert_exit_code(&output, 64);
        assert!(output.stdout.is_empty());
        assert!(common::stderr_text(&output).contains(expected_stderr));
    }
}

// C-GAP-05: an explicit invocation scope wins without parsing malformed env.
#[test]
fn explicit_scope_precedes_malformed_parent_environment() {
    let (db, _store) = common::init_temp_store();
    let invocation = invocation(15).to_string();
    let args = vec![
        "list".to_string(),
        "--db".to_string(),
        db.path_arg(),
        "--invocation-uuid".to_string(),
        invocation,
        "--json".to_string(),
    ];

    let output = run_owned_with_env(&args, "OULIPOLY_PARENT_INVOCATION", "not JSON");

    assert_exact_text_output(&output, b"[]\n");
}

// C-GAP-05: parent environment is not a selector fallback for GC or scope.
#[test]
fn gc_and_scope_do_not_fall_back_to_parent_environment() {
    let (db, _store) = common::init_temp_store();
    let parent = common::parent_invocation_env(invocation(16));
    let cases = [
        vec!["gc".to_string(), "--db".to_string(), db.path_arg()],
        vec!["scope".to_string()],
    ];

    for args in cases {
        let output = run_owned_with_env(&args, "OULIPOLY_PARENT_INVOCATION", &parent);
        common::assert_exit_code(&output, 64);
        assert!(output.stdout.is_empty());
    }
}

// C-GAP-03: expired GC selector text retains normalized UTC RFC3339 output.
#[test]
fn expired_gc_selector_json_text_is_stable() {
    let (db, _store) = common::init_temp_store();
    let cutoff = "2026-01-02T03:04:05+02:00";

    let output = common::run_agent_scratchpad(&[
        "gc",
        "--db",
        &db.path_arg(),
        "--expired-before",
        cutoff,
        "--dry-run",
        "--json",
    ]);
    let actual: Value = serde_json::from_slice(&output.stdout).expect("stdout is JSON");
    let evaluated = actual
        .get("evaluated_at")
        .and_then(Value::as_str)
        .expect("evaluated_at string");

    assert_json_output(
        &output,
        &json!({
            "selector": "expired_before:2026-01-02T01:04:05+00:00",
            "dry_run": true,
            "tombstoned_rows": [],
            "already_tombstoned_rows": [],
            "actor": "agent-scratchpad-gc",
            "reason": "scratchpad gc expired",
            "evaluated_at": evaluated,
        }),
    );
}

// C-GAP-03: all delete selector spellings are stable JSON strings.
#[test]
fn delete_json_selector_strings_are_stable() {
    let (db, _store, invocation) = setup_delete_selector_fixture();
    let cases = [
        ("latest.md", Vec::<&str>::new(), "latest"),
        ("version.md", vec!["--version", "1"], "version:1"),
        ("all.md", vec!["--all-versions"], "all_versions"),
    ];

    for (name, selector_args, expected_selector) in cases {
        let mut args = vec![
            "delete".to_string(),
            "--db".to_string(),
            db.path_arg(),
            "--invocation-uuid".to_string(),
            invocation.to_string(),
            "--name".to_string(),
            name.to_string(),
        ];
        args.extend(selector_args.into_iter().map(str::to_string));
        args.push("--json".to_string());

        let output = run_owned(&args);
        let json = common::stdout_json(&output);
        assert_eq!(
            json.get("selector").and_then(Value::as_str),
            Some(expected_selector)
        );
        assert_eq!(output.stdout.last(), Some(&b'\n'));
    }
}

#[test]
fn list_name_validation_occurs_after_successful_open() {
    let (db, _store) = common::init_temp_store();
    let invocation = invocation(19).to_string();
    let invalid_name = "scratchpad:reserved";
    let unopened = db.output_path("uninitialized.sqlite");

    let open_error = common::run_agent_scratchpad(&[
        "list",
        "--db",
        unopened.to_str().expect("UTF-8 uninitialized path"),
        "--invocation-uuid",
        &invocation,
        "--name",
        invalid_name,
    ]);
    common::assert_exit_code(&open_error, 73);
    assert!(open_error.stdout.is_empty());
    assert!(common::stderr_text(&open_error).contains("database schema migration required"));

    let validation_error = common::run_agent_scratchpad(&[
        "list",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation,
        "--name",
        invalid_name,
    ]);
    common::assert_exit_code(&validation_error, 64);
    assert!(validation_error.stdout.is_empty());
    assert!(
        common::stderr_text(&validation_error)
            .contains("scratchpad name must not start with reserved prefix scratchpad:")
    );
}

#[test]
fn executable_error_mappings_retain_collision_66_and_serialization_70() {
    let (db, _store) = common::init_temp_store();
    let invocation = invocation(20);
    let workflow = common::scratchpad_workflow(invocation);
    let input = db.output_path("collision-input.bin");
    common::write_file(&input, b"collision");
    let connection = rusqlite::Connection::open(db.path()).expect("open collision fixture");
    connection
        .execute_batch(&format!(
            "CREATE TRIGGER force_insert_collision \
             BEFORE INSERT ON artifact_versions \
             FOR EACH ROW \
             WHEN NEW.workflow_run_id = '{workflow}' \
              AND NEW.artifact_name = 'collision.md' \
             BEGIN \
                 SELECT RAISE(ABORT, 'forced insert collision'); \
             END;"
        ))
        .expect("install collision trigger");
    drop(connection);

    let collision = common::run_agent_scratchpad(&[
        "write",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--name",
        "collision.md",
        "--content-file",
        input.to_str().expect("UTF-8 collision input path"),
    ]);
    common::assert_exit_code(&collision, 66);
    assert!(collision.stdout.is_empty());
    assert!(common::stderr_text(&collision).contains("backing store collision"));

    let large_format = "x".repeat(64 * 1024);
    let serialization_args = vec![
        "write".to_string(),
        "--db".to_string(),
        db.path_arg(),
        "--invocation-uuid".to_string(),
        invocation.to_string(),
        "--name".to_string(),
        "serialization.md".to_string(),
        "--format".to_string(),
        large_format,
        "--content-file".to_string(),
        input
            .to_str()
            .expect("UTF-8 serialization input path")
            .to_string(),
        "--json".to_string(),
    ];
    let mut serialization_process = common::agent_scratchpad_cmd()
        .args(serialization_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn JSON command");
    drop(serialization_process.stdout.take());
    let serialization = serialization_process
        .wait_with_output()
        .expect("wait for JSON command with closed stdout");
    common::assert_exit_code(&serialization, 70);
    assert!(serialization.stdout.is_empty());
    assert!(common::stderr_text(&serialization).contains("json serialization error:"));
}
