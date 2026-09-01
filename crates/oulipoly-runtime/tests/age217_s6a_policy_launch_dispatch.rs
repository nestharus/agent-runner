#![cfg(unix)]

//! ## Declared roles
//!
//! Roles: orchestration, mapper, formatter, parser, accessor, predicate, validator.
//!
//! TEST: policy launch dispatch integration suite — fixture model/provider
//! mappers, script formatters, JSON parsers, env accessors, env-absence
//! predicates, assertion validators, and test orchestration.

use oulipoly_config::{
    InputDef, InputType, ModelConfig, PromptMode, ProviderConfig,
    provider_implementation_ref::ProviderImplementationRef,
};
use oulipoly_core::AutoWakeEnvironmentVariable;
use oulipoly_runtime::executor;
use oulipoly_runtime::executor::cli::{self, EffectiveExecuteRequest};
use oulipoly_runtime::provider_registry::{ProviderRegistry, ProviderRegistryOptions};
use oulipoly_runtime::services::{ExecutorServicePort, ExecutorServiceRequest, ServiceError};
use rusqlite::Connection;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

const SELECTED_PROVIDER_SETTINGS_ID: &str = "provider-a-account";
const GENERIC_PARENT_ENV_VALUE: &str = "unicode-\u{2603}";
const OPENAI_KEY_VALUE: &str = "ambient-openai-secret-for-provider-policy";
const OPENAI_BASE_URL_VALUE: &str = "https://ambient-openai.example.invalid";
const CHILD_CUSTODY_FAULT_ENV: &str = "OULIPOLY_CHILD_CUSTODY_TEST_FAULT";
const CHILD_CUSTODY_READY_FILE_ENV: &str = "OULIPOLY_CHILD_CUSTODY_TEST_READY_FILE";
const CHILD_PID_FILE_ENV: &str = "OULIPOLY_CHILD_PID_FILE";
static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static TEST_DATA_DIR: OnceLock<tempfile::TempDir> = OnceLock::new();

fn auto_wake_environment_names() -> Vec<&'static str> {
    AutoWakeEnvironmentVariable::ALL
        .into_iter()
        .map(AutoWakeEnvironmentVariable::name)
        .collect()
}

fn runner_private_environment_names() -> Vec<&'static str> {
    auto_wake_environment_names()
        .into_iter()
        .chain([
            oulipoly_state::COMPLETION_REGISTRATION_AUTHORITY_ENV,
            "OULIPOLY_PARENT_INVOCATION",
        ])
        .collect()
}

fn assert_test_catalog_extension() {
    assert!(
        AutoWakeEnvironmentVariable::ALL.contains(&AutoWakeEnvironmentVariable::TEST_SENTINEL),
        "external-provider tests must extend the production catalog"
    );
}

