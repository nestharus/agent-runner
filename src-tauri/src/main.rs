use agent_runner_lib::balancer;
use agent_runner_lib::config::{
    AgentConfig, ModelConfig, load_agent_file, load_agents, load_models,
};
use agent_runner_lib::diagnostics;
use agent_runner_lib::executor;
use agent_runner_lib::state::{CompositeInvocationId, InvocationStart, StateDb};
use agent_runner_lib::trace::{TraceOptions, render_ascii_trace, trace_invocation_with_sessions};

use clap::{Parser, Subcommand};
use std::collections::HashMap;
use std::io::{IsTerminal, Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use uuid::Uuid;

#[derive(Parser)]
#[command(
    name = "oulipoly-agent-runner",
    about = "LLM agent runner with load balancing",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Subcommands>,

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

    /// Pass model inputs as key=value pairs (repeatable)
    #[arg(short = 'i', long = "input", value_name = "KEY=VALUE")]
    inputs: Vec<String>,
}

#[derive(Clone, Debug, Subcommand)]
enum Subcommands {
    /// Walk the invocation tree from a UUID.
    Trace {
        /// The invocation UUID to start the walk from.
        invocation_uuid: String,

        /// Emit structured JSON instead of an ASCII tree.
        #[arg(long)]
        json: bool,

        /// Embed raw transcript records inline (PR-B returns null placeholders).
        #[arg(long, requires = "json")]
        inline_transcript: bool,

        /// Append a transcript placeholder after the tree in human mode.
        /// Per contract `tmp/01-pr-b-contract.md` §"`--transcript` (human
        /// mode)", this flag is mutually exclusive with `--json`. Use
        /// `--json --inline-transcript` for the structured equivalent.
        #[arg(long, conflicts_with = "json")]
        transcript: bool,

        /// Maximum tree depth before truncating descendants.
        #[arg(long, default_value = "64")]
        max_depth: usize,
    },
    /// Launch a model interactively without a prompt payload.
    Repl {
        /// Model id to launch interactively
        model: String,

        /// Resume an existing session by full UUID
        #[arg(long = "resume")]
        resume: Option<String>,

        /// Working directory for the wrapped CLI
        #[arg(short = 'p', long = "project")]
        project: Option<PathBuf>,

        /// Override models directory
        #[arg(long = "models-dir")]
        models_dir: Option<PathBuf>,
    },
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

/// Parse --input key=value flags into a map (repeated keys become arrays).
fn parse_inputs(raw: &[String]) -> Result<HashMap<String, Vec<String>>, String> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for entry in raw {
        let (key, value) = entry
            .split_once('=')
            .ok_or_else(|| format!("Invalid input format '{}': expected KEY=VALUE", entry))?;
        map.entry(key.to_string())
            .or_default()
            .push(value.to_string());
    }
    Ok(map)
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
    if let Some(ref path) = cli.file {
        return std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read prompt file: {e}"));
    }

    if let Some(text) = collect_positional_prompt(cli, include_agent_as_prompt) {
        return Ok(text);
    }

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
    default_models_dir()
}

fn default_models_dir() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("oulipoly-agent-runner").join("models"))
        .unwrap_or_else(|| PathBuf::from("models"))
}

fn run(cli: Cli) -> Result<i32, String> {
    if let Some(command) = cli.command.clone() {
        return match command {
            Subcommands::Trace {
                invocation_uuid,
                json,
                inline_transcript,
                transcript,
                max_depth,
            } => run_trace_command(
                TraceOptions {
                    max_depth,
                    json,
                    inline_transcript,
                    transcript,
                },
                &invocation_uuid,
            ),
            Subcommands::Repl {
                model,
                resume,
                project,
                models_dir,
            } => run_repl(
                &model,
                resume.as_deref(),
                project.as_deref(),
                models_dir.as_deref(),
            ),
        };
    }

    let models_dir = resolve_models_dir(&cli);
    let models = load_models(&models_dir)?;
    let extra_inputs = parse_inputs(&cli.inputs)?;

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

        return run_with_balancing(
            model,
            &prompt,
            &models,
            working_dir.as_deref(),
            &extra_inputs,
        );
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

    run_with_balancing(
        model,
        &full_prompt,
        &models,
        working_dir.as_deref(),
        &extra_inputs,
    )
}

