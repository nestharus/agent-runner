//! ## Declared roles
//! orchestration, accessor, formatter, mapper, parser, validator, filter
//!
//! Function role map:
//! - `CapturedStreams::new`: mapper
//! - `emit_invocation_to_stderr`: formatter
//! - `bad_return_receipt`, `returned_artifact_persist_failure_shell_body`: formatter
//! - `payload_as_object`, `strip_marker_prefix`: validator
//! - `json_key_set`: mapper
//! - `parse_marker_payload`: parser
//! - `stdout_text`, `stderr_text`: parser
//! - `result_marker_lines`: filter
//! - `assert_invocation_stderr_contract`, `assert_result_stdout_contract`: validator
//! - producer output helpers and `age154_*` tests: orchestration

#![cfg(unix)]

mod age153_support;

use age153_support::{
    Age153Fixture, nonzero_exit_with_non_quota_error_body, prolonged_silence_body, success_body,
};
use oulipoly_state::CompositeInvocationId;
use serde_json::Value;
use std::collections::BTreeSet;
use std::io::Write;
use std::process::Output;

const INVOCATION_PREFIX: &str = "OULIPOLY_INVOCATION=";
const RESULT_PREFIX: &str = "OULIPOLY_RESULT=";
const BAD_RECEIPT_UUID: &str = "33333333-3333-4333-8333-333333333333";

struct CapturedStreams {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl CapturedStreams {
    fn new() -> Self {
        Self {
            stdout: Vec::new(),
            stderr: Vec::new(),
        }
    }
}

fn emit_invocation_to_stderr(payload: &CompositeInvocationId, streams: &mut CapturedStreams) {
    write!(&mut streams.stderr, "{}", payload.stderr_line()).expect("write stderr marker");
}

fn payload_as_object(value: &Value) -> &serde_json::Map<String, Value> {
    value
        .as_object()
        .expect("marker payload must be a JSON object")
}

fn json_key_set(value: &Value) -> BTreeSet<&str> {
    payload_as_object(value)
        .keys()
        .map(String::as_str)
        .collect()
}

fn strip_marker_prefix<'a>(line: &'a str, prefix: &str) -> &'a str {
    line.strip_prefix(prefix)
        .unwrap_or_else(|| panic!("marker line must start with {prefix}: {line}"))
}

fn parse_marker_payload(line: &str, prefix: &str) -> Value {
    let raw = strip_marker_prefix(line, prefix);
    serde_json::from_str(raw).unwrap_or_else(|err| panic!("invalid marker JSON {raw}: {err}"))
}

fn stdout_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn result_marker_lines(stdout: &str) -> Vec<&str> {
    stdout
        .lines()
        .filter(|line| line.starts_with(RESULT_PREFIX))
        .collect()
}

fn assert_invocation_stderr_contract(streams: &CapturedStreams) {
    let stderr = String::from_utf8(streams.stderr.clone()).expect("stderr utf8");
    let stdout = String::from_utf8(streams.stdout.clone()).expect("stdout utf8");
    assert!(stdout.is_empty(), "invocation marker must not be stdout");
    assert!(
        stderr.starts_with(INVOCATION_PREFIX),
        "assumption-register: AGE-151 cleaned main.rs is the marker-compatibility baseline"
    );
    assert!(
        !stderr.ends_with('\n'),
        "expected observable signal: formatter does not own the trailing newline"
    );

    let payload = parse_marker_payload(&stderr, INVOCATION_PREFIX);
    assert_eq!(
        json_key_set(&payload),
        BTreeSet::from(["id", "source"]),
        "expected observable signal: compact JSON payload key set source/id"
    );
    assert_eq!(payload["source"], "claude-age154-invocation");
    assert_eq!(payload["id"], "11111111-1111-4111-8111-111111111111");
}

fn assert_result_stdout_contract(path: &str, output: &Output) -> Value {
    let stdout = stdout_text(output);
    let stderr = stderr_text(output);
    assert!(
        !stderr.contains(RESULT_PREFIX),
        "{path}: OULIPOLY_RESULT must stay stdout-only; stderr was:\n{stderr}"
    );
    let lines = result_marker_lines(&stdout);
    assert_eq!(
        lines.len(),
        1,
        "{path}: expected exactly one OULIPOLY_RESULT line in stdout:\n{stdout}"
    );
    let payload = parse_marker_payload(lines[0], RESULT_PREFIX);
    assert_eq!(
        json_key_set(&payload),
        BTreeSet::from([
            "error_category",
            "exit_code",
            "finished_at",
            "id",
            "status",
            "success",
            "terminal_reason",
        ]),
        "{path}: expected observable signal from proposal item 2"
    );
    assert!(payload["id"].is_string(), "{path}: id must be present");
    assert!(
        payload["status"].is_string(),
        "{path}: status must be present"
    );
    assert!(
        payload["success"].is_boolean(),
        "{path}: success must be present"
    );
    assert!(
        payload["exit_code"].is_number(),
        "{path}: exit_code must be present"
    );
    assert!(
        payload["finished_at"].is_string(),
        "{path}: finished_at must be present"
    );
    payload
}

