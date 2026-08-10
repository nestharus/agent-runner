//! Declared roles: mapper

use super::accessor::load_diagnostics_dependencies;
use agent_runner_lib::effective_provider_for_model_provider;
use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig, ProvidersConfig};
use oulipoly_runtime::diagnostics::Diagnosis;
use oulipoly_runtime::diagnostics::ErrorCategory;
use oulipoly_runtime::services::DiagnosticsServiceOutput;
use oulipoly_runtime::services::DiagnosticsServiceRequest;
use std::collections::HashMap;
use std::path::Path;

const EXTERNAL_PROVIDER_PROTOCOL_PREFIX: &str = "external provider protocol failed: ";

pub(super) struct DiagnosticsContext {
    pub(super) diag_model: ModelConfig,
    pub(super) provider: ProviderConfig,
    pub(super) prompt_mode: PromptMode,
}

pub(super) struct DiagnosticsDependencies {
    pub(super) diag_model: ModelConfig,
    pub(super) providers_cfg: ProvidersConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(super) struct DiagnosticsFailure {
    pub(super) stage: &'static str,
    pub(super) operation: String,
    pub(super) error_category: &'static str,
    pub(super) message: String,
    pub(super) provider_exit_code: i32,
}

pub(super) fn diagnostics_failure(error: &str, provider_exit_code: i32) -> DiagnosticsFailure {
    let protocol_operation = error.strip_prefix(EXTERNAL_PROVIDER_PROTOCOL_PREFIX);
    DiagnosticsFailure {
        stage: "diagnostics",
        operation: protocol_operation.unwrap_or("diagnose_error").to_string(),
        error_category: protocol_operation
            .map(|_| ErrorCategory::ProviderProtocol)
            .unwrap_or(ErrorCategory::DiagnosticsFailure)
            .as_str(),
        message: error.to_string(),
        provider_exit_code,
    }
}

pub(super) fn diagnostics_dependencies(
    diag_model: ModelConfig,
    providers_cfg: ProvidersConfig,
) -> DiagnosticsDependencies {
    DiagnosticsDependencies {
        diag_model,
        providers_cfg,
    }
}

pub(super) fn diagnostics_context(
    models: &HashMap<String, ModelConfig>,
) -> Option<DiagnosticsContext> {
    diagnostics_context_from_dependencies(load_diagnostics_dependencies(models)?)
}

pub(super) fn diagnostics_context_from_dependencies(
    dependencies: DiagnosticsDependencies,
) -> Option<DiagnosticsContext> {
    let (provider, prompt_mode) = effective_provider_for_model_provider(
        &dependencies.diag_model,
        0,
        &dependencies.providers_cfg,
    )
    .ok()?;
    Some(DiagnosticsContext {
        diag_model: dependencies.diag_model,
        provider,
        prompt_mode,
    })
}

pub(super) fn diagnostics_service_request(
    context: DiagnosticsContext,
    provider_output: &str,
    exit_code: i32,
    working_dir: Option<&Path>,
) -> DiagnosticsServiceRequest {
    DiagnosticsServiceRequest::DiagnoseError {
        diagnostics_model: context.diag_model,
        effective_provider: context.provider,
        provider_index: 0,
        prompt_mode: context.prompt_mode,
        exit_code,
        stderr: provider_output.to_string(),
        working_dir: working_dir.map(Path::to_path_buf),
    }
}

pub(super) fn diagnosis_from_output(output: DiagnosticsServiceOutput) -> Option<Diagnosis> {
    match output {
        DiagnosticsServiceOutput::Diagnosis { diagnosis } => Some(diagnosis),
        DiagnosticsServiceOutput::ExhaustionClassification { .. }
        | DiagnosticsServiceOutput::TerminalClassification(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_lookup_failure_retains_operation_and_provider_exit() {
        let failure = diagnostics_failure("external provider protocol failed: registry_lookup", 7);

        assert_eq!(failure.stage, "diagnostics");
        assert_eq!(failure.operation, "registry_lookup");
        assert_eq!(failure.error_category, "provider_protocol");
        assert_eq!(failure.provider_exit_code, 7);
    }

    #[test]
    fn non_protocol_failure_has_typed_diagnostics_category() {
        let failure = diagnostics_failure("diagnostics service unavailable", 1);

        assert_eq!(failure.operation, "diagnose_error");
        assert_eq!(failure.error_category, "diagnostics_failure");
    }
}