struct ScriptFixture {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

struct ExternalFixture {
    _dir: tempfile::TempDir,
    provider_path: PathBuf,
    order_path: PathBuf,
    process_env_record_path: PathBuf,
    policy_record_path: PathBuf,
    launch_record_path: PathBuf,
    legacy_record_path: PathBuf,
    legacy_trap_path: PathBuf,
}

#[derive(Clone, Copy)]
struct Capabilities {
    policy: bool,
    launch: bool,
}

#[derive(Clone, Copy)]
enum PolicyMode {
    Accept,
    Reject,
    Transform,
    PrivateEnvTransform,
    HybridShape,
}

#[derive(Clone, Copy)]
enum LaunchMode {
    Success,
    NonzeroFinal,
    ProviderNonzeroAfterFinal,
    MalformedProtocol,
    MissingFinal,
    InvalidBase64,
    HostTransportFailure,
    ProviderNonzeroNoFinal,
    CancelledFinal,
    UnknownTerminalWithQuotaText,
    HostCancelledBeforeFinal,
    LargeOutput,
}

#[derive(Debug, Default)]
struct DispatchSeamObservation {
    registry_injected: bool,
}

struct EnvScope {
    previous: Vec<(&'static str, Option<OsString>)>,
}

impl EnvScope {
    fn set(vars: &[(&'static str, &str)]) -> Self {
        Self::set_optional(
            &vars
                .iter()
                .map(|(key, value)| (*key, Some(*value)))
                .collect::<Vec<_>>(),
        )
    }

    fn set_optional(vars: &[(&'static str, Option<&str>)]) -> Self {
        let previous = vars
            .iter()
            .map(|(key, _)| (*key, std::env::var_os(key)))
            .collect();
        for (key, value) in vars {
            set_env(key, value.map(std::ffi::OsStr::new));
        }
        Self { previous }
    }
}

impl Drop for EnvScope {
    fn drop(&mut self) {
        for (key, value) in self.previous.drain(..) {
            set_env(key, value.as_deref());
        }
    }
}

fn env_lock() -> MutexGuard<'static, ()> {
    env_mutex().lock().unwrap_or_else(|err| err.into_inner())
}

fn env_mutex() -> &'static Mutex<()> {
    ENV_LOCK.get_or_init(|| {
        let data_dir = TEST_DATA_DIR.get_or_init(|| tempfile::tempdir().expect("test data dir"));
        if std::env::var_os(oulipoly_state::paths::DATA_DIR_ENV).is_none() {
            unsafe {
                std::env::set_var(oulipoly_state::paths::DATA_DIR_ENV, data_dir.path());
            }
        }
        Mutex::new(())
    })
}

fn ensure_test_data_dir() {
    let _ = env_mutex();
}

fn set_env(key: &str, value: Option<&std::ffi::OsStr>) {
    // SAFETY: tests that call this helper hold ENV_LOCK for the full EnvScope lifetime.
    unsafe {
        if let Some(value) = value {
            std::env::set_var(key, value);
        } else {
            std::env::remove_var(key);
        }
    }
}

fn fixture_script(body: &str) -> ScriptFixture {
    ensure_test_data_dir();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("provider.sh");
    write_executable(
        &path,
        &format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
    );
    ScriptFixture { _dir: dir, path }
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).expect("write script");
    let mut perms = fs::metadata(path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod script");
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

fn provider_ref_binary(name: &str) -> ProviderImplementationRef {
    ProviderImplementationRef {
        path: None,
        crate_name: None,
        version: None,
        binary: Some(name.to_string()),
        script: None,
    }
}

fn provider_ref_script(path: &Path) -> ProviderImplementationRef {
    ProviderImplementationRef {
        path: None,
        crate_name: None,
        version: None,
        binary: None,
        script: Some(path.display().to_string()),
    }
}

fn provider_ref_crate() -> ProviderImplementationRef {
    ProviderImplementationRef {
        path: None,
        crate_name: Some("provider-a".to_string()),
        version: Some("0.1.0".to_string()),
        binary: None,
        script: None,
    }
}

fn model_without_external_ref(script: &ScriptFixture) -> ModelConfig {
    model_with_provider_ref(script.path.to_string_lossy().as_ref(), None)
}

fn external_model(fixture: &ExternalFixture) -> ModelConfig {
    external_model_with_ref(fixture, provider_ref_path(&fixture.provider_path))
}

fn external_model_with_ref(
    fixture: &ExternalFixture,
    provider_ref: ProviderImplementationRef,
) -> ModelConfig {
    model_with_provider_ref(
        fixture.legacy_trap_path.to_string_lossy().as_ref(),
        Some(provider_ref),
    )
}

fn crate_external_model(legacy_trap: &ScriptFixture) -> ModelConfig {
    model_with_provider_ref(
        legacy_trap.path.to_string_lossy().as_ref(),
        Some(provider_ref_crate()),
    )
}

fn model_with_provider_ref(
    command: &str,
    provider: Option<ProviderImplementationRef>,
) -> ModelConfig {
    ModelConfig {
        name: "provider-a-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig {
            name: "provider-a-account".to_string(),
            command: command.to_string(),
            args: vec!["--provider-arg".to_string(), "from-pool".to_string()],
            environment: BTreeMap::new(),
            unset_environment: Vec::new(),
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
        provider,
    }
}

fn model_with_provider_ref_and_inputs(
    command: &str,
    provider: Option<ProviderImplementationRef>,
    inputs: Vec<InputDef>,
) -> ModelConfig {
    let mut model = model_with_provider_ref(command, provider);
    model.inputs = inputs;
    model
}

fn assert_execution_equivalent(
    service: &executor::ExecutionResult,
    direct: &executor::ExecutionResult,
) {
    assert_eq!(service.stdout, direct.stdout);
    assert_eq!(service.stderr, direct.stderr);
    assert_eq!(service.exit_code, direct.exit_code);
    assert_eq!(service.provider_index, direct.provider_index);
    assert_eq!(service.terminal_reason, direct.terminal_reason);
    assert_terminal_signal_equivalent_except_observed_at(
        service.terminal_signal.as_ref(),
        direct.terminal_signal.as_ref(),
    );
    assert_eq!(
        service.resume_acceptance, direct.resume_acceptance,
        "resume acceptance/finalization input must stay identical"
    );
    assert_eq!(
        service.captured_child_invocations, direct.captured_child_invocations,
        "child marker capture must stay identical"
    );
    assert_eq!(
        service.returned_artifacts, direct.returned_artifacts,
        "returned artifact finalization inputs must stay identical"
    );
    assert_eq!(
        service.session_capture.session_id,
        direct.session_capture.session_id
    );
    assert_eq!(
        service.session_capture.method.db_value(),
        direct.session_capture.method.db_value()
    );
}

fn assert_terminal_signal_equivalent_except_observed_at(
    service: Option<&oulipoly_runtime::executor::terminal_signal::TerminalSignal>,
    direct: Option<&oulipoly_runtime::executor::terminal_signal::TerminalSignal>,
) {
    let (Some(service), Some(direct)) = (service, direct) else {
        assert_eq!(
            service.is_some(),
            direct.is_some(),
            "terminal signal presence must stay identical"
        );
        return;
    };

    assert_eq!(service.kind, direct.kind);
    assert_eq!(service.provider_name, direct.provider_name);
    assert_eq!(service.evidence, direct.evidence);
    // observed_at is wall-clock evidence metadata captured independently by
    // each execution path, so it is not part of byte/output preservation.
    assert!(
        service
            .observed_at
            .duration_since(std::time::UNIX_EPOCH)
            .is_ok(),
        "service observed_at should be a well-formed post-epoch timestamp"
    );
    assert!(
        direct
            .observed_at
            .duration_since(std::time::UNIX_EPOCH)
            .is_ok(),
        "direct observed_at should be a well-formed post-epoch timestamp"
    );
}

fn dispatch_registry_for_models(models: &[ModelConfig]) -> ProviderRegistry {
    dispatch_registry_for_models_with_options(models, ProviderRegistryOptions::default())
}

fn dispatch_registry_for_models_with_options(
    models: &[ModelConfig],
    options: ProviderRegistryOptions,
) -> ProviderRegistry {
    ProviderRegistry::from_model_configs(models, options)
        .expect("dispatch registry should construct from test models")
}

fn execute_dispatch_aware_service(
    registry: oulipoly_runtime::provider_registry::ProviderRegistry,
    request: ExecutorServiceRequest,
) -> Result<(executor::ExecutionResult, DispatchSeamObservation), ServiceError> {
    assert!(
        !registry.configured_artifact_keys().is_empty(),
        "dispatch-aware test must inject a populated ProviderRegistry"
    );
    let service = executor::RuntimeExecutorService::new(Arc::new(registry));
    let result = service.execute(request)?.result;
    Ok((
        result,
        DispatchSeamObservation {
            registry_injected: true,
        },
    ))
}

fn execute_dispatch_aware_result(
    registry: oulipoly_runtime::provider_registry::ProviderRegistry,
    request: ExecutorServiceRequest,
) -> Result<executor::ExecutionResult, ServiceError> {
    execute_dispatch_aware_service(registry, request).map(|(result, _)| result)
}

fn execute_external_fixture(
    fixture: &ExternalFixture,
) -> Result<executor::ExecutionResult, ServiceError> {
    execute_external_fixture_with_options(fixture, ProviderRegistryOptions::default())
}

fn execute_external_fixture_with_options(
    fixture: &ExternalFixture,
    options: ProviderRegistryOptions,
) -> Result<executor::ExecutionResult, ServiceError> {
    let model = external_model(fixture);
    let registry = dispatch_registry_for_models_with_options(std::slice::from_ref(&model), options);
    execute_dispatch_aware_result(
        registry,
        ExecutorServiceRequest::Facade {
            model,
            provider_index: 0,
            prompt: "prompt-value".to_string(),
            working_dir: None,
            models_dir: None,
            extra_inputs: HashMap::new(),
            parent_invocation_env: None,
        },
    )
}

fn execute_external_fixture_effective(
    fixture: &ExternalFixture,
    working_dir: Option<PathBuf>,
    extra_inputs: HashMap<String, Vec<String>>,
    parent_invocation_env: Option<String>,
) -> Result<executor::ExecutionResult, ServiceError> {
    let model = external_model(fixture);
    execute_external_model_effective(model, working_dir, extra_inputs, parent_invocation_env)
}

fn execute_external_model_effective(
    model: ModelConfig,
    working_dir: Option<PathBuf>,
    extra_inputs: HashMap<String, Vec<String>>,
    parent_invocation_env: Option<String>,
) -> Result<executor::ExecutionResult, ServiceError> {
    let provider = model.providers[0].clone();
    let registry = dispatch_registry_for_models(std::slice::from_ref(&model));
    execute_dispatch_aware_result(
        registry,
        ExecutorServiceRequest::Effective {
            model,
            provider,
            provider_index: 0,
            prompt_mode: PromptMode::Arg,
            prompt: "prompt-value".to_string(),
            working_dir,
            models_dir: None,
            extra_inputs,
            parent_invocation_env,
        },
    )
}

fn make_external_fixture(
    capabilities: Capabilities,
    policy_mode: PolicyMode,
    launch_mode: LaunchMode,
) -> ExternalFixture {
    ensure_test_data_dir();
    let dir = tempfile::tempdir().expect("tempdir");
    let order_path = dir.path().join("order.txt");
    let process_env_record_path = dir.path().join("process-env.jsonl");
    let policy_record_path = dir.path().join("policy-request.json");
    let launch_record_path = dir.path().join("launch-request.json");
    let legacy_record_path = dir.path().join("legacy-record.txt");
    let legacy_trap_path = dir.path().join("legacy-trap.sh");
    let provider_path = dir.path().join("fake-provider.py");

    write_executable(
        &legacy_trap_path,
        &format!(
            "#!/usr/bin/env bash\nprintf 'legacy fallback invoked' > {}\nprintf 'legacy-fallback\\n'\nexit 77\n",
            shell_quote(&legacy_record_path)
        ),
    );

    write_executable(
        &provider_path,
        &fake_provider_body(
            capabilities,
            policy_mode,
            launch_mode,
            &order_path,
            &process_env_record_path,
            &policy_record_path,
            &launch_record_path,
        ),
    );

    ExternalFixture {
        _dir: dir,
        provider_path,
        order_path,
        process_env_record_path,
        policy_record_path,
        launch_record_path,
        legacy_record_path,
        legacy_trap_path,
    }
}

fn make_describe_replacing_external_fixture() -> ExternalFixture {
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::Success,
    );
    let replacement_path = fixture._dir.path().join("replacement-provider.sh");
    write_executable(
        &replacement_path,
        &format!(
            "#!/bin/sh\ncat >/dev/null\nprintf 'replacement artifact invoked' > {}\nexit 77\n",
            shell_quote(&fixture.legacy_record_path)
        ),
    );
    let body = fs::read_to_string(&fixture.provider_path).expect("provider source");
    let describe_start = "def describe(request):\n    response";
    let replacement = format!(
        "def describe(request):\n    os.replace({}, {})\n    response",
        serde_json::to_string(&replacement_path.display().to_string()).unwrap(),
        serde_json::to_string(&fixture.provider_path.display().to_string()).unwrap(),
    );
    let body = body.replacen(describe_start, &replacement, 1);
    let body = body.replacen("\"terminal\": False", "\"terminal\": True", 1);
    assert_ne!(body, fs::read_to_string(&fixture.provider_path).unwrap());
    write_executable(&fixture.provider_path, &body);
    fixture
}

fn enable_terminal_capability(fixture: &ExternalFixture) {
    let body = fs::read_to_string(&fixture.provider_path).expect("provider source");
    let body = body.replacen("\"terminal\": False", "\"terminal\": True", 1);
    assert_ne!(
        body,
        fs::read_to_string(&fixture.provider_path).expect("provider source")
    );
    write_executable(&fixture.provider_path, &body);
}

fn enable_prompt_acceptance_capability(fixture: &ExternalFixture) {
    let body = fs::read_to_string(&fixture.provider_path).expect("provider source");
    let body = body.replacen(
        "\"launch\": CAP_LAUNCH,",
        "\"launch\": CAP_LAUNCH,\n            \"prompt_acceptance_v1\": True,",
        1,
    );
    assert_ne!(
        body,
        fs::read_to_string(&fixture.provider_path).expect("provider source")
    );
    write_executable(&fixture.provider_path, &body);
}

fn disable_launch_output_capability(fixture: &ExternalFixture) {
    let body = fs::read_to_string(&fixture.provider_path).expect("provider source");
    let body = body.replacen(
        "\"launch_output_v1\": True",
        "\"launch_output_v1\": False",
        1,
    );
    assert_ne!(
        body,
        fs::read_to_string(&fixture.provider_path).expect("provider source")
    );
    write_executable(&fixture.provider_path, &body);
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn fake_provider_body(
    capabilities: Capabilities,
    policy_mode: PolicyMode,
    launch_mode: LaunchMode,
    order_path: &Path,
    process_env_record_path: &Path,
    policy_record_path: &Path,
    launch_record_path: &Path,
) -> String {
    fake_provider_script_body(
        capabilities,
        policy_mode_wire(policy_mode),
        launch_mode_wire(launch_mode),
        order_path,
        process_env_record_path,
        policy_record_path,
        launch_record_path,
    )
}

fn policy_mode_wire(policy_mode: PolicyMode) -> &'static str {
    match policy_mode {
        PolicyMode::Accept => "accept",
        PolicyMode::Reject => "reject",
        PolicyMode::Transform => "transform",
        PolicyMode::PrivateEnvTransform => "private_env_transform",
        PolicyMode::HybridShape => "hybrid_shape",
    }
}

fn launch_mode_wire(launch_mode: LaunchMode) -> &'static str {
    match launch_mode {
        LaunchMode::Success => "success",
        LaunchMode::NonzeroFinal => "nonzero_final",
        LaunchMode::ProviderNonzeroAfterFinal => "provider_nonzero_after_final",
        LaunchMode::MalformedProtocol => "malformed_protocol",
        LaunchMode::MissingFinal => "missing_final",
        LaunchMode::InvalidBase64 => "invalid_base64",
        LaunchMode::HostTransportFailure => "host_transport_failure",
        LaunchMode::ProviderNonzeroNoFinal => "provider_nonzero_no_final",
        LaunchMode::CancelledFinal => "cancelled_final",
        LaunchMode::UnknownTerminalWithQuotaText => "unknown_terminal_with_quota_text",
        LaunchMode::HostCancelledBeforeFinal => "host_cancelled_before_final",
        LaunchMode::LargeOutput => "large_output",
    }
}

fn fake_provider_script_body(
    capabilities: Capabilities,
    policy_mode: &str,
    launch_mode: &str,
    order_path: &Path,
    process_env_record_path: &Path,
    policy_record_path: &Path,
    launch_record_path: &Path,
) -> String {
    let runner_private_env_names = AutoWakeEnvironmentVariable::ALL
        .into_iter()
        .map(AutoWakeEnvironmentVariable::name)
        .chain([
            oulipoly_state::COMPLETION_REGISTRATION_AUTHORITY_ENV,
            "OULIPOLY_PARENT_INVOCATION",
        ])
        .collect::<Vec<_>>();
    format!(
        r#"#!/usr/bin/env python3
import json
import base64
import hashlib
import os
import pathlib
import sys

CONTRACT = "oulipoly.provider/v1"
ORDER = pathlib.Path({order_path})
PROCESS_ENV_RECORD = pathlib.Path({process_env_record_path})
POLICY_RECORD = pathlib.Path({policy_record_path})
LAUNCH_RECORD = pathlib.Path({launch_record_path})
CAP_POLICY = {cap_policy}
CAP_LAUNCH = {cap_launch}
POLICY_MODE = {policy_mode}
LAUNCH_MODE = {launch_mode}

PID_FILE = os.environ.get("OULIPOLY_CHILD_PID_FILE")
READY_FILE = os.environ.get("OULIPOLY_CHILD_CUSTODY_TEST_READY_FILE")
SUBCOMMAND = sys.argv[1] if len(sys.argv) > 1 else ""
RUNNER_PRIVATE_ENV_NAMES = {runner_private_env_names}
with PROCESS_ENV_RECORD.open("a") as stream:
    stream.write(json.dumps({{
        "subcommand": SUBCOMMAND,
        "env": {{name: os.environ[name] for name in RUNNER_PRIVATE_ENV_NAMES if name in os.environ}},
    }}, sort_keys=True) + "\n")
if PID_FILE and READY_FILE and SUBCOMMAND == "launch":
    pathlib.Path(PID_FILE).write_text(str(os.getpid()))
    pathlib.Path(READY_FILE).touch()

def clear_custody_readiness():
    if READY_FILE:
        pathlib.Path(READY_FILE).unlink(missing_ok=True)

def read_request():
    text = sys.stdin.read()
    if not text:
        return {{}}
    return json.loads(text)

def write_json(value):
    sys.stdout.write(json.dumps(value, separators=(",", ":")) + "\n")
    sys.stdout.flush()

def write_jsonl(value):
    write_json(value)

def append_order(value):
    existing = ORDER.read_text() if ORDER.exists() else ""
    ORDER.write_text(existing + value + "\n")

def response(request, result):
    write_json({{
        "contract": request.get("contract", CONTRACT),
        "request_id": request.get("request_id", "request-example-001"),
        "ok": True,
        "result": result,
    }})

def describe(request):
    response(request, {{
        "provider_id": "fake-provider",
        "display_name": "Fake Provider",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": {{
            "launch": CAP_LAUNCH,
            "launch_output_v1": True,
            "policy": CAP_POLICY,
            "quota": False,
            "session": False,
            "terminal": False,
            "rotation": False,
            "discovery": False,
            "settings": False,
            "setup_brain": False,
            "setup": False,
            "migration": False
        }}
    }})
    clear_custody_readiness()

def policy(request):
    append_order("policy.evaluate")
    POLICY_RECORD.write_text(json.dumps(request, sort_keys=True))
    if POLICY_MODE == "hybrid_shape":
        response(request, hybrid_policy_result(request))
        return
    accepted = POLICY_MODE != "reject"
    result = {{
        "accepted": accepted,
        "stdin": None,
        "prompt": None,
        "diagnostics": [{{
            "severity": "error",
            "code": "policy_live_contract_reject",
            "path": "params.launch.argv",
            "message": "provider expected account-specific policy argv shape"
        }}] if not accepted else [],
        "markers": []
    }}
    if POLICY_MODE == "transform":
        result["argv"] = ["provider-a-bin", "--from-policy"]
        result["env"] = {{"POLICY_TRANSFORM_COUNT": "1"}}
        result["stdin"] = "stdin-from-policy"
        result["prompt"] = "prompt-from-policy"
    if POLICY_MODE == "private_env_transform":
        result["env"] = {{name: "policy-private" for name in RUNNER_PRIVATE_ENV_NAMES}}
        result["env"].update({{
            "OULIPOLY_COMPLETION_REGISTRATION_AUTHORITY": "policy-private",
            "OULIPOLY_PARENT_INVOCATION": json.dumps({{
                "source": "policy-private",
                "id": "22222222-2222-4222-8222-222222222222",
                "_oulipoly_completion_registration_authority": "cd" * 32,
            }}),
        }})
    response(request, result)
    clear_custody_readiness()

def terminal_classify(request):
    append_order("terminal.classify")
    response(request, {{
        "terminal_signal": {{
            "kind": "clean_exit",
            "evidence": "selected provider terminal classification",
            "observed_at_unix_ms": 2005
        }}
    }})

def hybrid_policy_result(request):
    params = request.get("params", {{}})
    launch = params.get("launch", {{}})
    model = params.get("model", {{}})
    expected_prefix = [launch.get("command")] + launch.get("args", []) + model.get("provider_args", [])
    argv = launch.get("argv", [])
    prompt = model.get("inputs", {{}}).get("prompt")
    diagnostics = []
    if params.get("settings_id") != request.get("provider_instance_id"):
        diagnostics.append({{"severity": "error", "code": "unexpected_settings_id", "message": str(params.get("settings_id"))}})
    if argv[:len(expected_prefix)] != expected_prefix:
        diagnostics.append({{"severity": "error", "code": "unexpected_argv_prefix", "message": json.dumps(argv)}})
    if not argv or argv[-1] != prompt:
        diagnostics.append({{"severity": "error", "code": "missing_prompt_tail", "message": json.dumps(argv)}})
    return {{
        "accepted": not diagnostics,
        "stdin": None,
        "prompt": prompt,
        "diagnostics": diagnostics,
        "markers": []
    }}

def request_id(request):
    return request.get("request_id", "request-example-001")

def exit_event(request, seq, code, signal):
    return {{
        "contract": CONTRACT,
        "request_id": request_id(request),
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "exit",
        "status": {{"kind": "exited", "code": code}},
        "terminal_signal": {{
            "kind": signal,
            "evidence": "fake-provider exit event",
            "observed_at_unix_ms": 1000 + seq
        }},
        "session": {{"provider_session_id": "example-session"}}
    }}

def output_completion_event(request, seq, stdout_chunks, stderr_chunks):
    stdout = b"".join(base64.b64decode(chunk) for chunk in stdout_chunks)
    stderr = b"".join(base64.b64decode(chunk) for chunk in stderr_chunks)
    return {{
        "contract": CONTRACT,
        "request_id": request_id(request),
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "marker",
        "name": "oulipoly.launch_output_complete/v1",
        "value": {{
            "protocol": "oulipoly.launch_output/v1",
            "stdout": {{"bytes": len(stdout), "sha256": hashlib.sha256(stdout).hexdigest()}},
            "stderr": {{"bytes": len(stderr), "sha256": hashlib.sha256(stderr).hexdigest()}},
            "data_event_count": len(stdout_chunks) + len(stderr_chunks),
        }},
    }}

def launch(_request):
    append_order("launch")
    LAUNCH_RECORD.write_text(json.dumps(_request, sort_keys=True))
    reqid = request_id(_request)
    if LAUNCH_MODE == "malformed_protocol":
        sys.stdout.write("{{not-json}}\n")
        sys.stdout.flush()
        return 0
    if LAUNCH_MODE == "missing_final":
        write_jsonl({{"contract": CONTRACT, "request_id": reqid, "seq": 1, "time_unix_ms": 1001, "kind": "stdout", "data_base64": "YQ=="}})
        return 0
    if LAUNCH_MODE == "invalid_base64":
        write_jsonl({{"contract": CONTRACT, "request_id": reqid, "seq": 1, "time_unix_ms": 1001, "kind": "stdout", "data_base64": "@@not-base64@@"}})
        write_jsonl(exit_event(_request, 2, 0, "clean_exit"))
        return 0
    if LAUNCH_MODE == "host_transport_failure":
        sys.stdout.write("x" * (1024 * 1024 + 1))
        sys.stdout.flush()
        return 0
    if LAUNCH_MODE == "provider_nonzero_no_final":
        write_jsonl({{"contract": CONTRACT, "request_id": reqid, "seq": 1, "time_unix_ms": 1001, "kind": "stdout", "data_base64": "YQ=="}})
        return 8
    if LAUNCH_MODE == "cancelled_final":
        write_jsonl({{"contract": CONTRACT, "request_id": reqid, "seq": 1, "time_unix_ms": 1001, "kind": "stdout", "data_base64": "YQ=="}})
        write_jsonl(output_completion_event(_request, 2, ["YQ=="], []))
        write_jsonl({{
            "contract": CONTRACT,
            "request_id": reqid,
            "seq": 3,
            "time_unix_ms": 1003,
            "kind": "exit",
            "status": {{"kind": "cancelled"}},
            "terminal_signal": {{"kind": "cancelled", "evidence": "fake-provider cancellation", "observed_at_unix_ms": 1003}},
            "session": {{"provider_session_id": "example-session"}}
        }})
        return 0
    if LAUNCH_MODE == "unknown_terminal_with_quota_text":
        write_jsonl({{"contract": CONTRACT, "request_id": reqid, "seq": 1, "time_unix_ms": 1001, "kind": "stderr", "data_base64": "cXVvdGFfZXhoYXVzdGVkX2luYmFuZA=="}})
        write_jsonl(output_completion_event(_request, 2, [], ["cXVvdGFfZXhoYXVzdGVkX2luYmFuZA=="]))
        write_jsonl(exit_event(_request, 3, 0, "unknown"))
        return 0
    if LAUNCH_MODE == "host_cancelled_before_final":
        import time
        time.sleep(5)
        return 0
    if LAUNCH_MODE == "large_output":
        chunk = base64.b64encode(b"x" * 8192).decode()
        for seq in range(1, 257):
            write_jsonl({{"contract": CONTRACT, "request_id": reqid, "seq": seq, "time_unix_ms": 1000 + seq, "kind": "stdout", "data_base64": chunk}})
        write_jsonl(output_completion_event(_request, 257, [chunk] * 256, []))
        write_jsonl(exit_event(_request, 258, 0, "clean_exit"))
        return 0
    code = 9 if LAUNCH_MODE == "nonzero_final" else 0
    write_jsonl({{"contract": CONTRACT, "request_id": reqid, "seq": 1, "time_unix_ms": 1001, "kind": "stdout", "data_base64": "AAH/"}})
    write_jsonl({{"contract": CONTRACT, "request_id": reqid, "seq": 2, "time_unix_ms": 1002, "kind": "stderr", "data_base64": "ZXJy//4="}})
    write_jsonl(output_completion_event(_request, 3, ["AAH/"], ["ZXJy//4="]))
    write_jsonl(exit_event(_request, 4, code, "nonzero_exit" if code else "clean_exit"))
    return 6 if LAUNCH_MODE == "provider_nonzero_after_final" else 0

def main():
    request = read_request()
    if SUBCOMMAND == "describe":
        describe(request)
        return 0
    if SUBCOMMAND == "policy.evaluate":
        policy(request)
        return 0
    if SUBCOMMAND == "launch":
        return launch(request)
    if SUBCOMMAND == "terminal.classify":
        terminal_classify(request)
        return 0
    return 64

if __name__ == "__main__":
    raise SystemExit(main())
"#,
        order_path = serde_json::to_string(&order_path.display().to_string()).unwrap(),
        process_env_record_path =
            serde_json::to_string(&process_env_record_path.display().to_string()).unwrap(),
        policy_record_path =
            serde_json::to_string(&policy_record_path.display().to_string()).unwrap(),
        launch_record_path =
            serde_json::to_string(&launch_record_path.display().to_string()).unwrap(),
        cap_policy = py_bool(capabilities.policy),
        cap_launch = py_bool(capabilities.launch),
        policy_mode = serde_json::to_string(policy_mode).unwrap(),
        launch_mode = serde_json::to_string(launch_mode).unwrap(),
        runner_private_env_names = serde_json::to_string(&runner_private_env_names).unwrap(),
    )
}

fn py_bool(value: bool) -> &'static str {
    if value { "True" } else { "False" }
}

