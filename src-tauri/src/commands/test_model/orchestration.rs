//! ## Declared roles
//!
//! `orchestration`

use oulipoly_config::repositories::FilesystemProvidersConfigRepository;
use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig, ProvidersConfig};
use oulipoly_state::repositories::ProductionStateDbOpener;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::app_state::AppState;

use super::{diagnostics_fallback, dispatch, lookup, mapper, validator};
use mapper::{TestModelResult, TestModelServices};

#[tauri::command]
pub(crate) async fn test_model(
    state: tauri::State<'_, AppState>,
    name: String,
) -> Result<TestModelResult, String> {
    let (model, db_path) =
        lookup::test_model_command_inputs(&state.models, &state.models_dir, &name)?;
    let models_dir = state.models_dir.clone();
    let state_db_opener = state.state_db_opener.clone();
    let providers_config = state.providers_config.clone();
    let routing_service = state.routing_service.clone();
    let executor_service = state.executor_service.clone();
    let diagnostics_service = state.diagnostics_service.clone();

    tauri::async_runtime::spawn_blocking(move || {
        let services = mapper::test_model_services_from_parts(
            &*state_db_opener,
            &*providers_config,
            &*routing_service,
            &*executor_service,
            &*diagnostics_service,
        );
        test_model_with_db_path(
            services,
            model,
            models_dir,
            db_path,
            "Say hello in one sentence.",
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

pub fn test_model_with_db_path(
    services: TestModelServices<'_>,
    model: ModelConfig,
    models_dir: PathBuf,
    db_path: PathBuf,
    prompt: &str,
) -> Result<TestModelResult, String> {
    let context = lookup::test_model_context(
        services.state_db_opener,
        services.providers_repository,
        &models_dir,
        &db_path,
    )?;
    let provider_index =
        dispatch::select_test_model_route(services.routing_service, &model, &context.db)?;
    let (provider, prompt_mode) =
        effective_provider_for_model_provider(&model, provider_index, &context.providers_cfg)?;
    let request = mapper::build_effective_executor_request(
        model,
        provider.clone(),
        provider_index,
        prompt_mode,
        prompt,
    );
    let result = dispatch::execute_effective_request(services.executor_service, request)?;
    apply_exhaustion_disposition(&services, &context.db, &provider.name, &result)?;
    Ok(mapper::map_test_model_result(&result))
}

fn apply_exhaustion_disposition(
    services: &TestModelServices<'_>,
    db: &oulipoly_state::StateDb,
    provider_name: &str,
    result: &oulipoly_runtime::executor::ExecutionResult,
) -> Result<(), String> {
    if !validator::should_run_diagnostics_fallback(result.exit_code) {
        return Ok(());
    }

    let should_mark_exhausted = if let Some(signal) = result.terminal_signal.as_ref() {
        validator::typed_signal_is_quota_exhausted_inband(signal.kind)
    } else {
        diagnostics_fallback::diagnostics_fallback_should_mark_exhausted(services, result)?
    };

    if validator::should_mark_quota_exhausted(should_mark_exhausted) {
        dispatch::mark_effective_provider_exhausted(db, provider_name)?;
    }
    Ok(())
}

pub fn test_model_for_test(
    models: HashMap<String, ModelConfig>,
    models_dir: PathBuf,
    name: &str,
) -> Result<TestModelResult, String> {
    let (model, db_path) = lookup::test_model_for_test_inputs(&models, &models_dir, name)?;
    let state_db_opener = ProductionStateDbOpener;
    let providers_config = FilesystemProvidersConfigRepository;
    let routing_service = oulipoly_runtime::services::ProductionRoutingService;
    let executor_service = oulipoly_runtime::executor::RuntimeExecutorService;
    let diagnostics_service = oulipoly_runtime::diagnostics::RuntimeDiagnosticsService;
    let services = mapper::test_model_services_from_parts(
        &state_db_opener,
        &providers_config,
        &routing_service,
        &executor_service,
        &diagnostics_service,
    );
    test_model_with_db_path(
        services,
        model,
        models_dir,
        db_path,
        "Say hello in one sentence.",
    )
}

pub fn effective_provider_for_model_provider(
    model: &ModelConfig,
    provider_index: usize,
    providers_cfg: &ProvidersConfig,
) -> Result<(ProviderConfig, PromptMode), String> {
    validator::validate_provider_index(model, provider_index)?;
    let provider = lookup::pool_member_provider_at_index(model, provider_index);
    match lookup::configured_effective_provider_for_provider(providers_cfg, &provider) {
        Ok(effective) => Ok(effective),
        Err(_)
            if validator::model_command_provider_is_non_empty(
                lookup::model_command_provider_name(&provider),
            ) =>
        {
            Ok(mapper::map_effective_provider_from_sources(
                provider,
                model.prompt_mode,
            ))
        }
        Err(err) => Err(err),
    }
}
