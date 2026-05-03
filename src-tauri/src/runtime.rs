use std::path::Path;

pub use agent_runner_runtime::{DefaultRuntimePaths, RuntimePaths, RuntimeServices};

pub fn cli_services(models_dir_override: Option<&Path>) -> RuntimeServices {
    agent_runner_runtime::cli_services(models_dir_override).with_session_scanner(
        |provider_name, sessions_cfg, state| {
            agent_runner_session::scan_provider(provider_name, sessions_cfg, state).errors
        },
    )
}
