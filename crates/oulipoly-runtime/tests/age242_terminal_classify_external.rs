//! AGE-242 S6c external-provider terminal classify coverage.
//!
//! Role: orchestration, formatter, mapper, accessor, parser, validator.

#![cfg(unix)]

use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, provider_implementation_ref::ProviderImplementationRef,
};
use oulipoly_runtime::executor;
use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;
use oulipoly_runtime::provider_registry::{
    ProviderRegistry, ProviderRegistryHandle, ProviderRegistryOptions,
};
use oulipoly_runtime::services::{ExecutorServicePort, ExecutorServiceRequest};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

const MODEL_NAME: &str = "neutral-external-terminal-model";
const PROVIDER_NAME: &str = "neutral-external-provider";
static TEST_DATA_DIR: OnceLock<tempfile::TempDir> = OnceLock::new();

struct ScriptFixture {
    _dir: tempfile::TempDir,
    path: PathBuf,
    terminal_request_record: Option<PathBuf>,
}

struct FakeProviderOptions {
    terminal_mode: &'static str,
    terminal_capability: bool,
    record_terminal_request: bool,
}

#[derive(Clone, Copy)]
struct ExpectedTerminal {
    kind: TerminalSignalKind,
    reason: Option<&'static str>,
    exit_code: i32,
}

fn fake_provider(options: FakeProviderOptions) -> ScriptFixture {
    TEST_DATA_DIR.get_or_init(|| {
        let dir = tempfile::tempdir().expect("test data dir");
        unsafe {
            std::env::set_var(oulipoly_state::paths::DATA_DIR_ENV, dir.path());
        }
        dir
    });
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("neutral-provider.sh");
    let record = options
        .record_terminal_request
        .then(|| dir.path().join("terminal-request.json"));
    write_executable_script(&path, &fake_provider_script(&options, record.as_deref()));
    ScriptFixture {
        _dir: dir,
        path,
        terminal_request_record: record,
    }
}

fn fake_provider_script(options: &FakeProviderOptions, record: Option<&Path>) -> String {
    let terminal_capability = options.terminal_capability;
    let terminal_mode = options.terminal_mode;
    let record_path = record.map(shell_quote).unwrap_or_else(|| "''".to_string());
    format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
subcommand="${{1:-}}"
request="$(cat)"
request_id="$(printf '%s' "$request" | sed -n 's/.*"request_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
if [ -z "$request_id" ]; then
  request_id="request-fallback"
fi
terminal_capability="{terminal_capability}"
terminal_mode="{terminal_mode}"
record_path={record_path}

success_envelope() {{
  printf '{{"contract":"oulipoly.provider/v1","request_id":"%s","ok":true,"result":%s}}\n' "$request_id" "$1"
}}

case "$subcommand" in
  describe)
    success_envelope '{{"provider_id":"neutral-provider","display_name":"Neutral Provider","contract_versions":["oulipoly.provider/v1"],"preferred_contract":"oulipoly.provider/v1","capabilities":{{"launch":true,"launch_output_v1":true,"policy":true,"quota":false,"session":false,"terminal":'"$terminal_capability"',"rotation":false,"discovery":false,"settings":false,"setup_brain":false,"setup":false,"migration":false}},"concurrency":{{"safe_for_parallel_invocation":true,"state_locking":"none"}}}}'
    ;;
  policy.evaluate)
    success_envelope '{{"accepted":true,"stdin":null,"prompt":null,"diagnostics":[],"markers":[]}}'
    ;;
  launch)
    printf '{{"contract":"oulipoly.provider/v1","request_id":"%s","seq":1,"time_unix_ms":1001,"kind":"stdout","data_base64":"cmF3AP9a"}}\n' "$request_id"
    printf '{{"contract":"oulipoly.provider/v1","request_id":"%s","seq":2,"time_unix_ms":1002,"kind":"stderr","data_base64":"ZXJy"}}\n' "$request_id"
    printf '{{"contract":"oulipoly.provider/v1","request_id":"%s","seq":3,"time_unix_ms":1003,"kind":"marker","name":"oulipoly.launch_output_complete/v1","value":{{"protocol":"oulipoly.launch_output/v1","stdout":{{"bytes":6,"sha256":"007e39f0ae9498f1f2ac715977240ba901385a4cb20f8fc9d7865c5fd5b62292"}},"stderr":{{"bytes":3,"sha256":"d9eb253e06987fa74a5d3189f73d9f7a8104cca786fafbb52bc9555972f5477f"}},"data_event_count":2}}}}\n' "$request_id"
    if [ "$terminal_mode" = "cancelled" ]; then
      printf '{{"contract":"oulipoly.provider/v1","request_id":"%s","seq":4,"time_unix_ms":1004,"kind":"exit","status":{{"kind":"cancelled"}},"terminal_signal":{{"kind":"cancelled","evidence":"launch-cancelled","observed_at_unix_ms":1004}}}}\n' "$request_id"
    else
      printf '{{"contract":"oulipoly.provider/v1","request_id":"%s","seq":4,"time_unix_ms":1004,"kind":"exit","status":{{"kind":"exited","code":23}},"terminal_signal":{{"kind":"nonzero_exit","evidence":"launch-nonzero","observed_at_unix_ms":1004}}}}\n' "$request_id"
    fi
    ;;
  terminal.classify)
    if [ -n "$record_path" ]; then
      printf '%s' "$request" > "$record_path"
    fi
    if [ "$terminal_mode" = "classify-failure" ]; then
      exit 7
    fi
    success_envelope '{{"terminal_signal":{{"kind":"'"$terminal_mode"'","evidence":"terminal-classify-'"$terminal_mode"'","observed_at_unix_ms":2005}}}}'
    ;;
  *)
    printf 'unexpected subcommand %s\n' "$subcommand" >&2
    exit 64
    ;;
