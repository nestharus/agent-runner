//! AGE-242 Phase 2.5 characterization for built-in terminal preservation.
//!
//! These tests pin the current no-external-provider behavior before S6c adds
//! external-provider `terminal.classify` dispatch. They intentionally exercise
//! the dispatch-aware runtime executor service with a populated unrelated
//! provider registry while the executed model has no provider implementation
//! reference.

#![cfg(unix)]

use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, provider_implementation_ref::ProviderImplementationRef,
};
use oulipoly_runtime::executor;
use oulipoly_runtime::executor::cli::{self, EffectiveExecuteRequest};
use oulipoly_runtime::executor::terminal_signal::{TerminalSignal, TerminalSignalKind};
use oulipoly_runtime::provider_registry::{ProviderRegistry, ProviderRegistryOptions};
use oulipoly_runtime::services::{ExecutorServicePort, ExecutorServiceRequest};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

const BUILTIN_PROVIDER: &str = "neutral-built-in-provider";
const OPENCODE_PROVIDER: &str = "opencode";
const UNRELATED_MODEL: &str = "neutral-unrelated-external-model";
static TEST_DATA_DIR: OnceLock<tempfile::TempDir> = OnceLock::new();

struct ScriptFixture {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

#[derive(Debug)]
struct ExecutionPair {
    direct: executor::ExecutionResult,
    service: executor::ExecutionResult,
}

enum TerminalCase {
    Script {
        label: &'static str,
        body: &'static str,
    },
    SpawnError,
}

impl TerminalCase {
    fn label(&self) -> &'static str {
        match self {
            TerminalCase::Script { label, .. } => label,
            TerminalCase::SpawnError => "spawn_error",
        }
    }
}

fn fixture_script(name: &str, body: &str) -> ScriptFixture {
    TEST_DATA_DIR.get_or_init(|| {
        let dir = tempfile::tempdir().expect("test data dir");
        unsafe {
            std::env::set_var(oulipoly_state::paths::DATA_DIR_ENV, dir.path());
        }
        dir
    });
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(name);
    write_executable_script(&path, &script_body(body));
    ScriptFixture { _dir: dir, path }
}

fn script_body(body: &str) -> String {
    format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n")
}