fn run_trace_command(options: TraceOptions, invocation_uuid: &str) -> Result<i32, String> {
    let state = StateDb::open_default()?;
    let config_root = dirs::config_dir()
        .map(|d| d.join("oulipoly-agent-runner"))
        .unwrap_or_else(|| PathBuf::from("."));
    let sessions_path = config_root.join("sessions.toml");
    // Per V10 (failures observable, never silent): a malformed
    // sessions.toml must surface as an error, not silently degrade
    // every transcript_state to "no_locator". An ABSENT file is fine
    // — `SessionsConfig::load` returns an empty config in that case.
    let sessions_cfg = agent_runner_lib::config::SessionsConfig::load(&sessions_path)
        .map_err(|e| format!("Failed to load {}: {e}", sessions_path.display()))?;
    let report =
        match trace_invocation_with_sessions(&state, invocation_uuid, options, Some(&sessions_cfg))
        {
            Ok(report) => report,
            Err(err) if err.starts_with("Invocation not found:") => {
                eprintln!("{err}");
                return Ok(1);
            }
            Err(err) => return Err(err),
        };

    if options.json {
        let json = serde_json::to_string_pretty(&report)
            .map_err(|e| format!("Failed to serialize trace report: {e}"))?;
        println!("{json}");
    } else {
        print!("{}", render_ascii_trace(&report));
    }

    Ok(0)
}

fn resolve_agent(cli: &Cli) -> Result<AgentConfig, String> {
    if let Some(ref path) = cli.agent_file {
        return load_agent_file(path);
    }

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

struct FinalizerGuard<'a> {
    db: &'a StateDb,
    invocation_id: i64,
    finalized: bool,
}

impl<'a> FinalizerGuard<'a> {
    fn new(db: &'a StateDb, invocation_id: i64) -> Self {
        Self {
            db,
            invocation_id,
            finalized: false,
        }
    }

    fn mark_finalized(&mut self) {
        self.finalized = true;
    }
}

impl Drop for FinalizerGuard<'_> {
    fn drop(&mut self) {
        if self.finalized {
            return;
        }

        if let Err(err) =
            self.db
                .finalize_invocation(self.invocation_id, false, -1, Some("guard_drop"), None)
        {
            eprintln!("Warning: Failed to finalize invocation in guard: {err}");
        }
    }
}

fn should_emit_invocation_line(is_terminal: bool) -> bool {
    !is_terminal
}

fn resume_model_pool_mismatch_message(
    models: &HashMap<String, ModelConfig>,
    model_name: &str,
    session_id: &str,
    provider_name: &str,
) -> String {
    let mut suggestions: Vec<String> = models
        .values()
        .filter(|model| {
            model
                .providers
                .iter()
                .any(|provider| provider.name == provider_name)
        })
        .map(|model| model.name.clone())
        .collect();
    suggestions.sort();
    suggestions.dedup();

    if suggestions.is_empty() {
        format!(
            "session {session_id} belongs to provider {provider_name}, which is not in model {model_name}'s provider pool.\nTry a model that includes {provider_name}: (no other model in the loaded config includes {provider_name})"
        )
    } else {
        format!(
            "session {session_id} belongs to provider {provider_name}, which is not in model {model_name}'s provider pool.\nTry a model that includes {provider_name}: {}",
            suggestions.join(", ")
        )
    }
}

