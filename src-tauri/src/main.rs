use agent_runner_lib::balancer;
use agent_runner_lib::config::{AgentConfig, ModelConfig, load_agent_file, load_agents, load_models};
use agent_runner_lib::diagnostics;
use agent_runner_lib::executor;
use agent_runner_lib::state::StateDb;

use clap::Parser;
use std::collections::HashMap;
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "oulipoly-agent-runner",
    about = "LLM agent runner with load balancing"
)]
struct Cli {
    /// Agent name (from agents directory)
    agent: Option<String>,

    /// Prompt text (remaining arguments joined)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    prompt_args: Vec<String>,

    /// Execute a model directly (no agent)
    #[arg(short, long)]
    model: Option<String>,

    /// Path to an agent .md file
    #[arg(short = 'a', long = "agent-file")]
    agent_file: Option<PathBuf>,

    /// Read prompt from file
    #[arg(short, long)]
    file: Option<PathBuf>,

    /// Working directory
    #[arg(short = 'p', long = "project")]
    project: Option<PathBuf>,

    /// Models directory (default: ~/.config/oulipoly-agent-runner/models/)
    #[arg(long)]
    models_dir: Option<PathBuf>,

    /// Agents directory
    #[arg(long)]
    agents_dir: Option<PathBuf>,
}

#[derive(Debug)]
struct AppConfig {
    diagnostics_model: Option<String>,
}

fn load_app_config() -> AppConfig {
    let config_dir = dirs::config_dir()
        .map(|d| d.join("oulipoly-agent-runner"))
        .unwrap_or_else(|| PathBuf::from("."));

    let config_path = config_dir.join("config.toml");

    if let Ok(content) = std::fs::read_to_string(&config_path)
        && let Ok(table) = content.parse::<toml::Table>()
    {
        return AppConfig {
            diagnostics_model: table
                .get("diagnostics_model")
                .and_then(|v| v.as_str())
                .map(String::from),
        };
    }

    AppConfig {
        diagnostics_model: None,
    }
}

