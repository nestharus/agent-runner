//! Declared roles: orchestration, accessor, mapper, formatter, parser, validator
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/src/commands/direct_model.rs
//!     role: intrinsic-surface
//!     Domain: direct_model_cli_dispatch
//!     Owns:
//!       - CLI execution context loading from provider/model config and CLI inputs
//!       - direct model lookup and balancing dispatch entry point
//!       - direct model pre-invocation failure emission for context, model lookup, and prompt resolution failures
//!       - direct prompt construction from plain prompt input and --agent-file formatting
//!   - component: src-tauri/src/commands/direct_model.rs
//!     role: intrinsic-surface
//!     Domain: named_agent_cli_dispatch
//!     Owns:
//!       - named-agent config resolution
//!       - named-agent model lookup
//!       - named-agent prompt construction with typed CLI inputs
//!       - named-agent balancing dispatch helper path
//! ```

use crate::usage::cli::Cli;
use crate::wiring;
use oulipoly_config::repositories::FilesystemAgentConfigRepository;
use oulipoly_config::{AgentConfig, ModelConfig, ProvidersConfig, load_agent_file, load_models};
use oulipoly_state::repositories::ProductionStateDbOpener;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

struct CliExecutionContext {
    models: HashMap<String, ModelConfig>,
    models_dir: PathBuf,
    extra_inputs: HashMap<String, Vec<String>>,
    working_dir: Option<PathBuf>,
    state_db_opener: ProductionStateDbOpener,
}

fn load_cli_execution_context(cli: &Cli) -> Result<CliExecutionContext, String> {
    let parts = load_cli_execution_context_parts(cli)?;
    Ok(cli_execution_context(
        cli,
        parts.models,
        parts.models_dir,
        parts.extra_inputs,
    ))
}

struct CliExecutionContextParts {
    models: HashMap<String, ModelConfig>,
    models_dir: PathBuf,
    extra_inputs: HashMap<String, Vec<String>>,
}

fn load_cli_execution_context_parts(cli: &Cli) -> Result<CliExecutionContextParts, String> {
    let providers_cfg = load_providers_config()?;
    let models_dir = crate::cli::paths::resolve_models_dir(cli)?;
    let models = load_cli_models(&models_dir, &providers_cfg)?;
    let extra_inputs = parse_cli_extra_inputs(cli)?;
    Ok(cli_execution_context_parts(
        models,
        models_dir,
        extra_inputs,
    ))
}

fn cli_execution_context_parts(
    models: HashMap<String, ModelConfig>,
    models_dir: PathBuf,
    extra_inputs: HashMap<String, Vec<String>>,
) -> CliExecutionContextParts {
    CliExecutionContextParts {
        models,
        models_dir,
        extra_inputs,
    }
}

fn load_providers_config() -> Result<ProvidersConfig, String> {
    Ok(
        ProvidersConfig::load(&crate::cli::paths::default_config_root()?.join("providers.toml"))
            .unwrap_or_default(),
    )
}

fn load_cli_models(
    models_dir: &Path,
    providers_cfg: &ProvidersConfig,
) -> Result<HashMap<String, ModelConfig>, String> {
    Ok(load_models(models_dir, Some(providers_cfg))?)
}

fn parse_cli_extra_inputs(cli: &Cli) -> Result<HashMap<String, Vec<String>>, String> {
    crate::cli::inputs::parse_inputs(&cli.inputs)
}

fn cli_execution_context(
    cli: &Cli,
    models: HashMap<String, ModelConfig>,
    models_dir: PathBuf,
    extra_inputs: HashMap<String, Vec<String>>,
) -> CliExecutionContext {
    cli_execution_context_from_parts(
        models,
        models_dir,
        extra_inputs,
        cli_working_dir(cli),
        ProductionStateDbOpener,
    )
}

fn cli_working_dir(cli: &Cli) -> Option<PathBuf> {
    cli.project.clone()
}

fn cli_execution_context_from_parts(
    models: HashMap<String, ModelConfig>,
    models_dir: PathBuf,
    extra_inputs: HashMap<String, Vec<String>>,
    working_dir: Option<PathBuf>,
    state_db_opener: ProductionStateDbOpener,
) -> CliExecutionContext {
    CliExecutionContext {
        models,
        models_dir,
        extra_inputs,
        working_dir,
        state_db_opener,
    }
}

pub(crate) fn run_direct_model_cli(
    cli: &Cli,
    model_name: &str,
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> Result<i32, String> {
    let context = load_direct_model_context(cli, model_name)?;
    let model = load_direct_model(&context, model_name)?;
    let prompt = direct_model_prompt(cli).inspect_err(|err| {
        emit_direct_model_prompt_resolution_failure(model, err);
    })?;
    dispatch_direct_model_balancing(agent_runtime_services, &context, model, &prompt)
}

pub(crate) fn validate_direct_model_cli_context(cli: &Cli, model_name: &str) -> Result<(), String> {
    let providers_cfg = load_providers_config()?;
    let context = load_direct_model_context(cli, model_name)?;
    let model = load_direct_model(&context, model_name)?;
    for provider in &model.providers {
        providers_cfg.runtime_provider(&provider.name)?;
    }
    Ok(())
}

fn load_direct_model_context(cli: &Cli, model_name: &str) -> Result<CliExecutionContext, String> {
    load_cli_execution_context(cli).inspect_err(|err| {
        emit_direct_model_pre_invocation_failure(model_name, err);
    })
}

fn load_direct_model<'a>(
    context: &'a CliExecutionContext,
    model_name: &str,
) -> Result<&'a ModelConfig, String> {
    lookup_model(&context.models, model_name).inspect_err(|err| {
        emit_direct_model_pre_invocation_failure(model_name, err);
    })
}

