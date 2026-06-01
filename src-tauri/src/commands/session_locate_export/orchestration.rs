//! Declared roles: orchestration, accessor, formatter, validator, mapper
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/src/commands/session_locate_export/orchestration.rs
//!     role: intrinsic-surface
//!     Domain: session locate/export command orchestration
//!     Owns:
//!       - session locate command validation and metadata dispatch
//!       - session export command validation and service dispatch
//!       - default config/model/provider loading for locate metadata
//!       - external provider identity resolver sequencing for export requests
//! ```

use super::formatter::{emit_export_error, emit_metadata_error};
use super::mapper::{self, ExportOutputOutcome};
use super::validator::{validate_locate_session_id, validate_session_export_args};
use crate::cli::paths::{default_config_root, default_models_dir};
use crate::wiring;
use oulipoly_config::{ModelConfig, ProvidersConfig, load_models};
use oulipoly_runtime::provider_registry::{ProviderRegistry, ProviderRegistryOptions};
use oulipoly_runtime::session_export::ExportError;
use oulipoly_runtime::session_metadata::locate_session_metadata_with_provider_dispatch;
use oulipoly_state::StateDb;
use std::collections::HashMap;
use std::io::Write as _;

pub(crate) fn run_session_locate(session_id: &str, _json: bool) -> Result<i32, String> {
    validate_locate_session_id(session_id).map_or_else(
        || run_valid_session_locate(session_id),
        |rejection| Ok(handle_locate_session_id_rejection(&rejection)),
    )
}

fn handle_locate_session_id_rejection(rejection: &mapper::LocateSessionIdRejection) -> i32 {
    super::formatter::emit_locate_session_id_rejection(rejection);
    mapper::locate_session_id_rejection_exit_code(rejection)
}

fn run_valid_session_locate(session_id: &str) -> Result<i32, String> {
    let env = match load_session_locate_environment_or_exit() {
        Ok(env) => env,
        Err(exit_code) => return Ok(exit_code),
    };
    crate::session_metadata_cli::render_session_metadata(
        locate_session_metadata_with_provider_dispatch(
            &env.state,
            &env.models,
            &env.providers_cfg,
            &env.sessions_cfg,
            Some(&env.provider_registry),
            session_id,
        ),
    )
}

fn load_session_locate_environment_or_exit() -> Result<SessionLocateEnvironment, i32> {
    load_session_locate_environment()
}

pub(super) struct SessionLocateEnvironment {
    pub(super) state: StateDb,
    pub(super) providers_cfg: ProvidersConfig,
    pub(super) models: HashMap<String, ModelConfig>,
    pub(super) sessions_cfg: oulipoly_config::SessionsConfig,
    pub(super) provider_registry: ProviderRegistry,
}

impl SessionLocateEnvironment {
    fn new(
        state: StateDb,
        providers_cfg: ProvidersConfig,
        models: HashMap<String, ModelConfig>,
        sessions_cfg: oulipoly_config::SessionsConfig,
        provider_registry: ProviderRegistry,
    ) -> Self {
        mapper::session_locate_environment(
            state,
            providers_cfg,
            models,
            sessions_cfg,
            provider_registry,
        )
    }
}

fn load_session_locate_environment() -> Result<SessionLocateEnvironment, i32> {
    load_session_locate_environment_result()
        .map_err(map_session_locate_environment_error)
        .map_err(emit_session_locate_environment_error)
}

fn map_session_locate_environment_error(
    message: String,
) -> oulipoly_runtime::session_metadata::MetadataError {
    mapper::operational_metadata_error(message)
}

fn emit_session_locate_environment_error(
    error: oulipoly_runtime::session_metadata::MetadataError,
) -> i32 {
    emit_metadata_error(&error);
    1
}

fn load_session_locate_environment_result() -> Result<SessionLocateEnvironment, String> {
    let config_root = default_config_root();
    let state = open_default_locate_state()?;
    let providers_cfg = load_default_locate_providers(&config_root);
    let models = load_default_locate_models(&providers_cfg)?;
    let sessions_cfg = load_default_locate_sessions(&config_root);
    let provider_registry = build_session_locate_provider_registry(&models, config_root)?;
    Ok(SessionLocateEnvironment::new(
        state,
        providers_cfg,
        models,
        sessions_cfg,
        provider_registry,
    ))
}

