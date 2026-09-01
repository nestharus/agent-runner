#![cfg(unix)]

//! Declared roles: accessor, formatter, mapper, parser, filter,
//! orchestration, validator.

mod age153_support;
mod provider_authority_fixture;

use age153_support::{
    Age153Fixture, FORCE_TERMINAL_SIGNAL_KIND, nonzero_exit_with_non_quota_error_body, toml_string,
};
use chrono::{Duration, Utc};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Output;

const BAD_RECEIPT_UUID: &str = "33333333-3333-4333-8333-333333333333";
const FIXED_SESSION_ID: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const EXPECTED_FAILURE_KEYS: [&str; 11] = [
    "agent_runner_chain_id",
    "agent_runner_invocation_id",
    "error_category",
    "exit_code",
    "finished_at",
    "id",
    "provider_name",
    "provider_session_id",
    "status",
    "success",
    "terminal_reason",
];

fn stdout_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn key_set(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .expect("payload object")
        .keys()
        .map(String::as_str)
        .collect()
}

fn assert_object_contains_keys(value: &Value, keys: &[&str], context: &str) {
    let object = value
        .as_object()
        .unwrap_or_else(|| panic!("{context} must be an object: {value}"));
    for key in keys {
        assert!(
            object.contains_key(*key),
            "{context} missing required key {key}: {value}"
        );
    }
}

fn expected_failure_key_set() -> BTreeSet<&'static str> {
    BTreeSet::from(EXPECTED_FAILURE_KEYS)
}

fn result_lines(stdout: &str) -> Vec<&str> {
    stdout
        .lines()
        .filter(|line| line.starts_with("OULIPOLY_RESULT="))
        .collect()
}

fn failure_lines(stdout: &str) -> Vec<&str> {
    stdout
        .lines()
        .filter(|line| line.starts_with("OULIPOLY_FAILURE="))
        .collect()
}

fn result_payload(output: &Output) -> Value {
    let stdout = stdout_text(output);
    let lines = result_lines(&stdout);
    assert_eq!(
        lines.len(),
        1,
        "expected exactly one OULIPOLY_RESULT line in stdout:\n{stdout}"
    );
    serde_json::from_str(lines[0].strip_prefix("OULIPOLY_RESULT=").unwrap()).unwrap()
}

fn raw_result_payload(fixture: &Age153Fixture, invocation_id: &str) -> Value {
    let path = fixture
        .data_home
        .join("oulipoly-agent-runner")
        .join("invocations")
        .join(format!("{invocation_id}.result"));
    serde_json::from_slice(&fs::read(&path).unwrap_or_else(|err| {
        panic!(
            "failed to read raw result artifact {}: {err}",
            path.display()
        )
    }))
    .unwrap()
}

