#![cfg(unix)]

//! ## Declared roles
//!
//! `orchestration`, `parser`, `filter`, `validator`, `formatter`, `mapper`, `accessor`
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/tests/age181_prompt_resolution_failure.rs::prompt_resolution_failure_marker_tests
//!     role: intrinsic-surface
//!     Domain: prompt_resolution_failure_marker_tests
//!     Owns:
//!       - AGE-181 direct-model prompt-resolution marker tests
//!       - local strict marker parser and assertions
//!       - stdout marker line counting and absence filters
//!       - no-OULIPOLY_RESULT assertions
//!       - null identity-field assertions
//!       - exact four-key detail assertions
//!       - representative prompt-resolution failure fixtures
//!       - subordinate fixture setup for isolated config/data homes
//! ```

use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const MODEL: &str = "age181-model";
const PROVIDER: &str = "age181-provider";
const PROVIDER_TEXT: &str = "AGE181_PROVIDER_EXECUTED";
const FAILURE_PREFIX: &str = "OULIPOLY_FAILURE=";
const RESULT_PREFIX: &str = "OULIPOLY_RESULT=";
const UNIX_NOT_FOUND_OS_ERROR: i32 = 2;

struct Age181Fixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
    provider_marker: PathBuf,
}

struct FixtureFiles {
    provider_script: PathBuf,
    model_file: PathBuf,
    providers_file: PathBuf,
}

impl Age181Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let fixture = fixture_from_dir(dir);
        fixture.prepare();
        fixture
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env("HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd
    }

    fn direct_model_command(&self) -> Command {
        let mut cmd = self.command();
        cmd.arg("--models-dir")
            .arg(&self.models_dir)
            .arg("--model")
            .arg(MODEL);
        cmd
    }

    fn run_with_args(&self, args: &[&str]) -> Output {
        let mut cmd = self.direct_model_command();
        cmd.args(args);
        cmd.output().unwrap()
    }

    fn run_with_stdin(&self, stdin: &[u8]) -> Output {
        let mut cmd = self.direct_model_command();
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        child.stdin.as_mut().unwrap().write_all(stdin).unwrap();
        child.wait_with_output().unwrap()
    }

    fn missing_path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn provider_script_path(&self) -> PathBuf {
        self.dir.path().join("age181-provider.sh")
    }

    fn assert_provider_not_executed(&self, output: &Output) {
        assert_provider_marker_absent(provider_marker_exists(&self.provider_marker), self);
        assert_provider_text_absent(output);
    }

    fn write_model_and_provider(&self) {
        let files = self.fixture_files();
        write_executable(
            &files.provider_script,
            &provider_script_body(&self.provider_marker),
        );
        fs::write(files.model_file, model_toml()).unwrap();
        fs::write(files.providers_file, providers_toml(&files.provider_script)).unwrap();
    }

    fn prepare(&self) {
        fs::create_dir_all(&self.models_dir).unwrap();
        self.write_model_and_provider();
    }

    fn fixture_files(&self) -> FixtureFiles {
        FixtureFiles {
            provider_script: self.provider_script_path(),
            model_file: self.models_dir.join(format!("{MODEL}.toml")),
            providers_file: self.app_config_dir.join("providers.toml"),
        }
    }
}

fn fixture_from_dir(dir: tempfile::TempDir) -> Age181Fixture {
    let config_home = dir.path().join("config");
    let data_home = dir.path().join("data");
    let app_config_dir = config_home.join("oulipoly-agent-runner");
    let models_dir = app_config_dir.join("models");
    let provider_marker = dir.path().join("age181-provider-executed.txt");
    Age181Fixture {
        dir,
        config_home,
        data_home,
        app_config_dir,
        models_dir,
        provider_marker,
    }
}

fn provider_script_body(provider_marker: &Path) -> String {
    format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
printf '{PROVIDER_TEXT}\n'
printf '{PROVIDER_TEXT}\n' >&2
printf 'executed\n' > "{}"
"#,
        provider_marker.display()
    )
}

fn model_toml() -> String {
    format!(
        r#"[[providers]]
name = "{PROVIDER}"
args = []
interactive_args = []
"#
    )
}