fn collect_positional_prompt(cli: &Cli, include_agent: bool) -> Option<String> {
    let mut parts = Vec::new();
    if include_agent && let Some(ref a) = cli.agent {
        parts.push(a.as_str());
    }
    for arg in &cli.prompt_args {
        parts.push(arg.as_str());
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

fn resolve_prompt(cli: &Cli, include_agent_as_prompt: bool) -> Result<String, String> {
    // Priority: --file > positional args > stdin
    if let Some(ref path) = cli.file {
        return std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read prompt file: {e}"));
    }

    if let Some(text) = collect_positional_prompt(cli, include_agent_as_prompt) {
        return Ok(text);
    }

    // Try stdin (non-blocking check)
    if std::io::stdin().is_terminal() {
        return Err("No prompt provided. Pass as argument, --file, or pipe to stdin.".to_string());
    }

    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| format!("Failed to read stdin: {e}"))?;

    if input.trim().is_empty() {
        return Err("Empty prompt from stdin.".to_string());
    }

    Ok(input)
}

fn resolve_models_dir(cli: &Cli) -> PathBuf {
    if let Some(ref dir) = cli.models_dir {
        return dir.clone();
    }
    dirs::config_dir()
        .map(|d| d.join("oulipoly-agent-runner").join("models"))
        .unwrap_or_else(|| PathBuf::from("models"))
}

fn run(cli: Cli) -> Result<i32, String> {
    let models_dir = resolve_models_dir(&cli);
    let models = load_models(&models_dir)?;

    let working_dir = cli.project.clone();

    // Direct model execution (--model)
    if let Some(ref model_name) = cli.model {
        let model = models
            .get(model_name)
            .ok_or_else(|| format!("Unknown model: {model_name}"))?;

        let prompt = if let Some(ref agent_path) = cli.agent_file {
            let agent = load_agent_file(agent_path)?;
            let raw_prompt = resolve_prompt(&cli, true)?;
            format!("{}\n\n{}", agent.instructions, raw_prompt)
        } else {
            resolve_prompt(&cli, true)?
        };

        return run_with_balancing(model, &prompt, &models, working_dir.as_deref());
    }

    // Agent-based execution
    let agent = resolve_agent(&cli)?;

    let model = models.get(&agent.model).ok_or_else(|| {
        format!(
            "Unknown model '{}' referenced by agent '{}'",
            agent.model, agent.name
        )
    })?;

    let raw_prompt = resolve_prompt(&cli, false)?;
    let full_prompt = if agent.instructions.is_empty() {
        raw_prompt
    } else {
        format!("{}\n\n{}", agent.instructions, raw_prompt)
    };

    run_with_balancing(model, &full_prompt, &models, working_dir.as_deref())
}

fn resolve_agent(cli: &Cli) -> Result<AgentConfig, String> {
    // --agent-file takes priority
    if let Some(ref path) = cli.agent_file {
        return load_agent_file(path);
    }

    // Named agent from agents directory
    if let Some(ref name) = cli.agent {
        let agents_dir = cli.agents_dir.clone().unwrap_or_else(|| {
            dirs::config_dir()
                .map(|d| d.join("oulipoly-agent-runner").join("agents"))
                .unwrap_or_else(|| PathBuf::from("agents"))
        });
        let agents = load_agents(&agents_dir)?;
        return agents
            .get(name)
            .cloned()
            .ok_or_else(|| format!("Unknown agent: {name}"));
    }

    Err("No agent specified. Use a positional argument or --agent-file.".to_string())
}

fn run_with_balancing(
    model: &ModelConfig,
    prompt: &str,
    all_models: &HashMap<String, ModelConfig>,
    working_dir: Option<&Path>,
) -> Result<i32, String> {
    let state = StateDb::open_default().unwrap_or_else(|e| {
        eprintln!("Warning: Could not open state DB ({e}), running without state tracking.");
        StateDb::open(std::path::Path::new(":memory:")).unwrap()
    });

    let provider_index = balancer::select_provider(model, &state);
    let result = executor::execute(model, provider_index, prompt, working_dir)?;

    let success = result.exit_code == 0;

    // Run diagnostics on failure
    let error_category = if !success {
        run_diagnostics(&result.stderr, result.exit_code, all_models, working_dir)
    } else {
        None
    };

    state
        .record_invocation(
            &model.name,
            provider_index,
            success,
            result.exit_code,
            error_category.as_deref(),
            if success { None } else { Some(&result.stderr) },
        )
        .unwrap_or_else(|e| eprintln!("Warning: Failed to record invocation: {e}"));

    if success {
        print!("{}", result.stdout);
    } else {
        eprintln!("{}", result.stderr);
        if let Some(ref cat) = error_category {
            eprintln!("[diagnostics: {cat}]");
        }
    }

    Ok(result.exit_code)
}

fn run_diagnostics(
    stderr: &str,
    exit_code: i32,
    models: &HashMap<String, ModelConfig>,
    working_dir: Option<&Path>,
) -> Option<String> {
    let app_config = load_app_config();
    let diag_model_name = app_config.diagnostics_model?;
    let diag_model = models.get(&diag_model_name)?;

    match diagnostics::diagnose_error(stderr, exit_code, diag_model, models, working_dir) {
        Ok(diagnosis) => {
            eprintln!(
                "[diagnostics] {}: {}",
                diagnosis.category.as_str(),
                diagnosis.summary
            );
            Some(diagnosis.category.as_str().to_string())
        }
        Err(e) => {
            eprintln!("[diagnostics] Failed to diagnose: {e}");
            None
        }
    }
}

fn main() -> ExitCode {
    // No arguments → launch Tauri UI
    // Arguments provided → headless CLI mode
    if std::env::args().len() <= 1 {
        agent_runner_lib::run_tauri();
        return ExitCode::SUCCESS;
    }

    let cli = Cli::parse();

    match run(cli) {
        Ok(0) => ExitCode::SUCCESS,
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::FAILURE
        }
    }
}