fn read_json(path: &Path) -> serde_json::Value {
    parse_json_value(&read_json_text(path))
}

fn read_json_text(path: &Path) -> String {
    fs::read_to_string(path).expect("record should exist")
}

fn read_json_lines(path: &Path) -> Vec<Value> {
    read_json_text(path).lines().map(parse_json_value).collect()
}

fn parse_json_value(text: &str) -> serde_json::Value {
    serde_json::from_str(text).expect("record should be json")
}

fn json_string_array(value: &serde_json::Value) -> Vec<String> {
    json_array_items(value)
        .iter()
        .map(json_value_to_string)
        .collect()
}

fn json_array_items(value: &serde_json::Value) -> &[serde_json::Value] {
    value.as_array().expect("value should be an array")
}

fn json_value_to_string(item: &serde_json::Value) -> String {
    item.as_str()
        .expect("array item should be a string")
        .to_string()
}

fn order_lines(path: &Path) -> Vec<String> {
    lines_to_strings(&read_order_text(path))
}

fn read_order_text(path: &Path) -> String {
    fs::read_to_string(path).expect("order should be recorded")
}

fn lines_to_strings(text: &str) -> Vec<String> {
    text.lines().map(str::to_string).collect()
}

fn assert_model_request_carries_effective_inputs(request: &serde_json::Value) {
    let model = &request["params"]["model"];
    assert_eq!(model["name"], "provider-a-model");
    assert_eq!(
        model["provider_args"],
        serde_json::json!(["--provider-arg", "from-pool"])
    );
    assert_eq!(model["inputs"]["prompt"], "prompt-value");
    assert_eq!(
        model["inputs"]["named"]["quality"],
        serde_json::json!(["high"])
    );
    assert_eq!(
        model["inputs"]["named"]["style"],
        serde_json::json!(["plain", "dense"])
    );
}