fn providers_toml(provider_script: &Path) -> String {
    format!(
        r#"[{PROVIDER}]
command = "{}"
args = []
interactive_args = []
prompt_mode = "arg"
"#,
        provider_script.display()
    )
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn provider_marker_exists(provider_marker: &Path) -> bool {
    provider_marker.exists()
}

fn assert_provider_marker_absent(marker_exists: bool, fixture: &Age181Fixture) {
    assert!(
        !marker_exists,
        "provider marker should not exist at {}",
        fixture.provider_marker.display()
    );
}

fn assert_provider_text_absent(output: &Output) {
    assert_no_provider_text("stdout", &stdout_text(output));
    assert_no_provider_text("stderr", &stderr_text(output));
}

fn assert_no_provider_text(stream_name: &str, text: &str) {
    assert!(
        !contains_provider_text(text),
        "provider text leaked to {stream_name}:\n{text}"
    );
}

fn contains_provider_text(text: &str) -> bool {
    text.contains(PROVIDER_TEXT)
}

fn stdout_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn prefixed_lines<'a>(text: &'a str, prefix: &str) -> Vec<&'a str> {
    text.lines()
        .filter(|line| line.starts_with(prefix))
        .collect()
}

fn failure_lines(stdout: &str) -> Vec<&str> {
    prefixed_lines(stdout, FAILURE_PREFIX)
}

fn result_lines(stdout: &str) -> Vec<&str> {
    prefixed_lines(stdout, RESULT_PREFIX)
}

fn payload_object(value: &Value) -> &serde_json::Map<String, Value> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("{}", expected_json_object_message(value)))
}

fn expected_json_object_message(value: &Value) -> String {
    format!("expected JSON object, got: {value}")
}

fn object_key_set(object: &serde_json::Map<String, Value>) -> BTreeSet<&str> {
    object.keys().map(String::as_str).collect()
}

fn expected_top_level_keys() -> BTreeSet<&'static str> {
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
}

fn expected_detail_keys() -> BTreeSet<&'static str> {
    BTreeSet::from([
        "attempted_providers",
        "model_name",
        "provider_index",
        "reason",
    ])
}

fn decode_single_failure(output: &Output) -> Value {
    let failure_body = single_failure_body(output);
    parse_failure_payload(&failure_body)
}

fn single_failure_body(output: &Output) -> String {
    let stdout = stdout_text(output);
    let stderr = stderr_text(output);
    let failure_line = single_stdout_failure_line(&stdout);
    assert_no_stderr_failure(&stderr);
    assert_no_stdout_result(&stdout);
    marker_body(failure_line)
}

fn single_stdout_failure_line(stdout: &str) -> &str {
    let failures = collect_stdout_failure_lines(stdout);
    assert_single_failure_line_count(&failures, stdout);
    first_failure_line(&failures)
}

fn collect_stdout_failure_lines(stdout: &str) -> Vec<&str> {
    failure_lines(stdout)
}

fn assert_single_failure_line_count(failures: &[&str], stdout: &str) {
    assert_eq!(
        failures.len(),
        1,
        "expected exactly one stdout OULIPOLY_FAILURE line:\n{stdout}"
    );
}

fn first_failure_line<'stdout>(failures: &[&'stdout str]) -> &'stdout str {
    failures[0]
}

fn assert_no_stderr_failure(stderr: &str) {
    assert!(
        prefixed_lines(stderr, FAILURE_PREFIX).is_empty(),
        "stderr must not contain OULIPOLY_FAILURE:\n{stderr}"
    );
}

fn assert_no_stdout_result(stdout: &str) {
    assert!(
        result_lines(stdout).is_empty(),
        "stdout must not contain OULIPOLY_RESULT:\n{stdout}"
    );
}

fn marker_body(line: &str) -> String {
    line.strip_prefix(FAILURE_PREFIX)
        .expect("failure prefix")
        .to_owned()
}

fn parse_failure_payload(failure_body: &str) -> Value {
    serde_json::from_str(failure_body).expect("invalid OULIPOLY_FAILURE JSON")
}