fn open_default_locate_state() -> Result<StateDb, String> {
    StateDb::open_default()
}

fn load_default_locate_providers(config_root: &std::path::Path) -> ProvidersConfig {
    oulipoly_config::ProvidersConfig::load(&config_root.join("providers.toml")).unwrap_or_default()
}

fn load_default_locate_models(
    providers_cfg: &ProvidersConfig,
) -> Result<HashMap<String, ModelConfig>, String> {
    load_models(&default_models_dir(), Some(providers_cfg)).map_err(|error| error.to_string())
}

fn load_default_locate_sessions(config_root: &std::path::Path) -> oulipoly_config::SessionsConfig {
    oulipoly_config::SessionsConfig::load(&config_root.join("sessions.toml")).unwrap_or_default()
}

fn build_session_locate_provider_registry(
    models: &HashMap<String, ModelConfig>,
    config_root: std::path::PathBuf,
) -> Result<ProviderRegistry, String> {
    ProviderRegistry::from_model_configs(
        &models.values().cloned().collect::<Vec<_>>(),
        ProviderRegistryOptions::default()
            .with_config_root(config_root)
            .with_data_root(default_data_root()?),
    )
    .map_err(|error| error.to_string())
}

fn default_data_root() -> Result<std::path::PathBuf, String> {
    oulipoly_state::StateDb::default_path()?
        .parent()
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| "Could not determine data root".to_string())
}

pub(crate) fn run_session_export(
    session_id: &str,
    format: &str,
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> Result<i32, String> {
    if let Some(rejection) = validate_session_export_args(session_id, format) {
        return Ok(handle_session_export_args_rejection(&rejection));
    }

    let external_provider =
        match crate::commands::session_external_provider_identity::resolve_session_external_provider_identity(
            session_id,
        ) {
            Ok(identity) => identity,
            Err(message) => return Ok(handle_export_error(&ExportError::Operational { message })),
        };

    let service_output = agent_runtime_services
        .session_export_service
        .export_session(mapper::session_export_service_request(
            session_id,
            external_provider,
        ))
        .map_err(|err| err.to_string())?;

    let output = match unwrap_export_output(service_output.result) {
        Ok(output) => output,
        Err(exit_code) => return Ok(exit_code),
    };
    write_session_export_output(&output)
}

fn unwrap_export_output(result: Result<Vec<u8>, ExportError>) -> Result<Vec<u8>, i32> {
    match mapper::export_output_outcome(result) {
        ExportOutputOutcome::Output(output) => Ok(output),
        ExportOutputOutcome::Error(err) => Err(handle_export_error(&err)),
    }
}

fn write_session_export_output(output: &[u8]) -> Result<i32, String> {
    write_stdout_bytes(output).map_or_else(
        |err| Ok(handle_export_write_error(&err)),
        |_| Ok(mapper::export_success_exit_code()),
    )
}

fn handle_session_export_args_rejection(rejection: &mapper::SessionExportArgsRejection) -> i32 {
    super::formatter::emit_session_export_args_rejection(rejection);
    mapper::session_export_args_rejection_exit_code(rejection)
}

fn handle_export_error(err: &ExportError) -> i32 {
    emit_export_error(err);
    mapper::export_error_exit_code(err)
}

fn write_stdout_bytes(output: &[u8]) -> Result<(), std::io::Error> {
    std::io::stdout().write_all(output)
}

fn handle_export_write_error(err: &std::io::Error) -> i32 {
    super::formatter::emit_export_write_error(err);
    mapper::export_write_error_exit_code()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_runtime::session_export::ExportError;

    #[test]
    fn unwrap_export_output_returns_ok_bytes_unchanged() {
        assert_eq!(
            unwrap_export_output(Ok(b"abc".to_vec())),
            Ok(b"abc".to_vec())
        );
    }

    #[test]
    fn unwrap_export_output_maps_error_to_exit_code() {
        assert_eq!(
            unwrap_export_output(Err(ExportError::InvalidSessionId { input: "x".into() })),
            Err(2)
        );
    }

    #[test]
    fn write_session_export_output_returns_zero_for_successful_write() {
        assert_eq!(write_session_export_output(b"abc"), Ok(0));
    }
}