fn assert_external_dispatch_failure(
    error: ServiceError,
    expected_category: &str,
    expected_kind: &str,
) {
    let message = error.to_string();
    assert!(
        message.contains(expected_category),
        "expected external dispatch failure category {expected_category:?}, got {message:?}"
    );
    assert!(
        message.contains(expected_kind),
        "expected external dispatch failure kind {expected_kind:?}, got {message:?}"
    );
    assert!(
        !message.contains("exit 77"),
        "transport/protocol failures must not be mapped as legacy model exits: {message:?}"
    );
}

#[test]
fn runtime_executor_dispatch_no_ref_preserves_legacy_bytes_with_unrelated_registry() {
    let legacy = fixture_script(
        r#"printf 'out:%b:%s\n' '\000\377' "$1"
printf 'err:%b:%s\n' '\376' "$1" >&2"#,
    );
    let unrelated = fixture_script("printf 'unrelated external provider should not run\\n'");
    let unrelated_model = model_with_provider_ref(
        "unused-provider-command",
        Some(provider_ref_path(&unrelated.path)),
    );
    let registry = dispatch_registry_for_models(&[unrelated_model]);
    assert_eq!(registry.configured_artifact_keys().len(), 1);

    let model = model_without_external_ref(&legacy);
    let extra_inputs = HashMap::new();
    let direct = cli::execute_effective(EffectiveExecuteRequest {
        model: &model,
        provider: &model.providers[0],
        provider_index: 0,
        prompt_mode: PromptMode::Arg,
        prompt: "prompt-value",
        working_dir: None,
        models_dir: None,
        extra_inputs: &extra_inputs,
        parent_invocation_env: None,
    })
    .expect("direct legacy execution should succeed");

    let (service, observation) = execute_dispatch_aware_service(
        registry,
        ExecutorServiceRequest::Facade {
            model,
            provider_index: 0,
            prompt: "prompt-value".to_string(),
            working_dir: None,
            models_dir: None,
            extra_inputs: HashMap::new(),
            parent_invocation_env: None,
        },
    )
    .expect("dispatch-aware service should execute no-ref model");

    assert_execution_equivalent(&service, &direct);
    assert!(
        observation.registry_injected,
        "test must run through the dispatch-aware service seam with the populated registry injected"
    );
}

#[test]
fn runtime_executor_dispatch_no_ref_does_not_construct_or_invoke_provider_client() {
    let legacy = fixture_script("printf 'legacy:%s\\n' \"$1\"");
    let counter = tempfile::NamedTempFile::new().expect("counter");
    fs::write(counter.path(), "0").expect("initialize counter");
    let external = fixture_script(&format!(
        "count=$(cat {counter}); count=$((count + 1)); printf '%s' \"$count\" > {counter}; printf '{json}\\n'",
        counter = shell_quote(counter.path()),
        json = "{\"contract\":\"oulipoly.provider/v1\",\"request_id\":\"request-example-001\",\"ok\":true,\"result\":{}}"
    ));
    let unrelated_model =
        model_with_provider_ref("unused", Some(provider_ref_path(&external.path)));
    let registry = dispatch_registry_for_models(&[unrelated_model]);
    assert_eq!(registry.configured_artifact_keys().len(), 1);

    let (result, observation) = execute_dispatch_aware_service(
        registry,
        ExecutorServiceRequest::Facade {
            model: model_without_external_ref(&legacy),
            provider_index: 0,
            prompt: "prompt-value".to_string(),
            working_dir: None,
            models_dir: None,
            extra_inputs: HashMap::new(),
            parent_invocation_env: None,
        },
    )
    .expect("dispatch-aware no-ref execution should pass");

    assert_eq!(result.stdout, b"legacy:--provider-arg\n");
    assert!(
        observation.registry_injected,
        "bypass proof must observe the dispatch-aware service seam"
    );
    assert_eq!(
        fs::read_to_string(counter.path()).expect("counter readable"),
        "0",
        "no-ref execution must not describe, construct, or invoke the provider client"
    );
    assert!(
        !String::from_utf8_lossy(&result.stdout).contains(SELECTED_PROVIDER_SETTINGS_ID),
        "no-ref execution must not read or leak the selected provider settings id"
    );
}

#[test]
fn external_provider_runtime_disabled_crate_fails_before_provider_call() {
    let legacy = fixture_script("printf 'legacy fallback\\n'; exit 77");
    let model = crate_external_model(&legacy);
    let registry = dispatch_registry_for_models(std::slice::from_ref(&model));

    let error = execute_dispatch_aware_result(
        registry,
        ExecutorServiceRequest::Facade {
            model,
            provider_index: 0,
            prompt: "prompt-value".to_string(),
            working_dir: None,
            models_dir: None,
            extra_inputs: HashMap::new(),
            parent_invocation_env: None,
        },
    )
    .expect_err("crate provider refs must fail before legacy fallback or provider invocation");

    assert!(error.to_string().contains("runtime-disabled"));
}

#[test]
fn external_provider_missing_policy_or_launch_capability_fails_without_builtin_fallback() {
    for capabilities in [
        Capabilities {
            policy: false,
            launch: true,
        },
        Capabilities {
            policy: true,
            launch: false,
        },
    ] {
        let fixture = make_external_fixture(capabilities, PolicyMode::Accept, LaunchMode::Success);

        let error = execute_external_fixture(&fixture)
            .expect_err("missing required policy or launch capability must fail fast");

        assert!(error.to_string().contains("missing required capability"));
        assert!(!fixture.legacy_record_path.exists());
        assert!(!fixture.launch_record_path.exists());
    }
}

#[test]
fn external_provider_missing_launch_output_capability_requires_provider_upgrade() {
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::Success,
    );
    disable_launch_output_capability(&fixture);

    let error = execute_external_fixture(&fixture)
        .expect_err("launch_output_v1 is mandatory for external-provider dispatch");

    assert!(
        error
            .to_string()
            .contains("complete_launch_output_unsupported")
    );
    assert!(!fixture.policy_record_path.exists());
    assert!(!fixture.launch_record_path.exists());
    assert!(!fixture.legacy_record_path.exists());
}

#[test]
fn external_provider_policy_evaluate_runs_before_launch_and_uses_selected_provider_settings() {
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::Success,
    );

    let config_root = fixture._dir.path().join("config-root");
    let data_root = fixture._dir.path().join("data-root");
    let result = execute_external_fixture_with_options(
        &fixture,
        ProviderRegistryOptions::default()
            .with_config_root(config_root.clone())
            .with_data_root(data_root.clone()),
    )
    .expect("external dispatch should succeed");

    assert_eq!(
        order_lines(&fixture.order_path),
        ["policy.evaluate", "launch"]
    );
    assert_eq!(result.stdout, vec![0, 1, 255]);
    assert_eq!(
        read_json(&fixture.policy_record_path)["params"]["settings_id"],
        SELECTED_PROVIDER_SETTINGS_ID
    );
    assert_eq!(
        read_json(&fixture.launch_record_path)["params"]["settings_id"],
        SELECTED_PROVIDER_SETTINGS_ID
    );
    for request_path in [&fixture.policy_record_path, &fixture.launch_record_path] {
        let request = read_json(request_path);
        assert_eq!(
            request["host"]["config_root"],
            config_root.display().to_string()
        );
        assert_eq!(
            request["host"]["data_root"],
            data_root.display().to_string()
        );
    }
    assert!(!fixture.legacy_record_path.exists());
}

#[test]
fn external_dispatch_keeps_the_capability_advertiser_after_path_replacement() {
    let fixture = make_describe_replacing_external_fixture();

    let result = execute_external_fixture(&fixture)
        .expect("the selected capability advertiser should perform policy and launch");

    assert_eq!(
        order_lines(&fixture.order_path),
        ["policy.evaluate", "launch", "terminal.classify"]
    );
    assert_eq!(result.stdout, vec![0, 1, 255]);
    assert!(
        !fixture.legacy_record_path.exists(),
        "the replacement artifact must not inherit negotiated authority"
    );
}