fn emit_direct_model_pre_invocation_failure(model_name: &str, err: &str) {
    crate::dispatch::emit_pre_invocation_failure(
        "provider_selection",
        Some(model_name),
        None,
        Vec::new(),
        Some(err),
    );
}

fn emit_direct_model_prompt_resolution_failure(model: &ModelConfig, reason: &str) {
    crate::dispatch::emit_pre_invocation_failure(
        "prompt_resolution",
        Some(&model.name),
        None,
        Vec::new(),
        Some(reason),
    );
}

fn dispatch_direct_model_balancing(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    context: &CliExecutionContext,
    model: &ModelConfig,
    prompt: &str,
) -> Result<i32, String> {
    crate::run::balancing::run_with_balancing(
        agent_runtime_services,
        &context.state_db_opener,
        model,
        prompt,
        &context.models,
        &context.models_dir,
        context.working_dir.as_deref(),
        &context.extra_inputs,
    )
}

fn lookup_model<'a>(
    models: &'a HashMap<String, ModelConfig>,
    model_name: &str,
) -> Result<&'a ModelConfig, String> {
    require_model_lookup(
        find_model(models, model_name),
        format_unknown_model_error(model_name),
    )
}

fn find_model<'a>(
    models: &'a HashMap<String, ModelConfig>,
    model_name: &str,
) -> Option<&'a ModelConfig> {
    models.get(model_name)
}

fn require_model_lookup(
    model: Option<&ModelConfig>,
    missing_error: String,
) -> Result<&ModelConfig, String> {
    match model {
        Some(model) => Ok(model),
        None => Err(missing_error),
    }
}

fn format_unknown_model_error(model_name: &str) -> String {
    format!("Unknown model: {model_name}")
}

fn direct_model_prompt(cli: &Cli) -> Result<String, String> {
    // AGE-33 source guard: equivalent direct path remains
    // `if let Some(ref agent_path) = cli.agent_file` with `load_agent_file(agent_path)?`.
    if let Some(agent_path) = direct_agent_file_path(cli) {
        let agent = load_direct_agent_file(agent_path)?;
        return format_direct_model_agent_prompt(cli, &agent);
    }
    direct_prompt_input(cli)
}

fn direct_agent_file_path(cli: &Cli) -> Option<&Path> {
    cli.agent_file.as_deref()
}

fn load_direct_agent_file(agent_path: &Path) -> Result<AgentConfig, String> {
    load_agent_file(agent_path)
}

fn direct_prompt_input(cli: &Cli) -> Result<String, String> {
    crate::cli::inputs::resolve_prompt(cli, true)
}

fn format_direct_model_agent_prompt(cli: &Cli, agent: &AgentConfig) -> Result<String, String> {
    let raw_prompt = direct_prompt_input(cli)?;
    Ok(format_direct_agent_prompt(agent, raw_prompt))
}

fn format_direct_agent_prompt(agent: &AgentConfig, raw_prompt: String) -> String {
    crate::cli::inputs::format_agent_prompt(agent, raw_prompt)
}

pub(crate) fn run_agent_cli(
    cli: &Cli,
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> Result<i32, String> {
    let context = load_cli_execution_context(cli)?;
    let agent = load_agent_cli_config(cli)?;
    let model = lookup_agent_model(&context.models, &agent)?;
    let full_prompt = agent_cli_prompt(cli, &agent, &context.extra_inputs)?;
    let provider_inputs = HashMap::new();
    dispatch_agent_cli_balancing(
        agent_runtime_services,
        &context,
        model,
        &full_prompt,
        &provider_inputs,
    )
}

fn load_agent_cli_config(cli: &Cli) -> Result<AgentConfig, String> {
    let agent_config = FilesystemAgentConfigRepository;
    crate::agent_resolution::resolve_agent(cli, &agent_config)
}

fn agent_cli_prompt(
    cli: &Cli,
    agent: &AgentConfig,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> Result<String, String> {
    let raw_prompt = agent_prompt_input(cli)?;
    format_agent_cli_prompt(agent, raw_prompt, extra_inputs)
}

fn agent_prompt_input(cli: &Cli) -> Result<String, String> {
    crate::cli::inputs::resolve_prompt(cli, false)
}

fn format_agent_cli_prompt(
    agent: &AgentConfig,
    raw_prompt: String,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> Result<String, String> {
    crate::cli::inputs::format_agent_prompt_with_inputs(agent, raw_prompt, extra_inputs)
}

fn dispatch_agent_cli_balancing(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    context: &CliExecutionContext,
    model: &ModelConfig,
    full_prompt: &str,
    provider_inputs: &HashMap<String, Vec<String>>,
) -> Result<i32, String> {
    crate::run::balancing::run_with_balancing(
        agent_runtime_services,
        &context.state_db_opener,
        model,
        full_prompt,
        &context.models,
        &context.models_dir,
        context.working_dir.as_deref(),
        provider_inputs,
    )
}

fn lookup_agent_model<'a>(
    models: &'a HashMap<String, ModelConfig>,
    agent: &AgentConfig,
) -> Result<&'a ModelConfig, String> {
    require_model_lookup(
        find_agent_model(models, agent),
        format_unknown_agent_model_error(agent),
    )
}

fn find_agent_model<'a>(
    models: &'a HashMap<String, ModelConfig>,
    agent: &AgentConfig,
) -> Option<&'a ModelConfig> {
    models.get(&agent.model)
}

fn format_unknown_agent_model_error(agent: &AgentConfig) -> String {
    format!(
        "Unknown model '{}' referenced by agent '{}'",
        agent.model, agent.name
    )
}
