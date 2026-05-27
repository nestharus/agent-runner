//! Declared roles: orchestration, accessor, formatter, validator

use super::formatter::{emit_export_error, emit_metadata_error};
use super::mapper::{self, ExportOutputOutcome};
use super::validator::{validate_locate_session_id, validate_session_export_args};
use crate::cli::paths::{default_config_root, default_models_dir};
use crate::wiring;
use oulipoly_config::{ModelConfig, ProvidersConfig, load_models};
use oulipoly_runtime::session_export::ExportError;
use oulipoly_runtime::session_metadata::locate_session_metadata;
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
    crate::session_metadata_cli::render_session_metadata(locate_session_metadata(
        &env.state,
        &env.models,
        &env.providers_cfg,
        &env.sessions_cfg,
        session_id,
    ))
}

fn load_session_locate_environment_or_exit() -> Result<SessionLocateEnvironment, i32> {
    load_session_locate_environment()
}

pub(super) struct SessionLocateEnvironment {
    pub(super) state: StateDb,
    pub(super) providers_cfg: ProvidersConfig,
    pub(super) models: HashMap<String, ModelConfig>,
    pub(super) sessions_cfg: oulipoly_config::SessionsConfig,
}

impl SessionLocateEnvironment {
    fn new(
        state: StateDb,
        providers_cfg: ProvidersConfig,
        models: HashMap<String, ModelConfig>,
        sessions_cfg: oulipoly_config::SessionsConfig,
    ) -> Self {
        mapper::session_locate_environment(state, providers_cfg, models, sessions_cfg)
    }
}

fn load_session_locate_environment() -> Result<SessionLocateEnvironment, i32> {
    load_session_locate_environment_result().map_err(|message| {
        let err = mapper::operational_metadata_error(message);
        emit_metadata_error(&err);
        1
    })
}

fn load_session_locate_environment_result() -> Result<SessionLocateEnvironment, String> {
    let state = StateDb::open_default()?;
    let config_root = default_config_root();
    let providers_cfg = oulipoly_config::ProvidersConfig::load(&config_root.join("providers.toml"))
        .unwrap_or_default();
    let models = load_models(&default_models_dir(), Some(&providers_cfg))?;
    let sessions_cfg = oulipoly_config::SessionsConfig::load(&config_root.join("sessions.toml"))
        .unwrap_or_default();
    Ok(SessionLocateEnvironment::new(
        state,
        providers_cfg,
        models,
        sessions_cfg,
    ))
}

pub(crate) fn run_session_export(
    session_id: &str,
    format: &str,
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> Result<i32, String> {
    if let Some(rejection) = validate_session_export_args(session_id, format) {
        return Ok(handle_session_export_args_rejection(&rejection));
    }

    let service_output = agent_runtime_services
        .session_export_service
        .export_session(mapper::session_export_service_request(session_id))
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