#[test]
fn external_provider_policy_request_passes_hybrid_launch_shape() {
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::HybridShape,
        LaunchMode::Success,
    );

    execute_external_fixture(&fixture)
        .expect("hybrid-shaped policy should accept host launch request");

    let policy = read_json(&fixture.policy_record_path);
    let launch = &policy["params"]["launch"];
    assert_eq!(
        policy["params"]["settings_id"],
        SELECTED_PROVIDER_SETTINGS_ID
    );
    assert_eq!(
        policy["provider_instance_id"],
        SELECTED_PROVIDER_SETTINGS_ID
    );
    assert_eq!(
        launch["command"],
        fixture.legacy_trap_path.display().to_string()
    );
    assert_eq!(launch["args"], serde_json::json!([]));
    assert_eq!(
        launch["argv"][0],
        fixture.legacy_trap_path.display().to_string()
    );
    assert_eq!(launch["argv"][1], "--provider-arg");
    assert_eq!(launch["argv"][2], "from-pool");
    assert_eq!(
        launch["argv"].as_array().and_then(|argv| argv.last()),
        Some(&serde_json::json!("prompt-value"))
    );
}

#[test]
fn external_provider_launch_request_carries_selected_settings_id_and_effective_inputs() {
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::Success,
    );
    let working_dir = explicit_working_dir(&fixture);
    let extra_inputs = effective_extra_inputs();
    let parent_invocation_env = parent_invocation_env();

    execute_external_fixture_effective(
        &fixture,
        Some(working_dir.clone()),
        extra_inputs,
        Some(parent_invocation_env.clone()),
    )
    .expect("external dispatch should carry effective inputs into provider requests");

    let policy = read_json(&fixture.policy_record_path);
    let launch = read_json(&fixture.launch_record_path);
    assert_selected_settings_ids(&policy, &launch);
    assert_model_request_carries_effective_inputs(&policy);
    assert_model_request_carries_effective_inputs(&launch);
    assert_launch_working_dir(&launch, &working_dir);
    assert_launch_parent_invocation(&launch, &parent_invocation_env);
    let launch_argv = json_string_array(&launch["params"]["argv"]);
    assert_launch_argv_prefix(&launch_argv, &fixture);
    assert_launch_argv_tail(&launch_argv);
    assert_effective_input_flags(&launch_argv);
    assert_no_arg_mode_stdin(&launch);
    assert_eq!(
        launch["params"]["output_delivery"]["protocol"],
        "oulipoly.launch_output/v1"
    );
    assert_eq!(launch["host"]["env"]["OULIPOLY_HOST_LAUNCH_OUTPUT_V1"], "1");
}

fn explicit_working_dir(fixture: &ExternalFixture) -> PathBuf {
    let working_dir = fixture._dir.path().join("explicit-working-directory");
    fs::create_dir(&working_dir).expect("working dir");
    working_dir
}

fn effective_extra_inputs() -> HashMap<String, Vec<String>> {
    HashMap::from([
        ("quality".to_string(), vec!["high".to_string()]),
        (
            "style".to_string(),
            vec!["plain".to_string(), "dense".to_string()],
        ),
    ])
}

fn parent_invocation_env() -> String {
    "parent-provider-a-invocation".to_string()
}

struct TestTempPath {
    _dir: tempfile::TempDir,
    path: String,
}

fn tempdir_path(label: &str) -> TestTempPath {
    let dir = tempfile::tempdir().expect(label);
    TestTempPath {
        path: dir.path().display().to_string(),
        _dir: dir,
    }
}

fn runner_data_dir_from_xdg(xdg_data: &str) -> String {
    Path::new(xdg_data)
        .join("oulipoly-agent-runner")
        .display()
        .to_string()
}

fn external_model_for_provider(
    fixture: &ExternalFixture,
    provider_name: &str,
    command: &str,
) -> ModelConfig {
    let mut model = external_model(fixture);
    model.providers[0].name = provider_name.to_string();
    model.providers[0].command = command.to_string();
    model
}

fn dispatch_registry_for_model(model: &ModelConfig) -> ProviderRegistry {
    dispatch_registry_for_models(std::slice::from_ref(model))
}

fn assert_selected_settings_ids(policy: &Value, launch: &Value) {
    assert_eq!(
        policy["params"]["settings_id"],
        SELECTED_PROVIDER_SETTINGS_ID
    );
    assert_eq!(
        launch["params"]["settings_id"],
        SELECTED_PROVIDER_SETTINGS_ID
    );
}

fn assert_launch_working_dir(launch: &Value, working_dir: &Path) {
    assert_eq!(
        launch["params"]["working_directory"],
        working_dir.display().to_string()
    );
}

fn assert_launch_parent_invocation(launch: &Value, parent_invocation_env: &str) {
    assert_eq!(
        launch["params"]["env"]["OULIPOLY_PARENT_INVOCATION"],
        parent_invocation_env
    );
}

fn assert_launch_argv_prefix(launch_argv: &[String], fixture: &ExternalFixture) {
    assert_eq!(
        &launch_argv[..3],
        [
            fixture.legacy_trap_path.display().to_string(),
            "--provider-arg".to_string(),
            "from-pool".to_string(),
        ],
        "launch argv should begin with command and pool/provider args"
    );
}

fn assert_launch_argv_tail(launch_argv: &[String]) {
    assert_eq!(
        launch_argv.last().map(String::as_str),
        Some("prompt-value"),
        "arg-mode prompt should remain the argv tail"
    );
}

fn assert_effective_input_flags(launch_argv: &[String]) {
    for expected_pair in effective_input_flag_pairs() {
        assert_effective_input_flag(launch_argv, expected_pair);
    }
}

fn effective_input_flag_pairs() -> [[&'static str; 2]; 3] {
    [
        ["--quality", "high"],
        ["--style", "plain"],
        ["--style", "dense"],
    ]
}

fn assert_effective_input_flag(launch_argv: &[String], expected_pair: [&str; 2]) {
    assert!(
        launch_argv
            .windows(2)
            .any(|pair| pair[0] == expected_pair[0] && pair[1] == expected_pair[1]),
        "launch argv should include effective input flag pair {expected_pair:?}: {launch_argv:?}"
    );
}

fn assert_no_arg_mode_stdin(launch: &Value) {
    assert!(
        launch["params"].get("stdin").is_none(),
        "arg-mode provider launches must not send the prompt on stdin"
    );
}

#[test]
fn external_provider_post_spawn_failures_reap_before_fenced_generation_exit() {
    let _lock = env_lock();
    for (fault, invocation_uuid, terminal_reason, expect_bound_pid) in [
        (
            "external_spawn_observer",
            "72727272-7272-4272-8272-727272727272",
            "startup_failed",
            false,
        ),
        (
            "external_status_poll",
            "73737373-7373-4373-8373-737373737373",
            "abnormal_termination",
            true,
        ),
    ] {
        run_external_child_custody_fault(fault, invocation_uuid, terminal_reason, expect_bound_pid);
    }
}

#[test]
fn external_provider_success_reaps_and_completes_the_exact_generation_orderly() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("custody tempdir");
    let data_dir = dir.path().join("data");
    fs::create_dir_all(&data_dir).expect("custody data dir");
    let pid_path = dir.path().join("provider.pid");
    let ready_path = dir.path().join("provider.ready");
    let data_dir_text = data_dir.to_string_lossy().into_owned();
    let pid_path_text = pid_path.to_string_lossy().into_owned();
    let ready_path_text = ready_path.to_string_lossy().into_owned();
    let _env = EnvScope::set_optional(&[
        ("OULIPOLY_DATA_DIR", Some(&data_dir_text)),
        (CHILD_CUSTODY_FAULT_ENV, None),
        (CHILD_CUSTODY_READY_FILE_ENV, Some(&ready_path_text)),
        (CHILD_PID_FILE_ENV, Some(&pid_path_text)),
    ]);
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::Success,
    );
    let invocation_uuid = "74747474-7474-4474-8474-747474747474";
    let invocation = serde_json::to_string(&oulipoly_state::CompositeInvocationId {
        source: "external-custody-test".to_string(),
        id: invocation_uuid.to_string(),
    })
    .expect("parent invocation");

    execute_external_fixture_effective(&fixture, None, HashMap::new(), Some(invocation))
        .expect("external custody success must complete");

    let pid = fs::read_to_string(&pid_path)
        .expect("provider pid")
        .trim()
        .parse::<libc::pid_t>()
        .expect("numeric provider pid");
    assert_external_child_reaped(pid);
    assert_external_terminal_generation(
        &data_dir,
        invocation_uuid,
        "orderly_completion",
        Some(i64::from(pid)),
    );
}

fn run_external_child_custody_fault(
    fault: &str,
    invocation_uuid: &str,
    terminal_reason: &str,
    expect_bound_pid: bool,
) {
    let dir = tempfile::tempdir().expect("custody tempdir");
    let data_dir = dir.path().join("data");
    fs::create_dir_all(&data_dir).expect("custody data dir");
    let pid_path = dir.path().join("provider.pid");
    let ready_path = dir.path().join("provider.ready");
    let data_dir_text = data_dir.to_string_lossy().into_owned();
    let pid_path_text = pid_path.to_string_lossy().into_owned();
    let ready_path_text = ready_path.to_string_lossy().into_owned();
    let _env = EnvScope::set(&[
        ("OULIPOLY_DATA_DIR", &data_dir_text),
        (CHILD_CUSTODY_FAULT_ENV, fault),
        (CHILD_CUSTODY_READY_FILE_ENV, &ready_path_text),
        (CHILD_PID_FILE_ENV, &pid_path_text),
    ]);
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::Success,
    );
    let invocation = serde_json::to_string(&oulipoly_state::CompositeInvocationId {
        source: "external-custody-test".to_string(),
        id: invocation_uuid.to_string(),
    })
    .expect("parent invocation");

    execute_external_fixture_effective(&fixture, None, HashMap::new(), Some(invocation))
        .expect_err("external custody fault must fail dispatch");

    let pid = fs::read_to_string(&pid_path)
        .expect("provider pid")
        .trim()
        .parse::<libc::pid_t>()
        .expect("numeric provider pid");
    assert_external_child_reaped(pid);
    assert_external_terminal_generation(
        &data_dir,
        invocation_uuid,
        terminal_reason,
        expect_bound_pid.then_some(i64::from(pid)),
    );
}

fn assert_external_child_reaped(pid: libc::pid_t) {
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "provider remained live");
    assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    let mut status = 0;
    assert_eq!(
        unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
        -1
    );
    assert_eq!(
        io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD),
        "provider child was not reaped"
    );
}