esac
"#
    )
}

fn write_executable_script(path: &Path, contents: &str) {
    fs::write(path, contents).expect("write script");
    let mut permissions = fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("chmod");
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn provider_ref_path(path: &Path) -> ProviderImplementationRef {
    ProviderImplementationRef {
        path: Some(path.display().to_string()),
        crate_name: None,
        version: None,
        binary: None,
        script: None,
    }
}

fn external_model(script: &ScriptFixture) -> ModelConfig {
    ModelConfig {
        name: MODEL_NAME.to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig {
            environment: Default::default(),
            unset_environment: Default::default(),
            name: PROVIDER_NAME.to_string(),
            command: "neutral-provider-command".to_string(),
            args: Vec::new(),
            interactive_args: None,
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        }],
        inputs: Vec::new(),
        provider: Some(provider_ref_path(&script.path)),
    }
}

fn execution_request(model: ModelConfig) -> ExecutorServiceRequest {
    ExecutorServiceRequest::Facade {
        model,
        provider_index: 0,
        prompt: "prompt-value".to_string(),
        working_dir: None,
        models_dir: None,
        extra_inputs: HashMap::new(),
        parent_invocation_env: None,
    }
}

fn provider_registry(model: &ModelConfig) -> ProviderRegistry {
    ProviderRegistry::from_model_configs(
        std::slice::from_ref(model),
        ProviderRegistryOptions::default(),
    )
    .expect("provider registry")
}

fn execute_with_provider(script: &ScriptFixture) -> executor::ExecutionResult {
    let model = external_model(script);
    let registry = provider_registry(&model);
    let handle = ProviderRegistryHandle::new(Arc::new(registry));
    executor::RuntimeExecutorService::with_registry_handle(handle)
        .execute(execution_request(model))
        .expect("external provider execution")
        .result
}

fn assert_terminal(result: &executor::ExecutionResult, expected: ExpectedTerminal) {
    let signal = result
        .terminal_signal
        .as_ref()
        .expect("terminal signal should be present");
    assert_eq!(signal.kind, expected.kind);
    assert_eq!(result.terminal_reason.as_deref(), expected.reason);
    assert_eq!(result.exit_code, expected.exit_code);
}

fn recorded_terminal_request(script: &ScriptFixture) -> Value {
    parse_terminal_request_json(&recorded_terminal_request_text(script))
}

fn recorded_terminal_request_text(script: &ScriptFixture) -> String {
    let path = script
        .terminal_request_record
        .as_ref()
        .expect("record path should be configured");
    fs::read_to_string(path).expect("terminal classify request should be recorded")
}

