#![cfg(unix)]

//! Declared roles: formatter, parser, validator, orchestration, filter.

mod age153_support;
mod provider_authority_fixture;

use age153_support::{Age153Fixture, FORCE_TERMINAL_SIGNAL_KIND, toml_string};
use chrono::{Duration, Utc};
use serde_json::Value;
use std::fs;
use std::process::Output;

const FINISHED_AT_PLACEHOLDER: &str = "<FINISHED_AT>";
const INVOCATION_ID_PLACEHOLDER: &str = "<INVOCATION_ID>";

const PROVIDER_SELECTION_FAILURE_GOLDEN: &str = "OULIPOLY_FAILURE={\"agent_runner_chain_id\":null,\"agent_runner_invocation_id\":null,\"detail\":{\"attempted_providers\":[],\"model_name\":\"age175-provider-selection\",\"provider_index\":null,\"reason\":\"all providers in pool age175-provider-selection are quota-exhausted: fixture-age175-selection-a, fixture-age175-selection-b\"},\"error_category\":null,\"exit_code\":null,\"failure_kind\":\"pre_invocation\",\"finished_at\":\"<FINISHED_AT>\",\"message\":\"provider_selection: all providers in pool age175-provider-selection are quota-exhausted: fixture-age175-selection-a, fixture-age175-selection-b\",\"provider_name\":null,\"provider_session_id\":null,\"stage\":\"provider_selection\",\"status\":\"failed\",\"success\":false,\"terminal_reason\":\"pre_invocation_failure\"}";
const PROVIDER_RESOLUTION_FAILURE_GOLDEN: &str = "OULIPOLY_FAILURE={\"agent_runner_chain_id\":null,\"agent_runner_invocation_id\":null,\"detail\":{\"attempted_providers\":[\"fixture-age175-missing\"],\"model_name\":\"age175-provider-resolution\",\"provider_index\":0,\"reason\":\"provider fixture-age175-missing is missing from providers.toml\"},\"error_category\":null,\"exit_code\":null,\"failure_kind\":\"pre_invocation\",\"finished_at\":\"<FINISHED_AT>\",\"message\":\"provider_resolution: provider fixture-age175-missing is missing from providers.toml\",\"provider_name\":null,\"provider_session_id\":null,\"stage\":\"provider_resolution\",\"status\":\"failed\",\"success\":false,\"terminal_reason\":\"pre_invocation_failure\"}";
const POOL_EXHAUSTED_FAILURE_GOLDEN: &str = "OULIPOLY_FAILURE={\"agent_runner_chain_id\":null,\"agent_runner_invocation_id\":null,\"detail\":{\"attempted_providers\":[\"fixture-age175-pool-a\",\"fixture-age175-pool-b\"],\"model_name\":\"age175-pool\",\"provider_index\":null,\"reason\":\"all providers in pool age175-pool are quota-exhausted: fixture-age175-pool-a, fixture-age175-pool-b\"},\"error_category\":null,\"exit_code\":null,\"failure_kind\":\"pre_invocation\",\"finished_at\":\"<FINISHED_AT>\",\"message\":\"pool_exhausted: all providers in pool age175-pool are quota-exhausted: fixture-age175-pool-a, fixture-age175-pool-b\",\"provider_name\":null,\"provider_session_id\":null,\"stage\":\"pool_exhausted\",\"status\":\"failed\",\"success\":false,\"terminal_reason\":\"pre_invocation_failure\"}";
const SUCCESS_RESULT_GOLDEN: &str = "OULIPOLY_RESULT={\"error_category\":null,\"exit_code\":0,\"finished_at\":\"<FINISHED_AT>\",\"id\":\"<INVOCATION_ID>\",\"status\":\"succeeded\",\"success\":true,\"terminal_reason\":null}";
const SUCCESS_SESSION_GOLDEN: &str = "OULIPOLY_SESSION={\"agent_runner_chain_id\":\"<CHAIN_ID>\",\"agent_runner_invocation_id\":\"<INVOCATION_ID>\",\"id\":\"<INVOCATION_ID>\",\"provider_name\":\"fixture-age182-success\",\"provider_session_id\":\"<SESSION_ID>\",\"resume_input_id\":null,\"session_id\":\"<SESSION_ID>\"}";

#[test]
fn pre_invocation_failure_marker_bytes_are_characterized() {
    let output = provider_selection_failure_output();
    assert_failure_exit(&output);
    assert_eq!(
        normalized_failure_line(&output),
        PROVIDER_SELECTION_FAILURE_GOLDEN
    );

    let output = provider_resolution_failure_output();
    assert_failure_exit(&output);
    assert_eq!(
        normalized_failure_line(&output),
        PROVIDER_RESOLUTION_FAILURE_GOLDEN
    );

    let output = pool_exhausted_failure_output();
    assert_failure_exit(&output);
    assert_eq!(
        normalized_failure_line(&output),
        POOL_EXHAUSTED_FAILURE_GOLDEN
    );
}

#[test]
fn success_marker_bytes_are_characterized() {
    let output = success_with_session_output();

    assert_success_exit(&output);
    assert_eq!(normalized_result_line(&output), SUCCESS_RESULT_GOLDEN);
    assert_eq!(normalized_session_line(&output), SUCCESS_SESSION_GOLDEN);
}

