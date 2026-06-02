//! Declared roles: mapper

use super::accessor::load_diagnostics_dependencies;
use agent_runner_lib::effective_provider_for_model_provider;
use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig, ProvidersConfig};
use oulipoly_runtime::diagnostics::Diagnosis;
use oulipoly_runtime::services::DiagnosticsServiceOutput;
use oulipoly_runtime::services::DiagnosticsServiceRequest;
use std::collections::HashMap;
use std::path::Path;

pub(super) struct DiagnosticsContext {
    pub(super) diag_model: ModelConfig,
    pub(super) provider: ProviderConfig,
    pub(super) prompt_mode: PromptMode,
}

pub(super) struct DiagnosticsDependencies {
    pub(super) diag_model: ModelConfig,
    pub(super) providers_cfg: ProvidersConfig,
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