fn parse_terminal_request_json(text: &str) -> Value {
    serde_json::from_str(text).expect("terminal classify request should be JSON")
}

fn assert_recorded_bytes(request: &Value) {
    assert_eq!(request["params"]["stdout_base64"], "cmF3AP9a");
    assert_eq!(request["params"]["stderr_base64"], "ZXJy");
}

fn assert_terminal_classify_not_invoked(script: &ScriptFixture) {
    assert!(
        !script
            .terminal_request_record
            .as_ref()
            .expect("record path")
            .exists(),
        "missing terminal capability should not invoke terminal.classify"
    );
}

fn terminal_mode_expectations() -> Vec<(&'static str, ExpectedTerminal)> {
    vec![
        (
            "quota_exhausted_inband",
            ExpectedTerminal {
                kind: TerminalSignalKind::QuotaExhaustedInband,
                reason: Some("quota_exhausted_inband"),
                exit_code: 23,
            },
        ),
        (
            "maybe_quota_exhausted",
            ExpectedTerminal {
                kind: TerminalSignalKind::MaybeQuotaExhausted,
                reason: Some("maybe_quota_exhausted"),
                exit_code: 23,
            },
        ),
        (
            "rate_limited",
            ExpectedTerminal {
                kind: TerminalSignalKind::RateLimited,
                reason: Some("rate_limited"),
                exit_code: 23,
            },
        ),
        (
            "cancelled",
            ExpectedTerminal {
                kind: TerminalSignalKind::Unknown,
                reason: Some("cancelled"),
                exit_code: 130,
            },
        ),
    ]
}

fn terminal_mode_provider(terminal_mode: &'static str) -> ScriptFixture {
    fake_provider(FakeProviderOptions {
        terminal_mode,
        terminal_capability: true,
        record_terminal_request: false,
    })
}

fn classify_failure_provider() -> ScriptFixture {
    fake_provider(FakeProviderOptions {
        terminal_mode: "classify-failure",
        terminal_capability: true,
        record_terminal_request: false,
    })
}

fn missing_capability_provider() -> ScriptFixture {
    fake_provider(FakeProviderOptions {
        terminal_mode: "quota_exhausted_inband",
        terminal_capability: false,
        record_terminal_request: true,
    })
}

fn recording_quota_provider() -> ScriptFixture {
    fake_provider(FakeProviderOptions {
        terminal_mode: "quota_exhausted_inband",
        terminal_capability: true,
        record_terminal_request: true,
    })
}

fn s6a_nonzero_expected() -> ExpectedTerminal {
    ExpectedTerminal {
        kind: TerminalSignalKind::NonzeroExit,
        reason: Some("exit_nonzero"),
        exit_code: 23,
    }
}

fn quota_expected() -> ExpectedTerminal {
    ExpectedTerminal {
        kind: TerminalSignalKind::QuotaExhaustedInband,
        reason: Some("quota_exhausted_inband"),
        exit_code: 23,
    }
}

#[test]
fn external_terminal_classify_maps_quota_maybe_rate_and_cancelled_modes() {
    for (terminal_mode, expected) in terminal_mode_expectations() {
        let script = terminal_mode_provider(terminal_mode);
        assert_terminal(&execute_with_provider(&script), expected);
    }
}

#[test]
fn terminal_classify_failure_after_launch_success_falls_back_to_s6a_mapping() {
    let script = classify_failure_provider();
    assert_terminal(&execute_with_provider(&script), s6a_nonzero_expected());
}

#[test]
fn missing_terminal_capability_after_launch_success_falls_back_to_s6a_mapping() {
    let script = missing_capability_provider();
    assert_terminal(&execute_with_provider(&script), s6a_nonzero_expected());
    assert_terminal_classify_not_invoked(&script);
}

#[test]
fn terminal_classify_request_preserves_raw_stdout_and_stderr_bytes() {
    let script = recording_quota_provider();
    let result = execute_with_provider(&script);
    assert_terminal(&result, quota_expected());
    assert_recorded_bytes(&recorded_terminal_request(&script));
}