fn assert_prompt_resolution_failure(output: &Output, expected_reason: &str, stderr_prefix: &str) {
    assert!(
        !output.status.success(),
        "prompt-resolution failure should exit nonzero"
    );

    let payload = decode_single_failure(output);
    assert_eq!(
        object_key_set(payload_object(&payload)),
        expected_top_level_keys(),
        "{payload}"
    );
    assert_eq!(payload["failure_kind"], "pre_invocation");
    assert_eq!(payload["stage"], "prompt_resolution");
    assert_eq!(payload["status"], "failed");
    assert_eq!(payload["success"], false);
    assert_eq!(payload["terminal_reason"], "pre_invocation_failure");
    assert!(payload["exit_code"].is_null(), "{payload}");
    assert!(payload["error_category"].is_null(), "{payload}");
    assert!(
        payload["finished_at"]
            .as_str()
            .is_some_and(is_rfc3339_timestamp),
        "{payload}"
    );
    assert!(
        payload["message"]
            .as_str()
            .is_some_and(|value| value.starts_with("prompt_resolution: ")),
        "{payload}"
    );
    assert!(payload["agent_runner_invocation_id"].is_null(), "{payload}");
    assert!(payload["provider_name"].is_null(), "{payload}");
    assert!(payload["provider_session_id"].is_null(), "{payload}");
    assert!(payload["agent_runner_chain_id"].is_null(), "{payload}");

    let detail = &payload["detail"];
    assert_eq!(
        object_key_set(payload_object(detail)),
        expected_detail_keys(),
        "{payload}"
    );
    assert_eq!(detail["model_name"], MODEL);
    assert!(detail["provider_index"].is_null(), "{payload}");
    assert_eq!(detail["attempted_providers"], Value::Array(Vec::new()));
    assert_eq!(detail["reason"], expected_reason);

    let stderr = stderr_text(output);
    assert!(
        stderr.contains(stderr_prefix),
        "stderr should preserve error prefix {stderr_prefix:?}:\n{stderr}"
    );
}

#[test]
fn missing_file_emits_prompt_resolution_pre_invocation_failure() {
    let fixture = Age181Fixture::new();
    let missing_prompt = fixture.missing_path("missing-prompt.md");
    let output = fixture.run_with_args(&["--file", missing_prompt.to_str().unwrap()]);
    let expected_reason = missing_prompt_file_reason();
    assert_prompt_resolution_failure(&output, &expected_reason, &stderr_error(&expected_reason));
    fixture.assert_provider_not_executed(&output);
}

#[test]
fn whitespace_stdin_emits_prompt_resolution_pre_invocation_failure() {
    let fixture = Age181Fixture::new();
    let output = fixture.run_with_stdin(b" \n\t");

    assert_prompt_resolution_failure(
        &output,
        "Empty prompt from stdin.",
        "Error: Empty prompt from stdin.",
    );
    fixture.assert_provider_not_executed(&output);
}

#[test]
fn missing_agent_file_emits_prompt_resolution_pre_invocation_failure() {
    let fixture = Age181Fixture::new();
    let missing_agent = fixture.missing_path("missing-agent.md");
    let output =
        fixture.run_with_args(&["--agent-file", missing_agent.to_str().unwrap(), "prompt"]);
    let expected_reason = missing_agent_file_reason(&missing_agent);
    assert_prompt_resolution_failure(&output, &expected_reason, &stderr_error(&expected_reason));
    fixture.assert_provider_not_executed(&output);
}

fn missing_agent_file_reason(missing_agent: &Path) -> String {
    format!(
        "Failed to read agent file {}: {}",
        missing_agent.display(),
        not_found_os_error()
    )
}

fn missing_prompt_file_reason() -> String {
    format!("Failed to read prompt file: {}", not_found_os_error())
}

fn not_found_os_error() -> String {
    std::io::Error::from_raw_os_error(UNIX_NOT_FOUND_OS_ERROR).to_string()
}

fn is_rfc3339_timestamp(value: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(value).is_ok()
}

fn stderr_error(reason: &str) -> String {
    format!("Error: {reason}")
}
