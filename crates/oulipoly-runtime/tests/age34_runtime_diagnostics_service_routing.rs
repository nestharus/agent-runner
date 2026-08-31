#![cfg(unix)]

// Declared roles: mapper, formatter, accessor, orchestration, validator.

use oulipoly_config::{
    InvocationMode, ModelConfig, PromptMode, ProviderConfig,
    provider_implementation_ref::ProviderImplementationRef,
};
use oulipoly_runtime::diagnostics::{self, ErrorCategory};
use oulipoly_runtime::provider_registry::{ProviderRegistry, ProviderRegistryOptions};
use oulipoly_runtime::services::{
    DiagnosticsServiceOutput, DiagnosticsServicePort, DiagnosticsServiceRequest,
};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

static TEST_DATA_DIR: OnceLock<tempfile::TempDir> = OnceLock::new();

fn write_executable(path: &Path, body: &str) {
    TEST_DATA_DIR.get_or_init(|| {
        let dir = tempfile::tempdir().expect("test data dir");
        unsafe {
            std::env::set_var(oulipoly_state::paths::DATA_DIR_ENV, dir.path());
        }
        dir
    });
    std::fs::write(path, body).expect("write script");
    let mut perms = std::fs::metadata(path).expect("metadata").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).expect("chmod script");
}

fn migrated_diagnostic_model() -> ModelConfig {
    ModelConfig {
        name: "diagnostic".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::model_provider(
            "diagnostic-provider",
            vec!["--raw-model-arg".to_string()],
        )],
        inputs: vec![],
        provider: None,
    }
}

fn effective_diagnostic_provider(script_path: PathBuf) -> ProviderConfig {
    ProviderConfig {
        environment: Default::default(),
        unset_environment: Default::default(),
        name: "diagnostic-provider".to_string(),
        command: script_path.to_string_lossy().into_owned(),
        args: vec!["--effective-provider-arg".to_string()],
        interactive_args: None,
        resume: None,
        session_capture: None,
        resume_acceptance: None,
        session_storage: None,
        system_prompt_override: None,
        tool_restrictions: None,
        invocation_mode: Default::default(),
    }
}

struct RecordingDiagnosticsService {
    received_effective_provider: Arc<Mutex<Option<ProviderConfig>>>,
}

fn capture_request_provider(request: &DiagnosticsServiceRequest) -> Option<ProviderConfig> {
    match request {
        DiagnosticsServiceRequest::DiagnoseError {
            effective_provider, ..
        } => Some(effective_provider.clone()),
        _ => None,
    }
}

fn store_captured_provider(
    received_effective_provider: &Arc<Mutex<Option<ProviderConfig>>>,
    provider: ProviderConfig,
) {
    *received_effective_provider.lock().unwrap() = Some(provider);
}

fn source_from<'a>(source: &'a str, start: &str) -> &'a str {
    let start_idx = source
        .find(start)
        .unwrap_or_else(|| panic!("missing {start}"));
    &source[start_idx..]
}

impl DiagnosticsServicePort for RecordingDiagnosticsService {
    fn diagnose(
        &self,
        request: DiagnosticsServiceRequest,
    ) -> Result<DiagnosticsServiceOutput, oulipoly_runtime::services::ServiceError> {
        if let Some(provider) = capture_request_provider(&request) {
            store_captured_provider(&self.received_effective_provider, provider);
        }
        diagnostics::RuntimeDiagnosticsService::default().diagnose(request)
    }
}

// Risk: R-A6 / proposal T13 - diagnostics service classify mode must delegate
// to classify_exhaustion and return data only, preserving heuristic semantics.
// Level: unit.
// Source: AGE-34 contract "DiagnosticsServiceRequest" classify branch;
// assumption A6.
#[test]
fn runtime_diagnostics_service_classify_mode_matches_direct_classifier() {
    let stderr = "Billing limit reached for this workspace";
    let direct = diagnostics::classify_exhaustion(stderr);

    let service_impl = diagnostics::RuntimeDiagnosticsService::default();
    let service: &dyn DiagnosticsServicePort = &service_impl;
    let output = service
        .diagnose(DiagnosticsServiceRequest::ClassifyExhaustion {
            stderr: stderr.to_string(),
        })
        .expect("service classify");

    match output {
        DiagnosticsServiceOutput::ExhaustionClassification { is_exhausted } => {
            assert_eq!(is_exhausted, direct);
        }
        other => panic!("expected ExhaustionClassification output, got {other:?}"),
    }
}