fn run_repl(
    model_name: &str,
    resume: Option<&str>,
    working_dir: Option<&Path>,
    models_dir_override: Option<&Path>,
) -> Result<i32, String> {
    let state = StateDb::open_default()?;
    let models_dir = models_dir_override
        .map(Path::to_path_buf)
        .unwrap_or_else(default_models_dir);
    let models = load_models(&models_dir)?;
    let model = models
        .get(model_name)
        .ok_or_else(|| format!("Unknown model: {model_name}"))?;

    let config_root = dirs::config_dir()
        .map(|d| d.join("oulipoly-agent-runner"))
        .unwrap_or_else(|| PathBuf::from("."));
    let providers_path = config_root.join("providers.toml");
    let sessions_path = config_root.join("sessions.toml");
    let providers_cfg =
        agent_runner_lib::config::ProvidersConfig::load(&providers_path).unwrap_or_default();
    let sessions_cfg =
        agent_runner_lib::config::SessionsConfig::load(&sessions_path).unwrap_or_default();
    let in_flight = agent_runner_lib::quota::InFlight::new();
    let ctx = balancer::BalanceContext {
        providers_cfg: &providers_cfg,
        sessions_cfg: &sessions_cfg,
        in_flight: &in_flight,
    };

    let parent_invocation_id = resolve_parent_invocation_id(&state);
    let stderr_is_terminal = std::io::stderr().is_terminal();
    let (provider_index, resume_payload) = if let Some(session_id) = resume {
        if Uuid::parse_str(session_id).is_err() {
            eprintln!("invalid session UUID: {session_id}");
            return Ok(1);
        }

        let matches = state.find_provider_for_session(session_id)?;
        if matches.is_empty() {
            eprintln!(
                "No session found matching {session_id}. Check that session ingestion is configured and that the provider still has resumable local state."
            );
            return Ok(1);
        }

        let selected_provider = &matches[0].provider_name;
        eprintln!("[resume] -> {selected_provider}");
        if matches.len() > 1 && !stderr_is_terminal {
            let providers = matches
                .iter()
                .map(|matched| matched.provider_name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            eprintln!(
                "[resume] session {session_id} matched {providers}; selected {selected_provider} by latest turn timestamp"
            );
        }

        let Some(provider_index) = model
            .providers
            .iter()
            .position(|provider| provider.name == *selected_provider)
        else {
            eprintln!(
                "{}",
                resume_model_pool_mismatch_message(
                    &models,
                    model_name,
                    session_id,
                    selected_provider,
                )
            );
            return Ok(1);
        };

        let provider = &model.providers[provider_index];
        let Some(strategy) = provider.resume.as_ref() else {
            eprintln!(
                "provider {} has no [providers.resume] block; cannot resume",
                provider.name
            );
            return Ok(1);
        };

        (
            provider_index,
            Some(executor::cli::ResumePayload {
                session_id,
                strategy,
            }),
        )
    } else {
        (balancer::select_provider(model, &state, Some(&ctx)), None)
    };
    let provider = &model.providers[provider_index];
    if provider.interactive_args.is_none() {
        return Err(format!(
            "Provider {} has no interactive_args; cannot launch interactively",
            provider.name
        ));
    }

    let invocation = CompositeInvocationId {
        source: provider.name.clone(),
        id: Uuid::new_v4().to_string(),
    };
    let invocation_row_id = state.start_invocation(&InvocationStart {
        invocation_uuid: invocation.id.clone(),
        model_name: model.name.clone(),
        provider_name: provider.name.clone(),
        provider_index,
        parent_invocation_id,
    })?;
    let mut guard = FinalizerGuard::new(&state, invocation_row_id);
    let invocation_env = serde_json::to_string(&invocation)
        .map_err(|e| format!("Failed to serialize invocation id: {e}"))?;

    if let Some(session_id) = resume {
        state.update_session_capture(invocation_row_id, Some(session_id), "resumed")?;
    }

    if should_emit_invocation_line(stderr_is_terminal) {
        eprintln!("{}", invocation.stderr_line());
    }

    match executor::cli::execute_interactive(
        provider,
        working_dir,
        Some(&invocation_env),
        resume_payload,
    ) {
        Ok(exit_code) => {
            if resume.is_none() {
                state.update_session_capture(invocation_row_id, None, "none")?;
            }
            state.finalize_invocation(invocation_row_id, exit_code == 0, exit_code, None, None)?;
            guard.mark_finalized();
            Ok(exit_code)
        }
        Err(spawn_err) => {
            if resume.is_none() {
                state.update_session_capture(invocation_row_id, None, "none")?;
            }
            state.finalize_invocation(
                invocation_row_id,
                false,
                1,
                Some("spawn_error"),
                Some(&spawn_err),
            )?;
            guard.mark_finalized();
            Ok(1)
        }
    }
}

fn run_with_balancing(
    model: &ModelConfig,
    prompt: &str,
    all_models: &HashMap<String, ModelConfig>,
    working_dir: Option<&Path>,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> Result<i32, String> {
    let state = StateDb::open_default().unwrap_or_else(|e| {
        eprintln!("Warning: Could not open state DB ({e}), running without state tracking.");
        StateDb::open(std::path::Path::new(":memory:")).unwrap()
    });

    // Load providers.toml from the same config dir as models; quota refresh
    // only runs when actual load-balancing is possible (n > 1 providers).
    let config_root = dirs::config_dir()
        .map(|d| d.join("oulipoly-agent-runner"))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let providers_path = config_root.join("providers.toml");
    let sessions_path = config_root.join("sessions.toml");
    let providers_cfg =
        agent_runner_lib::config::ProvidersConfig::load(&providers_path).unwrap_or_default();
    let sessions_cfg =
        agent_runner_lib::config::SessionsConfig::load(&sessions_path).unwrap_or_default();
    let in_flight = agent_runner_lib::quota::InFlight::new();
    let ctx = balancer::BalanceContext {
        providers_cfg: &providers_cfg,
        sessions_cfg: &sessions_cfg,
        in_flight: &in_flight,
    };
    // Resolve parent invocation BEFORE provider selection so the provider
    // selection itself can be attributed to a parent context if needed
    // (matches contract `tmp/01-pr-a-contract.md` lifecycle ordering).
    let parent_invocation_id = resolve_parent_invocation_id(&state);
    let provider_index = balancer::select_provider(model, &state, Some(&ctx));
    let provider_name = &model.providers[provider_index].name;
    let invocation = CompositeInvocationId {
        source: provider_name.clone(),
        id: Uuid::new_v4().to_string(),
    };
    let invocation_row_id = state.start_invocation(&InvocationStart {
        invocation_uuid: invocation.id.clone(),
        model_name: model.name.clone(),
        provider_name: provider_name.clone(),
        provider_index,
        parent_invocation_id,
    })?;
    let invocation_env = serde_json::to_string(&invocation)
        .map_err(|e| format!("Failed to serialize invocation id: {e}"))?;
    eprintln!("{}", invocation.stderr_line());

    let result = match executor::execute_with_inputs_and_env(
        model,
        provider_index,
        prompt,
        working_dir,
        extra_inputs,
        Some(&invocation_env),
    ) {
        Ok(result) => result,
        Err(err) => {
            state
                .finalize_invocation(invocation_row_id, false, -1, None, Some(&err))
                .unwrap_or_else(|finalize_err| {
                    eprintln!("Warning: Failed to finalize invocation: {finalize_err}")
                });
            return Err(err);
        }
    };

    if let executor::SessionCaptureMethod::Failed(reason) = &result.session_capture.method {
        eprintln!("[session-capture] {reason}");
    }

    state
        .update_session_capture(
            invocation_row_id,
            result.session_capture.session_id.as_deref(),
            result.session_capture.method.db_value(),
        )
        .unwrap_or_else(|e| eprintln!("Warning: Failed to update session capture: {e}"));

    let success = result.exit_code == 0;

    let error_category = if !success {
        run_diagnostics(&result.stderr, result.exit_code, all_models, working_dir)
    } else {
        None
    };

    state
        .finalize_invocation(
            invocation_row_id,
            success,
            result.exit_code,
            error_category.as_deref(),
            if success { None } else { Some(&result.stderr) },
        )
        .unwrap_or_else(|e| eprintln!("Warning: Failed to finalize invocation: {e}"));

    // Bump calls_since_refresh for this provider (account). Errors here are
    // non-fatal — missing a tick just slightly skews the next projection.
    state
        .increment_calls_since_refresh(provider_name)
        .unwrap_or_else(|e| eprintln!("Warning: Failed to bump quota tick: {e}"));

    if success {
        let _ = std::io::stdout().write_all(&result.stdout);
    } else {
        eprintln!("{}", result.stderr);
        if let Some(ref cat) = error_category {
            eprintln!("[diagnostics: {cat}]");
        }
    }

    Ok(result.exit_code)
}

fn resolve_parent_invocation_id(state: &StateDb) -> Option<i64> {
    let raw = std::env::var("OULIPOLY_PARENT_INVOCATION").ok()?;
    let composite = CompositeInvocationId::parse_env_value(&raw).ok()?;
    let record = state.get_invocation_by_uuid(&composite.id).ok()??;
    if record.provider_name.as_deref() == Some(composite.source.as_str()) {
        Some(record.id)
    } else {
        None
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use agent_runner_lib::state::InvocationStatus;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::{Mutex, OnceLock};

    const TRACE_UUID: &str = "11111111-1111-1111-1111-111111111111";
    const REPL_MODEL: &str = "fixture-model";

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn with_parent_invocation_env(value: Option<&str>, test: impl FnOnce()) {
        let _guard = env_lock().lock().unwrap();
        let previous = std::env::var_os("OULIPOLY_PARENT_INVOCATION");
        match value {
            Some(value) => unsafe {
                std::env::set_var("OULIPOLY_PARENT_INVOCATION", value);
            },
            None => unsafe {
                std::env::remove_var("OULIPOLY_PARENT_INVOCATION");
            },
        }

        let result = catch_unwind(AssertUnwindSafe(test));

        match previous {
            Some(value) => unsafe {
                std::env::set_var("OULIPOLY_PARENT_INVOCATION", value);
            },
            None => unsafe {
                std::env::remove_var("OULIPOLY_PARENT_INVOCATION");
            },
        }

        if let Err(payload) = result {
            std::panic::resume_unwind(payload);
        }
    }

    fn test_db() -> StateDb {
        StateDb::open(Path::new(":memory:")).unwrap()
    }

    #[test]
    fn trace_subcommand_parses_json_and_inline_transcript_flags() {
        let cli = Cli::try_parse_from([
            "oulipoly-agent-runner",
            "trace",
            TRACE_UUID,
            "--json",
            "--inline-transcript",
            "--max-depth",
            "10",
        ])
        .unwrap();

        match cli.command {
            Some(Subcommands::Trace {
                invocation_uuid,
                json,
                inline_transcript,
                transcript,
                max_depth,
            }) => {
                assert_eq!(invocation_uuid, TRACE_UUID);
                assert!(json);
                assert!(inline_transcript);
                assert!(!transcript);
                assert_eq!(max_depth, 10);
            }
            _ => panic!("expected trace subcommand"),
        }
    }

    #[test]
    fn trace_subcommand_rejects_inline_transcript_without_json() {
        let err = match Cli::try_parse_from([
            "oulipoly-agent-runner",
            "trace",
            TRACE_UUID,
            "--inline-transcript",
        ]) {
            Ok(_) => panic!("expected clap to reject --inline-transcript without --json"),
            Err(err) => err,
        };

        let rendered = err.to_string();
        assert!(rendered.contains("--json"), "{rendered}");
    }

    #[test]
    fn no_subcommand_still_parses_existing_model_flow() {
        // `agent` is the first positional in `Cli`, so a single bare arg
        // (`ping`) lands there — `prompt_args` only captures any *additional*
        // trailing args. The existing `collect_positional_prompt(_, true)`
        // path joins agent + prompt_args, so the runtime prompt still
        // becomes "ping". Exercised end-to-end by the integration test
        // `default_cli_flow_still_runs_without_subcommand`.
        let cli =
            Cli::try_parse_from(["oulipoly-agent-runner", "--model", "fixture", "ping"]).unwrap();

        assert!(cli.command.is_none());
        assert_eq!(cli.model.as_deref(), Some("fixture"));
        assert_eq!(cli.agent.as_deref(), Some("ping"));
        assert!(cli.prompt_args.is_empty());

        // Two trailing args: the first goes to `agent`, the rest into
        // `prompt_args`. This is the existing PR-A behavior preserved
        // under subcommand dispatch.
        let cli = Cli::try_parse_from([
            "oulipoly-agent-runner",
            "--model",
            "fixture",
            "ping",
            "pong",
        ])
        .unwrap();
        assert_eq!(cli.agent.as_deref(), Some("ping"));
        assert_eq!(cli.prompt_args, vec!["pong"]);
    }

    #[test]
    fn trace_subcommand_rejects_transcript_with_json() {
        // Per contract: `--transcript` is the human-mode footer; `--json`
        // surfaces transcripts via `--inline-transcript` instead. Clap
        // must reject the combination.
        let err = match Cli::try_parse_from([
            "oulipoly-agent-runner",
            "trace",
            "00000000-0000-0000-0000-000000000000",
            "--json",
            "--transcript",
        ]) {
            Ok(_) => panic!("expected clap to reject --json --transcript"),
            Err(e) => e,
        };
        let rendered = err.to_string();
        assert!(
            rendered.contains("--transcript") || rendered.contains("--json"),
            "{rendered}"
        );
    }

    #[test]
    fn repl_subcommand_parses_required_model_without_optional_paths() {
        let cli = Cli::try_parse_from(["oulipoly-agent-runner", "repl", REPL_MODEL]).unwrap();

        match cli.command {
            Some(Subcommands::Repl {
                model,
                resume,
                project,
                models_dir,
            }) => {
                assert_eq!(model, REPL_MODEL);
                assert_eq!(resume, None);
                assert_eq!(project, None);
                assert_eq!(models_dir, None);
            }
            _ => panic!("expected repl subcommand"),
        }
    }

    #[test]
    fn repl_subcommand_parses_project_path() {
        let cli = Cli::try_parse_from(["oulipoly-agent-runner", "repl", REPL_MODEL, "-p", "/tmp"])
            .unwrap();

        match cli.command {
            Some(Subcommands::Repl {
                resume, project, ..
            }) => {
                assert_eq!(resume, None);
                assert_eq!(project, Some(PathBuf::from("/tmp")));
            }
            _ => panic!("expected repl subcommand"),
        }
    }

    #[test]
    fn repl_subcommand_parses_models_dir_override() {
        let cli = Cli::try_parse_from([
            "oulipoly-agent-runner",
            "repl",
            "--models-dir",
            "/tmp/models",
            REPL_MODEL,
        ])
        .unwrap();

        match cli.command {
            Some(Subcommands::Repl {
                resume, models_dir, ..
            }) => {
                assert_eq!(resume, None);
                assert_eq!(models_dir, Some(PathBuf::from("/tmp/models")));
            }
            _ => panic!("expected repl subcommand"),
        }
    }

    #[test]
    fn repl_subcommand_parses_resume_after_model() {
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
        let cli = Cli::try_parse_from([
            "oulipoly-agent-runner",
            "repl",
            REPL_MODEL,
            "--resume",
            session_id,
        ])
        .unwrap();

        match cli.command {
            Some(Subcommands::Repl { model, resume, .. }) => {
                assert_eq!(model, REPL_MODEL);
                assert_eq!(resume.as_deref(), Some(session_id));
            }
            _ => panic!("expected repl subcommand"),
        }
    }

    #[test]
    fn repl_subcommand_parses_resume_before_model() {
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
        let cli = Cli::try_parse_from([
            "oulipoly-agent-runner",
            "repl",
            "--resume",
            session_id,
            REPL_MODEL,
        ])
        .unwrap();

        match cli.command {
            Some(Subcommands::Repl { model, resume, .. }) => {
                assert_eq!(model, REPL_MODEL);
                assert_eq!(resume.as_deref(), Some(session_id));
            }
            _ => panic!("expected repl subcommand"),
        }
    }

    #[test]
    fn repl_subcommand_requires_model_argument() {
        let err = match Cli::try_parse_from(["oulipoly-agent-runner", "repl"]) {
            Ok(_) => panic!("expected clap to reject repl without a model"),
            Err(err) => err,
        };

        let rendered = err.to_string().to_lowercase();
        assert!(rendered.contains("model"), "{rendered}");
    }

    #[test]
    fn repl_subcommand_requires_model_argument_even_with_resume() {
        let err = match Cli::try_parse_from([
            "oulipoly-agent-runner",
            "repl",
            "--resume",
            "5169694d-de0f-40d1-890c-6e28e55bab27",
        ]) {
            Ok(_) => panic!("expected clap to reject repl without a model"),
            Err(err) => err,
        };

        let rendered = err.to_string().to_lowercase();
        assert!(rendered.contains("model"), "{rendered}");
    }

    #[test]
    fn repl_subcommand_requires_resume_value() {
        let err =
            match Cli::try_parse_from(["oulipoly-agent-runner", "repl", REPL_MODEL, "--resume"]) {
                Ok(_) => panic!("expected clap to reject --resume without a value"),
                Err(err) => err,
            };

        let rendered = err.to_string();
        assert!(rendered.contains("--resume"), "{rendered}");
    }

    #[test]
    fn repl_subcommand_rejects_extra_positional_arguments() {
        let err = match Cli::try_parse_from(["oulipoly-agent-runner", "repl", REPL_MODEL, "extra"])
        {
            Ok(_) => panic!("expected clap to reject extra repl positional arguments"),
            Err(err) => err,
        };

        let rendered = err.to_string();
        assert!(rendered.contains("extra"), "{rendered}");
    }

    #[test]
    fn resolve_parent_invocation_id_returns_none_when_env_is_unset() {
        let db = test_db();

        with_parent_invocation_env(None, || {
            assert_eq!(resolve_parent_invocation_id(&db), None);
        });
    }

    #[test]
    fn resolve_parent_invocation_id_returns_existing_parent_rowid() {
        let db = test_db();
        let parent = CompositeInvocationId {
            source: "fixture-provider".to_string(),
            id: Uuid::new_v4().to_string(),
        };
        let row_id = db
            .start_invocation(&InvocationStart {
                invocation_uuid: parent.id.clone(),
                model_name: "fixture-model".to_string(),
                provider_name: parent.source.clone(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        let parent_env = serde_json::to_string(&parent).unwrap();

        with_parent_invocation_env(Some(&parent_env), || {
            assert_eq!(resolve_parent_invocation_id(&db), Some(row_id));
        });
    }

    #[test]
    fn resolve_parent_invocation_id_returns_none_for_malformed_json() {
        let db = test_db();
        with_parent_invocation_env(Some("not-json"), || {
            assert_eq!(resolve_parent_invocation_id(&db), None);
        });
    }

    #[test]
    fn resolve_parent_invocation_id_returns_none_for_unknown_uuid() {
        let db = test_db();
        let raw = r#"{"source":"fixture-provider","id":"00000000-0000-0000-0000-000000000000"}"#;
        with_parent_invocation_env(Some(raw), || {
            assert_eq!(resolve_parent_invocation_id(&db), None);
        });
    }

    #[test]
    fn resolve_parent_invocation_id_returns_none_for_invalid_uuid_format() {
        let db = test_db();
        let raw = r#"{"source":"fixture-provider","id":"not-a-uuid"}"#;
        with_parent_invocation_env(Some(raw), || {
            assert_eq!(resolve_parent_invocation_id(&db), None);
        });
    }

    #[test]
    fn stderr_emission_helper_emits_for_non_tty_stderr() {
        assert!(should_emit_invocation_line(false));
    }

    #[test]
    fn stderr_emission_helper_suppresses_for_tty_stderr() {
        assert!(!should_emit_invocation_line(true));
    }

    #[test]
    fn finalizer_guard_mark_finalized_makes_drop_a_no_op() {
        let db = test_db();
        let start = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "fixture-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        };
        let invocation_id = db.start_invocation(&start).unwrap();

        {
            let mut guard = FinalizerGuard::new(&db, invocation_id);
            db.finalize_invocation(invocation_id, true, 0, None, None)
                .unwrap();
            guard.mark_finalized();
        }

        let row = db
            .get_invocation_by_uuid(&start.invocation_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(row.status, InvocationStatus::Succeeded);
        assert_eq!(row.success, Some(true));
        assert_eq!(row.exit_code, Some(0));
    }

    #[test]
    fn finalizer_guard_drop_finalizes_failed_row_during_panic_unwind() {
        let db = test_db();
        let start = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "fixture-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        };
        let invocation_id = db.start_invocation(&start).unwrap();

        let panic_result = catch_unwind(AssertUnwindSafe(|| {
            let _guard = FinalizerGuard::new(&db, invocation_id);
            panic!("force guard drop");
        }));
        assert!(panic_result.is_err());

        let row = db
            .get_invocation_by_uuid(&start.invocation_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(row.status, InvocationStatus::Failed);
        assert_eq!(row.success, Some(false));
        assert_eq!(row.exit_code, Some(-1));
        assert_eq!(row.error_category.as_deref(), Some("guard_drop"));
    }

    #[test]
    fn finalizer_guard_drop_is_no_op_after_explicit_spawn_error_finalize() {
        let db = test_db();
        let start = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "fixture-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        };
        let invocation_id = db.start_invocation(&start).unwrap();

        {
            let mut guard = FinalizerGuard::new(&db, invocation_id);
            db.finalize_invocation(
                invocation_id,
                false,
                1,
                Some("spawn_error"),
                Some("spawn failed"),
            )
            .unwrap();
            guard.mark_finalized();
        }

        let row = db
            .get_invocation_by_uuid(&start.invocation_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(row.status, InvocationStatus::Failed);
        assert_eq!(row.success, Some(false));
        assert_eq!(row.exit_code, Some(1));
        assert_eq!(row.error_category.as_deref(), Some("spawn_error"));
    }
}