fn success_output() -> Output {
    let fixture = Age153Fixture::new();
    let marker = fixture.dir.path().join("age154-result-success.txt");
    fixture.write_model("age154-result-success", &["claude-age154-success"]);
    fixture.write_providers_with_bodies(&[(
        "claude-age154-success",
        &success_body(&marker, "age154 success stdout"),
    )]);
    fixture.run_one_shot("age154-result-success")
}

fn non_quota_failure_output() -> Output {
    let fixture = Age153Fixture::new();
    let marker = fixture.dir.path().join("age154-result-non-quota.txt");
    fixture.write_model("age154-result-non-quota", &["claude-age154-non-quota"]);
    fixture.write_providers_with_bodies(&[(
        "claude-age154-non-quota",
        &nonzero_exit_with_non_quota_error_body(&marker),
    )]);
    fixture.run_one_shot("age154-result-non-quota")
}

fn typed_terminal_failure_output() -> Output {
    let fixture = Age153Fixture::new();
    let marker = fixture.dir.path().join("age154-result-typed-terminal.txt");
    fixture.write_model("age154-result-typed-terminal", &["claude-age154-terminal"]);
    fixture.write_providers_with_bodies(&[(
        "claude-age154-terminal",
        &prolonged_silence_body(&marker),
    )]);
    fixture.run_one_shot_with_env(
        "age154-result-typed-terminal",
        &[("OULIPOLY_TEST_BOUNDED_SILENCE_MS", "120")],
    )
}

fn balanced_spawn_error_output() -> Output {
    let fixture = Age153Fixture::new();
    let missing_command = fixture.dir.path().join("missing-age154-provider-command");
    fixture.write_model("age154-result-spawn-error", &["claude-age154-spawn"]);
    fixture.write_providers_with_command_paths(&[("claude-age154-spawn", &missing_command)]);
    fixture.run_one_shot("age154-result-spawn-error")
}

fn bad_return_receipt() -> String {
    format!(
        r#"{{"version_id":"store://return/{BAD_RECEIPT_UUID}/wrong-invocation.md/1","name":"wrong-invocation.md","store_address":{{"workflow_run_id":"return:{BAD_RECEIPT_UUID}","artifact_name":"wrong-invocation.md","version":1}},"sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","content_len":5,"format_hint":"text/markdown","verdict_line":"APPROVED: ready","source":{{"kind":"inline_bytes"}},"producer_invocation_uuid":"{BAD_RECEIPT_UUID}","returned_at":"2026-05-07T12:00:00Z"}}"#
    )
}

fn returned_artifact_persist_failure_shell_body(receipt: &str) -> String {
    format!(
        r#"printf '%s\n' '{receipt}' >> "${{OULIPOLY_RETURN_CHANNEL:?missing}}"
printf 'provider stdout'"#
    )
}

fn returned_artifact_persist_failure_output() -> Output {
    let fixture = Age153Fixture::new();
    let receipt = bad_return_receipt();
    let script = returned_artifact_persist_failure_shell_body(&receipt);
    fixture.write_model("age154-result-return-failure", &["claude-age154-return"]);
    fixture.write_providers_with_bodies(&[("claude-age154-return", &script)]);
    fixture.run_one_shot("age154-result-return-failure")
}

#[test]
fn age154_oulipoly_invocation_stderr_matches_age151_baseline_source_id_payload() {
    let payload = CompositeInvocationId {
        source: "claude-age154-invocation".to_string(),
        id: "11111111-1111-4111-8111-111111111111".to_string(),
    };
    let mut streams = CapturedStreams::new();

    emit_invocation_to_stderr(&payload, &mut streams);

    assert_invocation_stderr_contract(&streams);
}

#[test]
fn age154_oulipoly_result_stdout_key_set_matches_every_named_producer_path() {
    // assumption-register: AGE-151 cleaned main.rs is the marker-compatibility baseline.
    // residual-risk-not-verified: this proves structural ABI, not semantic equivalence
    // of every status/error-category value beyond these captured producer paths.
    let cases = vec![
        ("existing success path", success_output()),
        ("non-quota failure call site", non_quota_failure_output()),
        (
            "typed terminal failure call site",
            typed_terminal_failure_output(),
        ),
        (
            "balanced one-shot spawn-error call site",
            balanced_spawn_error_output(),
        ),
        (
            "returned-artifact persistence failure call site",
            returned_artifact_persist_failure_output(),
        ),
    ];

    for (path, output) in cases {
        assert_result_stdout_contract(path, &output);
    }
}