fn provider_selection_failure_output() -> Output {
    let fixture = Age153Fixture::new();
    fixture.write_model(
        "age175-provider-selection",
        &["fixture-age175-selection-a", "fixture-age175-selection-b"],
    );
    fixture.write_providers_with_bodies(&[
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
    let provider_selection_db = fixture.open_db();
    for provider in ["fixture-age175-selection-a", "fixture-age175-selection-b"] {
        provider_selection_db
            .record_provider_unavailable(provider, Some(unavailable_until), "test")
            .unwrap();
    }
    fixture.run_one_shot("age175-provider-selection")
}

fn provider_resolution_failure_output() -> Output {
    let fixture = Age153Fixture::new();
    fixture.write_model("age175-provider-resolution", &["fixture-age175-missing"]);
    fs::write(fixture.app_config_dir.join("providers.toml"), "").unwrap();
    fixture.run_one_shot("age175-provider-resolution")
}

fn pool_exhausted_failure_output() -> Output {
    let fixture = Age153Fixture::new();
    let first_marker = fixture.dir.path().join("age175-pool-a.txt");
    let second_marker = fixture.dir.path().join("age175-pool-b.txt");
    fixture.write_model(
        "age175-pool",
        &["fixture-age175-pool-a", "fixture-age175-pool-b"],
    );
    fixture.write_providers_with_bodies(&[
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
    fixture.run_one_shot_with_env(
        "age175-pool",
        &[(
            FORCE_TERMINAL_SIGNAL_KIND,
            "QuotaExhaustedInband,QuotaExhaustedInband",
        )],
    )
}

fn success_with_session_output() -> Output {
    let fixture = Age153Fixture::new();
    let command = fixture.write_script(
        "fixture-age182-success-command.sh",
        r#"requested=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --session-id) requested="$2"; shift 2 ;;
    *) shift ;;
  esac
done
printf '{"type":"system","subtype":"init","session_id":"%s"}\n' "$requested"
"#,
    );
    fixture.write_model("age182-success", &["fixture-age182-success"]);
    fs::write(
        fixture.app_config_dir.join("providers.toml"),
        provider_authority_fixture::with_explicit_provider_authority(&format!(
            r#"[fixture-age182-success]
command = {}
args = []
interactive_args = ["interactive"]
prompt_mode = "arg"

[fixture-age182-success.session_capture]
kind = "forced_flag_verified"
flag = "--session-id"

"#,
            toml_string(&command.display().to_string())
        )),
    )
    .unwrap();
    fixture.run_one_shot("age182-success")
}

fn normalized_failure_line(output: &Output) -> String {
    normalize_line(
        &single_prefixed_line(&output.stdout, "OULIPOLY_FAILURE="),
        |value| {
            value["finished_at"] = Value::String(FINISHED_AT_PLACEHOLDER.to_string());
        },
    )
}

fn normalized_result_line(output: &Output) -> String {
    normalize_line(
        &single_prefixed_line(&output.stdout, "OULIPOLY_RESULT="),
        |value| {
            value["finished_at"] = Value::String(FINISHED_AT_PLACEHOLDER.to_string());
            value["id"] = Value::String(INVOCATION_ID_PLACEHOLDER.to_string());
        },
    )
}

fn normalized_session_line(output: &Output) -> String {
    normalize_line(
        &single_prefixed_line(&output.stderr, "OULIPOLY_SESSION="),
        |value| {
            value["id"] = Value::String(INVOCATION_ID_PLACEHOLDER.to_string());
            value["session_id"] = Value::String("<SESSION_ID>".to_string());
            value["agent_runner_invocation_id"] =
                Value::String(INVOCATION_ID_PLACEHOLDER.to_string());
            value["provider_session_id"] = Value::String("<SESSION_ID>".to_string());
            value["agent_runner_chain_id"] = Value::String("<CHAIN_ID>".to_string());
        },
    )
}

fn assert_failure_exit(output: &Output) {
    assert_ne!(output.status.code(), Some(0), "{output:?}");
}

fn assert_success_exit(output: &Output) {
    assert_eq!(output.status.code(), Some(0), "{output:?}");
}

fn single_prefixed_line(stream: &[u8], prefix: &str) -> String {
    let text = String::from_utf8_lossy(stream);
    let lines = prefixed_lines(&text, prefix);
    assert_eq!(
        lines.len(),
        1,
        "expected exactly one {prefix} marker line:\n{text}"
    );
    lines[0].to_string()
}

fn prefixed_lines<'a>(text: &'a str, prefix: &str) -> Vec<&'a str> {
    text.lines()
        .filter(|line| line.starts_with(prefix))
        .collect::<Vec<_>>()
}

fn normalize_line(line: &str, normalize: impl FnOnce(&mut Value)) -> String {
    let (prefix, mut value) = split_marker_line(line);
    normalize(&mut value);
    render_marker_line(&prefix, &value)
}

fn split_marker_line(line: &str) -> (String, serde_json::Value) {
    let (prefix, raw) = line.split_once('=').expect("marker line contains equals");
    let value = serde_json::from_str(raw).unwrap();
    (prefix.to_string(), value)
}

fn render_marker_line(prefix: &str, value: &serde_json::Value) -> String {
    format!("{prefix}={}", serde_json::to_string(value).unwrap())
}
