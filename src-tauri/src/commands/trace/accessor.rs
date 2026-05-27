//! Declared roles: accessor

use oulipoly_state::StateDb;
use std::path::Path;

pub(super) struct TraceEnvironment {
    pub(super) state: StateDb,
    pub(super) sessions_cfg: oulipoly_config::SessionsConfig,
}

pub(super) fn load_trace_environment() -> Result<TraceEnvironment, String> {
    let state = StateDb::open_default()?;
    let sessions_path = crate::cli::paths::default_config_root().join("sessions.toml");
    let sessions_cfg = load_trace_sessions_config(&sessions_path)?;
    Ok(super::mapper::trace_environment(state, sessions_cfg))
}

pub(super) fn load_trace_sessions_config(
    sessions_path: &Path,
) -> Result<oulipoly_config::SessionsConfig, String> {
    oulipoly_config::SessionsConfig::load(sessions_path)
        .map_err(|e| super::formatter::format_trace_sessions_config_load_error(sessions_path, e))
}