// Risk: R-A6 / proposal T13 - diagnostics service model-backed mode must
// consume an already-effective provider and delegate to diagnose_error without
// rewriting prompt/parser/fallback behavior.
// Level: unit/component.
// Source: AGE-34 contract "DiagnosticsServiceRequest" model-backed branch and
// AGE-27 invariant.
#[test]
fn runtime_diagnostics_service_model_backed_mode_matches_direct_diagnose_error() {
    let fixture = diagnostic_provider_fixture();
    let model = migrated_diagnostic_model();
    let provider = effective_diagnostic_provider(fixture.script);
    let stderr = "opaque child failure from primary provider";

    let direct = direct_diagnosis(&model, &provider, 7, stderr, fixture.dir.path());
    let output = service_diagnosis_output(&model, &provider, 7, stderr, fixture.dir.path());

    assert_service_diagnosis_matches_direct(output, direct);
    assert_diagnostic_prompt_dump(&fixture.prompt_dump, stderr);
}

#[test]
fn runtime_diagnostics_service_does_not_dispatch_parse_only_provider_reference() {
    let external_dir = tempfile::tempdir().expect("external tempdir");
    let external_script = external_dir.path().join("external-diagnostic-provider.sh");
    write_executable(&external_script, external_diagnostic_provider_script());
    let fixture = diagnostic_provider_fixture();
    let mut model = migrated_diagnostic_model();
    model.provider = Some(ProviderImplementationRef {
        path: Some(external_script.display().to_string()),
        crate_name: None,
        version: None,
        binary: None,
        script: None,
    });
    let provider = effective_diagnostic_provider(fixture.script);
    let registry = ProviderRegistry::from_model_configs(
        std::slice::from_ref(&model),
        ProviderRegistryOptions::default(),
    )
    .expect("provider registry");
    let service = diagnostics::RuntimeDiagnosticsService::new(Arc::new(registry));

    let output = service
        .diagnose(DiagnosticsServiceRequest::DiagnoseError {
            diagnostics_model: model,
            effective_provider: provider,
            provider_index: 0,
            prompt_mode: PromptMode::Stdin,
            exit_code: 7,
            stderr: "opaque primary provider failure".to_string(),
            working_dir: Some(fixture.dir.path().to_path_buf()),
        })
        .expect("diagnostics should use the effective CLI provider");

    match output {
        DiagnosticsServiceOutput::Diagnosis { diagnosis } => {
            assert_eq!(diagnosis.category, ErrorCategory::NetworkError);
            assert_eq!(diagnosis.summary, "Diagnostic model saw network trouble");
        }
        other => panic!("expected Diagnosis output, got {other:?}"),
    }
    assert_diagnostic_prompt_dump(&fixture.prompt_dump, "opaque primary provider failure");
}

fn external_diagnostic_provider_script() -> &'static str {
    r#"#!/usr/bin/env bash
set -euo pipefail
subcommand="${1:-}"
request="$(cat)"
request_id="$(printf '%s' "$request" | sed -n 's/.*"request_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"

success_envelope() {
  printf '{"contract":"oulipoly.provider/v1","request_id":"%s","ok":true,"result":%s}\n' "$request_id" "$1"
}

case "$subcommand" in
  describe)
    success_envelope '{"provider_id":"external-diagnostics","display_name":"External Diagnostics","contract_versions":["oulipoly.provider/v1"],"preferred_contract":"oulipoly.provider/v1","capabilities":{"launch":true,"policy":true,"quota":false,"session":false,"terminal":false,"rotation":false,"discovery":false,"settings":false,"setup_brain":false,"setup":false,"migration":false},"concurrency":{"safe_for_parallel_invocation":true,"state_locking":"none"}}'
    ;;
  policy.evaluate)
    success_envelope '{"accepted":true,"stdin":null,"prompt":null,"diagnostics":[],"markers":[]}'
    ;;
  launch)
    printf '{"contract":"oulipoly.provider/v1","request_id":"%s","seq":1,"time_unix_ms":1001,"kind":"stdout","data_base64":"bmV0d29ya19lcnJvcgpleHRlcm5hbCByZWdpc3RyeSBkaWFnbm9zdGljcyB3b3JrZWQK"}\n' "$request_id"
    printf '{"contract":"oulipoly.provider/v1","request_id":"%s","seq":2,"time_unix_ms":1002,"kind":"exit","status":{"kind":"exited","code":0},"terminal_signal":{"kind":"clean_exit","evidence":"diagnostics-complete","observed_at_unix_ms":1002}}\n' "$request_id"
    ;;
  *)
    exit 64
    ;;
esac
"#
}

struct DiagnosticProviderFixture {
    dir: tempfile::TempDir,
    script: PathBuf,
    prompt_dump: PathBuf,
}