fn assert_external_terminal_generation(
    data_dir: &Path,
    invocation_uuid: &str,
    terminal_reason: &str,
    expected_pid: Option<i64>,
) {
    let connection = Connection::open(data_dir.join("pid-identity.db")).expect("identity db");
    let (count, exited, reason, spawned_pid): (i64, i64, String, Option<i64>) = connection
        .query_row(
            "SELECT COUNT(*),
                    SUM(lifecycle_state = 'exited'),
                    MIN(terminal_reason),
                    MIN(spawned_os_pid)
             FROM runtime_generation
             WHERE spawn_invocation_uuid = ?1",
            [invocation_uuid],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("terminal generation row");
    assert_eq!(count, 1);
    assert_eq!(exited, 1);
    assert_eq!(reason, terminal_reason);
    assert_eq!(spawned_pid, expected_pid);
}

#[test]
fn external_provider_launch_env_inherits_parent_environment_with_runner_overrides() {
    let _lock = env_lock();
    let xdg_data = tempdir_path("xdg data tempdir");
    let expected_data_dir = runner_data_dir_from_xdg(&xdg_data.path);
    let parent_invocation_env = parent_invocation_env();
    let agent_runner_bin = "/tmp/target-release/oulipoly-agent-runner";
    let _env = EnvScope::set_optional(&[
        ("XDG_DATA_HOME", Some(xdg_data.path.as_str())),
        ("AGENT_BASH_AGENT_RUNNER_BIN", Some(agent_runner_bin)),
        ("OULIPOLY_DATA_DIR", Some(expected_data_dir.as_str())),
        (
            "GENERIC_PARENT_ENV_SENTINEL",
            Some(GENERIC_PARENT_ENV_VALUE),
        ),
        ("OPENAI_API_KEY", Some(OPENAI_KEY_VALUE)),
        ("OPENAI_BASE_URL", Some(OPENAI_BASE_URL_VALUE)),
    ]);
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::Success,
    );

    execute_external_fixture_effective(
        &fixture,
        None,
        HashMap::new(),
        Some(parent_invocation_env.clone()),
    )
    .expect("external dispatch should declare host linkage env");

    let policy = read_json(&fixture.policy_record_path);
    let launch = read_json(&fixture.launch_record_path);
    assert_host_linkage_envs(
        &policy,
        &launch,
        &expected_data_dir,
        &parent_invocation_env,
        agent_runner_bin,
    );
}

#[test]
fn external_provider_launch_env_separates_completion_authority_from_parent_identity() {
    let _lock = env_lock();
    let data_dir = tempdir_path("completion authority data tempdir");
    let _env = EnvScope::set_optional(&[
        ("OULIPOLY_AUTO_WAKE", None),
        ("OULIPOLY_PARENT_INVOCATION", None),
        ("OULIPOLY_DATA_DIR", Some(data_dir.path.as_str())),
        (oulipoly_state::COMPLETION_REGISTRATION_AUTHORITY_ENV, None),
    ]);
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::Success,
    );
    let invocation = oulipoly_state::CompositeInvocationId {
        source: "fixture-provider".to_string(),
        id: "11111111-1111-4111-8111-111111111111".to_string(),
    };
    let authority =
        oulipoly_state::CompletionRegistrationAuthority::from_process_environment_value(
            "ab".repeat(32),
        )
        .expect("valid completion registration authority");
    let launch_environment = authority
        .invocation_launch_environment(&invocation)
        .expect("launch environment");

    execute_external_fixture_effective(&fixture, None, HashMap::new(), Some(launch_environment))
        .expect("external dispatch should separate completion authority from parent identity");

    let policy = read_json(&fixture.policy_record_path);
    let launch = read_json(&fixture.launch_record_path);
    let policy_env = json_object(&policy["params"]["launch"]["env"]);
    assert!(
        policy_env
            .get(oulipoly_state::COMPLETION_REGISTRATION_AUTHORITY_ENV)
            .is_none(),
        "provider policy diagnostics must not receive reusable completion authority"
    );
    let launch_env = json_object(&launch["params"]["env"]);
    for env in [policy_env, launch_env] {
        let parent_identity = env["OULIPOLY_PARENT_INVOCATION"]
            .as_str()
            .expect("parent invocation text");
        assert_eq!(
            serde_json::from_str::<Value>(parent_identity).expect("parent identity"),
            serde_json::to_value(&invocation).expect("expected parent identity")
        );
        assert!(
            !parent_identity
                .contains(oulipoly_state::COMPLETION_REGISTRATION_AUTHORITY_LAUNCH_FIELD)
        );
    }
    assert_eq!(
        launch_env[oulipoly_state::COMPLETION_REGISTRATION_AUTHORITY_ENV],
        authority.process_environment_value()
    );
}

#[test]
fn external_provider_launch_env_inherits_application_agnostic_parent_entries() {
    const JIRA_ENV: &str = "JIRA_API_KEY_REALLY";
    const JIRA_VALUE: &str = "synthetic-jira-api-key-really";
    const SENTINEL_ENV: &str = "UNRELATED_AMBIENT_SENTINEL";
    const SENTINEL_VALUE: &str = "synthetic-unrelated-ambient-value";

    let _lock = env_lock();
    let _env = EnvScope::set_optional(&[
        (JIRA_ENV, Some(JIRA_VALUE)),
        (SENTINEL_ENV, Some(SENTINEL_VALUE)),
    ]);
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::Success,
    );

    execute_external_fixture_effective(&fixture, None, HashMap::new(), None)
        .expect("external dispatch should inherit Unicode parent environment entries");

    let policy = read_json(&fixture.policy_record_path);
    let launch = read_json(&fixture.launch_record_path);
    for env in provider_launch_envs(&policy, &launch) {
        assert_env_value(
            env,
            JIRA_ENV,
            JIRA_VALUE,
            "policy and launch env must carry application-specific entries generically",
        );
        assert_env_value(
            env,
            SENTINEL_ENV,
            SENTINEL_VALUE,
            "unrelated ambient entries must cross the generic provider boundary",
        );
    }
}

#[test]
fn external_provider_launch_env_removes_runner_private_entries() {
    assert_test_catalog_extension();
    let private_names = runner_private_environment_names();
    let _lock = env_lock();
    let inherited = private_names
        .iter()
        .map(|name| (*name, Some("inherited-private")))
        .collect::<Vec<_>>();
    let _env = EnvScope::set_optional(&inherited);
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::Success,
    );
    let mut model = external_model(&fixture);
    model.providers[0].environment = private_names
        .iter()
        .map(|name| (name.to_string(), "configured-private".to_string()))
        .collect();

    execute_external_model_effective(model, None, HashMap::new(), None)
        .expect("external dispatch should remove runner-private entries");

    let policy = read_json(&fixture.policy_record_path);
    let launch = read_json(&fixture.launch_record_path);
    for env in provider_launch_envs(&policy, &launch) {
        for name in &private_names {
            assert!(
                env.get(*name).is_none(),
                "runner-private {name} must not cross the provider boundary"
            );
        }
    }
}

#[test]
fn external_provider_subcommands_do_not_inherit_runner_private_authority() {
    assert_test_catalog_extension();
    let private_names = runner_private_environment_names();
    let _lock = env_lock();
    let invocation = oulipoly_state::CompositeInvocationId {
        source: "fixture-provider".to_string(),
        id: "11111111-1111-4111-8111-111111111111".to_string(),
    };
    let authority =
        oulipoly_state::CompletionRegistrationAuthority::from_process_environment_value(
            "ab".repeat(32),
        )
        .expect("valid completion registration authority");
    let parent_environment = authority
        .invocation_launch_environment(&invocation)
        .expect("parent launch environment");
    let mut inherited = auto_wake_environment_names()
        .into_iter()
        .map(|name| (name, Some("inherited-private")))
        .collect::<Vec<_>>();
    inherited.push((
        oulipoly_state::COMPLETION_REGISTRATION_AUTHORITY_ENV,
        Some(authority.process_environment_value()),
    ));
    inherited.push((
        "OULIPOLY_PARENT_INVOCATION",
        Some(parent_environment.as_str()),
    ));
    let _env = EnvScope::set_optional(&inherited);
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::Success,
    );
    enable_terminal_capability(&fixture);

    execute_external_fixture_effective(&fixture, None, HashMap::new(), Some(parent_environment))
        .expect("external dispatch should sanitize every provider subprocess");

    let records = read_json_lines(&fixture.process_env_record_path);
    assert_eq!(
        records
            .iter()
            .map(|record| record["subcommand"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["describe", "policy.evaluate", "launch", "terminal.classify"]
    );
    for record in records {
        let process_env = json_object(&record["env"]);
        for name in &private_names {
            assert!(
                process_env.get(*name).is_none(),
                "provider {} inherited runner-private {name}",
                record["subcommand"]
            );
        }
    }
}

#[test]
fn external_provider_policy_cannot_reintroduce_runner_private_launch_authority() {
    assert_test_catalog_extension();
    let auto_wake_names = auto_wake_environment_names();
    let _lock = env_lock();
    let invocation = oulipoly_state::CompositeInvocationId {
        source: "fixture-provider".to_string(),
        id: "11111111-1111-4111-8111-111111111111".to_string(),
    };
    let authority =
        oulipoly_state::CompletionRegistrationAuthority::from_process_environment_value(
            "ab".repeat(32),
        )
        .expect("valid completion registration authority");
    let parent_environment = authority
        .invocation_launch_environment(&invocation)
        .expect("parent launch environment");
    let _env = EnvScope::set_optional(&[
        ("OULIPOLY_AUTO_WAKE", Some("1")),
        (
            "OULIPOLY_PARENT_INVOCATION",
            Some(parent_environment.as_str()),
        ),
        (
            oulipoly_state::COMPLETION_REGISTRATION_AUTHORITY_ENV,
            Some(authority.process_environment_value()),
        ),
    ]);
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::PrivateEnvTransform,
        LaunchMode::Success,
    );

    execute_external_fixture_effective(&fixture, None, HashMap::new(), Some(parent_environment))
        .expect("external dispatch should reject policy private-carrier delegation");

    let launch = read_json(&fixture.launch_record_path);
    let launch_env = json_object(&launch["params"]["env"]);
    for name in auto_wake_names {
        assert!(
            launch_env.get(name).is_none(),
            "policy reintroduced runner-private {name}"
        );
    }
    assert_eq!(
        launch_env[oulipoly_state::COMPLETION_REGISTRATION_AUTHORITY_ENV],
        authority.process_environment_value(),
        "typed launch authority must override policy output"
    );
    let parent_identity = launch_env["OULIPOLY_PARENT_INVOCATION"]
        .as_str()
        .expect("parent identity text");
    assert_eq!(
        serde_json::from_str::<Value>(parent_identity).expect("parent identity"),
        serde_json::to_value(invocation).expect("expected parent identity")
    );
    assert!(!parent_identity.contains("_oulipoly_completion_registration_authority"));
}