fn assert_failure_identity(
    payload: &Value,
    provider_name: &str,
    provider_session_id: Option<&str>,
    chain_id: Option<&str>,
) {
    assert_eq!(key_set(payload), expected_failure_key_set(), "{payload}");
    assert_eq!(payload["status"], "failed");
    assert_eq!(payload["success"], false);
    assert!(payload["id"].as_str().is_some(), "{payload}");
    assert_eq!(payload["agent_runner_invocation_id"], payload["id"]);
    assert_eq!(payload["provider_name"], provider_name);
    match provider_session_id {
        Some(expected) => assert_eq!(payload["provider_session_id"], expected),
        None => assert!(
            payload["provider_session_id"].is_null(),
            "provider_session_id must be explicit null when unavailable: {payload}"
        ),
    }
    match chain_id {
        Some(expected) => assert_eq!(payload["agent_runner_chain_id"], expected),
        None => assert!(
            payload["agent_runner_chain_id"].is_null(),
            "agent_runner_chain_id must be explicit null when unavailable: {payload}"
        ),
    }
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

fn write_forced_capture_provider_with_command(
    fixture: &Age153Fixture,
    model_name: &str,
    provider_name: &str,
    command: &Path,
) {
    fixture.write_model(model_name, &[provider_name]);
    fs::write(
        fixture.app_config_dir.join("providers.toml"),
        provider_authority_fixture::with_explicit_provider_authority(&format!(
            r#"[{provider_name}]
command = {}
args = []
interactive_args = ["interactive"]
prompt_mode = "arg"

[{provider_name}.session_capture]
kind = "forced_flag_verified"
flag = "--session-id"

"#,
            toml_string(&command.display().to_string())
        )),
    )
    .unwrap();
}

fn write_stdout_json_capture_provider(
    fixture: &Age153Fixture,
    model_name: &str,
    provider_name: &str,
    command_body: &str,
) {
    let command = fixture.write_script(&format!("{provider_name}-command.sh"), command_body);
    fixture.write_model(model_name, &[provider_name]);
    fs::write(
        fixture.app_config_dir.join("providers.toml"),
        provider_authority_fixture::with_explicit_provider_authority(&format!(
            r#"[{provider_name}]
command = {}
args = []
interactive_args = ["interactive"]
prompt_mode = "arg"

[{provider_name}.session_capture]
kind = "stdout_json_event"
json_flag = "--json"
last_message_flag = "--last-message"
event_type = "thread.started"
event_id_path = "thread_id"

"#,
            toml_string(&command.display().to_string())
        )),
    )
    .unwrap();
}

fn stdout_json_session_body(session_id: &str, exit_code: i32) -> String {
    format!(
        r#"printf '{{"type":"thread.started","thread_id":"{session_id}"}}\n'
printf 'fixture provider failed\n' >&2
exit {exit_code}"#
    )
}

fn zero_turn_then_nonzero_forced_capture_body(
    marker: &Path,
    transcript: &Path,
    counter: &Path,
) -> String {
    format!(
        r#"session_id=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--session-id" ]; then
    session_id="$2"
    shift 2
  else
    shift
  fi
done
count=0
if [ -f {} ]; then
  count="$(cat {})"
fi
count=$((count + 1))
printf '%s\n' "$count" > {}
printf '%s\n' "$session_id" >> {}
printf '{{"type":"system","subtype":"init","session_id":"%s"}}\n' "$session_id"
if [ "$count" -ge 2 ]; then
  printf '{{"session_id":"%s","turn_id":"turn-%s","timestamp":"2026-04-17T08:00:01Z","role":"assistant"}}\n' "$session_id" "$$" >> {}
  printf 'second attempt failed after existing chain\n' >&2
  exit 17
fi
exit 1"#,
        toml_string(&counter.display().to_string()),
        toml_string(&counter.display().to_string()),
        toml_string(&counter.display().to_string()),
        toml_string(&marker.display().to_string()),
        toml_string(&transcript.display().to_string())
    )
}

fn write_sessions_toml(fixture: &Age153Fixture, provider_name: &str, transcript: &Path) {
    fs::write(
        fixture.app_config_dir.join("sessions.toml"),
        format!(
            r#"[{provider_name}]
turn_script = 'cat "{}"'
state_dir = '{}'

"#,
            transcript.display(),
            fixture
                .dir
                .path()
                .join(format!("{provider_name}-session-state"))
                .display()
        ),
    )
    .unwrap();
}

fn assert_detail_string_or_null(detail: &Value, key: &str) {
    assert!(
        detail[key].is_string() || detail[key].is_null(),
        "detail.{key} must be a string when available or null when unavailable: {detail}"
    );
}

fn assert_detail_number_or_null(detail: &Value, key: &str) {
    assert!(
        detail[key].is_number() || detail[key].is_null(),
        "detail.{key} must be a number when available or null when unavailable: {detail}"
    );
}

fn assert_detail_array_of_strings(detail: &Value, key: &str) {
    let values = detail[key]
        .as_array()
        .unwrap_or_else(|| panic!("detail.{key} must be an array: {detail}"));
    assert!(
        values.iter().all(Value::is_string),
        "detail.{key} entries must all be strings: {detail}"
    );
}

fn assert_optional_string_detail(detail: &Value, key: &str, expected: Option<&str>) {
    match expected {
        Some(expected) => assert_eq!(detail[key], expected, "detail.{key} mismatch: {detail}"),
        None => assert!(
            detail[key].is_null(),
            "detail.{key} must be null when unavailable: {detail}"
        ),
    }
}

fn assert_optional_number_detail(detail: &Value, key: &str, expected: Option<i64>) {
    match expected {
        Some(expected) => assert_eq!(detail[key], expected, "detail.{key} mismatch: {detail}"),
        None => assert!(
            detail[key].is_null(),
            "detail.{key} must be null when unavailable: {detail}"
        ),
    }
}

fn assert_attempted_providers_detail(detail: &Value, expected: &[&str]) {
    let attempted = detail["attempted_providers"]
        .as_array()
        .expect("detail.attempted_providers array");
    let actual = attempted
        .iter()
        .map(|provider| provider.as_str().expect("attempted provider string"))
        .collect::<Vec<_>>();
    assert_eq!(
        actual.as_slice(),
        expected,
        "detail.attempted_providers mismatch: {detail}"
    );
}

fn parse_pre_invocation_failure_payload(
    output: &Output,
    expected_stage: &str,
    expected_model_name: Option<&str>,
    expected_provider_index: Option<i64>,
    expected_attempted_providers: &[&str],
    expect_reason: bool,
) -> Value {
    let payload = decode_pre_invocation_failure_payload(output);
    validate_pre_invocation_failure_transport(output);
    validate_pre_invocation_failure_envelope(&payload, expected_stage);
    validate_pre_invocation_failure_detail(
        &payload,
        expected_model_name,
        expected_provider_index,
        expected_attempted_providers,
        expect_reason,
    );
    payload
}

fn decode_pre_invocation_failure_payload(output: &Output) -> Value {
    let stdout = stdout_text(output);
    let line = failure_lines(&stdout)
        .into_iter()
        .next()
        .expect("OULIPOLY_FAILURE line");
    serde_json::from_str(line.strip_prefix("OULIPOLY_FAILURE=").unwrap()).unwrap()
}

fn validate_pre_invocation_failure_transport(output: &Output) {
    assert_ne!(output.status.code(), Some(0), "{output:?}");
    let stdout = stdout_text(output);
    let stderr = stderr_text(output);
    assert!(
        !stderr.contains("OULIPOLY_FAILURE="),
        "OULIPOLY_FAILURE must be stdout-only; stderr was:\n{stderr}"
    );
    assert!(
        result_lines(&stdout).is_empty(),
        "pre-invocation fast-fail must not forge an OULIPOLY_RESULT id:\n{stdout}"
    );
    let lines = failure_lines(&stdout);
    assert_eq!(
        lines.len(),
        1,
        "expected exactly one OULIPOLY_FAILURE line in stdout:\n{stdout}"
    );
}

fn validate_pre_invocation_failure_envelope(payload: &Value, expected_stage: &str) {
    assert_object_contains_keys(
        payload,
        &[
            "failure_kind",
            "stage",
            "status",
            "success",
            "exit_code",
            "terminal_reason",
            "error_category",
            "finished_at",
            "message",
            "detail",
            "agent_runner_invocation_id",
            "provider_name",
            "provider_session_id",
            "agent_runner_chain_id",
        ],
        "OULIPOLY_FAILURE payload",
    );
    assert_eq!(
        key_set(payload),
        BTreeSet::from([
            "agent_runner_chain_id",
            "agent_runner_invocation_id",
            "detail",
            "error_category",
            "exit_code",
            "failure_kind",
            "finished_at",
            "message",
            "provider_name",
            "provider_session_id",
            "stage",
            "status",
            "success",
            "terminal_reason",
        ])
    );
    assert_eq!(payload["failure_kind"], "pre_invocation");
    assert_eq!(payload["stage"], expected_stage);
    assert_eq!(payload["status"], "failed");
    assert_eq!(payload["success"], false);
    assert!(payload["exit_code"].is_null());
    assert_eq!(payload["terminal_reason"], "pre_invocation_failure");
    assert!(payload["error_category"].is_null());
    assert!(payload["agent_runner_invocation_id"].is_null());
    assert!(payload["provider_name"].is_null());
    assert!(payload["provider_session_id"].is_null());
    assert!(payload["agent_runner_chain_id"].is_null());
    assert!(payload["finished_at"].as_str().is_some());
    assert!(payload["message"].as_str().is_some());
}

fn validate_pre_invocation_failure_detail(
    payload: &Value,
    expected_model_name: Option<&str>,
    expected_provider_index: Option<i64>,
    expected_attempted_providers: &[&str],
    expect_reason: bool,
) {
    let detail = &payload["detail"];
    assert_object_contains_keys(
        detail,
        &[
            "attempted_providers",
            "model_name",
            "provider_index",
            "reason",
        ],
        "OULIPOLY_FAILURE detail",
    );
    assert_eq!(
        key_set(detail),
        BTreeSet::from([
            "attempted_providers",
            "model_name",
            "provider_index",
            "reason",
        ])
    );
    assert_detail_string_or_null(detail, "model_name");
    assert_detail_number_or_null(detail, "provider_index");
    assert_detail_string_or_null(detail, "reason");
    assert_detail_array_of_strings(detail, "attempted_providers");
    assert_optional_string_detail(detail, "model_name", expected_model_name);
    assert_optional_number_detail(detail, "provider_index", expected_provider_index);
    assert_attempted_providers_detail(detail, expected_attempted_providers);
    if expect_reason {
        assert!(
            detail["reason"].is_string(),
            "detail.reason must be a string when the stage has a concrete failure reason: {detail}"
        );
    } else {
        assert!(
            detail["reason"].is_null(),
            "detail.reason must be null when unavailable: {detail}"
        );
    }
}

fn unknown_diagnostic_lines(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|line| line.starts_with("OULIPOLY_UNKNOWN_DIAGNOSTIC="))
        .collect()
}

