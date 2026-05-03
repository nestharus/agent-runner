use agent_runner_config::load_app_config_from_path;
use agent_runner_runtime::RuntimeServices;
use agent_runner_runtime::repl::{ReplOptions, run_repl_with_services_and_stderr};
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

pub struct AgentRunOptions {
    pub argv: Vec<String>,
    pub config_path: PathBuf,
    pub models_dir_override: Option<PathBuf>,
}

pub fn run(
    options: AgentRunOptions,
    services: RuntimeServices,
    stderr: &mut dyn Write,
) -> ExitCode {
    if options.argv.len() > 1 {
        let _ = writeln!(stderr, "error: agent accepts no flags or subcommands");
        return ExitCode::from(2);
    }

    let config = match load_app_config_from_path(&options.config_path) {
        Ok(config) => config,
        Err(err) => {
            let _ = writeln!(
                stderr,
                "error: agent requires `default_model` in {}; {err}",
                options.config_path.display()
            );
            return ExitCode::from(2);
        }
    };
    let model = match config.default_model {
        Some(model) => model,
        None => {
            let _ = writeln!(
                stderr,
                "error: agent requires `default_model` in {}",
                options.config_path.display()
            );
            return ExitCode::from(2);
        }
    };

    let opts = ReplOptions {
        model: Some(model),
        resume: None,
        migrate: None,
        working_dir: None,
        models_dir_override: options.models_dir_override,
    };

    match run_repl_with_services_and_stderr(opts, services, stderr) {
        Ok(code) => ExitCode::from(code as u8),
        Err(err) => {
            let _ = writeln!(stderr, "error: {err}");
            ExitCode::FAILURE
        }
    }
}

pub fn default_config_path() -> PathBuf {
    dirs::config_dir()
        .map(|dir| dir.join("oulipoly-agent-runner").join("config.toml"))
        .unwrap_or_else(|| PathBuf::from("config.toml"))
}

pub fn agent_services(models_dir_override: Option<&std::path::Path>) -> RuntimeServices {
    agent_runner_runtime::cli_services(models_dir_override).with_session_scanner(
        |provider_name, sessions_cfg, state| {
            agent_runner_session::scan_provider(provider_name, sessions_cfg, state).errors
        },
    )
}