#[test]
fn external_provider_launch_env_applies_configured_removals_then_overlays() {
    const REMOVED_ENV: &str = "CONFIG_REMOVED_ENV";
    const OVERLAID_ENV: &str = "CONFIG_OVERLAID_ENV";

    let _lock = env_lock();
    let _env = EnvScope::set_optional(&[
        (REMOVED_ENV, Some("ambient-remove-me")),
        (OVERLAID_ENV, Some("ambient-value")),
    ]);
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::Success,
    );
    let mut model = external_model(&fixture);
    model.providers[0].unset_environment = vec![REMOVED_ENV.to_string(), OVERLAID_ENV.to_string()];
    model.providers[0].environment = BTreeMap::from([
        (OVERLAID_ENV.to_string(), "configured-value".to_string()),
        ("CONFIG_ADDED_ENV".to_string(), "added-value".to_string()),
    ]);

    execute_external_model_effective(model, None, HashMap::new(), None)
        .expect("external dispatch should apply provider environment configuration");

    let policy = read_json(&fixture.policy_record_path);
    let launch = read_json(&fixture.launch_record_path);
    for env in provider_launch_envs(&policy, &launch) {
        assert!(
            env.get(REMOVED_ENV).is_none(),
            "configured removals must delete inherited entries"
        );
        assert_env_value(
            env,
            OVERLAID_ENV,
            "configured-value",
            "configured values must replace inherited entries after removals",
        );
        assert_env_value(
            env,
            "CONFIG_ADDED_ENV",
            "added-value",
            "configured values must add entries absent from the parent",
        );
    }
}

fn assert_host_linkage_envs(
    policy: &Value,
    launch: &Value,
    expected_data_dir: &str,
    parent_invocation_env: &str,
    agent_runner_bin: &str,
) {
    for env in provider_launch_envs(policy, launch) {
        assert_env_value(
            env,
            "OULIPOLY_DATA_DIR",
            expected_data_dir,
            "external launch params.env must pin the runner data dir when the process env is scrubbed",
        );
        assert_env_value(
            env,
            "OULIPOLY_PARENT_INVOCATION",
            parent_invocation_env,
            "external launch params.env must carry parent invocation linkage",
        );
        assert_env_value(
            env,
            "AGENT_BASH_AGENT_RUNNER_BIN",
            agent_runner_bin,
            "external launch params.env must let provider-spawned agent-bash notify via the same runner binary",
        );
        assert_env_value(
            env,
            "GENERIC_PARENT_ENV_SENTINEL",
            GENERIC_PARENT_ENV_VALUE,
            "external launch params.env must preserve Unicode parent values",
        );
        assert_env_value(
            env,
            "OPENAI_API_KEY",
            OPENAI_KEY_VALUE,
            "generic runner transport must leave credential policy to the provider",
        );
        assert_env_value(
            env,
            "OPENAI_BASE_URL",
            OPENAI_BASE_URL_VALUE,
            "generic runner transport must leave endpoint policy to the provider",
        );
    }
}

#[test]
fn external_provider_launch_env_does_not_apply_opencode_account_policy() {
    let _lock = env_lock();
    let home = tempdir_path("home tempdir");
    let ambient_xdg = tempdir_path("ambient xdg tempdir");
    let _env = EnvScope::set(&[
        ("HOME", home.path.as_str()),
        ("XDG_DATA_HOME", ambient_xdg.path.as_str()),
        ("OPENAI_API_KEY", "ambient-openai-secret-do-not-propagate"),
        ("OPENAI_BASE_URL", "https://ambient-openai.example.invalid"),
    ]);
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::Success,
    );
    let model = external_model_for_provider(&fixture, "opencode2", "opencode2");
    let registry = dispatch_registry_for_model(&model);

    execute_dispatch_aware_result(
        registry,
        ExecutorServiceRequest::Facade {
            model,
            provider_index: 0,
            prompt: "prompt-value".to_string(),
            working_dir: None,
            models_dir: None,
            extra_inputs: HashMap::new(),
            parent_invocation_env: None,
        },
    )
    .expect("external dispatch should declare selected account auth env");

    let policy = read_json(&fixture.policy_record_path);
    let launch = read_json(&fixture.launch_record_path);
    assert_provider_neutral_parent_envs(&policy, &launch, &home.path, &ambient_xdg.path);
}

fn assert_provider_neutral_parent_envs(
    policy: &Value,
    launch: &Value,
    home: &str,
    ambient_xdg: &str,
) {
    for env in provider_launch_envs(policy, launch) {
        assert_env_value(env, "HOME", home, "parent HOME should be inherited");
        assert_env_value(
            env,
            "XDG_DATA_HOME",
            ambient_xdg,
            "generic runner transport must not rewrite provider account state",
        );
        assert_env_value(
            env,
            "OPENAI_API_KEY",
            "ambient-openai-secret-do-not-propagate",
            "provider policy owns credential filtering",
        );
        assert_env_value(
            env,
            "OPENAI_BASE_URL",
            "https://ambient-openai.example.invalid",
            "provider policy owns endpoint filtering",
        );
    }
}

#[test]
fn external_provider_launch_env_preserves_ambient_xdg_for_provider_policy() {
    let _lock = env_lock();
    let ambient_xdg = tempdir_path("ambient xdg tempdir");
    let _env = EnvScope::set(&[
        ("XDG_DATA_HOME", ambient_xdg.path.as_str()),
        ("OPENAI_API_KEY", "ambient-openai-secret-do-not-propagate"),
    ]);
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::Success,
    );
    let model = external_model_for_provider(&fixture, "opencode", "opencode1");
    let registry = dispatch_registry_for_model(&model);

    execute_dispatch_aware_result(
        registry,
        ExecutorServiceRequest::Facade {
            model,
            provider_index: 0,
            prompt: "prompt-value".to_string(),
            working_dir: None,
            models_dir: None,
            extra_inputs: HashMap::new(),
            parent_invocation_env: None,
        },
    )
    .expect("external dispatch should preserve default opencode auth env");

    let policy = read_json(&fixture.policy_record_path);
    let launch = read_json(&fixture.launch_record_path);
    assert_default_provider_parent_envs(&policy, &launch, &ambient_xdg.path);
}

fn assert_default_provider_parent_envs(policy: &Value, launch: &Value, ambient_xdg: &str) {
    for env in provider_launch_envs(policy, launch) {
        assert_env_value(
            env,
            "XDG_DATA_HOME",
            ambient_xdg,
            "generic runner transport must preserve ambient XDG_DATA_HOME",
        );
        assert_env_value(
            env,
            "OPENAI_API_KEY",
            "ambient-openai-secret-do-not-propagate",
            "provider policy owns OpenAI credential filtering",
        );
    }
}

fn provider_launch_envs<'a>(
    policy: &'a Value,
    launch: &'a Value,
) -> [&'a serde_json::Map<String, Value>; 2] {
    [
        json_object(&policy["params"]["launch"]["env"]),
        json_object(&launch["params"]["env"]),
    ]
}

fn json_object(value: &Value) -> &serde_json::Map<String, Value> {
    value.as_object().expect("params.env should be an object")
}

fn assert_env_value(
    env: &serde_json::Map<String, Value>,
    key: &str,
    expected: &str,
    message: &str,
) {
    assert_eq!(env_string(env, key), Some(expected), "{message}");
}

fn env_string<'a>(env: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    env.get(key).and_then(|value| value.as_str())
}

#[test]
fn external_provider_validates_schema_inputs_before_policy_or_launch() {
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::Success,
    );
    let model = model_with_provider_ref_and_inputs(
        fixture.legacy_trap_path.to_string_lossy().as_ref(),
        Some(provider_ref_path(&fixture.provider_path)),
        vec![InputDef {
            name: "quality".to_string(),
            input_type: InputType::Enum {
                options: vec!["high".to_string(), "low".to_string()],
            },
            required: true,
            default_input: false,
            default: None,
            description: None,
            flag: Some("--quality".to_string()),
        }],
    );
    let registry = dispatch_registry_for_models(std::slice::from_ref(&model));
    let mut extra_inputs = HashMap::new();
    extra_inputs.insert("quality".to_string(), vec!["invalid".to_string()]);

    let error = execute_dispatch_aware_result(
        registry,
        ExecutorServiceRequest::Facade {
            model,
            provider_index: 0,
            prompt: "prompt-value".to_string(),
            working_dir: None,
            models_dir: None,
            extra_inputs,
            parent_invocation_env: None,
        },
    )
    .expect_err("invalid schema inputs must fail before external provider policy");

    assert!(
        error
            .to_string()
            .contains("external provider input validation failed"),
        "input validation should be surfaced at the runner boundary: {error}"
    );
    assert!(!fixture.order_path.exists());
    assert!(!fixture.policy_record_path.exists());
    assert!(!fixture.launch_record_path.exists());
    assert!(!fixture.legacy_record_path.exists());
}

#[test]
fn external_provider_policy_rejection_skips_launch() {
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Reject,
        LaunchMode::Success,
    );

    let error = execute_external_fixture(&fixture)
        .expect_err("policy rejection should be an external dispatch failure");

    let message = error.to_string();
    assert!(message.contains("policy rejected"), "error was: {message}");
    assert!(
        message.contains("policy_live_contract_reject"),
        "provider diagnostic code should be visible: {message}"
    );
    assert!(
        message.contains("params.launch.argv"),
        "provider diagnostic path should be visible: {message}"
    );
    assert!(
        message.contains("provider expected account-specific policy argv shape"),
        "provider diagnostic message should be visible: {message}"
    );
    assert_eq!(order_lines(&fixture.order_path), ["policy.evaluate"]);
    assert!(!fixture.launch_record_path.exists());
    assert!(!fixture.legacy_record_path.exists());
}

#[test]
fn external_provider_policy_request_preserves_provider_owned_settings_id() {
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::Success,
    );
    let mut model = external_model(&fixture);
    model.name = "gpt-low".to_string();
    model.providers[0].name = "opencode".to_string();
    model.providers[0].args = vec![
        "-m".to_string(),
        "openai/gpt-5.5".to_string(),
        "--variant".to_string(),
        "low".to_string(),
    ];
    let mut provider = model.providers[0].clone();
    provider.command = "opencode1".to_string();
    provider.args = vec![
        "run".to_string(),
        "--dangerously-skip-permissions".to_string(),
        "-m".to_string(),
        "openai/gpt-5.5".to_string(),
        "--variant".to_string(),
        "low".to_string(),
    ];
    let registry = dispatch_registry_for_models(std::slice::from_ref(&model));

    execute_dispatch_aware_result(
        registry,
        ExecutorServiceRequest::Effective {
            model,
            provider,
            provider_index: 0,
            prompt_mode: PromptMode::Arg,
            prompt: "prompt-value".to_string(),
            working_dir: None,
            models_dir: None,
            extra_inputs: HashMap::new(),
            parent_invocation_env: None,
        },
    )
    .expect("external dispatch should preserve the provider-owned settings identity");

    let request = read_json(&fixture.policy_record_path);
    assert_eq!(request["provider_instance_id"], "opencode");
    assert_eq!(request["params"]["settings_id"], "opencode");
    assert_eq!(request["params"]["launch"]["command"], "opencode1");
    assert_eq!(
        request["params"]["launch"]["argv"][0],
        serde_json::json!("opencode1")
    );
}