fn write_executable_script(path: &Path, contents: &str) {
    fs::write(path, contents).expect("write script");
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

fn builtin_model_for_command(command: &str) -> ModelConfig {
    builtin_model_for_provider_command(BUILTIN_PROVIDER, command)
}

fn builtin_model_for_provider_command(provider_name: &str, command: &str) -> ModelConfig {
    ModelConfig {
        name: "neutral-built-in-terminal-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig {
            environment: Default::default(),
            unset_environment: Default::default(),
            name: provider_name.to_string(),
            command: command.to_string(),
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
        provider: None,
    }
}

fn opencode_builtin_model_for_command(command: &str) -> ModelConfig {
    builtin_model_for_provider_command(OPENCODE_PROVIDER, command)
}

fn unrelated_external_model(script: &ScriptFixture) -> ModelConfig {
    ModelConfig {
        name: UNRELATED_MODEL.to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig {
            environment: Default::default(),
            unset_environment: Default::default(),
            name: "neutral-unrelated-provider-account".to_string(),
            command: "neutral-unrelated-command".to_string(),
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

fn unrelated_registry(script: &ScriptFixture) -> ProviderRegistry {
    ProviderRegistry::from_model_configs(
        &[unrelated_external_model(script)],
        ProviderRegistryOptions::default(),
    )
    .expect("unrelated provider registry should construct")
}

fn assert_unrelated_registry_is_populated(registry: &ProviderRegistry) {
    assert_eq!(registry.configured_model_names(), [UNRELATED_MODEL]);
    assert_eq!(registry.configured_artifact_keys().len(), 1);
}

fn direct_execute(model: &ModelConfig) -> Result<executor::ExecutionResult, String> {
    let extra_inputs = empty_extra_inputs();
    invoke_direct_execute(direct_execute_request(model, &extra_inputs))
}

fn execute_opencode_fixture(body: &str) -> ExecutionPair {
    let script = fixture_script("opencode-built-in.sh", body);
    let model = opencode_builtin_model_for_command(&script.path.to_string_lossy());
    let unrelated = fixture_script(
        "neutral-unrelated-provider.sh",
        "printf 'unrelated external provider should not run\\n'",
    );
    let registry = unrelated_registry(&unrelated);
    let direct = direct_execute(&model).expect("direct opencode execution should succeed");
    let service =
        service_execute(registry, model).expect("service opencode execution should succeed");
    ExecutionPair { direct, service }
}

fn empty_extra_inputs() -> HashMap<String, Vec<String>> {
    HashMap::new()
}

fn direct_execute_request<'a>(
    model: &'a ModelConfig,
    extra_inputs: &'a HashMap<String, Vec<String>>,
) -> EffectiveExecuteRequest<'a> {
    EffectiveExecuteRequest {
        model,
        provider: &model.providers[0],
        provider_index: 0,
        prompt_mode: PromptMode::Arg,
        prompt: "prompt-value",
        working_dir: None,
        models_dir: None,
        extra_inputs,
        parent_invocation_env: None,
    }
}

fn invoke_direct_execute(
    request: EffectiveExecuteRequest<'_>,
) -> Result<executor::ExecutionResult, String> {
    cli::execute_effective(request)
}

fn service_execute(
    registry: ProviderRegistry,
    model: ModelConfig,
) -> Result<executor::ExecutionResult, String> {
    assert_unrelated_registry_is_populated(&registry);
    let service = runtime_executor_service(registry);
    let request = service_facade_request(model, empty_extra_inputs());
    invoke_service_execute(&service, request).map_err(format_service_error)
}

fn runtime_executor_service(registry: ProviderRegistry) -> executor::RuntimeExecutorService {
    executor::RuntimeExecutorService::new(Arc::new(registry))
}

fn service_facade_request(
    model: ModelConfig,
    extra_inputs: HashMap<String, Vec<String>>,
) -> ExecutorServiceRequest {
    ExecutorServiceRequest::Facade {
        model,
        provider_index: 0,
        prompt: "prompt-value".to_string(),
        working_dir: None,
        models_dir: None,
        extra_inputs,
        parent_invocation_env: None,
    }
}

fn invoke_service_execute(
    service: &executor::RuntimeExecutorService,
    request: ExecutorServiceRequest,
) -> Result<executor::ExecutionResult, oulipoly_runtime::services::ServiceError> {
    service.execute(request).map(|output| output.result)
}

fn format_service_error(error: oulipoly_runtime::services::ServiceError) -> String {
    error.to_string()
}

fn run_case_with_registry(
    case: TerminalCase,
    registry: ProviderRegistry,
) -> Result<ExecutionPair, (String, String)> {
    match case {
        TerminalCase::Script { body, .. } => {
            let script = fixture_script("neutral-built-in.sh", body);
            let model = builtin_model_for_command(&script.path.to_string_lossy());
            let direct = direct_execute(&model).expect("direct built-in execution should succeed");
            let service =
                service_execute(registry, model).expect("service no-ref execution should succeed");
            Ok(ExecutionPair { direct, service })
        }
        TerminalCase::SpawnError => {
            let dir = tempfile::tempdir().expect("tempdir");
            let missing = dir.path().join("missing-neutral-provider");
            let model = builtin_model_for_command(&missing.to_string_lossy());
            let direct = direct_execute(&model).expect_err("direct spawn error expected");
            let service =
                service_execute(registry, model).expect_err("service spawn error expected");
            Err((direct, service))
        }
    }
}

fn run_case(case: TerminalCase) -> Result<ExecutionPair, (String, String)> {
    let unrelated = fixture_script(
        "neutral-unrelated-provider.sh",
        "printf 'unrelated external provider should not run\\n'",
    );
    run_case_with_registry(case, unrelated_registry(&unrelated))
}

fn success_cases() -> Vec<TerminalCase> {
    vec![
        TerminalCase::Script {
            label: "clean_exit",
            body: "printf 'clean stdout:%s\\n' \"$1\"\nprintf 'clean stderr\\n' >&2\nexit 0",
        },
        TerminalCase::Script {
            label: "nonzero_exit",
            body: "printf 'nonzero stdout:%s\\n' \"$1\"\nprintf 'nonzero stderr\\n' >&2\nexit 17",
        },
        TerminalCase::Script {
            label: "signal_exit",
            body: "printf 'signal stdout\\n'\nprintf 'signal stderr\\n' >&2\nkill -TERM $$",
        },
        TerminalCase::Script {
            label: "quota_looking_stdout_clean",
            body: "printf 'quota exhausted maybe rate limited\\n'\nexit 0",
        },
        TerminalCase::Script {
            label: "quota_looking_stderr_nonzero",
            body: "printf 'quota exhausted maybe rate limited\\n' >&2\nexit 23",
        },
        TerminalCase::Script {
            label: "binary_stdout",
            body: "printf 'raw\\000\\377Z'\nexit 0",
        },
    ]
}

fn assert_execution_observables_match(pair: &ExecutionPair, label: &str) {
    assert_eq!(pair.service.stdout, pair.direct.stdout, "{label}: stdout");
    assert_eq!(pair.service.stderr, pair.direct.stderr, "{label}: stderr");
    assert_eq!(
        pair.service.exit_code, pair.direct.exit_code,
        "{label}: exit_code"
    );
    assert_eq!(
        pair.service.terminal_reason, pair.direct.terminal_reason,
        "{label}: terminal_reason"
    );
    assert_terminal_signal_matches_except_observed_at(
        pair.service.terminal_signal.as_ref(),
        pair.direct.terminal_signal.as_ref(),
        label,
    );
}

fn assert_terminal_signal_matches_except_observed_at(
    service: Option<&TerminalSignal>,
    direct: Option<&TerminalSignal>,
    label: &str,
) {
    let (Some(service), Some(direct)) = (service, direct) else {
        assert_eq!(
            service.is_some(),
            direct.is_some(),
            "{label}: terminal signal presence"
        );
        return;
    };

    assert_eq!(service.kind, direct.kind, "{label}: terminal_signal.kind");
    assert_eq!(
        service.provider_name, direct.provider_name,
        "{label}: terminal_signal.provider_name"
    );
    assert_eq!(
        service.evidence, direct.evidence,
        "{label}: terminal_signal.evidence"
    );
}

fn terminal_signal(result: &executor::ExecutionResult) -> &TerminalSignal {
    result
        .terminal_signal
        .as_ref()
        .expect("built-in execution should carry terminal signal evidence")
}

fn initialize_counter_file() -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().expect("counter");
    fs::write(file.path(), "0").expect("initialize counter");
    file
}

fn counter_provider_script(counter_path: &Path) -> String {
    format!(
        "count=$(cat {counter}); count=$((count + 1)); printf '%s' \"$count\" > {counter}; printf '{{\"contract\":\"oulipoly.provider/v1\",\"request_id\":\"request-example-001\",\"ok\":true,\"result\":{{}}}}\\n'",
        counter = shell_quote(counter_path)
    )
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn assert_counter_unchanged(path: &Path) {
    assert_eq!(
        fs::read_to_string(path).expect("counter readable"),
        "0",
        "no-ref terminal classification must not describe or invoke the unrelated provider"
    );
}

fn assert_spawn_error_matches_direct((direct_error, service_error): (String, String)) {
    assert_eq!(service_error, direct_error, "spawn_error: public error");
    assert_spawn_error_prefix(&service_error);
}

fn assert_spawn_error_prefix(error: &str) {
    assert!(
        error.starts_with("Failed to spawn"),
        "spawn_error: expected canonical spawn prefix, got {error:?}"
    );
}

#[test]
fn builtin_no_ref_terminal_matrix_preserves_direct_executor_observables_with_unrelated_registry() {
    for case in success_cases() {
        let label = case.label();
        let pair = run_case(case).expect("success case should execute");
        assert_execution_observables_match(&pair, label);
    }

    assert_spawn_error_matches_direct(spawn_error_pair());
}

fn spawn_error_pair() -> (String, String) {
    run_case(TerminalCase::SpawnError).expect_err("spawn-error case should fail")
}

struct EvidenceExpectation {
    case: TerminalCase,
    expected_kind: TerminalSignalKind,
    expected_evidence: &'static str,
    expected_reason: Option<&'static str>,
}

fn evidence_expectations() -> Vec<EvidenceExpectation> {
    vec![
        EvidenceExpectation {
            case: TerminalCase::Script {
                label: "nonzero_evidence",
                body: "printf 'status text stderr\\n' >&2\nexit 37",
            },
            expected_kind: TerminalSignalKind::NonzeroExit,
            expected_evidence: "exit_code=37",
            expected_reason: Some("exit_nonzero"),
        },
        EvidenceExpectation {
            case: TerminalCase::Script {
                label: "signal_evidence",
                body: "kill -TERM $$",
            },
            expected_kind: TerminalSignalKind::SignalExit,
            expected_evidence: "signal=15",
            expected_reason: Some("signal:SIGTERM"),
        },
    ]
}

fn assert_builtin_evidence_expectation(expectation: EvidenceExpectation) {
    let label = expectation.case.label();
    let expected_kind = expectation.expected_kind;
    let expected_evidence = expectation.expected_evidence;
    let expected_reason = expectation.expected_reason;
    let pair = run_case(expectation.case).expect("evidence case should execute");
    assert_execution_observables_match(&pair, label);
    assert_service_signal_evidence(&pair, expected_kind, expected_evidence, label);
    assert_service_terminal_reason(&pair, expected_reason, label);
}

fn assert_service_signal_evidence(
    pair: &ExecutionPair,
    expected_kind: TerminalSignalKind,
    expected_evidence: &str,
    label: &str,
) {
    let signal = terminal_signal(&pair.service);
    assert_eq!(signal.kind, expected_kind, "{label}: kind");
    assert_eq!(signal.provider_name, BUILTIN_PROVIDER, "{label}: provider");
    assert_eq!(signal.evidence, expected_evidence, "{label}: evidence");
}

fn assert_service_terminal_reason(
    pair: &ExecutionPair,
    expected_reason: Option<&str>,
    label: &str,
) {
    assert_eq!(
        pair.service.terminal_reason.as_deref(),
        expected_reason,
        "{label}: terminal_reason"
    );
}

#[test]
fn builtin_no_ref_terminal_signal_evidence_preserves_provider_name_and_status_text() {
    for expectation in evidence_expectations() {
        assert_builtin_evidence_expectation(expectation);
    }
}

fn counter_backed_external_fixture() -> (tempfile::NamedTempFile, ScriptFixture) {
    let counter = initialize_counter_file();
    let external = fixture_script(
        "neutral-provider.sh",
        &counter_provider_script(counter.path()),
    );
    (counter, external)
}

fn no_ref_registry_bypass_cases() -> Vec<TerminalCase> {
    vec![
        TerminalCase::Script {
            label: "clean_exit",
            body: "printf 'ok\\n'\nexit 0",
        },
        TerminalCase::Script {
            label: "nonzero_exit",
            body: "printf 'failed\\n' >&2\nexit 9",
        },
        TerminalCase::Script {
            label: "quota_looking_clean",
            body: "printf 'quota exhausted maybe rate limited\\n'\nexit 0",
        },
    ]
}

fn assert_service_signal_provider_is_builtin(pair: &ExecutionPair) {
    assert_eq!(
        terminal_signal(&pair.service).provider_name,
        BUILTIN_PROVIDER
    );
}

#[test]
fn builtin_no_ref_does_not_invoke_unrelated_provider_registry_for_terminal_classification() {
    let (counter, external) = counter_backed_external_fixture();

    for case in no_ref_registry_bypass_cases() {
        let pair = run_case_with_registry(case, unrelated_registry(&external))
            .expect("no-ref execution should stay on built-in path");
        assert_service_signal_provider_is_builtin(&pair);
    }
    assert_counter_unchanged(counter.path());
}

#[test]
fn opencode_json_error_429_is_rate_limited() {
    let pair = execute_opencode_fixture(
        r#"printf '%s\n' '{"type":"error","sessionID":"ses_fixture","error":{"data":{"message":"Rate limit exceeded","statusCode":429}}}'
exit 1"#,
    );

    for (label, result) in [("direct", &pair.direct), ("service", &pair.service)] {
        let signal = terminal_signal(result);
        assert_eq!(signal.kind, TerminalSignalKind::RateLimited, "{label}");
        assert_eq!(signal.provider_name, OPENCODE_PROVIDER, "{label}");
        assert!(
            signal.evidence.contains("Rate limit exceeded"),
            "{label}: evidence={:?}",
            signal.evidence
        );
        assert_eq!(
            result.terminal_reason.as_deref(),
            Some("rate_limited"),
            "{label}"
        );
    }
}

#[test]
fn opencode_json_error_persistent_quota_is_quota_exhausted() {
    let pair = execute_opencode_fixture(
        r#"printf '%s\n' '{"type":"error","sessionID":"ses_fixture","error":{"data":{"message":"Insufficient quota for this account"}}}' >&2
exit 1"#,
    );

    for (label, result) in [("direct", &pair.direct), ("service", &pair.service)] {
        let signal = terminal_signal(result);
        assert_eq!(
            signal.kind,
            TerminalSignalKind::QuotaExhaustedInband,
            "{label}"
        );
        assert_eq!(signal.provider_name, OPENCODE_PROVIDER, "{label}");
        assert!(
            signal.evidence.contains("Insufficient quota"),
            "{label}: evidence={:?}",
            signal.evidence
        );
        assert_eq!(
            result.terminal_reason.as_deref(),
            Some("quota_exhausted_inband"),
            "{label}"
        );
    }
}
