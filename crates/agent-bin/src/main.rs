use agent_runner_agent_bin::{AgentRunOptions, agent_services, default_config_path, run};
use std::process::ExitCode;

fn main() -> ExitCode {
    let config_path = default_config_path();
    let services = agent_services(None);
    let mut stderr = std::io::stderr();
    run(
        AgentRunOptions {
            argv: std::env::args().collect(),
            config_path,
            models_dir_override: None,
        },
        services,
        &mut stderr,
    )
}
