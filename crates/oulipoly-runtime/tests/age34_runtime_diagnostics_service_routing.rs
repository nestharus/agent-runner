#![cfg(unix)]

use oulipoly_config::{InvocationMode, ModelConfig, PromptMode, ProviderConfig};
use oulipoly_runtime::diagnostics::{self, ErrorCategory};
use oulipoly_runtime::services::{
    DiagnosticsServiceOutput, DiagnosticsServicePort, DiagnosticsServiceRequest,
};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

fn write_executable(path: &Path, body: &str) {
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
    }
}

fn effective_diagnostic_provider(script_path: PathBuf) -> ProviderConfig {
    ProviderConfig {
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

impl DiagnosticsServicePort for RecordingDiagnosticsService {
    fn diagnose(
        &self,
        request: DiagnosticsServiceRequest,
    ) -> Result<DiagnosticsServiceOutput, oulipoly_runtime::services::ServiceError> {
        if let Some(provider) = capture_request_provider(&request) {
            store_captured_provider(&self.received_effective_provider, provider);
        }
        diagnostics::RuntimeDiagnosticsService.diagnose(request)
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

    let service: &dyn DiagnosticsServicePort = &diagnostics::RuntimeDiagnosticsService;
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
    let dir = tempfile::tempdir().expect("tempdir");
    let prompt_dump = dir.path().join("diagnostic-prompt.txt");
    let script = dir.path().join("diagnostic-provider.sh");
    write_executable(
        &script,
        &format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
cat > "{prompt_dump}"
printf 'network_error\nDiagnostic model saw network trouble\n'
"#,
            prompt_dump = prompt_dump.display()
        ),
    );
    let model = migrated_diagnostic_model();
    let provider = effective_diagnostic_provider(script);
    let stderr = "opaque child failure from primary provider";

    let direct = diagnostics::diagnose_error(
        &model,
        &provider,
        0,
        PromptMode::Stdin,
        7,
        stderr,
        Some(dir.path()),
    )
    .expect("direct diagnose");

    let service: &dyn DiagnosticsServicePort = &diagnostics::RuntimeDiagnosticsService;
    let output = service
        .diagnose(DiagnosticsServiceRequest::DiagnoseError {
            diagnostics_model: model.clone(),
            effective_provider: provider.clone(),
            provider_index: 0,
            prompt_mode: PromptMode::Stdin,
            exit_code: 7,
            stderr: stderr.to_string(),
            working_dir: Some(dir.path().to_path_buf()),
        })
        .expect("service diagnose");

    match output {
        DiagnosticsServiceOutput::Diagnosis { diagnosis } => {
            assert_eq!(diagnosis.category, direct.category);
            assert_eq!(diagnosis.summary, direct.summary);
            assert_eq!(diagnosis.category, ErrorCategory::NetworkError);
        }
        other => panic!("expected Diagnosis output, got {other:?}"),
    }

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