fn diagnostic_provider_fixture() -> DiagnosticProviderFixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let prompt_dump = dir.path().join("diagnostic-prompt.txt");
    let script = dir.path().join("diagnostic-provider.sh");
    write_executable(&script, &diagnostic_provider_script(&prompt_dump));
    DiagnosticProviderFixture {
        dir,
        script,
        prompt_dump,
    }
}

fn diagnostic_provider_script(prompt_dump: &Path) -> String {
    format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
cat > "{prompt_dump}"
printf 'network_error\nDiagnostic model saw network trouble\n'
"#,
        prompt_dump = prompt_dump.display()
    )
}

fn direct_diagnosis(
    model: &ModelConfig,
    provider: &ProviderConfig,
    exit_code: i32,
    stderr: &str,
    working_dir: &Path,
) -> diagnostics::Diagnosis {
    diagnostics::diagnose_error(
        model,
        provider,
        0,
        PromptMode::Stdin,
        exit_code,
        stderr,
        Some(working_dir),
    )
    .expect("direct diagnose")
}

fn service_diagnosis_output(
    model: &ModelConfig,
    provider: &ProviderConfig,
    exit_code: i32,
    stderr: &str,
    working_dir: &Path,
) -> DiagnosticsServiceOutput {
    let service_impl = diagnostics::RuntimeDiagnosticsService::default();
    let service: &dyn DiagnosticsServicePort = &service_impl;
    service
        .diagnose(DiagnosticsServiceRequest::DiagnoseError {
            diagnostics_model: model.clone(),
            effective_provider: provider.clone(),
            provider_index: 0,
            prompt_mode: PromptMode::Stdin,
            exit_code,
            stderr: stderr.to_string(),
            working_dir: Some(working_dir.to_path_buf()),
        })
        .expect("service diagnose")
}

fn assert_service_diagnosis_matches_direct(
    output: DiagnosticsServiceOutput,
    direct: diagnostics::Diagnosis,
) {
    match output {
        DiagnosticsServiceOutput::Diagnosis { diagnosis } => {
            assert_eq!(diagnosis.category, direct.category);
            assert_eq!(diagnosis.summary, direct.summary);
            assert_eq!(diagnosis.category, ErrorCategory::NetworkError);
        }
        other => panic!("expected Diagnosis output, got {other:?}"),
    }
}

fn assert_diagnostic_prompt_dump(prompt_dump: &Path, stderr: &str) {
    let prompt = std::fs::read_to_string(prompt_dump).expect("prompt dump");
    assert!(prompt.contains("Exit code: 7"), "{prompt}");
    assert!(prompt.contains(stderr), "{prompt}");
    assert!(
        !prompt.contains("Empty command"),
        "service must use the already-effective provider supplied by the caller"
    );
}

#[test]
fn runtime_diagnostics_service_preserves_invocation_mode() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("diagnostic-provider.sh");
    write_executable(
        &script,
        r#"#!/usr/bin/env bash
set -euo pipefail
cat >/dev/null
printf 'network_error\nDiagnostic model saw network trouble\n'
"#,
    );
    let model = migrated_diagnostic_model();
    let mut provider = effective_diagnostic_provider(script);
    provider.invocation_mode = InvocationMode::Proxy;

    let received_effective_provider = Arc::new(Mutex::new(None));
    let recording_service = RecordingDiagnosticsService {
        received_effective_provider: Arc::clone(&received_effective_provider),
    };
    let service: &dyn DiagnosticsServicePort = &recording_service;
    let output = service
        .diagnose(DiagnosticsServiceRequest::DiagnoseError {
            diagnostics_model: model,
            effective_provider: provider.clone(),
            provider_index: 0,
            prompt_mode: PromptMode::Stdin,
            exit_code: 7,
            stderr: "opaque child failure from primary provider".to_string(),
            working_dir: Some(dir.path().to_path_buf()),
        })
        .expect("service diagnose");

    assert_eq!(
        received_effective_provider
            .lock()
            .unwrap()
            .as_ref()
            .map(|provider| provider.invocation_mode),
        Some(InvocationMode::Proxy)
    );
    assert!(matches!(output, DiagnosticsServiceOutput::Diagnosis { .. }));
}

#[test]
fn diagnose_error_preserves_invocation_mode_into_executor_reentry() {
    let source = include_str!("../src/diagnostics/mod.rs");
    let body = source_from(source, "pub fn diagnose_error");
    let reentry_idx = body
        .find("executor::cli::execute_effective(request)")
        .expect("diagnose_error must re-enter the effective-provider executor");
    let before_reentry = &body[..reentry_idx];

    assert!(
        body.contains("provider: effective_provider"),
        "diagnose_error must thread the caller-supplied effective provider into executor re-entry"
    );
    assert!(
        !before_reentry.contains("ProviderConfig {"),
        "diagnose_error must not rebuild ProviderConfig before executor re-entry"
    );
}