fn run_unknown_fixture(main_command_body: &str, suffix: &str) -> Output {
    let fixture = Age153Fixture::new();
    let main_command = fixture.write_script(
        &format!("age175-unknown-primary-{suffix}.sh"),
        main_command_body,
    );
    let diagnostic_command = fixture.write_script(
        &format!("age175-unknown-diagnostic-{suffix}.sh"),
        r#"cat >/dev/null
printf 'diagnostic model unavailable\n' >&2
exit 12"#,
    );
    fixture.write_model(
        &format!("age175-unknown-{suffix}"),
        &[&format!("fixture-age175-unknown-{suffix}")],
    );
    fixture.write_model(
        &format!("age175-diagnostic-{suffix}"),
        &[&format!("fixture-age175-diagnostic-{suffix}")],
    );
    fs::write(
        fixture.app_config_dir.join("config.toml"),
        format!(r#"diagnostics_model = "age175-diagnostic-{suffix}""#),
    )
    .unwrap();
    fs::write(
        fixture.app_config_dir.join("providers.toml"),
        provider_authority_fixture::with_explicit_provider_authority(&format!(
            r#"[fixture-age175-unknown-{suffix}]
command = {}
args = []
interactive_args = ["interactive"]
prompt_mode = "arg"

[fixture-age175-diagnostic-{suffix}]
command = {}
args = []
interactive_args = ["interactive"]
prompt_mode = "stdin"

"#,
            toml_string(&main_command.display().to_string()),
            toml_string(&diagnostic_command.display().to_string())
        )),
    )
    .unwrap();

    let output = fixture.run_one_shot(&format!("age175-unknown-{suffix}"));

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    output
}

fn run_unknown_diagnostic_fixture(main_command_body: &str, suffix: &str) -> (Output, Value) {
    let output = run_unknown_fixture(main_command_body, suffix);
    let stderr = stderr_text(&output);
    let lines = unknown_diagnostic_lines(&stderr);
    assert_eq!(
        lines.len(),
        1,
        "expected one OULIPOLY_UNKNOWN_DIAGNOSTIC line:\n{stderr}"
    );
    let payload: Value = serde_json::from_str(
        lines[0]
            .strip_prefix("OULIPOLY_UNKNOWN_DIAGNOSTIC=")
            .unwrap(),
    )
    .unwrap();
    assert!(
        stderr.contains("[diagnostics] unknown: Heuristic classification based on stderr content"),
        "legacy unknown diagnostics line must remain:\n{stderr}"
    );
    (output, payload)
}

fn assert_unknown_diagnostic_schema(payload: &Value, provider: &str, exit_code: i64) {
    assert_eq!(
        key_set(payload),
        BTreeSet::from([
            "account_window_state",
            "error_category",
            "exit_code",
            "provider",
            "provider_index",
            "retry_rotation_disposition",
            "stderr_excerpt",
        ])
    );
    assert_eq!(payload["error_category"], "unknown");
    assert_eq!(payload["provider"], provider);
    assert_eq!(payload["provider_index"], 0);
    assert_eq!(payload["exit_code"], exit_code);
    assert_eq!(payload["retry_rotation_disposition"], "no_retry");
    let account_window_state = &payload["account_window_state"];
    assert_eq!(
        key_set(account_window_state),
        BTreeSet::from(["quota", "quota_read_error", "windows", "windows_read_error"])
    );
    assert!(
        account_window_state["quota_read_error"].is_string()
            || account_window_state["quota_read_error"].is_null(),
        "quota_read_error must be a string or null: {account_window_state}"
    );
    assert!(
        account_window_state["windows_read_error"].is_string()
            || account_window_state["windows_read_error"].is_null(),
        "windows_read_error must be a string or null: {account_window_state}"
    );
    if !account_window_state["quota"].is_null() {
        assert_eq!(
            key_set(&account_window_state["quota"]),
            BTreeSet::from([
                "calls_since_refresh",
                "refreshed_at",
                "exhausted_at",
                "topology_peak_live_window_count",
                "last_topology_probe_at",
                "next_available_at",
                "last_refresh_at",
                "failure_class",
            ])
        );
    }
    for window in account_window_state["windows"].as_array().unwrap() {
        assert_eq!(
            key_set(window),
            BTreeSet::from([
                "window_id",
                "used_percent",
                "resets_at",
                "last_delta_percent",
                "last_delta_calls",
            ])
        );
    }
}

fn assert_excerpt_caps(excerpt: &str) {
    assert!(excerpt.len() <= 1024, "{excerpt}");
    assert!(
        excerpt
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count()
            <= 4,
        "{excerpt}"
    );
}

#[test]
fn failure_result_identity_is_present_on_named_failure_emit_sites() {
    let generic = Age153Fixture::new();
    let generic_marker = generic.dir.path().join("age175-generic-nonzero.txt");
    generic.write_model("age175-generic-nonzero", &["fixture-age175-generic"]);
    generic.write_providers_with_bodies(&[(
        "fixture-age175-generic",
        &nonzero_exit_with_non_quota_error_body(&generic_marker),
    )]);
    let generic_output = generic.run_one_shot("age175-generic-nonzero");
    assert_ne!(generic_output.status.code(), Some(0), "{generic_output:?}");
    assert_failure_identity(
        &result_payload(&generic_output),
        "fixture-age175-generic",
        None,
        None,
    );

    let spawn = Age153Fixture::new();
    let missing_command = spawn.dir.path().join("missing-age175-provider-command");
    spawn.write_model("age175-spawn", &["fixture-age175-spawn"]);
    spawn.write_providers_with_command_paths(&[("fixture-age175-spawn", &missing_command)]);
    let spawn_output = spawn.run_one_shot("age175-spawn");
    assert_ne!(spawn_output.status.code(), Some(0), "{spawn_output:?}");
    assert_failure_identity(
        &result_payload(&spawn_output),
        "fixture-age175-spawn",
        None,
        None,
    );

    let returned = Age153Fixture::new();
    let receipt = bad_return_receipt();
    returned.write_model("age175-return", &["fixture-age175-return"]);
    returned.write_providers_with_bodies(&[(
        "fixture-age175-return",
        &returned_artifact_persist_failure_shell_body(&receipt),
    )]);
    let returned_output = returned.run_one_shot("age175-return");
    assert_eq!(
        returned_output.status.code(),
        Some(1),
        "{returned_output:?}"
    );
    assert_failure_identity(
        &result_payload(&returned_output),
        "fixture-age175-return",
        None,
        None,
    );

    let typed = Age153Fixture::new();
    let typed_marker = typed.dir.path().join("age175-typed-terminal.txt");
    typed.write_model("age175-typed", &["fixture-age175-typed"]);
    typed.write_providers_with_bodies(&[(
        "fixture-age175-typed",
        &nonzero_exit_with_non_quota_error_body(&typed_marker),
    )]);
    let typed_output = typed.run_one_shot_with_env(
        "age175-typed",
        &[(FORCE_TERMINAL_SIGNAL_KIND, "ProlongedSilence")],
    );
    assert_ne!(typed_output.status.code(), Some(0), "{typed_output:?}");
    assert_failure_identity(
        &result_payload(&typed_output),
        "fixture-age175-typed",
        None,
        None,
    );
}

#[test]
fn spawn_setup_failure_preserves_start_known_provider_session_identity() {
    let fixture = Age153Fixture::new();
    let missing_command = fixture
        .dir
        .path()
        .join("missing-age175-start-known-provider-command");
    write_forced_capture_provider_with_command(
        &fixture,
        "age175-start-known-spawn",
        "fixture-age175-start-known",
        &missing_command,
    );

    let output = fixture.run_one_shot("age175-start-known-spawn");

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    let payload = result_payload(&output);
    assert_eq!(key_set(&payload), expected_failure_key_set(), "{payload}");
    assert_eq!(payload["status"], "failed");
    assert_eq!(payload["success"], false);
    assert_eq!(payload["agent_runner_invocation_id"], payload["id"]);
    assert_eq!(payload["provider_name"], "fixture-age175-start-known");
    let session = payload["provider_session_id"]
        .as_str()
        .expect("spawn/setup failure must preserve start-known provider_session_id");
    uuid::Uuid::parse_str(session).expect("start-known provider session must be a UUID");
}

#[test]
fn stdout_result_and_raw_result_artifact_have_lockstep_failure_identity_schema() {
    let fixture = Age153Fixture::new();
    let marker = fixture.dir.path().join("age175-lockstep.txt");
    fixture.write_model("age175-lockstep", &["fixture-age175-lockstep"]);
    fixture.write_providers_with_bodies(&[(
        "fixture-age175-lockstep",
        &nonzero_exit_with_non_quota_error_body(&marker),
    )]);

    let output = fixture.run_one_shot("age175-lockstep");

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    let stdout_payload = result_payload(&output);
    let invocation_id = stdout_payload["id"].as_str().unwrap();
    let raw_payload = raw_result_payload(&fixture, invocation_id);
    assert_eq!(key_set(&stdout_payload), expected_failure_key_set());
    assert_eq!(key_set(&raw_payload), expected_failure_key_set());
    assert_eq!(key_set(&stdout_payload), key_set(&raw_payload));
    for key in [
        "agent_runner_invocation_id",
        "provider_name",
        "provider_session_id",
        "agent_runner_chain_id",
    ] {
        assert_eq!(
            stdout_payload[key].is_null(),
            raw_payload[key].is_null(),
            "{key} nullability must match between stdout and raw result"
        );
        if !stdout_payload[key].is_null() {
            assert_eq!(stdout_payload[key], raw_payload[key], "{key} value drifted");
        }
    }
}

#[test]
fn pre_invocation_fast_failures_emit_stdout_only_failure_without_result_id() {
    let provider_selection = Age153Fixture::new();
    provider_selection.write_model(
        "age175-provider-selection",
        &["fixture-age175-selection-a", "fixture-age175-selection-b"],
    );
    provider_selection.write_providers_with_bodies(&[
        (
            "fixture-age175-selection-a",
            "printf 'selection provider a should not run\\n' >&2\nexit 11",
        ),
        (
            "fixture-age175-selection-b",
            "printf 'selection provider b should not run\\n' >&2\nexit 12",
        ),
    ]);
    let unavailable_until = Utc::now() + Duration::hours(1);
    let provider_selection_db = provider_selection.open_db();
    for provider in ["fixture-age175-selection-a", "fixture-age175-selection-b"] {
        provider_selection_db
            .record_provider_unavailable(provider, Some(unavailable_until), "test")
            .unwrap();
    }
    parse_pre_invocation_failure_payload(
        &provider_selection.run_one_shot("age175-provider-selection"),
        "provider_selection",
        Some("age175-provider-selection"),
        None,
        &[],
        true,
    );

    let provider_resolution = Age153Fixture::new();
    provider_resolution.write_model("age175-provider-resolution", &["fixture-age175-missing"]);
    fs::write(
        provider_resolution.app_config_dir.join("providers.toml"),
        "",
    )
    .unwrap();
    parse_pre_invocation_failure_payload(
        &provider_resolution.run_one_shot("age175-provider-resolution"),
        "provider_resolution",
        Some("age175-provider-resolution"),
        Some(0),
        &["fixture-age175-missing"],
        true,
    );

    let pool_exhausted = Age153Fixture::new();
    let first_marker = pool_exhausted.dir.path().join("age175-pool-a.txt");
    let second_marker = pool_exhausted.dir.path().join("age175-pool-b.txt");
    pool_exhausted.write_model(
        "age175-pool",
        &["fixture-age175-pool-a", "fixture-age175-pool-b"],
    );
    pool_exhausted.write_providers_with_bodies(&[
        (
            "fixture-age175-pool-a",
            &format!(
                "printf '%s\\n' ran >> {}\nprintf '%s\\n' 'quota exhausted for fixture a' >&2\nexit 42",
                toml_string(&first_marker.display().to_string())
            ),
        ),
        (
            "fixture-age175-pool-b",
            &format!(
                "printf '%s\\n' ran >> {}\nprintf '%s\\n' 'quota exhausted for fixture b' >&2\nexit 43",
                toml_string(&second_marker.display().to_string())
            ),
        ),
    ]);
    parse_pre_invocation_failure_payload(
        &pool_exhausted.run_one_shot_with_env(
            "age175-pool",
            &[(
                FORCE_TERMINAL_SIGNAL_KIND,
                "QuotaExhaustedInband,QuotaExhaustedInband",
            )],
        ),
        "pool_exhausted",
        Some("age175-pool"),
        None,
        &["fixture-age175-pool-a", "fixture-age175-pool-b"],
        true,
    );
}

#[test]
fn unknown_returned_artifact_persistence_failure_settles_category_before_diagnostic_emission() {
    let receipt = bad_return_receipt();
    let output = run_unknown_fixture(
        &format!(
            r#"printf 'unclassified provider failure before returned artifact persistence\n' >&2
printf '%s\n' '{receipt}' >> "${{OULIPOLY_RETURN_CHANNEL:?missing}}"
exit 7"#
        ),
        "returned-artifact-ordering",
    );

    let payload = result_payload(&output);
    assert_eq!(payload["error_category"], "returned_artifacts");
    assert_eq!(
        payload["terminal_reason"],
        "returned_artifacts_persist_failed"
    );

    let stderr = stderr_text(&output);
    let lines = unknown_diagnostic_lines(&stderr);
    assert!(
        lines.is_empty(),
        "unknown diagnostic must not be emitted when the final result category is returned_artifacts:\n{stderr}"
    );
}

#[test]
fn unknown_failure_emits_structured_unknown_diagnostic_with_redacted_excerpt() {
    let c4_secret_sentinels = [
        ("authorization bearer header", "sent-auth-bearer"),
        ("authorization basic header", "sent-auth-basic"),
        ("authorization token header", "sent-auth-token"),
        ("scheme-less authorization header", "sent-auth-raw"),
        ("bare bearer token", "sent-bare-bearer"),
        ("token equals", "sent-token-eq"),
        ("api_key equals", "sent-api-key-eq"),
        ("apikey equals", "sent-apikey-eq"),
        ("password equals", "sent-password-eq"),
        ("secret equals", "sent-secret-eq"),
        ("token colon", "sent-token-colon"),
        ("api_key colon", "sent-api-key-colon"),
        ("apikey colon", "sent-apikey-colon"),
        ("password colon", "sent-password-colon"),
        ("secret colon", "sent-secret-colon"),
        ("json api_key", "sent-api-key-json"),
        ("json password", "sent-password-json"),
        ("json secret", "sent-secret-json"),
        ("json apikey", "sent-apikey-json"),
        ("json token", "sent-token-json"),
    ];
    let (_output, payload) = run_unknown_diagnostic_fixture(
        r#"printf 'Authorization: Bearer sent-auth-bearer Authorization: Basic sent-auth-basic Authorization: token sent-auth-token Authorization: sent-auth-raw\n' >&2
	printf 'Bearer sent-bare-bearer token=sent-token-eq api_key=sent-api-key-eq apikey=sent-apikey-eq password=sent-password-eq secret=sent-secret-eq\n' >&2
	printf 'token: sent-token-colon api_key: sent-api-key-colon apikey: sent-apikey-colon password: sent-password-colon secret: sent-secret-colon\n' >&2
	printf '"api_key": "sent-api-key-json" "password": "sent-password-json" "secret": "sent-secret-json" "apikey": "sent-apikey-json" "token": "sent-token-json"\n' >&2
	printf 'fifth line should not be included\n' >&2
	exit 7"#,
        "redaction",
    );
    assert_unknown_diagnostic_schema(&payload, "fixture-age175-unknown-redaction", 7);
    let excerpt = payload["stderr_excerpt"].as_str().unwrap();
    assert_excerpt_caps(excerpt);
    for (form, secret) in c4_secret_sentinels {
        assert!(
            !excerpt.contains(secret),
            "stderr_excerpt leaked {form} sentinel {secret}: {excerpt}"
        );
    }
    assert!(
        excerpt.matches("[REDACTED]").count() >= c4_secret_sentinels.len(),
        "stderr_excerpt must replace every C4 redaction form with [REDACTED]: {excerpt}"
    );
    assert!(
        !excerpt.contains("fifth line should not be included"),
        "excerpt must be capped to the first four non-empty lines: {excerpt}"
    );

    let long_prefix = "x".repeat(950);
    let long_suffix = "y".repeat(200);
    let long_secret = "long-secret-token-before-truncation";
    let (_output, long_payload) = run_unknown_diagnostic_fixture(
        &format!(
            r#"printf 'first diagnostic line\n' >&2
printf 'second diagnostic line\n' >&2
printf '{} token={} {}\n' >&2
printf 'fourth diagnostic line\n' >&2
printf 'fifth line should not be included\n' >&2
exit 7"#,
            long_prefix, long_secret, long_suffix
        ),
        "long-redaction",
    );
    assert_unknown_diagnostic_schema(&long_payload, "fixture-age175-unknown-long-redaction", 7);
    let long_excerpt = long_payload["stderr_excerpt"].as_str().unwrap();
    assert_excerpt_caps(long_excerpt);
    assert!(
        long_excerpt.contains("[REDACTED]"),
        "secret-bearing line survives into the capped excerpt and must be redacted before truncation: {long_excerpt}"
    );
    assert!(
        !long_excerpt.contains(long_secret),
        "redact-before-truncate case leaked {long_secret}: {long_excerpt}"
    );
    assert!(
        !long_excerpt.contains("long-secret-token"),
        "redact-before-truncate case leaked a truncated secret prefix: {long_excerpt}"
    );
    assert!(
        !long_excerpt.contains("fifth line should not be included"),
        "long excerpt must still be capped to the first four non-empty lines: {long_excerpt}"
    );
}

#[test]
fn failure_identity_chain_id_uses_existing_segment_and_null_when_no_segment_exists() {
    let chain_fixture = Age153Fixture::new();
    let provider_name = "fixture-age175-chain";
    let marker = chain_fixture.dir.path().join("age175-chain-sessions.txt");
    let transcript = chain_fixture
        .dir
        .path()
        .join("age175-chain-transcript.jsonl");
    let counter = chain_fixture.dir.path().join("age175-chain-count.txt");
    fs::write(&transcript, "").unwrap();
    let command = chain_fixture.write_script(
        "age175-chain-provider.sh",
        &zero_turn_then_nonzero_forced_capture_body(&marker, &transcript, &counter),
    );
    write_forced_capture_provider_with_command(
        &chain_fixture,
        "age175-chain",
        provider_name,
        &command,
    );
    write_sessions_toml(&chain_fixture, provider_name, &transcript);

    let output = chain_fixture.run_one_shot_with_env(
        "age175-chain",
        &[(FORCE_TERMINAL_SIGNAL_KIND, "MaybeQuotaExhausted,None")],
    );

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    let payload = result_payload(&output);
    let provider_session_id = payload["provider_session_id"]
        .as_str()
        .expect("second attempt must carry the start-known provider session");
    let db = chain_fixture.open_db();
    let expected_chain = db
        .chain_id_for_segment(provider_name, provider_session_id)
        .unwrap()
        .expect("start-known provider session should already have an existing chain segment");
    assert_failure_identity(
        &payload,
        provider_name,
        Some(provider_session_id),
        Some(&expected_chain),
    );

    let absent_fixture = Age153Fixture::new();
    let absent_provider = "fixture-age175-chain-absent";
    write_stdout_json_capture_provider(
        &absent_fixture,
        "age175-chain-absent",
        absent_provider,
        &stdout_json_session_body(FIXED_SESSION_ID, 11),
    );
    let absent_output = absent_fixture.run_one_shot("age175-chain-absent");
    assert_ne!(absent_output.status.code(), Some(0), "{absent_output:?}");
    let absent_payload = result_payload(&absent_output);
    assert_failure_identity(
        &absent_payload,
        absent_provider,
        Some(FIXED_SESSION_ID),
        None,
    );
}