#[test]
fn external_provider_policy_transform_applies_once_and_no_legacy_double_policy() {
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Transform,
        LaunchMode::Success,
    );

    execute_external_fixture(&fixture).expect("external dispatch should succeed");

    let launch = read_json(&fixture.launch_record_path);
    assert_eq!(
        order_lines(&fixture.order_path),
        ["policy.evaluate", "launch"]
    );
    assert_eq!(
        launch["params"]["env"]["POLICY_TRANSFORM_COUNT"], "1",
        "policy env transform must be applied exactly once"
    );
    assert_eq!(
        launch["params"]["argv"],
        serde_json::json!(["provider-a-bin", "--from-policy"]),
        "policy argv transform must reach launch exactly once"
    );
    assert!(
        serde_json::to_string(&launch)
            .expect("launch json serializes")
            .contains("stdin-from-policy"),
        "policy stdin transform must reach launch"
    );
    assert!(
        serde_json::to_string(&launch)
            .expect("launch json serializes")
            .contains("prompt-from-policy"),
        "policy prompt transform must reach launch"
    );
    assert!(!fixture.legacy_record_path.exists());
}

#[test]
fn external_provider_enabled_binary_and_script_refs_dispatch_without_legacy_fallback() {
    for kind in ["binary", "script"] {
        let fixture = make_external_fixture(
            Capabilities {
                policy: true,
                launch: true,
            },
            PolicyMode::Accept,
            LaunchMode::Success,
        );
        let provider_ref = match kind {
            "binary" => provider_ref_binary(
                fixture
                    .provider_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .expect("provider path should have utf8 file name"),
            ),
            "script" => provider_ref_script(&fixture.provider_path),
            _ => unreachable!("test only covers binary and script refs"),
        };
        let model = external_model_with_ref(&fixture, provider_ref);
        let registry = dispatch_registry_for_models_with_options(
            std::slice::from_ref(&model),
            ProviderRegistryOptions::default().with_path_entries([fixture
                .provider_path
                .parent()
                .expect("provider parent")
                .to_path_buf()]),
        );

        let result = execute_dispatch_aware_result(
            registry,
            ExecutorServiceRequest::Facade {
                model,
                provider_index: 0,
                prompt: "prompt-value".to_string(),
                working_dir: None,
                models_dir: None,
                extra_inputs: HashMap::new(),
                parent_invocation_env: None,
            },
        )
        .expect("enabled binary/script refs should dispatch through provider");

        assert_eq!(result.exit_code, 0);
        assert_eq!(
            order_lines(&fixture.order_path),
            ["policy.evaluate", "launch"]
        );
        assert!(!fixture.legacy_record_path.exists());
    }
}

#[test]
fn script_ref_does_not_receive_prompt_acceptance_authority() {
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::Success,
    );
    enable_prompt_acceptance_capability(&fixture);
    let model = external_model_with_ref(&fixture, provider_ref_script(&fixture.provider_path));
    let registry = dispatch_registry_for_models(std::slice::from_ref(&model));

    let result = execute_dispatch_aware_result(
        registry,
        ExecutorServiceRequest::Facade {
            model,
            provider_index: 0,
            prompt: "prompt-value".to_string(),
            working_dir: None,
            models_dir: None,
            extra_inputs: HashMap::new(),
            parent_invocation_env: None,
        },
    )
    .expect("script provider should remain dispatchable");

    assert_eq!(result.exit_code, 0);
    assert!(
        read_json(&fixture.launch_record_path)["params"]["prompt_acceptance"].is_null(),
        "a script path cannot receive prompt-acceptance authority because its executable identity is not pinned through exec"
    );
    assert!(!fixture.legacy_record_path.exists());
}

#[test]
fn external_provider_launch_preserves_stdout_bytes_and_maps_stderr_boundary() {
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::Success,
    );

    let result = execute_external_fixture(&fixture).expect("external launch should succeed");

    assert_eq!(result.stdout, vec![0, 1, 255]);
    assert_eq!(
        result.stderr, "err\u{fffd}\u{fffd}",
        "stderr bytes should cross the existing lossy String boundary deliberately"
    );
    let output = result.output_spool.as_ref().expect("sealed output spool");
    assert_eq!(output.stdout_bytes().expect("complete stdout"), [0, 1, 255]);
    assert_eq!(
        output.stderr_bytes().expect("complete stderr"),
        [b'e', b'r', b'r', 0xff, 0xfe]
    );
    assert_eq!(result.exit_code, 0);
}

#[test]
fn external_provider_launch_spools_output_beyond_diagnostic_retention() {
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::LargeOutput,
    );

    let result = execute_external_fixture(&fixture).expect("large external launch should succeed");
    let output = result.output_spool.as_ref().expect("sealed output spool");
    let summary = output.summary().expect("output summary");

    assert_eq!(summary.stdout_bytes, 2 * 1024 * 1024);
    assert_eq!(summary.stderr_bytes, 0);
    assert_eq!(summary.data_event_count, 256);
    assert_eq!(result.stdout.len(), 1024 * 1024);
    assert_eq!(
        output.stdout_bytes().expect("complete stdout"),
        vec![b'x'; 2 * 1024 * 1024]
    );
}

#[test]
fn external_provider_launch_nonzero_final_exit_is_execution_result() {
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::NonzeroFinal,
    );

    let result =
        execute_external_fixture(&fixture).expect("nonzero final event is a model outcome");

    assert_eq!(result.exit_code, 9);
    assert_eq!(result.stdout, vec![0, 1, 255]);
}

#[test]
fn external_provider_launch_provider_nonzero_after_final_is_diagnostic_only() {
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::ProviderNonzeroAfterFinal,
    );

    let result =
        execute_external_fixture(&fixture).expect("provider nonzero after final is diagnostic");

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, vec![0, 1, 255]);
}

#[test]
fn external_provider_launch_malformed_stream_is_protocol_failure_not_model_exit() {
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::MalformedProtocol,
    );

    let error =
        execute_external_fixture(&fixture).expect_err("malformed stream is a protocol failure");

    assert_external_dispatch_failure(error, "provider protocol", "malformed");
    assert!(!fixture.legacy_record_path.exists());
}

#[test]
fn external_provider_launch_missing_final_is_protocol_failure_not_model_exit() {
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::MissingFinal,
    );

    let error =
        execute_external_fixture(&fixture).expect_err("missing final is a protocol failure");

    assert_external_dispatch_failure(error, "provider protocol", "missing_final");
    assert!(!fixture.legacy_record_path.exists());
}

#[test]
fn external_provider_launch_invalid_base64_is_protocol_failure_not_model_exit() {
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::InvalidBase64,
    );

    let error =
        execute_external_fixture(&fixture).expect_err("invalid base64 is a protocol failure");

    assert_external_dispatch_failure(error, "provider protocol", "invalid_base64");
    assert!(!fixture.legacy_record_path.exists());
}

#[test]
fn external_provider_launch_timeout_or_host_transport_failure_is_not_model_exit() {
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::HostTransportFailure,
    );

    let error = execute_external_fixture(&fixture)
        .expect_err("timeout or host transport failure is not a model exit");

    assert_external_dispatch_failure(error, "provider transport", "stdout_limit");
    assert!(!fixture.legacy_record_path.exists());
}

#[test]
fn external_provider_launch_host_cancelled_before_final_uses_cancellation_fallback_message() {
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::HostCancelledBeforeFinal,
    );
    let model = external_model(&fixture);
    let cancellation = oulipoly_provider::client::CancellationToken::new();
    cancellation.cancel_after(Duration::from_secs(2));
    let client_options = oulipoly_provider::client::ProviderClientOptions::default()
        .with_cancellation(Some(cancellation));
    let registry = dispatch_registry_for_models_with_options(
        std::slice::from_ref(&model),
        ProviderRegistryOptions::default().with_client_options(client_options),
    );

    let error = execute_dispatch_aware_result(
        registry,
        ExecutorServiceRequest::Facade {
            model,
            provider_index: 0,
            prompt: "prompt-value".to_string(),
            working_dir: None,
            models_dir: None,
            extra_inputs: HashMap::new(),
            parent_invocation_env: None,
        },
    )
    .expect_err("host cancellation before provider final event is a fallback path");

    assert_eq!(
        error.to_string(),
        "external provider launch cancelled before final event"
    );
    assert!(!fixture.legacy_record_path.exists());
}

#[test]
fn external_provider_launch_provider_nonzero_before_final_is_transport_failure_not_model_exit() {
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::ProviderNonzeroNoFinal,
    );

    let error = execute_external_fixture(&fixture)
        .expect_err("provider nonzero before final is not a model exit");

    assert_external_dispatch_failure(error, "provider transport", "provider_nonzero_before_final");
    assert!(!fixture.legacy_record_path.exists());
}

#[test]
fn external_provider_launch_provider_emitted_cancelled_final_event_maps_minimal_cancel_outcome() {
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::CancelledFinal,
    );

    let result =
        execute_external_fixture(&fixture).expect("provider-emitted cancellation finalizes");

    assert_eq!(result.terminal_reason.as_deref(), Some("cancelled"));
    assert_eq!(result.exit_code, 130);
}

#[test]
fn external_provider_launch_minimal_terminal_scope_uses_final_event_not_standalone_classify() {
    let fixture = make_external_fixture(
        Capabilities {
            policy: true,
            launch: true,
        },
        PolicyMode::Accept,
        LaunchMode::UnknownTerminalWithQuotaText,
    );

    let result = execute_external_fixture(&fixture)
        .expect("external launch should map only launch final terminal evidence");

    assert_eq!(result.exit_code, -1);
    assert_eq!(result.terminal_reason.as_deref(), Some("unknown_exit"));
    assert_eq!(
        result.terminal_signal.as_ref().map(|signal| &signal.kind),
        Some(&oulipoly_runtime::executor::terminal_signal::TerminalSignalKind::Unknown),
        "S6a must not run standalone terminal.classify over launch stderr text"
    );
}
