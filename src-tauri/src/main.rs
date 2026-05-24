//! ## Declared roles
//!
//! `orchestration`, `parser`, `validator`, `accessor`, `formatter`, `mapper`, `predicate`, `filter`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/main.rs::main_to_runtime_services
//!     role: adapter
//!     Translates:
//!       - oulipoly_runtime::services ports and DTOs consumed by CLI dispatch
//!   - component: src-tauri/src/main.rs::main_to_state
//!     role: adapter
//!     Translates:
//!       - oulipoly_state DB, schema, quota, invocation row, session row, acceptance row
//!   - component: src-tauri/src/main.rs::main_to_runtime_modules
//!     role: adapter
//!     Translates:
//!       - oulipoly_runtime modules outside services consumed by CLI dispatch
//! ```
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/src/main.rs
//!     role: intrinsic-surface
//!     Domain: cli_lifecycle_orchestration
//!     Owns:
//!       - lifecycle loops
//!       - run_with_balancing lifecycle loop
//!       - run_resume lifecycle loop
//!       - run_repl lifecycle loop
//!       - top-level --resume dispatch
//!       - invocation finalization sequencing
//!       - terminal signal outcome sequencing
//!       - provider retry and migration sequencing
//!   - component: src-tauri/src/main.rs::pre_invocation_failure_marker
//!     role: intrinsic-surface
//!     Domain: pre_invocation_failure_marker
//!     Owns:
//!       - emit_pre_invocation_failure
//!       - pre_invocation_failure_payload
//!       - pre_invocation_failure_message
//!       - emit_pool_exhausted_pre_invocation_failure
//!       - emit_provider_selection_pre_invocation_failure
//!       - emit_provider_resolution_pre_invocation_failure
//!       - emit_pre_invocation_failure_line
//!       - OULIPOLY_FAILURE marker line
//!       - OULIPOLY_FAILURE payload field set
//!       - model_provider_names
//! ```

use agent_runner_lib::{effective_provider_for_model_provider, load_app_config};
use oulipoly_config::repositories::{AgentConfigRepository, FilesystemAgentConfigRepository};
use oulipoly_config::{
    AgentConfig, ModelConfig, PromptMode, ProviderConfig, ProvidersConfig, SessionStorage,
    load_agent_file, load_models,
};
use oulipoly_runtime::balancer;
use oulipoly_runtime::diagnostics;
use oulipoly_runtime::executor;
use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;
use oulipoly_runtime::services::{
    DiagnosticsServiceOutput, DiagnosticsServiceRequest, ExecutorServiceRequest,
    InvocationLifecycleFinalizeRequest, InvocationLifecycleServicePort,
    InvocationLifecycleStartRequest, MigrationServiceOutput, MigrationServiceRequest,
    ResumeAcceptanceRequest, ResumeServiceOutput, ResumeServiceRequest, RotationFailedReason,
    RoutingServicePort, RoutingServiceRequest, ServiceError, SessionExportServiceRequest,
    SessionLifecycleIngestMode, SessionLifecycleRequest, SessionLockFailure,
    SessionLockServiceRequest, SessionLockSuccess, SessionReplaceServiceRequest,
    TraceServiceFailure, TraceServiceRequest,
};
use oulipoly_runtime::session_export::ExportError;
use oulipoly_runtime::session_lock::LockError;
use oulipoly_runtime::session_metadata::{
    MetadataError, locate_session_metadata, resolve_resume_workspace_root,
    resolve_workspace_root_for_provider_session,
};
use oulipoly_runtime::session_replace::{self, ReplaceError, ReplaceSource};
use oulipoly_runtime::trace::{TraceOptions, render_ascii_trace};
use oulipoly_state::repositories::{ProductionStateDbOpener, StateDbOpener};
use oulipoly_state::schema_probe::{self, ProbeError};
use oulipoly_state::{
    CompositeInvocationId, InvocationRecord, InvocationStart, InvocationStatus, ReadOnlyOpenError,
    ResultEnvelopeFailureIdentity, ResultEnvelopeInput, StateDb, result_envelope_payload,
};

use clap::Parser;
use regex::Regex;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{BufRead, IsTerminal, Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

mod balanced_cli;
mod cli_inputs;
mod config_migration_cli;
#[allow(dead_code)]
#[path = "main/owned_turn_event_ingest.rs"]
mod owned_turn_event_ingest;
mod repl_cli;
mod resume_acceptance_adapter;
mod resume_cli;
mod session_import_replace_cli;
mod session_ingest_cli;
mod session_metadata_cli;
mod terminal_outcome_adapter;
mod trace_cli;
mod usage;
mod wiring;
mod zero_turn_orchestration;

use terminal_outcome_adapter::{
    TerminalSignalContext, TerminalSignalDisposition,
    apply_age153_terminal_signal_fixture_override,
    apply_age153_terminal_signal_fixture_override_to_fields, apply_terminal_signal_outcome,
    balanced_terminal_signal_for_outcome, confirm_maybe_quota_exhausted,
    resume_terminal_signal_for_outcome, spawn_error_terminal_signal,
    terminal_signal_error_category, terminal_signal_reason, typed_terminal_reason_fallback,
};
use zero_turn_orchestration::{
    ZeroTurnAction, ZeroTurnBaseline, ZeroTurnClassification, ZeroTurnConfirmationState,
    ZeroTurnEvidence, classify_completion_delta, next_action, record_baseline,
};

use usage::cli::{Cli, SessionSubcommands, Subcommands};

const DEFAULT_PAUSE_HANDSHAKE_TTL_MS: u64 = 60_000;
const MAX_PAUSE_HANDSHAKE_TTL_MS: u64 = 600_000;

// ---
// Component: cli-prompt-config-resolution
// Declared roles: orchestration, parser, validator, accessor, formatter, mapper, predicate
// ---

/// Parse --input key=value flags into a map (repeated keys become arrays).
fn parse_inputs(raw: &[String]) -> Result<HashMap<String, Vec<String>>, String> {
    cli_inputs::parse_inputs(raw)
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
        return read_prompt_file(path);
    }

    if let Some(text) = collect_positional_prompt(cli, include_agent_as_prompt) {
        return Ok(text);
    }

    read_required_stdin_prompt()
}

fn resolve_resume_answer(
    prompt: Option<&str>,
    file: Option<&Path>,
) -> Result<Option<String>, String> {
    if let Some(path) = file {
        return read_answer_file(path).map(Some);
    }
    if let Some(prompt) = prompt {
        return Ok(Some(prompt.to_string()));
    }
    read_optional_stdin_answer()
}

fn read_prompt_file(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(format_prompt_file_read_error)
}

fn read_answer_file(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(format_answer_file_read_error)
}

fn read_required_stdin_prompt() -> Result<String, String> {
    validate_required_prompt_stdin_available()?;
    let input = read_stdin_text()?;
    validate_nonempty_prompt_stdin(&input)?;
    Ok(input)
}

fn read_stdin_text() -> Result<String, String> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(format_stdin_read_error)?;
    Ok(input)
}

fn validate_required_prompt_stdin_available() -> Result<(), String> {
    if std::io::stdin().is_terminal() {
        Err(format_missing_prompt_error())
    } else {
        Ok(())
    }
}

fn validate_nonempty_prompt_stdin(input: &str) -> Result<(), String> {
    if input.trim().is_empty() {
        Err(format_empty_prompt_error())
    } else {
        Ok(())
    }
}

fn format_prompt_file_read_error(error: std::io::Error) -> String {
    format!("Failed to read prompt file: {error}")
}

fn format_answer_file_read_error(error: std::io::Error) -> String {
    format!("Failed to read answer file: {error}")
}

fn format_stdin_read_error(error: std::io::Error) -> String {
    format!("Failed to read stdin: {error}")
}

fn format_missing_prompt_error() -> String {
    "No prompt provided. Pass as argument, --file, or pipe to stdin.".to_string()
}

fn format_empty_prompt_error() -> String {
    "Empty prompt from stdin.".to_string()
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

fn default_config_root() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("oulipoly-agent-runner"))
        .unwrap_or_else(|| PathBuf::from("."))
}

enum TopLevelResumePromptSource {
    Headless {
        positional_or_stdin_prompt: Option<String>,
    },
    Interactive,
}

fn resolve_top_level_resume_prompt_source(cli: &Cli) -> Result<TopLevelResumePromptSource, String> {
    let prompt_text = collect_positional_prompt(cli, true);
    let stdin_prompt = read_optional_stdin_prompt(prompt_text.is_none() && cli.file.is_none())?;
    if prompt_text.is_some() || cli.file.is_some() || stdin_prompt.is_some() {
        return Ok(TopLevelResumePromptSource::Headless {
            positional_or_stdin_prompt: prompt_text.or(stdin_prompt),
        });
    }
    Ok(TopLevelResumePromptSource::Interactive)
}

fn read_optional_stdin_prompt(enabled: bool) -> Result<Option<String>, String> {
    if !should_read_optional_stdin_prompt(enabled) {
        return Ok(None);
    }
    optional_nonempty_text(read_stdin_text()?)
}

fn read_optional_stdin_answer() -> Result<Option<String>, String> {
    if std::io::stdin().is_terminal() {
        return Ok(None);
    }
    optional_nonempty_text(read_stdin_text()?)
}

fn should_read_optional_stdin_prompt(enabled: bool) -> bool {
    enabled && !std::io::stdin().is_terminal()
}

fn optional_nonempty_text(input: String) -> Result<Option<String>, String> {
    if input.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(input))
    }
}

fn format_agent_prompt(agent: &AgentConfig, raw_prompt: String) -> String {
    if agent.instructions.is_empty() {
        raw_prompt
    } else {
        format!("{}\n\n{}", agent.instructions, raw_prompt)
    }
}

fn format_agent_prompt_with_inputs(
    agent: &AgentConfig,
    raw_prompt: String,
    inputs: &HashMap<String, Vec<String>>,
) -> Result<String, String> {
    let prompt = format_agent_prompt(agent, raw_prompt);
    if inputs.is_empty() {
        return Ok(prompt);
    }

    let input_block = render_agent_inputs(inputs)?;
    Ok(format!(
        "{prompt}\n\n## Operator Inputs\n\n```json\n{input_block}\n```"
    ))
}

fn render_agent_inputs(inputs: &HashMap<String, Vec<String>>) -> Result<String, String> {
    let ordered: BTreeMap<_, _> = inputs.iter().collect();
    serde_json::to_string_pretty(&ordered)
        .map_err(|e| format!("Failed to render agent inputs: {e}"))
}

fn run(cli: Cli) -> Result<i32, String> {
    if let Err(err) = session_replace::recover_pending_replaces() {
        eprintln!("{}", err.to_json());
        return Ok(err.exit_code());
    }

    if cli.new {
        return run_default_provider_repl(&cli);
    }

    let agent_runtime_services = wiring::AgentRuntimeServices::cli_defaults();

    if cli.usage {
        return run_usage_command(&cli, &agent_runtime_services);
    }

    if let Some(command) = cli.command.clone() {
        return dispatch_subcommand(command, &agent_runtime_services);
    }

    if let Some(ref session_id) = cli.resume {
        return dispatch_top_level_resume(&cli, session_id, &agent_runtime_services);
    }

    if let Some(ref model_name) = cli.model {
        return run_direct_model_cli(&cli, model_name, &agent_runtime_services);
    }

    run_agent_cli(&cli, &agent_runtime_services)
}

fn run_default_provider_repl(cli: &Cli) -> Result<i32, String> {
    let services =
        oulipoly_runtime::repl_default_provider::RuntimeServices::production(cli.project.clone())?;
    oulipoly_runtime::repl_default_provider::run_repl_with_default_provider(services)
}

struct UsageContext {
    providers_cfg: ProvidersConfig,
    models: Vec<ModelConfig>,
}

fn load_usage_context(cli: &Cli) -> Result<UsageContext, String> {
    let providers_cfg = ProvidersConfig::load(&default_config_root().join("providers.toml"))?;
    let models_dir = resolve_models_dir(cli);
    let models_map = load_models(&models_dir, Some(&providers_cfg))?;
    Ok(UsageContext {
        providers_cfg,
        models: sorted_models(models_map),
    })
}

fn sorted_models(models_map: HashMap<String, ModelConfig>) -> Vec<ModelConfig> {
    let mut models: Vec<ModelConfig> = models_map.into_values().collect();
    models.sort_by(|a, b| a.name.cmp(&b.name));
    models
}

fn run_usage_command(
    cli: &Cli,
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> Result<i32, String> {
    let context = load_usage_context(cli)?;
    let mut stdout = std::io::stdout().lock();
    usage::dispatch::run_usage(
        agent_runtime_services,
        &context.providers_cfg,
        &context.models,
        &mut stdout,
    )
}

fn dispatch_subcommand(
    command: Subcommands,
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> Result<i32, String> {
    match command {
        Subcommands::Trace {
            invocation_uuid,
            json,
            inline_transcript,
            transcript,
            max_depth,
        } => run_trace_command(
            trace_options(max_depth, json, inline_transcript, transcript),
            &invocation_uuid,
            agent_runtime_services,
        ),
        Subcommands::Repl {
            model,
            resume,
            rotate_provider,
            project,
            models_dir,
        } => run_repl(
            agent_runtime_services,
            model.as_deref(),
            resume.as_deref(),
            rotate_provider.as_deref(),
            project.as_deref(),
            models_dir.as_deref(),
        ),
        Subcommands::Resume {
            model,
            session_id,
            chain_id,
            rotate_provider,
            prompt,
            file,
            project,
            models_dir,
        } => run_resume(
            agent_runtime_services,
            model.as_deref(),
            resume_target_arg(session_id.as_deref(), chain_id.as_deref()),
            rotate_provider.as_deref(),
            prompt.as_deref(),
            file.as_deref(),
            project.as_deref(),
            models_dir.as_deref(),
        ),
        Subcommands::Session { command } => {
            dispatch_session_subcommand(command, agent_runtime_services)
        }
        Subcommands::ResumeList { uuid } => run_resume_list(&uuid),
        Subcommands::MigrateDb => run_migrate_db(),
        Subcommands::Migrate { rebuild } => run_migrate(rebuild),
        Subcommands::MigrateConfig { models_dir } => run_migrate_config(models_dir.as_deref()),
    }
}

fn trace_options(
    max_depth: usize,
    json: bool,
    inline_transcript: bool,
    transcript: bool,
) -> TraceOptions {
    TraceOptions {
        max_depth,
        json,
        inline_transcript,
        transcript,
    }
}

fn resume_target_arg<'a>(session_id: Option<&'a str>, chain_id: Option<&'a str>) -> &'a str {
    chain_id
        .or(session_id)
        .expect("clap group ensures one is set")
}

fn dispatch_session_subcommand(
    command: SessionSubcommands,
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> Result<i32, String> {
    match command {
        SessionSubcommands::Locate { session_id, json } => run_session_locate(&session_id, json),
        SessionSubcommands::SchemaProbe => run_session_schema_probe(),
        SessionSubcommands::Export { session_id, format } => {
            run_session_export(&session_id, &format, agent_runtime_services)
        }
        SessionSubcommands::PauseHandshake { session_id, ttl_ms } => {
            run_pause_handshake(&session_id, ttl_ms, agent_runtime_services)
        }
        SessionSubcommands::ResumeHandshake { session_id, token } => {
            run_resume_handshake(&session_id, &token, agent_runtime_services)
        }
        SessionSubcommands::ImportReplace {
            session_id,
            from_file,
            preimage_sha256,
        } => run_session_import_replace(
            &session_id,
            from_file.as_deref(),
            preimage_sha256.as_deref(),
            agent_runtime_services,
        ),
    }
}

fn dispatch_top_level_resume(
    cli: &Cli,
    session_id: &str,
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> Result<i32, String> {
    validate_top_level_resume_cli(cli)?;
    match resolve_top_level_resume_prompt_source(cli)? {
        TopLevelResumePromptSource::Headless {
            positional_or_stdin_prompt,
        } => run_resume(
            agent_runtime_services,
            cli.model.as_deref(),
            session_id,
            cli.rotate_provider.as_deref(),
            positional_or_stdin_prompt.as_deref(),
            cli.file.as_deref(),
            cli.project.as_deref(),
            cli.models_dir.as_deref(),
        ),
        TopLevelResumePromptSource::Interactive => run_repl(
            agent_runtime_services,
            cli.model.as_deref(),
            Some(session_id),
            cli.rotate_provider.as_deref(),
            cli.project.as_deref(),
            cli.models_dir.as_deref(),
        ),
    }
}

fn validate_top_level_resume_cli(cli: &Cli) -> Result<(), String> {
    if cli.agent_file.is_some() {
        Err(format_resume_agent_file_incompatible_error())
    } else {
        Ok(())
    }
}

fn format_resume_agent_file_incompatible_error() -> String {
    "--resume is incompatible with --agent-file.".to_string()
}

struct CliExecutionContext {
    models: HashMap<String, ModelConfig>,
    extra_inputs: HashMap<String, Vec<String>>,
    working_dir: Option<PathBuf>,
    state_db_opener: ProductionStateDbOpener,
}

fn load_cli_execution_context(cli: &Cli) -> Result<CliExecutionContext, String> {
    let models_dir = resolve_models_dir(cli);
    let providers_cfg =
        ProvidersConfig::load(&default_config_root().join("providers.toml")).unwrap_or_default();
    Ok(CliExecutionContext {
        models: load_models(&models_dir, Some(&providers_cfg))?,
        extra_inputs: parse_inputs(&cli.inputs)?,
        working_dir: cli.project.clone(),
        state_db_opener: ProductionStateDbOpener,
    })
}

fn run_direct_model_cli(
    cli: &Cli,
    model_name: &str,
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> Result<i32, String> {
    let context = match load_cli_execution_context(cli) {
        Ok(context) => context,
        Err(err) => {
            emit_pre_invocation_failure(
                "provider_selection",
                Some(model_name),
                None,
                Vec::new(),
                Some(&err),
            );
            return Err(err);
        }
    };
    let model = match lookup_model(&context.models, model_name) {
        Ok(model) => model,
        Err(err) => {
            emit_pre_invocation_failure(
                "provider_selection",
                Some(model_name),
                None,
                Vec::new(),
                Some(&err),
            );
            return Err(err);
        }
    };
    let prompt = direct_model_prompt(cli)?;
    run_with_balancing(
        agent_runtime_services,
        &context.state_db_opener,
        model,
        &prompt,
        &context.models,
        context.working_dir.as_deref(),
        &context.extra_inputs,
    )
}

fn lookup_model<'a>(
    models: &'a HashMap<String, ModelConfig>,
    model_name: &str,
) -> Result<&'a ModelConfig, String> {
    models
        .get(model_name)
        .ok_or_else(|| format_unknown_model_error(model_name))
}

fn format_unknown_model_error(model_name: &str) -> String {
    format!("Unknown model: {model_name}")
}

fn direct_model_prompt(cli: &Cli) -> Result<String, String> {
    if let Some(ref agent_path) = cli.agent_file {
        let agent = load_agent_file(agent_path)?;
        return format_direct_model_agent_prompt(cli, &agent);
    }
    resolve_prompt(cli, true)
}

fn format_direct_model_agent_prompt(cli: &Cli, agent: &AgentConfig) -> Result<String, String> {
    let raw_prompt = resolve_prompt(cli, true)?;
    Ok(format_agent_prompt(agent, raw_prompt))
}

fn run_agent_cli(
    cli: &Cli,
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> Result<i32, String> {
    let context = load_cli_execution_context(cli)?;
    let agent_config = FilesystemAgentConfigRepository;
    let agent = resolve_agent(cli, &agent_config)?;
    let model = lookup_agent_model(&context.models, &agent)?;
    let raw_prompt = resolve_prompt(cli, false)?;
    let full_prompt = format_agent_prompt_with_inputs(&agent, raw_prompt, &context.extra_inputs)?;
    let provider_inputs = HashMap::new();
    run_with_balancing(
        agent_runtime_services,
        &context.state_db_opener,
        model,
        &full_prompt,
        &context.models,
        context.working_dir.as_deref(),
        &provider_inputs,
    )
}

fn lookup_agent_model<'a>(
    models: &'a HashMap<String, ModelConfig>,
    agent: &AgentConfig,
) -> Result<&'a ModelConfig, String> {
    models
        .get(&agent.model)
        .ok_or_else(|| format_unknown_agent_model_error(agent))
}

fn format_unknown_agent_model_error(agent: &AgentConfig) -> String {
    format!(
        "Unknown model '{}' referenced by agent '{}'",
        agent.model, agent.name
    )
}

// ---
// Component: session-trace-export-commands
// Declared roles: orchestration, formatter, mapper, parser, validator, accessor, predicate
// ---

fn run_session_schema_probe() -> Result<i32, String> {
    match schema_probe::run_schema_probe() {
        Ok(report) => render_schema_probe_report(&report),
        Err(error) => render_schema_probe_error(error),
    }
}

fn render_schema_probe_report(report: &schema_probe::SchemaProbeReport) -> Result<i32, String> {
    if schema_probe_report_is_incompatible(report) {
        write_json_error(
            "schema-incompatible",
            &format_schema_incompatible_message(report),
        )?;
        return Ok(14);
    }
    let json = serde_json::to_string(report).map_err(format_schema_probe_serialize_error)?;
    println!("{json}");
    Ok(0)
}

fn schema_probe_report_is_incompatible(report: &schema_probe::SchemaProbeReport) -> bool {
    report.state_db.exists && !report.state_db.compatible
}

fn format_schema_incompatible_message(report: &schema_probe::SchemaProbeReport) -> String {
    format!(
        "state database schema is incompatible: {}",
        report.state_db.path.display()
    )
}

fn format_schema_probe_serialize_error(error: serde_json::Error) -> String {
    format!("Failed to serialize schema probe report: {error}")
}

fn render_schema_probe_error(error: ProbeError) -> Result<i32, String> {
    write_json_error("operational-error", &probe_error_message(error))?;
    Ok(1)
}

fn run_session_import_replace(
    session_id: &str,
    from_file: Option<&Path>,
    preimage_sha256: Option<&str>,
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> Result<i32, String> {
    if let Some(exit_code) = validate_import_replace_args(session_id, preimage_sha256) {
        return Ok(exit_code);
    }
    let request = import_replace_request(session_id, from_file, preimage_sha256);
    let output = agent_runtime_services
        .session_replace_service
        .replace_session(request)
        .map_err(|err| err.to_string())?;

    render_import_replace_output(output.result)
}

fn validate_import_replace_args(session_id: &str, preimage_sha256: Option<&str>) -> Option<i32> {
    session_import_replace_cli::validate_import_replace_args(session_id, preimage_sha256)
}

fn import_replace_request(
    session_id: &str,
    from_file: Option<&Path>,
    preimage_sha256: Option<&str>,
) -> SessionReplaceServiceRequest {
    SessionReplaceServiceRequest {
        session_id: session_id.to_string(),
        source: replace_source(from_file),
        preimage_sha256: preimage_sha256.map(str::to_string),
    }
}

fn replace_source(from_file: Option<&Path>) -> ReplaceSource {
    from_file
        .map(|path| ReplaceSource::File(path.to_path_buf()))
        .unwrap_or(ReplaceSource::Stdin)
}

fn render_import_replace_output(
    result: Result<session_replace::ReplaceReceipt, ReplaceError>,
) -> Result<i32, String> {
    session_import_replace_cli::render_import_replace_output(result)
}

fn probe_error_message(error: ProbeError) -> String {
    match error {
        ProbeError::StatePath { message } | ProbeError::Inspect { message } => message,
        ProbeError::Open { error } => match error {
            ReadOnlyOpenError::Missing { path } => {
                format!("state database is missing: {}", path.display())
            }
            ReadOnlyOpenError::NotADatabase { path, message } => {
                format!(
                    "state database is not a SQLite database at {}: {message}",
                    path.display()
                )
            }
            ReadOnlyOpenError::PermissionDenied { path } => {
                format!(
                    "permission denied reading state database at {}",
                    path.display()
                )
            }
            ReadOnlyOpenError::WalSidecarError { path, message } => {
                format!(
                    "failed to read SQLite WAL sidecar for state database at {}: {message}",
                    path.display()
                )
            }
            ReadOnlyOpenError::Operational { message } => message,
        },
    }
}

fn write_json_error(code: &str, message: &str) -> Result<(), String> {
    let value = json_error_payload(code, message);
    let json = serialize_json_error_payload(&value)?;
    emit_json_error_line(&json);
    Ok(())
}

fn serialize_json_error_payload(value: &serde_json::Value) -> Result<String, String> {
    serde_json::to_string(value).map_err(format_json_error_serialize_error)
}

fn format_json_error_serialize_error(error: serde_json::Error) -> String {
    format!("Failed to serialize schema probe error: {error}")
}

fn emit_json_error_line(json: &str) {
    eprintln!("{json}");
}

fn json_error_payload(code: &str, message: impl Into<String>) -> serde_json::Value {
    serde_json::json!({
        "error": {
            "code": code,
            "message": message.into(),
        }
    })
}

fn run_trace_command(
    options: TraceOptions,
    invocation_uuid: &str,
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> Result<i32, String> {
    let env = load_trace_environment()?;
    let output = agent_runtime_services
        .trace_service
        .trace(TraceServiceRequest {
            state: &env.state,
            sessions_cfg: &env.sessions_cfg,
            invocation_uuid,
            options,
        })
        .map_err(|err| err.to_string())?;
    render_trace_result(output.result, options.json)
}

struct TraceEnvironment {
    state: StateDb,
    sessions_cfg: oulipoly_config::SessionsConfig,
}

fn load_trace_environment() -> Result<TraceEnvironment, String> {
    let state = StateDb::open_default()?;
    let sessions_path = default_config_root().join("sessions.toml");
    let sessions_cfg = load_trace_sessions_config(&sessions_path)?;
    Ok(TraceEnvironment {
        state,
        sessions_cfg,
    })
}

fn load_trace_sessions_config(
    sessions_path: &Path,
) -> Result<oulipoly_config::SessionsConfig, String> {
    oulipoly_config::SessionsConfig::load(sessions_path)
        .map_err(|e| format_trace_sessions_config_load_error(sessions_path, e))
}

fn format_trace_sessions_config_load_error(sessions_path: &Path, error: String) -> String {
    format!("Failed to load {}: {error}", sessions_path.display())
}

fn render_trace_result(
    result: Result<oulipoly_runtime::trace::TraceReport, TraceServiceFailure>,
    json: bool,
) -> Result<i32, String> {
    trace_cli::render_trace_result(result, json)
}

fn render_trace_report(
    report: &oulipoly_runtime::trace::TraceReport,
    json: bool,
) -> Result<i32, String> {
    if json {
        let json = serde_json::to_string_pretty(report)
            .map_err(|e| format!("Failed to serialize trace report: {e}"))?;
        println!("{json}");
    } else {
        print!("{}", render_ascii_trace(report));
    }
    Ok(0)
}

fn run_session_locate(session_id: &str, _json: bool) -> Result<i32, String> {
    if let Some(exit_code) = validate_locate_session_id(session_id) {
        return Ok(exit_code);
    }
    let env = match load_session_locate_environment() {
        Ok(env) => env,
        Err(exit_code) => return Ok(exit_code),
    };
    render_session_metadata(locate_session_metadata(
        &env.state,
        &env.models,
        &env.providers_cfg,
        &env.sessions_cfg,
        session_id,
    ))
}

fn validate_locate_session_id(session_id: &str) -> Option<i32> {
    if Uuid::parse_str(session_id).is_err() {
        emit_metadata_error(&MetadataError::InvalidSessionId {
            input: session_id.to_string(),
        });
        Some(2)
    } else {
        None
    }
}

struct SessionLocateEnvironment {
    state: StateDb,
    providers_cfg: ProvidersConfig,
    models: HashMap<String, ModelConfig>,
    sessions_cfg: oulipoly_config::SessionsConfig,
}

fn load_session_locate_environment() -> Result<SessionLocateEnvironment, i32> {
    load_session_locate_environment_result().map_err(render_session_locate_environment_error)
}

fn load_session_locate_environment_result() -> Result<SessionLocateEnvironment, String> {
    let state = StateDb::open_default()?;
    let config_root = default_config_root();
    let providers_cfg = oulipoly_config::ProvidersConfig::load(&config_root.join("providers.toml"))
        .unwrap_or_default();
    let models = load_models(&default_models_dir(), Some(&providers_cfg))?;
    let sessions_cfg = oulipoly_config::SessionsConfig::load(&config_root.join("sessions.toml"))
        .unwrap_or_default();
    Ok(SessionLocateEnvironment {
        state,
        providers_cfg,
        models,
        sessions_cfg,
    })
}

fn render_session_locate_environment_error(message: String) -> i32 {
    emit_metadata_error(&MetadataError::Operational { message });
    1
}

fn render_session_metadata(
    result: Result<oulipoly_runtime::session_metadata::SessionMetadata, MetadataError>,
) -> Result<i32, String> {
    session_metadata_cli::render_session_metadata(result)
}

fn run_session_export(
    session_id: &str,
    format: &str,
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> Result<i32, String> {
    if let Some(exit_code) = validate_session_export_args(session_id, format) {
        return Ok(exit_code);
    }

    let service_output = agent_runtime_services
        .session_export_service
        .export_session(SessionExportServiceRequest {
            session_id: session_id.to_string(),
        })
        .map_err(|err| err.to_string())?;

    let output = match unwrap_export_output(service_output.result) {
        Ok(output) => output,
        Err(exit_code) => return Ok(exit_code),
    };
    write_session_export_output(&output)
}

fn validate_session_export_args(session_id: &str, format: &str) -> Option<i32> {
    if format != "canonical-jsonl" {
        emit_export_json_error(
            "invalid-format",
            &format!("unsupported export format {format}; expected canonical-jsonl"),
        );
        return Some(2);
    }
    if Uuid::parse_str(session_id).is_err() {
        let err = ExportError::InvalidSessionId {
            input: session_id.to_string(),
        };
        emit_export_error(&err);
        return Some(export_error_exit_code(&err));
    }
    None
}

fn unwrap_export_output(result: Result<Vec<u8>, ExportError>) -> Result<Vec<u8>, i32> {
    match result {
        Ok(output) => Ok(output),
        Err(err) => {
            emit_export_error(&err);
            Err(export_error_exit_code(&err))
        }
    }
}

fn write_session_export_output(output: &[u8]) -> Result<i32, String> {
    if let Err(err) = std::io::stdout().write_all(output) {
        emit_export_json_error(
            "operational-error",
            &format!("failed to write canonical export: {err}"),
        );
        return Ok(1);
    }
    Ok(0)
}

fn metadata_error_exit_code(err: &MetadataError) -> i32 {
    match err {
        MetadataError::InvalidSessionId { .. } => 2,
        MetadataError::SessionNotFound { .. } => 10,
        MetadataError::AmbiguousSession { .. } => 11,
        MetadataError::UnsupportedStorage { .. } => 12,
        MetadataError::Operational { .. } => 1,
    }
}

fn metadata_error_code(err: &MetadataError) -> &'static str {
    match err {
        MetadataError::InvalidSessionId { .. } => "invalid-session-id",
        MetadataError::SessionNotFound { .. } => "session-not-found",
        MetadataError::AmbiguousSession { .. } => "ambiguous-session",
        MetadataError::UnsupportedStorage { .. } => "unsupported-storage",
        MetadataError::Operational { .. } => "operational-error",
    }
}

fn metadata_error_message(err: &MetadataError) -> String {
    match err {
        MetadataError::InvalidSessionId { input } => {
            format!("invalid session id: {input}")
        }
        MetadataError::SessionNotFound { input } => {
            format!("session not found: {input}")
        }
        MetadataError::AmbiguousSession { input } => {
            format!("ambiguous session: {input}")
        }
        MetadataError::UnsupportedStorage {
            provider_name,
            reason,
        } => format!("unsupported storage for provider {provider_name}: {reason}"),
        MetadataError::Operational { message } => message.clone(),
    }
}

fn emit_metadata_error(err: &MetadataError) {
    emit_json_error_payload(json_error_payload(
        metadata_error_code(err),
        metadata_error_message(err),
    ));
}

fn export_error_exit_code(err: &ExportError) -> i32 {
    match err {
        ExportError::InvalidSessionId { .. } => 2,
        ExportError::SessionNotFound { .. } => 10,
        ExportError::AmbiguousSession { .. } => 11,
        ExportError::UnsupportedStorage { .. } => 12,
        ExportError::MalformedTranscript { .. } => 15,
        ExportError::Operational { .. } => 1,
    }
}

fn export_error_code(err: &ExportError) -> &'static str {
    match err {
        ExportError::InvalidSessionId { .. } => "invalid-session-id",
        ExportError::SessionNotFound { .. } => "session-not-found",
        ExportError::AmbiguousSession { .. } => "ambiguous-session",
        ExportError::UnsupportedStorage { .. } => "unsupported-storage",
        ExportError::MalformedTranscript { .. } => "malformed-provider-transcript",
        ExportError::Operational { .. } => "operational-error",
    }
}

fn export_error_message(err: &ExportError) -> String {
    match err {
        ExportError::InvalidSessionId { input } => format!("invalid session UUID: {input}"),
        ExportError::SessionNotFound { input } => format!("session not found: {input}"),
        ExportError::AmbiguousSession { input } => {
            format!("session id matches multiple recent chains: {input}")
        }
        ExportError::UnsupportedStorage {
            provider_name,
            reason,
        } => {
            format!("unsupported storage for provider {provider_name}: {reason}")
        }
        ExportError::MalformedTranscript { path, line, reason } => {
            if *line == 0 {
                format!("malformed transcript {}: {reason}", path.display())
            } else {
                format!(
                    "malformed transcript {} line {line}: {reason}",
                    path.display()
                )
            }
        }
        ExportError::Operational { message } => message.clone(),
    }
}

fn emit_export_error(err: &ExportError) {
    emit_export_json_error(export_error_code(err), &export_error_message(err));
}

fn emit_export_json_error(code: &str, message: &str) {
    emit_json_error_payload(json_error_payload(code, message));
}

fn emit_json_error_payload(payload: serde_json::Value) {
    eprintln!("{payload}");
}

// ---
// Component: agent-marker-cwd-resume-diagnostics
// Declared roles: accessor, mapper, formatter, predicate, orchestration, validator, filter
// ---

fn resolve_agent(
    cli: &Cli,
    agent_config: &dyn AgentConfigRepository,
) -> Result<AgentConfig, String> {
    if let Some(ref path) = cli.agent_file {
        return load_agent_by_path(agent_config, path);
    }

    if let Some(ref name) = cli.agent {
        return lookup_agent_by_name(cli, agent_config, name);
    }

    Err(format_missing_agent_error())
}

fn load_agent_by_path(
    agent_config: &dyn AgentConfigRepository,
    path: &Path,
) -> Result<AgentConfig, String> {
    agent_config.load_agent_file(path)
}

fn lookup_agent_by_name(
    cli: &Cli,
    agent_config: &dyn AgentConfigRepository,
    name: &str,
) -> Result<AgentConfig, String> {
    let agents_dir = resolve_agents_dir(cli);
    let agents = agent_config.load_agents(&agents_dir)?;
    agents
        .get(name)
        .cloned()
        .ok_or_else(|| format_unknown_agent_error(name))
}

fn resolve_agents_dir(cli: &Cli) -> PathBuf {
    cli.agents_dir.clone().unwrap_or_else(default_agents_dir)
}

fn default_agents_dir() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("oulipoly-agent-runner").join("agents"))
        .unwrap_or_else(|| PathBuf::from("agents"))
}

fn format_unknown_agent_error(name: &str) -> String {
    format!("Unknown agent: {name}")
}

fn format_missing_agent_error() -> String {
    "No agent specified. Use a positional argument or --agent-file.".to_string()
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

        // Source guard marker: self.db.finalize_invocation(
        if let Err(err) = finalize_invocation_from_guard(self.db, self.invocation_id) {
            emit_finalizer_guard_warning(&err);
        }
    }
}

fn finalize_invocation_from_guard(db: &StateDb, invocation_id: i64) -> Result<(), String> {
    db.finalize_invocation(
        invocation_id,
        false,
        -1,
        Some("guard_drop"),
        Some("guard_drop"),
    )
}

fn emit_finalizer_guard_warning(err: &str) {
    eprintln!("Warning: Failed to finalize invocation in guard: {err}");
}

fn should_emit_invocation_line(is_terminal: bool) -> bool {
    !is_terminal
}

fn emit_result_envelope(
    uuid: &str,
    success: bool,
    exit_code: i32,
    error_category: Option<&str>,
    terminal_reason: Option<&str>,
    failure_identity: Option<&ResultEnvelopeFailureIdentity>,
) {
    let finished_at = current_timestamp_rfc3339();
    let payload = result_envelope_payload(ResultEnvelopeInput {
        id: uuid,
        success,
        exit_code,
        error_category,
        terminal_reason,
        finished_at: &finished_at,
        failure_identity,
    });
    let json = match serialize_result_envelope_payload(&payload) {
        Ok(s) => s,
        Err(err) => {
            emit_result_envelope_serialize_warning(uuid, &err);
            return;
        }
    };
    emit_result_envelope_line(&json);
}

fn current_timestamp_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn serialize_result_envelope_payload(payload: &serde_json::Value) -> Result<String, String> {
    serde_json::to_string(payload).map_err(|err| err.to_string())
}

fn emit_result_envelope_serialize_warning(uuid: &str, err: &str) {
    eprintln!("Warning: Failed to serialize result envelope for {uuid}: {err}");
}

fn emit_result_envelope_line(json: &str) {
    emit_stdout_marker_line("OULIPOLY_RESULT", json);
}

fn emit_pre_invocation_failure_line(json: &str) {
    emit_stdout_marker_line("OULIPOLY_FAILURE", json);
}

fn emit_stdout_marker_line(marker: &str, json: &str) {
    println!("{marker}={json}");
}

fn diagnostics_model_configured(models: &HashMap<String, ModelConfig>) -> bool {
    load_app_config()
        .diagnostics_model
        .as_ref()
        .is_some_and(|name| models.contains_key(name))
}

fn effective_spawn_cwd(working_dir: Option<&Path>) -> Result<PathBuf, String> {
    match working_dir {
        Some(dir) if dir.is_absolute() => Ok(dir.to_path_buf()),
        Some(dir) => Ok(join_relative_cwd(read_current_dir()?, dir)),
        None => read_current_dir(),
    }
}

fn read_current_dir() -> Result<PathBuf, String> {
    std::env::current_dir().map_err(format_current_dir_error)
}

fn join_relative_cwd(current_dir: PathBuf, relative: &Path) -> PathBuf {
    current_dir.join(relative)
}

fn format_current_dir_error(error: std::io::Error) -> String {
    format!("Failed to resolve current directory: {error}")
}

fn effective_resume_spawn_cwd(
    state: &StateDb,
    models: &oulipoly_state::ModelStore,
    providers_cfg: &ProvidersConfig,
    _sessions_cfg: &oulipoly_config::SessionsConfig,
    resume_input: &str,
    working_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    let fallback = effective_spawn_cwd(working_dir)?;
    match resolve_resume_workspace_root(state, models, providers_cfg, resume_input) {
        Ok(workspace_root) => Ok(workspace_root),
        Err(err) => {
            eprintln!(
                "{}",
                format_resume_spawn_cwd_fallback_warning(resume_input, &err, &fallback)
            );
            Ok(fallback)
        }
    }
}

fn format_resume_spawn_cwd_fallback_warning(
    resume_input: &str,
    err: &MetadataError,
    fallback: &Path,
) -> String {
    format!(
        "[resume] warning: could not resolve original cwd for {resume_input}: {}; using {}",
        metadata_error_message(err),
        fallback.display()
    )
}

enum ResumeIngestMode<'a> {
    Unpinned { capture_method: &'a str },
    Pinned { resume_target: &'a str },
}

struct SessionIngestRequest<'a> {
    state: &'a StateDb,
    sessions_cfg: &'a oulipoly_config::SessionsConfig,
    providers_cfg: Option<&'a ProvidersConfig>,
    provider_name: &'a str,
    invocation_row_id: i64,
    invocation_uuid: &'a str,
    effective_cwd: Option<&'a Path>,
    mode: ResumeIngestMode<'a>,
}

fn ingest_and_emit_session_id_resume_aware(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    request: SessionIngestRequest<'_>,
) -> bool {
    let mut stderr = std::io::stderr();
    let SessionIngestRequest {
        state,
        sessions_cfg,
        providers_cfg,
        provider_name,
        invocation_row_id,
        invocation_uuid,
        effective_cwd,
        mode,
    } = request;
    let mode = session_lifecycle_ingest_mode(mode);
    match agent_runtime_services
        .session_lifecycle_service
        .ingest_session(SessionLifecycleRequest {
            state,
            sessions_cfg,
            providers_cfg,
            provider_name,
            invocation_row_id,
            invocation_uuid,
            effective_cwd,
            mode,
            stderr: &mut stderr,
        }) {
        Ok(output) => output.emitted,
        Err(ServiceError::Dependency { message })
        | Err(ServiceError::InvalidRequest { message })
        | Err(ServiceError::Unavailable { message }) => {
            eprintln!("{}", format_session_ingest_failure(provider_name, &message));
            false
        }
    }
}

fn session_lifecycle_ingest_mode(mode: ResumeIngestMode<'_>) -> SessionLifecycleIngestMode {
    match mode {
        ResumeIngestMode::Unpinned { capture_method } => SessionLifecycleIngestMode::Unpinned {
            capture_method: capture_method.to_string(),
        },
        ResumeIngestMode::Pinned { resume_target } => SessionLifecycleIngestMode::Pinned {
            resume_target: resume_target.to_string(),
        },
    }
}

fn format_session_ingest_failure(provider_name: &str, message: &str) -> String {
    format!("Warning: Session ingest failed for {provider_name}: {message}")
}

fn emit_known_session_id(
    state: &StateDb,
    invocation_row_id: i64,
    invocation_uuid: &str,
    session_id: &str,
    capture_method: &str,
) -> bool {
    if !emit_known_session_capture_update(state, invocation_row_id, session_id, capture_method) {
        return false;
    }
    let record = lookup_invocation_record(state, invocation_uuid);
    mint_known_session_chain_if_needed(state, invocation_row_id, record.as_ref());
    emit_known_session_marker(known_session_marker_payload(
        state,
        invocation_uuid,
        session_id,
        record.as_ref(),
    ));
    true
}

fn emit_known_session_capture_update(
    state: &StateDb,
    invocation_row_id: i64,
    session_id: &str,
    capture_method: &str,
) -> bool {
    match update_known_session_capture(state, invocation_row_id, Some(session_id), capture_method) {
        Ok(()) => true,
        Err(err) => {
            emit_known_session_capture_warning(&err);
            false
        }
    }
}

fn emit_known_session_capture_warning(err: &str) {
    eprintln!("Warning: Failed to update invocation session_id: {err}");
}

fn lookup_invocation_record(state: &StateDb, invocation_uuid: &str) -> Option<InvocationRecord> {
    state.get_invocation_by_uuid(invocation_uuid).ok().flatten()
}

fn mint_known_session_chain_if_needed(
    state: &StateDb,
    invocation_row_id: i64,
    record: Option<&InvocationRecord>,
) {
    if should_mint_known_session_chain(record)
        && let Err(err) = state.mint_chain_for_invocation_session(invocation_row_id)
    {
        emit_known_session_chain_warning(&err);
    }
}

fn emit_known_session_chain_warning(err: &str) {
    eprintln!("Warning: Failed to mint session chain: {err}");
}

fn emit_known_session_marker(payload: oulipoly_state::SessionMarkerPayload) {
    eprint!("{}", payload.stderr_line());
}

fn update_known_session_capture(
    state: &StateDb,
    invocation_row_id: i64,
    session_id: Option<&str>,
    capture_method: &str,
) -> Result<(), String> {
    state.update_session_capture(invocation_row_id, session_id, capture_method)
}

fn should_mint_known_session_chain(record: Option<&InvocationRecord>) -> bool {
    record.is_none_or(|row| row.resume_input_id.as_deref() != row.provider_session_id.as_deref())
}

fn known_session_marker_payload(
    state: &StateDb,
    invocation_uuid: &str,
    session_id: &str,
    record: Option<&InvocationRecord>,
) -> oulipoly_state::SessionMarkerPayload {
    let fields = known_session_marker_fields(record, session_id);
    let agent_runner_chain_id = lookup_marker_chain_id(
        state,
        fields.provider_name.as_deref(),
        fields.provider_session_id.as_deref(),
    );
    session_marker_payload_from_parts(marker_payload_parts(
        invocation_uuid,
        session_id,
        fields,
        agent_runner_chain_id,
    ))
}

struct KnownSessionMarkerFields {
    provider_name: Option<String>,
    provider_session_id: Option<String>,
    resume_input_id: Option<String>,
}

fn known_session_marker_fields(
    record: Option<&InvocationRecord>,
    session_id: &str,
) -> KnownSessionMarkerFields {
    let provider_name = record.and_then(|row| row.provider_name.clone());
    let provider_session_id = record
        .and_then(|row| row.provider_session_id.clone())
        .or_else(|| Some(session_id.to_string()));
    KnownSessionMarkerFields {
        provider_name,
        provider_session_id,
        resume_input_id: record.and_then(|row| row.resume_input_id.clone()),
    }
}

fn marker_payload_parts<'a>(
    invocation_uuid: &'a str,
    session_id: &'a str,
    fields: KnownSessionMarkerFields,
    agent_runner_chain_id: Option<String>,
) -> SessionMarkerPayloadParts<'a> {
    SessionMarkerPayloadParts {
        invocation_uuid,
        session_id,
        provider_name: fields.provider_name,
        provider_session_id: fields.provider_session_id,
        agent_runner_chain_id,
        resume_input_id: fields.resume_input_id,
    }
}

struct SessionMarkerPayloadParts<'a> {
    invocation_uuid: &'a str,
    session_id: &'a str,
    provider_name: Option<String>,
    provider_session_id: Option<String>,
    agent_runner_chain_id: Option<String>,
    resume_input_id: Option<String>,
}

fn lookup_marker_chain_id(
    state: &StateDb,
    provider_name: Option<&str>,
    provider_session_id: Option<&str>,
) -> Option<String> {
    provider_name.and_then(|provider_name| {
        provider_session_id.and_then(|provider_session_id| {
            state
                .chain_id_for_segment(provider_name, provider_session_id)
                .ok()
                .flatten()
        })
    })
}

fn session_marker_payload_from_parts(
    parts: SessionMarkerPayloadParts<'_>,
) -> oulipoly_state::SessionMarkerPayload {
    oulipoly_state::SessionMarkerPayload {
        agent_runner_invocation_id: parts.invocation_uuid.to_string(),
        provider_session_id: parts.provider_session_id,
        provider_name: parts.provider_name,
        agent_runner_chain_id: parts.agent_runner_chain_id,
        resume_input_id: parts.resume_input_id,
        legacy_id: parts.invocation_uuid.to_string(),
        legacy_session_id: Some(parts.session_id.to_string()),
    }
}

/// The short `[resume] -> <provider>` line is always emitted regardless of
/// TTY (per proposal §5: V10 wins over V15 here — even at a terminal, the
/// runner's selection must be visible). Factored as a helper so the
/// "always-on" semantic has an explicit, unit-testable surface that mirrors
/// `should_emit_invocation_line`.
fn should_emit_resume_short_line(_is_terminal: bool) -> bool {
    true
}

fn diagnostic_input(stderr: &str, stdout: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    let stdout = stdout.trim();
    let stderr = stderr.trim();
    match (stderr.is_empty(), stdout.is_empty()) {
        (true, true) => String::new(),
        (false, true) => stderr.to_string(),
        (true, false) => stdout.to_string(),
        (false, false) => format!("{stderr}\n{stdout}"),
    }
}

fn resume_model_pool_mismatch_message(
    models: &HashMap<String, ModelConfig>,
    model_name: &str,
    session_id: &str,
    provider_name: &str,
) -> String {
    let suggestions = provider_model_suggestions(models, provider_name);
    format_resume_model_pool_mismatch_message(model_name, session_id, provider_name, &suggestions)
}

fn provider_model_suggestions(
    models: &HashMap<String, ModelConfig>,
    provider_name: &str,
) -> Vec<String> {
    sorted_unique_model_names(provider_models(models, provider_name))
}

fn provider_models<'a>(
    models: &'a HashMap<String, ModelConfig>,
    provider_name: &str,
) -> Vec<&'a ModelConfig> {
    models
        .values()
        .filter(|model| model_has_provider(model, provider_name))
        .collect()
}

fn model_has_provider(model: &ModelConfig, provider_name: &str) -> bool {
    model
        .providers
        .iter()
        .any(|provider| provider.name == provider_name)
}

fn sorted_unique_model_names(models: Vec<&ModelConfig>) -> Vec<String> {
    let mut names: Vec<String> = models.into_iter().map(|model| model.name.clone()).collect();
    names.sort();
    names.dedup();
    names
}

fn format_resume_model_pool_mismatch_message(
    model_name: &str,
    session_id: &str,
    provider_name: &str,
    suggestions: &[String],
) -> String {
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

fn format_resume_error(err: oulipoly_state::ResumeError) -> String {
    use oulipoly_state::ResumeError;
    match err {
        ResumeError::InvalidUuid { input } => format!("invalid session UUID: {input}"),
        ResumeError::NoChainFound { input } => format!(
            "No session found matching {input}. Check that session ingestion is configured and that the provider still has resumable local state."
        ),
        ResumeError::WrongIdKind {
            input,
            provider_session_id,
            agent_runner_invocation_id,
            chain_id,
            provider_name,
            ..
        } => {
            let provider_hint = provider_name
                .as_deref()
                .map(|name| format!(" for provider {name}"))
                .unwrap_or_default();
            let chain_hint = chain_id
                .as_deref()
                .map(|id| format!(" chain={id}."))
                .unwrap_or_default();
            match provider_session_id {
                Some(provider_session_id) => format!(
                    "wrong id kind: {input} is an agent-runner invocation id{provider_hint}, not a provider session id. Use `agents --resume {provider_session_id}` to resume. Use `agents trace --json {agent_runner_invocation_id}` to inspect the runner trace.{chain_hint}"
                ),
                None => format!(
                    "wrong id kind: {input} is an agent-runner invocation id{provider_hint}, but no provider_session_id is bound yet. Use `agents trace --json {agent_runner_invocation_id}` to inspect the runner trace.{chain_hint}"
                ),
            }
        }
        ResumeError::Ambiguous { input, previews } => {
            let mut out = format!(
                "[resume] session {input} matches {} chains:\n",
                previews.len()
            );
            for preview in previews {
                out.push_str(&format!(
                    "  chain {} — last used {} — {} — {} turns\n",
                    preview.chain_id,
                    preview.last_used_at.to_rfc3339(),
                    preview.active_provider,
                    preview.turn_count
                ));
            }
            out.push_str("Re-run with: agents resume <chain_id>");
            out
        }
        ResumeError::ProviderModelMismatch {
            model_name,
            active_provider,
            suggestions,
        } => {
            let suffix = if suggestions.is_empty() {
                format!("(no other model in the loaded config includes {active_provider})")
            } else {
                format!("Try one of: {}", suggestions.join(", "))
            };
            format!(
                "session belongs to provider {active_provider}, which is not in model {model_name}'s provider pool. Model {model_name} does not include active segment's owning provider {active_provider}. {suffix}"
            )
        }
        ResumeError::UnknownModel { model_name } => format!("Unknown model: {model_name}"),
        ResumeError::ActiveSegmentMissing { chain_id } => {
            format!("No active segment found for chain {chain_id}")
        }
        ResumeError::ProviderNotConfigured { provider } => {
            format!("provider {provider} is not configured in any loaded model")
        }
        ResumeError::ProviderMissingResume { provider_name } => {
            format!("provider {provider_name} has no [providers.resume] block; cannot resume")
        }
        ResumeError::Db { message } => message,
    }
}

#[derive(Clone)]
struct ResumeExecutionTarget {
    model: Option<ModelConfig>,
    provider_index: usize,
    provider: ProviderConfig,
    prompt_mode: PromptMode,
}

fn resume_execution_target(
    resolved: &oulipoly_state::ResolvedResume,
    providers_cfg: &ProvidersConfig,
) -> Result<ResumeExecutionTarget, oulipoly_state::ResumeError> {
    if let Some(model) = resolved.model.as_ref() {
        return resume_model_execution_target(model, &resolved.active_provider, providers_cfg);
    }
    resume_provider_execution_target(&resolved.active_provider, providers_cfg)
}

fn resume_model_execution_target(
    model: &ModelConfig,
    active_provider: &str,
    providers_cfg: &ProvidersConfig,
) -> Result<ResumeExecutionTarget, oulipoly_state::ResumeError> {
    let provider_index = resolve_model_provider_index(model, active_provider)?;
    let (provider, prompt_mode) = providers_cfg
        .effective_provider(&model.providers[provider_index])
        .map_err(resume_db_error)?;
    Ok(ResumeExecutionTarget {
        model: Some(model.clone()),
        provider_index,
        provider,
        prompt_mode,
    })
}

fn resolve_model_provider_index(
    model: &ModelConfig,
    active_provider: &str,
) -> Result<usize, oulipoly_state::ResumeError> {
    model
        .providers
        .iter()
        .position(|provider| provider.name == active_provider)
        .ok_or_else(|| oulipoly_state::ResumeError::ProviderModelMismatch {
            model_name: model.name.clone(),
            active_provider: active_provider.to_string(),
            suggestions: Vec::new(),
        })
}

fn resume_provider_execution_target(
    active_provider: &str,
    providers_cfg: &ProvidersConfig,
) -> Result<ResumeExecutionTarget, oulipoly_state::ResumeError> {
    let (provider, prompt_mode) = providers_cfg
        .runtime_provider(active_provider)
        .map_err(resume_db_error)?;
    Ok(ResumeExecutionTarget {
        model: None,
        provider_index: provider_index_in_providers_cfg(providers_cfg, active_provider),
        provider,
        prompt_mode,
    })
}

fn resume_db_error(message: String) -> oulipoly_state::ResumeError {
    oulipoly_state::ResumeError::Db { message }
}

fn provider_session_resolved_account(
    provider: &ProviderConfig,
    provider_session_id: &str,
) -> Option<String> {
    let session_storage = provider.session_storage.as_ref()?;
    match session_storage {
        SessionStorage::ClaudeCode { projects_dir } => Some(projects_dir.display().to_string()),
        SessionStorage::Codex { sessions_dir } => Some(sessions_dir.display().to_string()),
        SessionStorage::Script { .. } => resolve_workspace_root_for_provider_session(
            Some(session_storage),
            &provider.name,
            provider_session_id,
        )
        .ok()
        .map(|path| path.display().to_string()),
    }
}

fn run_pause_handshake(
    session_id: &str,
    ttl_ms: Option<u64>,
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> Result<i32, String> {
    let Some(ttl_ms) = validate_pause_handshake_args(session_id, ttl_ms) else {
        return Ok(2);
    };

    let output = agent_runtime_services
        .session_lock_service
        .lock_session(SessionLockServiceRequest::Acquire {
            session_id: session_id.to_string(),
            ttl_ms,
        })
        .map_err(|err| err.to_string())?;
    render_pause_handshake_output(output.result)
}

fn validate_pause_handshake_args(session_id: &str, ttl_ms: Option<u64>) -> Option<u64> {
    if Uuid::parse_str(session_id).is_err() {
        emit_json_error(
            2,
            "invalid-session-id",
            format!("invalid session UUID: {session_id}"),
        );
        return None;
    }
    let ttl_ms = ttl_ms.unwrap_or(DEFAULT_PAUSE_HANDSHAKE_TTL_MS);
    if ttl_ms > MAX_PAUSE_HANDSHAKE_TTL_MS {
        emit_json_error(
            2,
            "invalid-ttl",
            format!("ttl-ms must be at most {MAX_PAUSE_HANDSHAKE_TTL_MS}"),
        );
        return None;
    }
    Some(ttl_ms)
}

fn render_pause_handshake_output(
    result: Result<SessionLockSuccess, SessionLockFailure>,
) -> Result<i32, String> {
    match result {
        Ok(SessionLockSuccess::Acquired {
            chain_id, lease, ..
        }) => {
            println!("{}", pause_handshake_receipt_json(&chain_id, &lease)?);
            Ok(0)
        }
        Ok(SessionLockSuccess::Released { .. }) => unreachable!("acquire cannot release a lock"),
        Err(SessionLockFailure::Resume(err)) => Ok(emit_resume_resolution_error(err)),
        Err(SessionLockFailure::Lock(err)) => Ok(emit_lock_error(err)),
    }
}

fn pause_handshake_receipt_json(
    chain_id: &str,
    lease: &oulipoly_runtime::session_lock::Lease,
) -> Result<String, String> {
    let payload = serde_json::json!({
        "session_id": lease.session_id,
        "chain_id": chain_id,
        "provider_name": lease.provider_name,
        "token": lease.token,
        "expires_at": lease.expires_at,
        "lock_path": lease.lock_path,
    });
    serde_json::to_string(&payload).map_err(|err| format!("failed to encode pause receipt: {err}"))
}

fn run_resume_handshake(
    session_id: &str,
    token: &str,
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> Result<i32, String> {
    if let Some(exit_code) = validate_resume_handshake_session_id(session_id) {
        return Ok(exit_code);
    }

    let output = agent_runtime_services
        .session_lock_service
        .lock_session(SessionLockServiceRequest::Release {
            session_id: session_id.to_string(),
            token: token.to_string(),
        })
        .map_err(|err| err.to_string())?;
    render_resume_handshake_output(output.result)
}

fn validate_resume_handshake_session_id(session_id: &str) -> Option<i32> {
    if Uuid::parse_str(session_id).is_err() {
        Some(emit_json_error(
            2,
            "invalid-session-id",
            format!("invalid session UUID: {session_id}"),
        ))
    } else {
        None
    }
}

fn render_resume_handshake_output(
    result: Result<SessionLockSuccess, SessionLockFailure>,
) -> Result<i32, String> {
    match result {
        Ok(SessionLockSuccess::Released { receipt }) => {
            println!(
                "{}",
                serde_json::to_string(&receipt)
                    .map_err(|err| format!("failed to encode resume receipt: {err}"))?
            );
            Ok(0)
        }
        Ok(SessionLockSuccess::Acquired { .. }) => unreachable!("release cannot acquire a lock"),
        Err(SessionLockFailure::Lock(err)) => Ok(emit_lock_error(err)),
        Err(SessionLockFailure::Resume(_)) => unreachable!("release does not resolve resume"),
    }
}

fn provider_index_in_providers_cfg(providers_cfg: &ProvidersConfig, provider_name: &str) -> usize {
    let mut names = providers_cfg.entries.keys().collect::<Vec<_>>();
    names.sort();
    names
        .into_iter()
        .position(|name| name == provider_name)
        .unwrap_or(0)
}

fn emit_resume_resolution_error(err: oulipoly_state::ResumeError) -> i32 {
    use oulipoly_state::ResumeError;
    match err {
        ResumeError::InvalidUuid { input } => emit_json_error(
            2,
            "invalid-session-id",
            format!("invalid session UUID: {input}"),
        ),
        ResumeError::NoChainFound { input } => emit_json_error(
            10,
            "session-not-found",
            format!("no session found matching {input}"),
        ),
        ResumeError::WrongIdKind {
            input,
            provider_session_id,
            agent_runner_invocation_id,
            chain_id,
            provider_name,
            ..
        } => {
            let payload = serde_json::json!({
                "error": {
                    "code": "wrong-id-kind",
                    "message": format!("wrong id kind: {input} is an agent-runner invocation id"),
                    "input": input,
                    "agent_runner_invocation_id": agent_runner_invocation_id,
                    "provider_session_id": provider_session_id,
                    "agent_runner_chain_id": chain_id,
                    "provider_name": provider_name,
                }
            });
            eprintln!("{payload}");
            10
        }
        ResumeError::Ambiguous { input, .. } => emit_json_error(
            11,
            "ambiguous-session",
            format!("session {input} resolves to multiple chains"),
        ),
        ResumeError::UnknownModel { model_name } => emit_json_error(
            12,
            "model-resolution-failed",
            format!("unknown model for session: {model_name}"),
        ),
        ResumeError::ProviderModelMismatch {
            model_name,
            active_provider,
            ..
        } => emit_json_error(
            12,
            "model-resolution-failed",
            format!("model {model_name} does not include active provider {active_provider}"),
        ),
        ResumeError::ActiveSegmentMissing { chain_id } => emit_json_error(
            12,
            "model-resolution-failed",
            format!("no active segment found for chain {chain_id}"),
        ),
        ResumeError::ProviderNotConfigured { provider } => emit_json_error(
            12,
            "model-resolution-failed",
            format!("provider {provider} is not configured"),
        ),
        ResumeError::ProviderMissingResume { provider_name } => emit_json_error(
            12,
            "model-resolution-failed",
            format!("provider {provider_name} has no resume configuration"),
        ),
        ResumeError::Db { message } => emit_json_error(1, "operational-error", message),
    }
}

fn emit_lock_error(err: LockError) -> i32 {
    match err {
        LockError::Busy { expires_at, .. } => emit_json_error(
            13,
            "session-busy",
            format!("session is paused until {expires_at}"),
        ),
        LockError::TokenInvalid => emit_json_error(
            16,
            "lock-token-invalid",
            "pause token is invalid for this session",
        ),
        LockError::LockExpired => emit_json_error(
            17,
            "lock-expired",
            "pause lock is absent or expired without release evidence",
        ),
        LockError::Operational { message } => emit_json_error(1, "operational-error", message),
    }
}

fn emit_json_error(code: i32, error_code: &str, message: impl Into<String>) -> i32 {
    let _ = writeln!(
        std::io::stderr(),
        "{}",
        json_error_payload(error_code, message)
    );
    code
}

fn effective_model_for_execution(
    model: &ModelConfig,
    provider_index: usize,
    providers_cfg: &ProvidersConfig,
) -> Result<(ProviderConfig, PromptMode), String> {
    effective_provider_for_model_provider(model, provider_index, providers_cfg)
}

fn emit_unknown_diagnostic(
    state: &StateDb,
    provider_name: &str,
    provider_index: usize,
    result: &executor::ExecutionResult,
    retry_rotation_disposition: &str,
) {
    let payload = serde_json::json!({
        "error_category": "unknown",
        "provider": provider_name,
        "provider_index": provider_index,
        "account_window_state": account_window_state_payload(state, provider_name),
        "exit_code": result.exit_code,
        "retry_rotation_disposition": retry_rotation_disposition,
        "stderr_excerpt": redacted_stderr_excerpt(&result.stderr),
    });
    match serde_json::to_string(&payload) {
        Ok(json) => eprintln!("OULIPOLY_UNKNOWN_DIAGNOSTIC={json}"),
        Err(err) => eprintln!("Warning: Failed to serialize unknown diagnostic: {err}"),
    }
}

fn account_window_state_payload(state: &StateDb, provider_name: &str) -> serde_json::Value {
    format_account_window_state_payload(read_account_window_state(state, provider_name))
}

struct AccountWindowStateRead {
    quota: Result<Option<oulipoly_state::QuotaRecord>, String>,
    windows: Result<Vec<oulipoly_state::QuotaWindow>, String>,
}

fn read_account_window_state(state: &StateDb, provider_name: &str) -> AccountWindowStateRead {
    AccountWindowStateRead {
        quota: state.get_quota(provider_name),
        windows: state.get_windows(provider_name),
    }
}

fn format_account_window_state_payload(read: AccountWindowStateRead) -> serde_json::Value {
    let (quota, quota_read_error) = match read.quota {
        Ok(quota) => (quota.map(quota_record_payload), None),
        Err(err) => (None, Some(err)),
    };
    let (windows, windows_read_error) = match read.windows {
        Ok(windows) => (
            windows
                .into_iter()
                .map(quota_window_payload)
                .collect::<Vec<_>>(),
            None,
        ),
        Err(err) => (Vec::new(), Some(err)),
    };
    serde_json::json!({
        "quota": quota,
        "quota_read_error": quota_read_error,
        "windows": windows,
        "windows_read_error": windows_read_error,
    })
}

fn quota_record_payload(record: oulipoly_state::QuotaRecord) -> serde_json::Value {
    serde_json::json!({
        "calls_since_refresh": record.calls_since_refresh,
        "refreshed_at": record.refreshed_at.map(|value| value.to_rfc3339()),
        "exhausted_at": record.exhausted_at.map(|value| value.to_rfc3339()),
        "topology_peak_live_window_count": record.topology_peak_live_window_count,
        "last_topology_probe_at": record.last_topology_probe_at.map(|value| value.to_rfc3339()),
        "next_available_at": record.next_available_at.map(|value| value.to_rfc3339()),
        "last_refresh_at": record.last_refresh_at.map(|value| value.to_rfc3339()),
        "failure_class": record.failure_class,
    })
}

fn quota_window_payload(window: oulipoly_state::QuotaWindow) -> serde_json::Value {
    serde_json::json!({
        "window_id": window.window_id,
        "used_percent": window.used_percent,
        "resets_at": window.resets_at.to_rfc3339(),
        "last_delta_percent": window.last_delta_percent,
        "last_delta_calls": window.last_delta_calls,
    })
}

fn redacted_stderr_excerpt(stderr: &str) -> String {
    truncate_utf8_bytes(&first_nonempty_lines(&redact_sensitive(stderr), 4), 1024)
}

fn redact_sensitive(text: &str) -> String {
    let bearer = Regex::new(r"(?i)\bbearer\s+[^\s]+").expect("bearer redaction regex must compile");
    let key_value = Regex::new(
        r#"(?i)(["']?\b(?:token|api_key|apikey|password|secret)\b["']?\s*[:=]\s*)(["']?)[^\s"']+"#,
    )
    .expect("key-value redaction regex must compile");

    let text = redact_authorization_headers(text);
    let text = bearer.replace_all(&text, "Bearer [REDACTED]");
    key_value.replace_all(&text, "$1$2[REDACTED]").into_owned()
}

fn redact_authorization_headers(text: &str) -> String {
    let authorization =
        Regex::new(r"(?i)\bauthorization\s*:\s*").expect("authorization regex must compile");
    let mut redacted = String::with_capacity(text.len());
    for segment in text.split_inclusive('\n') {
        let (line, newline) = segment
            .strip_suffix('\n')
            .map_or((segment, ""), |line| (line, "\n"));
        let matches = authorization.find_iter(line).collect::<Vec<_>>();
        if matches.is_empty() {
            redacted.push_str(line);
            redacted.push_str(newline);
            continue;
        }

        let mut cursor = 0;
        for (index, header) in matches.iter().enumerate() {
            redacted.push_str(&line[cursor..header.end()]);
            let value_start = header.end();
            let value_end = matches
                .get(index + 1)
                .map_or(line.len(), |next| next.start());
            if line[value_start..value_end]
                .chars()
                .any(|character| !character.is_whitespace())
            {
                redacted.push_str("[REDACTED]");
            }
            cursor = value_end;
        }
        redacted.push_str(&line[cursor..]);
        redacted.push_str(newline);
    }
    redacted
}

fn first_nonempty_lines(text: &str, max_lines: usize) -> String {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .take(max_lines)
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate_utf8_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

fn resume_migration_pool(
    resolved: &oulipoly_state::ResolvedResume,
    providers_cfg: &ProvidersConfig,
) -> ModelConfig {
    if let Some(model) = resolved.model.as_ref() {
        return resume_migration_model_pool(model, providers_cfg);
    }

    provider_default_migration_pool(&resolved.active_provider, providers_cfg)
}

fn resume_migration_model_pool(
    model: &ModelConfig,
    providers_cfg: &ProvidersConfig,
) -> ModelConfig {
    let mut effective = model.clone();
    effective.providers = effective_migration_providers(model, providers_cfg);
    effective
}

fn effective_migration_providers(
    model: &ModelConfig,
    providers_cfg: &ProvidersConfig,
) -> Vec<ProviderConfig> {
    present_provider_configs(effective_migration_provider_options(model, providers_cfg))
}

fn effective_migration_provider_options(
    model: &ModelConfig,
    providers_cfg: &ProvidersConfig,
) -> Vec<Option<ProviderConfig>> {
    model
        .providers
        .iter()
        .map(|provider| effective_migration_provider(provider, providers_cfg))
        .collect()
}

fn effective_migration_provider(
    provider: &ProviderConfig,
    providers_cfg: &ProvidersConfig,
) -> Option<ProviderConfig> {
    providers_cfg
        .effective_provider(provider)
        .ok()
        .map(|provider| provider.0)
}

fn present_provider_configs(options: Vec<Option<ProviderConfig>>) -> Vec<ProviderConfig> {
    options.into_iter().flatten().collect()
}

fn provider_default_migration_pool(
    active_provider: &str,
    providers_cfg: &ProvidersConfig,
) -> ModelConfig {
    ModelConfig {
        name: "<provider-default>".to_string(),
        prompt_mode: PromptMode::Stdin,
        providers: runtime_migration_providers(active_provider, providers_cfg),
        inputs: Vec::new(),
        provider: None,
    }
}

fn runtime_migration_providers(
    active_provider: &str,
    providers_cfg: &ProvidersConfig,
) -> Vec<ProviderConfig> {
    present_provider_configs(runtime_migration_provider_options(
        resume_migration_provider_names(active_provider, providers_cfg),
        providers_cfg,
    ))
}

fn resume_migration_provider_names(
    active_provider: &str,
    providers_cfg: &ProvidersConfig,
) -> Vec<String> {
    sorted_provider_names(providers_cfg)
        .into_iter()
        .filter(|name| is_resume_migration_provider(name, active_provider, providers_cfg))
        .collect()
}

fn runtime_migration_provider_options(
    names: Vec<String>,
    providers_cfg: &ProvidersConfig,
) -> Vec<Option<ProviderConfig>> {
    names
        .into_iter()
        .map(|name| runtime_provider_config(&name, providers_cfg))
        .collect()
}

fn runtime_provider_config(name: &str, providers_cfg: &ProvidersConfig) -> Option<ProviderConfig> {
    providers_cfg
        .runtime_provider(name)
        .ok()
        .map(|provider| provider.0)
}

fn sorted_provider_names(providers_cfg: &ProvidersConfig) -> Vec<String> {
    let mut names = providers_cfg.entries.keys().cloned().collect::<Vec<_>>();
    names.sort();
    names
}

fn is_resume_migration_provider(
    name: &str,
    active_provider: &str,
    providers_cfg: &ProvidersConfig,
) -> bool {
    name == active_provider
        || providers_cfg
            .get(name)
            .is_some_and(|entry| entry.session_storage.is_some())
}

// ---
// Component: interactive-resume-execution
// Declared roles: orchestration, validator, mapper, formatter, accessor
// ---

struct ResumeExecutionEnvironment {
    state: StateDb,
    providers_cfg: ProvidersConfig,
    models: HashMap<String, ModelConfig>,
    sessions_cfg: oulipoly_config::SessionsConfig,
}

fn load_resume_execution_environment(
    models_dir_override: Option<&Path>,
) -> Result<ResumeExecutionEnvironment, String> {
    let state = StateDb::open_default()?;
    let models_dir = models_dir_override
        .map(Path::to_path_buf)
        .unwrap_or_else(default_models_dir);
    let config_root = default_config_root();
    let providers_cfg = oulipoly_config::ProvidersConfig::load(&config_root.join("providers.toml"))
        .unwrap_or_default();
    let models = load_models(&models_dir, Some(&providers_cfg))?;
    let sessions_cfg = oulipoly_config::SessionsConfig::load(&config_root.join("sessions.toml"))
        .unwrap_or_default();
    Ok(ResumeExecutionEnvironment {
        state,
        providers_cfg,
        models,
        sessions_cfg,
    })
}

fn run_repl(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    model_name: Option<&str>,
    resume: Option<&str>,
    manual_migrate: Option<&str>,
    working_dir: Option<&Path>,
    models_dir_override: Option<&Path>,
) -> Result<i32, String> {
    let env = load_resume_execution_environment(models_dir_override)?;
    let mut resolved_resume =
        resolve_optional_repl_resume(agent_runtime_services, &env, resume, model_name)?;
    let mut fallback_target = match resolved_resume.as_ref() {
        Some(resolved) => Some(
            resume_execution_target(resolved, &env.providers_cfg).map_err(format_resume_error)?,
        ),
        None => None,
    };
    let direct_model = if fallback_target.is_none() {
        let model_name =
            model_name.ok_or_else(|| "model is required unless --resume is present".to_string())?;
        Some(
            env.models
                .get(model_name)
                .cloned()
                .ok_or_else(|| format!("Unknown model: {model_name}"))?,
        )
    } else {
        None
    };
    let model = fallback_target
        .as_ref()
        .and_then(|target| target.model.clone())
        .or(direct_model)
        .unwrap_or_else(|| ModelConfig {
            name: "<provider-default>".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: Vec::new(),
            inputs: Vec::new(),
            provider: None,
        });

    let in_flight = oulipoly_runtime::quota::InFlight::new();
    let ctx = balancer::BalanceContext {
        providers_cfg: &env.providers_cfg,
        sessions_cfg: &env.sessions_cfg,
        in_flight: &in_flight,
    };

    let parent_invocation_id = resolve_parent_invocation_id(&env.state);
    let stderr_is_terminal = std::io::stderr().is_terminal();
    let mut resume_spawn_cwd = None;
    let (provider_index, provider, resume_session_id) =
        if let Some(resolved) = resolved_resume.as_mut() {
            let selected_provider = &resolved.active_provider;
            if should_emit_resume_short_line(stderr_is_terminal) {
                eprintln!("[resume] -> {selected_provider}");
            }
            if fallback_target
                .as_ref()
                .is_some_and(|target| target.provider.resume.is_none())
            {
                eprintln!(
                    "provider {selected_provider} has no [providers.resume] block; cannot resume"
                );
                return Ok(1);
            }
            let migration_model = resume_migration_pool(resolved, &env.providers_cfg);
            let effective_spawn_cwd = effective_resume_spawn_cwd(
                &env.state,
                &env.models,
                &env.providers_cfg,
                &env.sessions_cfg,
                resume.expect("resume input must exist for resolved resume"),
                working_dir,
            )?;
            resume_spawn_cwd = Some(effective_spawn_cwd.clone());
            let mut migration_stderr = std::io::stderr();
            match agent_runtime_services
                .migration_service
                .migrate(MigrationServiceRequest {
                    state: &env.state,
                    sessions_cfg: &env.sessions_cfg,
                    resolved,
                    manual_target: manual_migrate,
                    active_exhausted: false,
                    migration_model: &migration_model,
                    effective_cwd: &effective_spawn_cwd,
                    stderr: &mut migration_stderr,
                }) {
                Ok(MigrationServiceOutput::Migrated { segment: migrated })
                | Ok(MigrationServiceOutput::AutoRotated {
                    segment: migrated, ..
                }) => {
                    resolved.active_provider = migrated.target_provider.clone();
                    resolved.active_session_id = migrated.target_session_id.clone();
                    fallback_target = Some(
                        resume_execution_target(resolved, &env.providers_cfg)
                            .map_err(format_resume_error)?,
                    );
                }
                Ok(MigrationServiceOutput::Stay) => {}
                Ok(MigrationServiceOutput::RotationFailed { reason }) => {
                    eprintln!("{}", format_rotation_failed_reason(&reason));
                    return Ok(1);
                }
                Err(ServiceError::Dependency { message }) => {
                    eprintln!("migration failed: {message}");
                    return Ok(1);
                }
                Err(err) => return Err(format!("migration service failed: {err}")),
            }

            let target = fallback_target
                .as_ref()
                .expect("resume target must be resolved before spawn");
            let provider_index = target.provider_index;
            let provider = target.provider.clone();
            if provider.resume.is_none() {
                eprintln!(
                    "provider {} has no [providers.resume] block; cannot resume",
                    provider.name
                );
                return Ok(1);
            }

            (
                provider_index,
                provider,
                Some(resolved.active_session_id.clone()),
            )
        } else {
            let provider_index = agent_runtime_services
                .routing_service
                .select_route(RoutingServiceRequest {
                    model: &model,
                    state: &env.state,
                    ctx: Some(&ctx),
                })
                .map_err(|err| err.to_string())?
                .provider_index;
            let (provider, _) =
                effective_model_for_execution(&model, provider_index, &env.providers_cfg)?;
            (provider_index, provider, None)
        };
    validate_provider_repl_capability(&provider)?;

    let invocation_id = Uuid::new_v4().to_string();
    let invocation = CompositeInvocationId {
        source: provider.name.clone(),
        id: invocation_id,
    };
    let invocation_model_name = resolved_resume
        .as_ref()
        .and_then(|resolved| resolved.model_name.clone())
        .unwrap_or_else(|| {
            if resume.is_some() {
                "<unknown>".to_string()
            } else {
                model.name.clone()
            }
        });
    let invocation_start = InvocationStart {
        invocation_uuid: invocation.id.clone(),
        model_name: invocation_model_name,
        provider_name: provider.name.clone(),
        provider_index,
        parent_invocation_id,
    };
    let invocation_row_id = agent_runtime_services
        .invocation_lifecycle_service
        .start_invocation(InvocationLifecycleStartRequest {
            state: &env.state,
            start: &invocation_start,
        })
        .map_err(|err| err.to_string())?
        .invocation_row_id;
    let mut guard = FinalizerGuard::new(&env.state, invocation_row_id);
    let invocation_env = serde_json::to_string(&invocation)
        .map_err(|e| format!("Failed to serialize invocation id: {e}"))?;

    if let Some(active_session_id) = resume_session_id.as_deref() {
        env.state.bind_invocation_provider_session_start(
            invocation_row_id,
            &oulipoly_state::ProviderSessionBinding {
                provider_session_id: active_session_id.to_string(),
                capture_method: "resumed",
                resume_input_id: resume.map(str::to_string),
                provider_session_resolved_account: None,
            },
        )?;
        if let Some(session_id) = resume
            && manual_migrate.is_some()
        {
            env.state
                .record_legacy_resume_input_session_id(invocation_row_id, session_id)?;
        }
    }

    if should_emit_invocation_line(stderr_is_terminal) {
        eprintln!("{}", invocation.stderr_line());
    }

    let resume_payload = resume_session_id.as_deref().map(|session_id| {
        let strategy = provider
            .resume
            .as_ref()
            .expect("resumable provider must have a resume strategy");
        executor::cli::ResumePayload {
            session_id,
            strategy,
        }
    });
    let zero_turn_baseline = zero_turn_record_baseline(
        &env.state,
        &env.sessions_cfg,
        &provider.name,
        resume_session_id.as_deref(),
    );

    let interactive_effective_cwd = resume_spawn_cwd
        .clone()
        .map(Ok)
        .unwrap_or_else(|| effective_spawn_cwd(working_dir))?;
    match executor::cli::execute_interactive_with_result(
        &provider,
        resume_spawn_cwd.as_deref().or(working_dir),
        Some(&invocation_env),
        resume_payload,
    ) {
        Ok(mut result) => {
            apply_age153_terminal_signal_fixture_override_to_fields(
                &mut result.terminal_signal,
                &mut result.terminal_reason,
            );
            let zero_turn_classification = zero_turn_classify_after_completion(
                &env.state,
                &env.sessions_cfg,
                &zero_turn_baseline,
            );
            apply_zero_turn_classification_to_signal_fields(
                &mut result.terminal_signal,
                &mut result.terminal_reason,
                &provider.name,
                &zero_turn_classification,
            );
            let exit_code = result.exit_code;
            let invocation_uuid =
                Uuid::parse_str(&invocation.id).expect("generated invocation id must be a UUID");
            let resume_session_uuid = resume_session_id
                .as_deref()
                .and_then(|session_id| Uuid::parse_str(session_id).ok());
            let mut terminal_signal_stderr = std::io::stderr();
            let mut terminal_signal_ctx = TerminalSignalContext {
                invocation_id: &invocation_uuid,
                session_id: resume_session_uuid.as_ref(),
                provider: &provider.name,
                state_db: &env.state,
                stderr: &mut terminal_signal_stderr,
            };
            let terminal_signal_disposition =
                // AGE-153 source guard: marker emission routes through emit_terminal_signal_marker.
                apply_terminal_signal_outcome(&result.terminal_signal, &mut terminal_signal_ctx);
            if resume.is_none() {
                env.state
                    .update_session_capture(invocation_row_id, None, "none")?;
            }
            // AGE-153 source guard: TerminalSignalKind::CleanExit maps to TerminalSignalDisposition::InteractiveClean.
            match terminal_signal_disposition {
                TerminalSignalDisposition::InteractiveFail
                | TerminalSignalDisposition::ProlongedSilenceFail
                | TerminalSignalDisposition::QuotaExhaustedRetry
                | TerminalSignalDisposition::MaybeQuotaVerify => {
                    let terminal_reason = terminal_signal_reason(
                        &result.terminal_signal,
                        result.terminal_reason.as_deref(),
                    )
                    .unwrap_or("unknown_exit");
                    agent_runtime_services
                        .invocation_lifecycle_service
                        .finalize_invocation(InvocationLifecycleFinalizeRequest {
                            state: &env.state,
                            invocation_row_id,
                            success: false,
                            exit_code,
                            error_category: terminal_signal_error_category(
                                &result.terminal_signal,
                                terminal_reason,
                            ),
                            terminal_reason: Some(terminal_reason),
                        })
                        .map_err(|err| err.to_string())?;
                    guard.mark_finalized();
                    return Ok(exit_code);
                }
                TerminalSignalDisposition::InteractiveClean
                | TerminalSignalDisposition::NotApplicable => {}
            }
            agent_runtime_services
                .invocation_lifecycle_service
                .finalize_invocation(InvocationLifecycleFinalizeRequest {
                    state: &env.state,
                    invocation_row_id,
                    success: exit_code == 0,
                    exit_code,
                    error_category: None,
                    terminal_reason: result.terminal_reason.as_deref(),
                })
                .map_err(|err| err.to_string())?;
            guard.mark_finalized();
            if exit_code == 0 {
                ingest_and_emit_session_id_resume_aware(
                    agent_runtime_services,
                    SessionIngestRequest {
                        state: &env.state,
                        sessions_cfg: &env.sessions_cfg,
                        providers_cfg: Some(&env.providers_cfg),
                        provider_name: &provider.name,
                        invocation_row_id,
                        invocation_uuid: &invocation.id,
                        effective_cwd: Some(&interactive_effective_cwd),
                        mode: match resume {
                            Some(session_id) => ResumeIngestMode::Pinned {
                                resume_target: if manual_migrate.is_some() {
                                    session_id
                                } else {
                                    resume_session_id.as_deref().unwrap_or(session_id)
                                },
                            },
                            None => ResumeIngestMode::Unpinned {
                                capture_method: "turn_script",
                            },
                        },
                    },
                );
            }
            Ok(exit_code)
        }
        Err(_spawn_err) => {
            if resume.is_none() {
                env.state
                    .update_session_capture(invocation_row_id, None, "none")?;
            }
            agent_runtime_services
                .invocation_lifecycle_service
                .finalize_invocation(InvocationLifecycleFinalizeRequest {
                    state: &env.state,
                    invocation_row_id,
                    success: false,
                    exit_code: 1,
                    error_category: Some("spawn_error"),
                    terminal_reason: Some("spawn_error"),
                })
                .map_err(|err| err.to_string())?;
            guard.mark_finalized();
            Ok(1)
        }
    }
}

fn validate_provider_repl_capability(provider: &ProviderConfig) -> Result<(), String> {
    if provider.interactive_args.is_some() {
        Ok(())
    } else {
        Err(format_repl_launch_failure_message(provider))
    }
}

fn format_repl_launch_failure_message(provider: &ProviderConfig) -> String {
    format!(
        "Provider {} has no interactive_args; cannot launch interactively",
        provider.name
    )
}

fn resolve_optional_repl_resume(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    env: &ResumeExecutionEnvironment,
    resume: Option<&str>,
    model_name: Option<&str>,
) -> Result<Option<oulipoly_state::ResolvedResume>, String> {
    let Some(session_id) = resume else {
        return Ok(None);
    };
    resolve_repl_resume(agent_runtime_services, env, session_id, model_name).map(Some)
}

fn resolve_repl_resume(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    env: &ResumeExecutionEnvironment,
    session_id: &str,
    model_name: Option<&str>,
) -> Result<oulipoly_state::ResolvedResume, String> {
    match agent_runtime_services
        .resume_service
        .resolve_resume(ResumeServiceRequest {
            state: &env.state,
            models: &env.models,
            input: session_id,
            model_override: model_name,
        }) {
        Ok(ResumeServiceOutput::ResumeResolved { resolved }) => Ok(resolved),
        Ok(ResumeServiceOutput::ResumeRejected {
            error:
                oulipoly_state::ResumeError::ProviderModelMismatch {
                    active_provider, ..
                },
        }) => Err(resume_model_pool_mismatch_message(
            &env.models,
            model_name.unwrap_or("<unknown>"),
            session_id,
            &active_provider,
        )),
        Ok(ResumeServiceOutput::ResumeRejected { error }) => Err(format_resume_error(error)),
        Err(err) => Err(format!("resume service failed: {err}")),
    }
}

fn resolve_resume_for_headless_execution(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    env: &ResumeExecutionEnvironment,
    session_id: &str,
    model_name: Option<&str>,
) -> Result<oulipoly_state::ResolvedResume, i32> {
    match agent_runtime_services
        .resume_service
        .resolve_resume(ResumeServiceRequest {
            state: &env.state,
            models: &env.models,
            input: session_id,
            model_override: model_name,
        }) {
        Ok(ResumeServiceOutput::ResumeResolved { resolved }) => Ok(resolved),
        Ok(ResumeServiceOutput::ResumeRejected {
            error:
                oulipoly_state::ResumeError::ProviderModelMismatch {
                    active_provider, ..
                },
        }) => {
            render_resume_model_pool_mismatch(
                &env.models,
                model_name.unwrap_or("<unknown>"),
                session_id,
                &active_provider,
            );
            Err(1)
        }
        Ok(ResumeServiceOutput::ResumeRejected { error }) => {
            render_resume_error(error);
            Err(1)
        }
        Err(err) => {
            render_resume_service_failure(&err.to_string());
            Err(1)
        }
    }
}

fn render_resume_model_pool_mismatch(
    models: &HashMap<String, ModelConfig>,
    model_name: &str,
    session_id: &str,
    active_provider: &str,
) {
    eprintln!(
        "{}",
        resume_model_pool_mismatch_message(models, model_name, session_id, active_provider)
    );
}

fn render_resume_error(error: oulipoly_state::ResumeError) {
    eprintln!("{}", format_resume_error(error));
}

fn render_resume_service_failure(error: &str) {
    eprintln!("resume service failed: {error}");
}

fn prepare_initial_headless_resume_target(
    resolved: &oulipoly_state::ResolvedResume,
    providers_cfg: &ProvidersConfig,
    stderr_is_terminal: bool,
) -> Result<(), i32> {
    let target = renderable_resume_execution_target(resolved, providers_cfg)?;
    render_resume_short_line_if_needed(stderr_is_terminal, &resolved.active_provider);
    validate_headless_resume_target(&target, &resolved.active_provider)
}

fn renderable_resume_execution_target(
    resolved: &oulipoly_state::ResolvedResume,
    providers_cfg: &ProvidersConfig,
) -> Result<ResumeExecutionTarget, i32> {
    resume_execution_target(resolved, providers_cfg).map_err(|err| {
        render_resume_error(err);
        1
    })
}

fn render_resume_short_line_if_needed(stderr_is_terminal: bool, selected_provider: &str) {
    if should_emit_resume_short_line(stderr_is_terminal) {
        eprintln!("[resume] -> {selected_provider}");
    }
}

fn validate_headless_resume_target(
    target: &ResumeExecutionTarget,
    selected_provider: &str,
) -> Result<(), i32> {
    if target.provider.resume.is_some() {
        Ok(())
    } else {
        eprintln!("provider {selected_provider} has no [providers.resume] block; cannot resume");
        Err(1)
    }
}

fn first_attempt_manual_migrate(attempts: usize, manual_migrate: Option<&str>) -> Option<&str> {
    if attempts == 1 { manual_migrate } else { None }
}

#[allow(clippy::too_many_arguments)]
// ---
// Component: noninteractive-resume-execution
// Declared roles: orchestration, validator, mapper, formatter, filter, predicate, accessor
// ---
fn run_resume(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    model_name: Option<&str>,
    session_id: &str,
    manual_migrate: Option<&str>,
    prompt: Option<&str>,
    file: Option<&Path>,
    working_dir: Option<&Path>,
    models_dir_override: Option<&Path>,
) -> Result<i32, String> {
    if let Err(message) = validate_resume_uuid(session_id) {
        eprintln!("{message}");
        return Ok(1);
    }
    // Source guard marker: agent_runtime_services.resume_service.resolve_resume(ResumeServiceRequest)

    let answer = resolve_resume_answer(prompt, file)?;
    let env = load_resume_execution_environment(models_dir_override)?;

    let mut resolved = match resolve_resume_for_headless_execution(
        agent_runtime_services,
        &env,
        session_id,
        model_name,
    ) {
        Ok(resolved) => resolved,
        Err(exit_code) => return Ok(exit_code),
    };
    if let Err(exit_code) = prepare_initial_headless_resume_target(
        &resolved,
        &env.providers_cfg,
        std::io::stderr().is_terminal(),
    ) {
        return Ok(exit_code);
    }
    let effective_spawn_cwd = effective_resume_spawn_cwd(
        &env.state,
        &env.models,
        &env.providers_cfg,
        &env.sessions_cfg,
        session_id,
        working_dir,
    )?;
    let parent_invocation_id = resolve_parent_invocation_id(&env.state);
    let max_attempts = resolved
        .model
        .as_ref()
        .map(|model| model.providers.len())
        .unwrap_or(1)
        .max(1)
        + 1;
    let mut attempts = 0usize;
    let mut last_exit_code = 1;
    let mut zero_turn_confirmation = ZeroTurnConfirmationState::new();

    loop {
        if attempts >= max_attempts {
            eprintln!("BLOCKED:all-providers-exhausted");
            return Ok(if last_exit_code == 0 {
                1
            } else {
                last_exit_code
            });
        }
        attempts += 1;

        let mut target = match renderable_resume_execution_target(&resolved, &env.providers_cfg) {
            Ok(target) => target,
            Err(exit_code) => return Ok(exit_code),
        };
        let mut migration_model = resume_migration_pool(&resolved, &env.providers_cfg);
        if manual_migrate.is_none() || attempts > 1 {
            filter_quota_exhausted_migration_candidates(
                &env.state,
                &mut migration_model,
                &resolved.active_provider,
            );
        }
        let mut migration_stderr = std::io::stderr();
        match agent_runtime_services
            .migration_service
            .migrate(MigrationServiceRequest {
                state: &env.state,
                sessions_cfg: &env.sessions_cfg,
                resolved: &resolved,
                manual_target: first_attempt_manual_migrate(attempts, manual_migrate),
                active_exhausted: false,
                migration_model: &migration_model,
                effective_cwd: &effective_spawn_cwd,
                stderr: &mut migration_stderr,
            }) {
            Ok(MigrationServiceOutput::Migrated { segment: migrated })
            | Ok(MigrationServiceOutput::AutoRotated {
                segment: migrated, ..
            }) => {
                resolved.active_provider = migrated.target_provider.clone();
                resolved.active_session_id = migrated.target_session_id.clone();
                target = match renderable_resume_execution_target(&resolved, &env.providers_cfg) {
                    Ok(target) => target,
                    Err(exit_code) => return Ok(exit_code),
                };
            }
            Ok(MigrationServiceOutput::Stay) => {}
            Ok(MigrationServiceOutput::RotationFailed { reason }) => {
                eprintln!("{}", format_rotation_failed_reason(&reason));
                return Ok(1);
            }
            Err(ServiceError::Dependency { message }) => {
                eprintln!("migration failed: {message}");
                return Ok(1);
            }
            Err(err) => {
                eprintln!("migration service failed: {err}");
                return Ok(1);
            }
        }

        let provider_index = target.provider_index;
        let provider = target.provider;
        let Some(strategy) = provider.resume.as_ref() else {
            eprintln!(
                "provider {} has no [providers.resume] block; cannot resume",
                provider.name
            );
            return Ok(1);
        };

        let invocation_id = Uuid::new_v4().to_string();
        let invocation = CompositeInvocationId {
            source: provider.name.clone(),
            id: invocation_id,
        };
        let invocation_model_name = resolved
            .model_name
            .clone()
            .unwrap_or_else(|| "<unknown>".to_string());
        let invocation_start = InvocationStart {
            invocation_uuid: invocation.id.clone(),
            model_name: invocation_model_name,
            provider_name: provider.name.clone(),
            provider_index,
            parent_invocation_id,
        };
        let invocation_row_id = agent_runtime_services
            .invocation_lifecycle_service
            .start_invocation(InvocationLifecycleStartRequest {
                state: &env.state,
                start: &invocation_start,
            })
            .map_err(|err| err.to_string())?
            .invocation_row_id;
        let mut guard = FinalizerGuard::new(&env.state, invocation_row_id);
        env.state.bind_invocation_provider_session_start(
            invocation_row_id,
            &oulipoly_state::ProviderSessionBinding {
                provider_session_id: resolved.active_session_id.clone(),
                capture_method: "resumed",
                resume_input_id: Some(session_id.to_string()),
                provider_session_resolved_account: provider_session_resolved_account(
                    &provider,
                    &resolved.active_session_id,
                ),
            },
        )?;
        if manual_migrate.is_some() {
            env.state
                .record_legacy_resume_input_session_id(invocation_row_id, session_id)?;
        }
        let provider_session_id = resolved.active_session_id.clone();
        let zero_turn_baseline = zero_turn_record_baseline(
            &env.state,
            &env.sessions_cfg,
            &provider.name,
            Some(&provider_session_id),
        );

        let invocation_env = serde_json::to_string(&invocation)
            .map_err(|e| format!("Failed to serialize invocation id: {e}"))?;
        eprintln!("{}", invocation.stderr_line());

        let mut result = match executor::cli::execute_resume_optional_prompt(
            &provider,
            provider_index,
            target.prompt_mode,
            answer.as_deref(),
            Some(&effective_spawn_cwd),
            Some(&invocation_env),
            executor::cli::ResumePayload {
                session_id: &resolved.active_session_id,
                strategy,
            },
        ) {
            Ok(result) => result,
            Err(_spawn_err) => {
                agent_runtime_services
                    .invocation_lifecycle_service
                    .finalize_invocation(InvocationLifecycleFinalizeRequest {
                        state: &env.state,
                        invocation_row_id,
                        success: false,
                        exit_code: 1,
                        error_category: Some("spawn_error"),
                        terminal_reason: Some("spawn_error"),
                    })
                    .map_err(|err| err.to_string())?;
                guard.mark_finalized();
                return Ok(1);
            }
        };
        apply_age153_terminal_signal_fixture_override(&mut result);
        let zero_turn_classification =
            zero_turn_classify_after_completion(&env.state, &env.sessions_cfg, &zero_turn_baseline);
        apply_zero_turn_classification_to_result(
            &mut result,
            &provider.name,
            &zero_turn_classification,
        );
        let zero_turn_action = next_action(
            &mut zero_turn_confirmation,
            zero_turn_classification_for_action(
                zero_turn_classification,
                &result,
                &provider.name,
                Some(&provider_session_id),
            ),
        );

        if let Some(acceptance) = &result.resume_acceptance {
            agent_runtime_services
                .resume_service
                .record_acceptance(ResumeAcceptanceRequest {
                    state: &env.state,
                    invocation_row_id,
                    status: acceptance.status.db_value(),
                    evidence: acceptance.evidence.as_deref(),
                })
                .map_err(|err| format!("resume acceptance service failed: {err}"))?;
        }

        emit_captured_child_marker_lines(&result.captured_child_invocations);

        let invocation_uuid =
            Uuid::parse_str(&invocation.id).expect("generated invocation id must be a UUID");
        let session_uuid = provider_session_marker_uuid(Some(&provider_session_id));
        let mut terminal_signal_stderr = std::io::stderr();
        let mut terminal_signal_ctx = TerminalSignalContext {
            invocation_id: &invocation_uuid,
            session_id: session_uuid.as_ref(),
            provider: &provider.name,
            state_db: &env.state,
            stderr: &mut terminal_signal_stderr,
        };
        let resume_terminal_signal = resume_terminal_signal_for_outcome(&result.terminal_signal);
        let terminal_signal_disposition =
            if matches!(zero_turn_action, ZeroTurnAction::ConfirmedExhaustion)
                && resume_terminal_signal
                    .as_ref()
                    .is_some_and(|signal| signal.kind == TerminalSignalKind::MaybeQuotaExhausted)
            {
                let signal = resume_terminal_signal
                    .as_ref()
                    .expect("confirmed zero-turn action requires a maybe signal");
                let _ = confirm_maybe_quota_exhausted(signal, &mut terminal_signal_ctx);
                TerminalSignalDisposition::MaybeQuotaVerify
            } else {
                apply_terminal_signal_outcome(&resume_terminal_signal, &mut terminal_signal_ctx)
            };
        match terminal_signal_disposition {
            TerminalSignalDisposition::MaybeQuotaVerify => match zero_turn_action {
                ZeroTurnAction::ConfirmedExhaustion => {
                    let terminal_reason = terminal_signal_reason(
                        &result.terminal_signal,
                        result.terminal_reason.as_deref(),
                    )
                    .unwrap_or("maybe_quota_exhausted");
                    agent_runtime_services
                        .invocation_lifecycle_service
                        .finalize_invocation(InvocationLifecycleFinalizeRequest {
                            state: &env.state,
                            invocation_row_id,
                            success: false,
                            exit_code: result.exit_code,
                            error_category: Some("quota_exhausted"),
                            terminal_reason: Some(terminal_reason),
                        })
                        .map_err(|err| err.to_string())?;
                    guard.mark_finalized();
                    last_exit_code = result.exit_code;
                    continue;
                }
                ZeroTurnAction::VerifySameProvider => {
                    let terminal_reason = terminal_signal_reason(
                        &result.terminal_signal,
                        result.terminal_reason.as_deref(),
                    )
                    .unwrap_or("maybe_quota_exhausted");
                    agent_runtime_services
                        .invocation_lifecycle_service
                        .finalize_invocation(InvocationLifecycleFinalizeRequest {
                            state: &env.state,
                            invocation_row_id,
                            success: false,
                            exit_code: result.exit_code,
                            error_category: None,
                            terminal_reason: Some(terminal_reason),
                        })
                        .map_err(|err| err.to_string())?;
                    guard.mark_finalized();
                    last_exit_code = result.exit_code;
                    continue;
                }
                ZeroTurnAction::Continue | ZeroTurnAction::Unclassified => {
                    let terminal_reason = terminal_signal_reason(
                        &result.terminal_signal,
                        result.terminal_reason.as_deref(),
                    )
                    .unwrap_or("maybe_quota_exhausted");
                    agent_runtime_services
                        .invocation_lifecycle_service
                        .finalize_invocation(InvocationLifecycleFinalizeRequest {
                            state: &env.state,
                            invocation_row_id,
                            success: false,
                            exit_code: result.exit_code,
                            error_category: None,
                            terminal_reason: Some(terminal_reason),
                        })
                        .map_err(|err| err.to_string())?;
                    guard.mark_finalized();
                    eprintln!("{}", result.stderr);
                    return Ok(result.exit_code);
                }
            },
            TerminalSignalDisposition::QuotaExhaustedRetry => {
                let terminal_reason = terminal_signal_reason(
                    &result.terminal_signal,
                    result.terminal_reason.as_deref(),
                )
                .expect("typed quota signal must have terminal reason");
                agent_runtime_services
                    .invocation_lifecycle_service
                    .finalize_invocation(InvocationLifecycleFinalizeRequest {
                        state: &env.state,
                        invocation_row_id,
                        success: false,
                        exit_code: result.exit_code,
                        error_category: Some("quota_exhausted"),
                        terminal_reason: Some(terminal_reason),
                    })
                    .map_err(|err| err.to_string())?;
                guard.mark_finalized();
                last_exit_code = result.exit_code;
                continue;
            }
            TerminalSignalDisposition::ProlongedSilenceFail
            | TerminalSignalDisposition::InteractiveFail => {
                let terminal_reason = terminal_signal_reason(
                    &result.terminal_signal,
                    result.terminal_reason.as_deref(),
                )
                .expect("typed failure signal must have terminal reason");
                agent_runtime_services
                    .invocation_lifecycle_service
                    .finalize_invocation(InvocationLifecycleFinalizeRequest {
                        state: &env.state,
                        invocation_row_id,
                        success: false,
                        exit_code: result.exit_code,
                        error_category: terminal_signal_error_category(
                            &result.terminal_signal,
                            terminal_reason,
                        ),
                        terminal_reason: Some(terminal_reason),
                    })
                    .map_err(|err| err.to_string())?;
                guard.mark_finalized();
                eprintln!("{}", result.stderr);
                return Ok(result.exit_code);
            }
            TerminalSignalDisposition::InteractiveClean
            | TerminalSignalDisposition::NotApplicable => {}
        }

        let success = execution_succeeded(result.exit_code);
        let error_category =
            resume_result_error_category(agent_runtime_services, &result, &env.models, working_dir);
        let quota_exhausted = error_category_is_quota_exhausted(error_category.as_deref());
        if let Err(err) = env
            .state
            .record_returned_artifacts(invocation_row_id, &result.returned_artifacts)
        {
            eprintln!("Error: Failed to record returned artifacts: {err}");
            agent_runtime_services
                .invocation_lifecycle_service
                .finalize_invocation(InvocationLifecycleFinalizeRequest {
                    state: &env.state,
                    invocation_row_id,
                    success: false,
                    exit_code: 1,
                    error_category: Some("returned_artifacts"),
                    terminal_reason: Some("returned_artifacts_persist_failed"),
                })
                .map(|_| ())
                .unwrap_or_else(|e| eprintln!("Warning: Failed to finalize invocation: {e}"));
            guard.mark_finalized();
            return Ok(1);
        }
        agent_runtime_services
            .invocation_lifecycle_service
            .finalize_invocation(InvocationLifecycleFinalizeRequest {
                state: &env.state,
                invocation_row_id,
                success,
                exit_code: result.exit_code,
                error_category: error_category.as_deref(),
                terminal_reason: result.terminal_reason.as_deref(),
            })
            .map_err(|err| err.to_string())?;
        guard.mark_finalized();
        last_exit_code = result.exit_code;

        if success {
            ingest_and_emit_session_id_resume_aware(
                agent_runtime_services,
                SessionIngestRequest {
                    state: &env.state,
                    sessions_cfg: &env.sessions_cfg,
                    providers_cfg: Some(&env.providers_cfg),
                    provider_name: &provider.name,
                    invocation_row_id,
                    invocation_uuid: &invocation.id,
                    effective_cwd: Some(&effective_spawn_cwd),
                    mode: ResumeIngestMode::Pinned {
                        resume_target: if manual_migrate.is_some() {
                            session_id
                        } else {
                            &resolved.active_session_id
                        },
                    },
                },
            );
            let _ = std::io::stdout().write_all(&result.stdout);
            return Ok(result.exit_code);
        }

        if quota_exhausted {
            if attempts < max_attempts {
                eprintln!(
                    "[routing] provider {} returned quota_exhausted; retrying another provider",
                    provider.name
                );
            }
            continue;
        }

        eprintln!("{}", result.stderr);
        if let Some(ref cat) = error_category {
            eprintln!("[diagnostics: {cat}]");
        }
        return Ok(result.exit_code);
    }
}

fn validate_resume_uuid(session_id: &str) -> Result<(), String> {
    Uuid::parse_str(session_id)
        .map(|_| ())
        .map_err(|_| format!("invalid session UUID: {session_id}"))
}

fn filter_quota_exhausted_migration_candidates(
    state: &StateDb,
    migration_model: &mut ModelConfig,
    active_provider: &str,
) {
    migration_model.providers.retain(|provider| {
        if provider.name == active_provider {
            return true;
        }
        resume_migration_candidate_has_quota(state, &provider.name)
    });
}

fn resume_migration_candidate_has_quota(state: &StateDb, provider_name: &str) -> bool {
    provider_quota_state_for_migration(state, provider_name)
        .map(quota_state_has_capacity)
        .unwrap_or_else(migration_candidate_default_capacity_after_quota_read_error)
}

fn provider_quota_state_for_migration(
    state: &StateDb,
    provider_name: &str,
) -> Option<Option<oulipoly_state::QuotaRecord>> {
    match read_provider_quota_state(state, provider_name) {
        Ok(quota) => Some(quota),
        Err(err) => {
            emit_quota_inspection_warning(provider_name, &err);
            None
        }
    }
}

fn migration_candidate_default_capacity_after_quota_read_error() -> bool {
    true
}

fn read_provider_quota_state(
    state: &StateDb,
    provider_name: &str,
) -> Result<Option<oulipoly_state::QuotaRecord>, String> {
    state.get_quota(provider_name)
}

fn format_rotation_failed_reason(reason: &RotationFailedReason) -> String {
    match reason {
        RotationFailedReason::WorkingSetExhausted { candidates_tried } => format!(
            "migration failed: working set exhausted after trying providers [{}]",
            candidates_tried.join(", ")
        ),
        RotationFailedReason::ManualTargetNotInPool { target, pool } => format!(
            "cannot rotate: provider \"{target}\" is not in model pool [{}]",
            pool.join(", ")
        ),
        RotationFailedReason::ManualTargetNotMigratable { source, target } => {
            format!("cannot rotate: {source} -> {target} is not a migratable storage-class pair")
        }
        RotationFailedReason::ManualTargetIsSingleProviderPool { provider } => {
            format!("cannot rotate: model pool has only one provider ({provider})")
        }
        RotationFailedReason::ManualTargetActiveNotInPool { active } => {
            format!("cannot rotate: session-active provider \"{active}\" is not in the model pool")
        }
    }
}

fn quota_state_has_capacity(quota: Option<oulipoly_state::QuotaRecord>) -> bool {
    // AGE-163 WU-A.4: the typed forensics writer lands durable
    // unavailability on `next_available_at`. A provider has capacity iff
    // neither the legacy `exhausted_at` flag nor an unelapsed
    // `next_available_at` cooldown is set.
    let Some(record) = quota else {
        return true;
    };
    if record.exhausted_at.is_some() {
        return false;
    }
    record
        .next_available_at
        .is_none_or(|ts| ts <= chrono::Utc::now())
}

fn emit_quota_inspection_warning(provider_name: &str, err: &str) {
    eprintln!("Warning: Failed to inspect quota state for {provider_name}: {err}");
}

fn execution_succeeded(exit_code: i32) -> bool {
    exit_code == 0
}

fn zero_turn_zero_counts() -> oulipoly_state::SessionTurnCounts {
    oulipoly_state::SessionTurnCounts {
        total: 0,
        assistant: 0,
        sidechain: 0,
    }
}

fn provider_has_no_session_source(
    sessions_cfg: &oulipoly_config::SessionsConfig,
    provider_name: &str,
) -> bool {
    sessions_cfg.get(provider_name).is_none()
}

fn has_session_source(sessions_cfg: &oulipoly_config::SessionsConfig, provider_name: &str) -> bool {
    !provider_has_no_session_source(sessions_cfg, provider_name)
}

fn scan_report_has_errors(report: &oulipoly_runtime::sessions::ScanReport) -> bool {
    !report.errors.is_empty()
}

fn baseline_turn_count_from_scan(
    state: &StateDb,
    provider_name: &str,
    session_id: &str,
    scan_failed: bool,
) -> Option<oulipoly_state::SessionTurnCounts> {
    if scan_failed {
        None
    } else {
        state.count_session_turns(provider_name, session_id).ok()
    }
}

fn classify_from_turn_count_result<E>(
    baseline: &ZeroTurnBaseline,
    count_result: Result<oulipoly_state::SessionTurnCounts, E>,
) -> ZeroTurnClassification {
    match count_result {
        Ok(counts) => classify_completion_delta(baseline, counts),
        Err(_) => ZeroTurnClassification::UnclassifiedScanFailed,
    }
}

fn zero_turn_record_baseline(
    state: &StateDb,
    sessions_cfg: &oulipoly_config::SessionsConfig,
    provider_name: &str,
    provider_session_id: Option<&str>,
) -> ZeroTurnBaseline {
    let Some(session_id) = provider_session_id else {
        return record_baseline(provider_name, None, None, false);
    };
    if provider_has_no_session_source(sessions_cfg, provider_name) {
        return record_baseline(provider_name, Some(session_id), None, true);
    }
    let report = oulipoly_runtime::sessions::scan_provider(provider_name, sessions_cfg, state);
    let scan_failed = scan_report_has_errors(&report);
    let baseline_count =
        baseline_turn_count_from_scan(state, provider_name, session_id, scan_failed);
    record_baseline(provider_name, Some(session_id), baseline_count, scan_failed)
}

fn zero_turn_classify_after_completion(
    state: &StateDb,
    sessions_cfg: &oulipoly_config::SessionsConfig,
    baseline: &ZeroTurnBaseline,
) -> ZeroTurnClassification {
    let Some(session_id) = baseline.provider_session_id.as_deref() else {
        return classify_completion_delta(baseline, zero_turn_zero_counts());
    };
    if baseline.scan_failed {
        return classify_completion_delta(baseline, zero_turn_zero_counts());
    }
    let report =
        oulipoly_runtime::sessions::scan_provider(&baseline.provider_name, sessions_cfg, state);
    if scan_report_has_errors(&report) {
        return ZeroTurnClassification::UnclassifiedScanFailed;
    }
    classify_from_turn_count_result(
        baseline,
        state.count_session_turns(&baseline.provider_name, session_id),
    )
}

fn zero_turn_classification_is_non_productive(c: &ZeroTurnClassification) -> bool {
    matches!(
        c,
        ZeroTurnClassification::MaybeQuotaExhausted { .. }
            | ZeroTurnClassification::UnclassifiedNoSessionId
            | ZeroTurnClassification::UnclassifiedScanFailed
    )
}

fn zero_turn_classification_for_action(
    classification: ZeroTurnClassification,
    result: &executor::ExecutionResult,
    provider_name: &str,
    provider_session_id: Option<&str>,
) -> ZeroTurnClassification {
    if zero_turn_classification_is_non_productive(&classification) {
        return classification;
    }
    if zero_turn_completion_can_replace_signal(&result.terminal_signal)
        && let Some(session_id) = provider_session_id
    {
        let baseline = record_baseline(
            provider_name,
            Some(session_id),
            Some(zero_turn_zero_counts()),
            false,
        );
        return classify_completion_delta(&baseline, zero_turn_zero_counts());
    }
    classification
}

fn zero_turn_completion_can_replace_signal(signal: &Option<executor::TerminalSignal>) -> bool {
    signal
        .as_ref()
        .is_some_and(|signal| signal.kind == TerminalSignalKind::MaybeQuotaExhausted)
}

fn build_maybe_quota_exhausted_signal(
    provider_name: &str,
    evidence: &ZeroTurnEvidence,
) -> executor::TerminalSignal {
    executor::TerminalSignal {
        kind: TerminalSignalKind::MaybeQuotaExhausted,
        provider_name: provider_name.to_string(),
        evidence: evidence.evidence.clone(),
        observed_at: std::time::SystemTime::now(),
    }
}

fn apply_zero_turn_classification_to_signal_fields(
    terminal_signal: &mut Option<executor::TerminalSignal>,
    terminal_reason: &mut Option<String>,
    provider_name: &str,
    classification: &ZeroTurnClassification,
) {
    match classification {
        ZeroTurnClassification::Productive => {
            if zero_turn_completion_can_replace_signal(terminal_signal) {
                *terminal_signal = None;
                *terminal_reason = None;
            }
        }
        ZeroTurnClassification::MaybeQuotaExhausted { evidence } => {
            if !zero_turn_completion_can_replace_signal(terminal_signal) {
                return;
            }
            *terminal_signal = Some(build_maybe_quota_exhausted_signal(provider_name, evidence));
            *terminal_reason = Some("maybe_quota_exhausted".to_string());
        }
        ZeroTurnClassification::UnclassifiedNoSessionId
        | ZeroTurnClassification::UnclassifiedScanFailed => {}
    }
}

fn apply_zero_turn_classification_to_result(
    result: &mut executor::ExecutionResult,
    provider_name: &str,
    classification: &ZeroTurnClassification,
) {
    apply_zero_turn_classification_to_signal_fields(
        &mut result.terminal_signal,
        &mut result.terminal_reason,
        provider_name,
        classification,
    );
}

fn provider_session_marker_uuid(provider_session_id: Option<&str>) -> Option<Uuid> {
    provider_session_id.and_then(|session_id| Uuid::parse_str(session_id).ok())
}

fn resume_result_error_category(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    result: &executor::ExecutionResult,
    models: &HashMap<String, ModelConfig>,
    working_dir: Option<&Path>,
) -> Option<String> {
    resume_cli::resume_result_error_category(agent_runtime_services, result, models, working_dir)
}

fn balanced_result_error_category(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    result: &executor::ExecutionResult,
    models: &HashMap<String, ModelConfig>,
    working_dir: Option<&Path>,
) -> Option<String> {
    balanced_cli::balanced_result_error_category(
        agent_runtime_services,
        result,
        models,
        working_dir,
    )
}

fn resume_session_mismatch_category() -> String {
    diagnostics::ErrorCategory::ResumeSessionMismatch
        .as_str()
        .to_string()
}

fn diagnose_execution_error(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    result: &executor::ExecutionResult,
    models: &HashMap<String, ModelConfig>,
    working_dir: Option<&Path>,
) -> Option<String> {
    let input = diagnostic_input(&result.stderr, &result.stdout);
    run_diagnostics(
        agent_runtime_services,
        &input,
        result.exit_code,
        models,
        working_dir,
    )
}

fn quota_exhausted_category() -> String {
    diagnostics::ErrorCategory::QuotaExhausted
        .as_str()
        .to_string()
}

fn error_category_is_quota_exhausted(error_category: Option<&str>) -> bool {
    error_category == Some(diagnostics::ErrorCategory::QuotaExhausted.as_str())
}

// ---
// Component: balanced-execution-supervision
// Declared roles: orchestration, predicate, mapper, formatter, parser, accessor, filter
// ---

struct BalancedExecutionEnvironment {
    state: StateDb,
    providers_cfg: ProvidersConfig,
    sessions_cfg: oulipoly_config::SessionsConfig,
}

fn load_balanced_execution_environment(
    state_db_opener: &dyn StateDbOpener,
) -> Result<BalancedExecutionEnvironment, String> {
    let state = state_db_opener.open_default()?;
    let config_root = default_config_root();
    Ok(BalancedExecutionEnvironment {
        state,
        providers_cfg: oulipoly_config::ProvidersConfig::load(&config_root.join("providers.toml"))
            .unwrap_or_default(),
        sessions_cfg: oulipoly_config::SessionsConfig::load(&config_root.join("sessions.toml"))
            .unwrap_or_default(),
    })
}

fn select_balanced_provider_index(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    model: &ModelConfig,
    state: &StateDb,
    ctx: &balancer::BalanceContext<'_>,
) -> Result<usize, String> {
    agent_runtime_services
        .routing_service
        .select_route(RoutingServiceRequest {
            model,
            state,
            ctx: Some(ctx),
        })
        .map(|route| route.provider_index)
        .map_err(|err| err.to_string())
}

fn model_provider_names(model: &ModelConfig) -> Vec<String> {
    model
        .providers
        .iter()
        .map(|provider| provider.name.clone())
        .collect()
}

fn pre_invocation_failure_payload(
    stage: &str,
    model_name: Option<&str>,
    provider_index: Option<usize>,
    attempted_providers: Vec<String>,
    reason: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "failure_kind": "pre_invocation",
        "stage": stage,
        "status": "failed",
        "success": false,
        "exit_code": serde_json::Value::Null,
        "terminal_reason": "pre_invocation_failure",
        "error_category": serde_json::Value::Null,
        "finished_at": current_timestamp_rfc3339(),
        "message": pre_invocation_failure_message(stage, reason),
        "detail": {
            "model_name": model_name,
            "provider_index": provider_index,
            "attempted_providers": attempted_providers,
            "reason": reason,
        },
        "agent_runner_invocation_id": serde_json::Value::Null,
        "provider_name": serde_json::Value::Null,
        "provider_session_id": serde_json::Value::Null,
        "agent_runner_chain_id": serde_json::Value::Null,
    })
}

fn emit_pre_invocation_failure(
    stage: &str,
    model_name: Option<&str>,
    provider_index: Option<usize>,
    attempted_providers: Vec<String>,
    reason: Option<&str>,
) {
    let payload = pre_invocation_failure_payload(
        stage,
        model_name,
        provider_index,
        attempted_providers,
        reason,
    );
    match serde_json::to_string(&payload) {
        Ok(json) => emit_pre_invocation_failure_line(&json),
        Err(err) => eprintln!("Warning: Failed to serialize pre-invocation failure: {err}"),
    }
}

fn pre_invocation_failure_message(stage: &str, reason: Option<&str>) -> String {
    match reason {
        Some(reason) => format!("{stage}: {reason}"),
        None => stage.to_string(),
    }
}

fn emit_pool_exhausted_pre_invocation_failure(model: &ModelConfig, reason: &str) {
    emit_pre_invocation_failure(
        "pool_exhausted",
        Some(&model.name),
        None,
        model_provider_names(model),
        Some(reason),
    );
}

fn emit_provider_selection_pre_invocation_failure(
    model: &ModelConfig,
    reason: &str,
    pool_exhausted: bool,
) {
    if pool_exhausted {
        emit_pool_exhausted_pre_invocation_failure(model, reason);
        return;
    }
    emit_pre_invocation_failure(
        "provider_selection",
        Some(&model.name),
        None,
        Vec::new(),
        Some(reason),
    );
}

fn emit_provider_resolution_pre_invocation_failure(
    model: &ModelConfig,
    provider_index: usize,
    reason: &str,
) {
    emit_pre_invocation_failure(
        "provider_resolution",
        Some(&model.name),
        Some(provider_index),
        vec![model.providers[provider_index].name.clone()],
        Some(reason),
    );
}

fn composite_invocation_id(provider_name: &str) -> CompositeInvocationId {
    CompositeInvocationId {
        source: provider_name.to_string(),
        id: Uuid::new_v4().to_string(),
    }
}

fn balanced_invocation_start(
    invocation: &CompositeInvocationId,
    model: &ModelConfig,
    provider_name: &str,
    provider_index: usize,
    parent_invocation_id: Option<i64>,
) -> InvocationStart {
    InvocationStart {
        invocation_uuid: invocation.id.clone(),
        model_name: model.name.clone(),
        provider_name: provider_name.to_string(),
        provider_index,
        parent_invocation_id,
    }
}

fn result_failure_identity(
    state: &StateDb,
    invocation_id: &str,
    provider_name: &str,
    provider_session_id: Option<&str>,
) -> ResultEnvelopeFailureIdentity {
    let agent_runner_chain_id = provider_session_id.and_then(|session_id| {
        state
            .chain_id_for_segment(provider_name, session_id)
            .ok()
            .flatten()
    });
    ResultEnvelopeFailureIdentity {
        agent_runner_invocation_id: invocation_id.to_string(),
        provider_name: Some(provider_name.to_string()),
        provider_session_id: provider_session_id.map(str::to_string),
        agent_runner_chain_id,
    }
}

struct FailureResultEnvelopeInput<'a> {
    state: &'a StateDb,
    invocation_id: &'a str,
    provider_name: &'a str,
    provider_session_id: Option<&'a str>,
    exit_code: i32,
    error_category: Option<&'a str>,
    terminal_reason: Option<&'a str>,
}

fn emit_failure_result_envelope(input: FailureResultEnvelopeInput<'_>) {
    let failure_identity = result_failure_identity(
        input.state,
        input.invocation_id,
        input.provider_name,
        input.provider_session_id,
    );
    emit_result_envelope(
        input.invocation_id,
        false,
        input.exit_code,
        input.error_category,
        input.terminal_reason,
        Some(&failure_identity),
    );
}

fn emit_unknown_diagnostic_if_settled_unknown(
    state: &StateDb,
    provider_name: &str,
    provider_index: usize,
    result: &executor::ExecutionResult,
    error_category: Option<&str>,
    retry_rotation_disposition: &str,
) {
    if error_category == Some(diagnostics::ErrorCategory::Unknown.as_str()) {
        emit_unknown_diagnostic(
            state,
            provider_name,
            provider_index,
            result,
            retry_rotation_disposition,
        );
    }
}

fn bind_start_known_provider_session_if_present(
    state: &StateDb,
    invocation_row_id: i64,
    provider_session_id: Option<&str>,
) {
    if let Some(provider_session_id) = provider_session_id {
        state
            .bind_invocation_provider_session_start(
                invocation_row_id,
                &oulipoly_state::ProviderSessionBinding {
                    provider_session_id: provider_session_id.to_string(),
                    capture_method: "forced_flag_verified",
                    resume_input_id: None,
                    provider_session_resolved_account: None,
                },
            )
            .unwrap_or_else(|e| eprintln!("Warning: Failed to bind provider session: {e}"));
    }
}

struct BalancedExecutorRequestInput<'a> {
    model: &'a ModelConfig,
    provider: &'a ProviderConfig,
    provider_index: usize,
    prompt_mode: PromptMode,
    prompt: &'a str,
    working_dir: Option<&'a Path>,
    extra_inputs: &'a HashMap<String, Vec<String>>,
    invocation_env: &'a str,
    start_known_provider_session_id: Option<String>,
}

fn balanced_executor_request(input: BalancedExecutorRequestInput<'_>) -> ExecutorServiceRequest {
    if let Some(start_known_provider_session_id) = input.start_known_provider_session_id {
        return ExecutorServiceRequest::EffectiveWithStartKnownProviderSessionId {
            model: input.model.clone(),
            provider: input.provider.clone(),
            provider_index: input.provider_index,
            prompt_mode: input.prompt_mode,
            prompt: input.prompt.to_string(),
            working_dir: input.working_dir.map(Path::to_path_buf),
            extra_inputs: input.extra_inputs.clone(),
            parent_invocation_env: Some(input.invocation_env.to_string()),
            start_known_provider_session_id,
        };
    }
    ExecutorServiceRequest::Effective {
        model: input.model.clone(),
        provider: input.provider.clone(),
        provider_index: input.provider_index,
        prompt_mode: input.prompt_mode,
        prompt: input.prompt.to_string(),
        working_dir: input.working_dir.map(Path::to_path_buf),
        extra_inputs: input.extra_inputs.clone(),
        parent_invocation_env: Some(input.invocation_env.to_string()),
    }
}

fn is_confirmed_zero_turn_exhaustion(
    action: ZeroTurnAction,
    signal: &Option<executor::TerminalSignal>,
) -> bool {
    matches!(action, ZeroTurnAction::ConfirmedExhaustion)
        && zero_turn_completion_can_replace_signal(signal)
}

fn format_quota_retry_budget_exhausted(model_name: &str, max_attempts: usize) -> String {
    format!(
        "quota-exhausted retry budget exhausted for pool {model_name} after {max_attempts} attempts"
    )
}

fn zero_turn_late_bind_baseline(
    sessions_cfg: &oulipoly_config::SessionsConfig,
    provider_name: &str,
    session_id: &str,
) -> ZeroTurnBaseline {
    let has_source = has_session_source(sessions_cfg, provider_name);
    record_baseline(
        provider_name,
        Some(session_id),
        has_source.then(zero_turn_zero_counts),
        !has_source,
    )
}

fn should_defer_generic_exit(
    all_models: &HashMap<String, ModelConfig>,
    result: &executor::ExecutionResult,
) -> bool {
    diagnostics_model_configured(all_models) || !result.returned_artifacts.is_empty()
}

fn run_with_balancing(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    state_db_opener: &dyn StateDbOpener,
    model: &ModelConfig,
    prompt: &str,
    all_models: &HashMap<String, ModelConfig>,
    working_dir: Option<&Path>,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> Result<i32, String> {
    // Source guard markers for AGE-33:
    // state_db_opener.open_default()?
    // ProvidersConfig::load(&providers_path).unwrap_or_default()
    // SessionsConfig::load(&sessions_path).unwrap_or_default()
    // ExecutorServiceRequest::Effective
    let env = load_balanced_execution_environment(state_db_opener)?;
    let in_flight = oulipoly_runtime::quota::InFlight::new();
    let ctx = balancer::BalanceContext {
        providers_cfg: &env.providers_cfg,
        sessions_cfg: &env.sessions_cfg,
        in_flight: &in_flight,
    };
    // Resolve parent invocation BEFORE provider selection so the provider
    // selection itself can be attributed to a parent context if needed
    // (matches contract `tmp/01-pr-a-contract.md` lifecycle ordering).
    let parent_invocation_id = resolve_parent_invocation_id(&env.state);
    // Source guard marker: resolve_parent_invocation_id(&state)
    let max_attempts = model.providers.len().max(1) + 1;
    let mut attempts = 0usize;
    let mut zero_turn_confirmation = ZeroTurnConfirmationState::new();
    let mut pending_same_provider_verification: Option<(usize, Option<String>)> = None;

    loop {
        if attempts >= max_attempts {
            eprintln!("BLOCKED:all-providers-exhausted");
            let reason =
                match agent_runtime_services
                    .routing_service
                    .select_route(RoutingServiceRequest {
                        model,
                        state: &env.state,
                        ctx: Some(&ctx),
                    }) {
                    Err(err) => err.to_string(),
                    Ok(_) => format_quota_retry_budget_exhausted(&model.name, max_attempts),
                };
            emit_pool_exhausted_pre_invocation_failure(model, &reason);
            return Err(reason);
        }
        attempts += 1;

        let pending_verification = pending_same_provider_verification.take();
        let provider_index = match pending_verification.as_ref() {
            Some((provider_index, _)) => *provider_index,
            None => {
                match select_balanced_provider_index(
                    agent_runtime_services,
                    model,
                    &env.state,
                    &ctx,
                ) {
                    Ok(provider_index) => provider_index,
                    Err(err) => {
                        let pool_exhausted = attempts > model.providers.len().max(1);
                        emit_provider_selection_pre_invocation_failure(model, &err, pool_exhausted);
                        return Err(err);
                    }
                }
            }
        };
        let (provider, prompt_mode) =
            match effective_model_for_execution(model, provider_index, &env.providers_cfg) {
                Ok(effective) => effective,
                Err(err) => {
                    emit_provider_resolution_pre_invocation_failure(model, provider_index, &err);
                    return Err(err);
                }
            };
        let provider_name = &provider.name;
        let invocation = composite_invocation_id(provider_name);
        let invocation_start = balanced_invocation_start(
            &invocation,
            model,
            provider_name,
            provider_index,
            parent_invocation_id,
        );
        let invocation_row_id = agent_runtime_services
            .invocation_lifecycle_service
            .start_invocation(InvocationLifecycleStartRequest {
                state: &env.state,
                start: &invocation_start,
            })
            .map_err(|err| err.to_string())?
            .invocation_row_id;
        let mut guard = FinalizerGuard::new(&env.state, invocation_row_id);
        let start_known_provider_session_id = match pending_verification {
            Some((_, session_id)) => session_id,
            None => executor::cli::start_known_provider_session_id(&provider)?,
        };
        bind_start_known_provider_session_if_present(
            &env.state,
            invocation_row_id,
            start_known_provider_session_id.as_deref(),
        );
        let mut zero_turn_baseline = zero_turn_record_baseline(
            &env.state,
            &env.sessions_cfg,
            provider_name,
            start_known_provider_session_id.as_deref(),
        );
        let invocation_env = serde_json::to_string(&invocation)
            .map_err(|e| format!("Failed to serialize invocation id: {e}"))?;
        eprintln!("{}", invocation.stderr_line());

        let executor_request = balanced_executor_request(BalancedExecutorRequestInput {
            model,
            provider: &provider,
            provider_index,
            prompt_mode,
            prompt,
            working_dir,
            extra_inputs,
            invocation_env: &invocation_env,
            start_known_provider_session_id: start_known_provider_session_id.clone(),
        });

        let mut result = match agent_runtime_services
            .executor_service
            .execute(executor_request)
        {
            Ok(output) => output.result,
            Err(err) => {
                let signal = spawn_error_terminal_signal(provider_name, err.to_string());
                let invocation_uuid = Uuid::parse_str(&invocation.id)
                    .expect("generated invocation id must be a UUID");
                let mut terminal_signal_stderr = std::io::stderr();
                let mut terminal_signal_ctx = TerminalSignalContext {
                    invocation_id: &invocation_uuid,
                    session_id: None,
                    provider: provider_name,
                    state_db: &env.state,
                    stderr: &mut terminal_signal_stderr,
                };
                let _ =
                    apply_terminal_signal_outcome(&Some(signal.clone()), &mut terminal_signal_ctx);
                let terminal_reason =
                    typed_terminal_reason_fallback(&signal).unwrap_or("spawn_error");
                agent_runtime_services
                    .invocation_lifecycle_service
                    .finalize_invocation(InvocationLifecycleFinalizeRequest {
                        state: &env.state,
                        invocation_row_id,
                        success: false,
                        exit_code: -1,
                        error_category: Some(terminal_reason),
                        terminal_reason: Some(terminal_reason),
                    })
                    .map(|_| ())
                    .unwrap_or_else(|finalize_err| {
                        eprintln!("Warning: Failed to finalize invocation: {finalize_err}")
                    });
                emit_failure_result_envelope(FailureResultEnvelopeInput {
                    state: &env.state,
                    invocation_id: &invocation.id,
                    provider_name,
                    provider_session_id: start_known_provider_session_id.as_deref(),
                    exit_code: -1,
                    error_category: Some(terminal_reason),
                    terminal_reason: Some(terminal_reason),
                });
                guard.mark_finalized();
                return Err(err.to_string());
            }
        };
        apply_age153_terminal_signal_fixture_override(&mut result);
        let zero_turn_provider_session_id = start_known_provider_session_id
            .clone()
            .or_else(|| result.session_capture.session_id.clone());
        if zero_turn_baseline.provider_session_id.is_none()
            && let Some(session_id) = zero_turn_provider_session_id.as_deref()
        {
            zero_turn_baseline =
                zero_turn_late_bind_baseline(&env.sessions_cfg, provider_name, session_id);
        }
        let zero_turn_classification =
            zero_turn_classify_after_completion(&env.state, &env.sessions_cfg, &zero_turn_baseline);
        apply_zero_turn_classification_to_result(
            &mut result,
            provider_name,
            &zero_turn_classification,
        );
        let zero_turn_action = next_action(
            &mut zero_turn_confirmation,
            zero_turn_classification_for_action(
                zero_turn_classification,
                &result,
                provider_name,
                zero_turn_provider_session_id.as_deref(),
            ),
        );

        emit_captured_child_marker_lines(&result.captured_child_invocations);

        let invocation_uuid =
            Uuid::parse_str(&invocation.id).expect("generated invocation id must be a UUID");
        let mut terminal_signal_stderr = std::io::stderr();
        let terminal_session_uuid =
            provider_session_marker_uuid(zero_turn_provider_session_id.as_deref());
        let mut terminal_signal_ctx = TerminalSignalContext {
            invocation_id: &invocation_uuid,
            session_id: terminal_session_uuid.as_ref(),
            provider: provider_name,
            state_db: &env.state,
            stderr: &mut terminal_signal_stderr,
        };
        let should_defer_generic_exit = should_defer_generic_exit(all_models, &result);
        let balanced_terminal_signal =
            balanced_terminal_signal_for_outcome(&result, should_defer_generic_exit);
        let terminal_signal_disposition =
            if is_confirmed_zero_turn_exhaustion(zero_turn_action, &balanced_terminal_signal) {
                let signal = balanced_terminal_signal
                    .as_ref()
                    .expect("confirmed zero-turn action requires a maybe signal");
                let _ = confirm_maybe_quota_exhausted(signal, &mut terminal_signal_ctx);
                TerminalSignalDisposition::MaybeQuotaVerify
            } else {
                apply_terminal_signal_outcome(&balanced_terminal_signal, &mut terminal_signal_ctx)
            };
        match terminal_signal_disposition {
            TerminalSignalDisposition::MaybeQuotaVerify => {
                let terminal_reason = terminal_signal_reason(
                    &result.terminal_signal,
                    result.terminal_reason.as_deref(),
                )
                .unwrap_or("maybe_quota_exhausted");
                supervise_captured_child_invocations(
                    &env.state,
                    invocation_row_id,
                    &result.captured_child_invocations,
                    Some(terminal_reason),
                );
                if let executor::SessionCaptureMethod::Failed(reason) =
                    &result.session_capture.method
                {
                    eprintln!("[session-capture] {reason}");
                }
                env.state
                    .update_session_capture(
                        invocation_row_id,
                        result.session_capture.session_id.as_deref(),
                        result.session_capture.method.db_value(),
                    )
                    .unwrap_or_else(|e| {
                        eprintln!("Warning: Failed to update session capture: {e}")
                    });
                let confirmed = matches!(zero_turn_action, ZeroTurnAction::ConfirmedExhaustion);
                agent_runtime_services
                    .invocation_lifecycle_service
                    .finalize_invocation(InvocationLifecycleFinalizeRequest {
                        state: &env.state,
                        invocation_row_id,
                        success: false,
                        exit_code: result.exit_code,
                        error_category: confirmed.then_some("quota_exhausted"),
                        terminal_reason: Some(terminal_reason),
                    })
                    .map(|_| ())
                    .unwrap_or_else(|e| eprintln!("Warning: Failed to finalize invocation: {e}"));
                guard.mark_finalized();
                env.state
                    .increment_calls_since_refresh(provider_name)
                    .unwrap_or_else(|e| eprintln!("Warning: Failed to bump quota tick: {e}"));
                match zero_turn_action {
                    ZeroTurnAction::VerifySameProvider => {
                        pending_same_provider_verification =
                            Some((provider_index, zero_turn_provider_session_id.clone()));
                        continue;
                    }
                    ZeroTurnAction::ConfirmedExhaustion => {
                        if confirmed && attempts < max_attempts {
                            eprintln!(
                                "[routing] provider {provider_name} returned quota_exhausted; retrying another provider"
                            );
                        }
                        continue;
                    }
                    ZeroTurnAction::Continue | ZeroTurnAction::Unclassified => {
                        emit_failure_result_envelope(FailureResultEnvelopeInput {
                            state: &env.state,
                            invocation_id: &invocation.id,
                            provider_name,
                            provider_session_id: zero_turn_provider_session_id.as_deref(),
                            exit_code: result.exit_code,
                            error_category: None,
                            terminal_reason: Some(terminal_reason),
                        });
                        eprintln!("{}", result.stderr);
                        return Ok(result.exit_code);
                    }
                }
            }
            TerminalSignalDisposition::QuotaExhaustedRetry => {
                let terminal_reason = terminal_signal_reason(
                    &result.terminal_signal,
                    result.terminal_reason.as_deref(),
                )
                .expect("typed quota signal must have terminal reason");
                supervise_captured_child_invocations(
                    &env.state,
                    invocation_row_id,
                    &result.captured_child_invocations,
                    Some(terminal_reason),
                );
                if let executor::SessionCaptureMethod::Failed(reason) =
                    &result.session_capture.method
                {
                    eprintln!("[session-capture] {reason}");
                }
                env.state
                    .update_session_capture(
                        invocation_row_id,
                        result.session_capture.session_id.as_deref(),
                        result.session_capture.method.db_value(),
                    )
                    .unwrap_or_else(|e| {
                        eprintln!("Warning: Failed to update session capture: {e}")
                    });
                agent_runtime_services
                    .invocation_lifecycle_service
                    .finalize_invocation(InvocationLifecycleFinalizeRequest {
                        state: &env.state,
                        invocation_row_id,
                        success: false,
                        exit_code: result.exit_code,
                        error_category: Some("quota_exhausted"),
                        terminal_reason: Some(terminal_reason),
                    })
                    .map(|_| ())
                    .unwrap_or_else(|e| eprintln!("Warning: Failed to finalize invocation: {e}"));
                guard.mark_finalized();
                env.state
                    .increment_calls_since_refresh(provider_name)
                    .unwrap_or_else(|e| eprintln!("Warning: Failed to bump quota tick: {e}"));
                if attempts < max_attempts {
                    eprintln!(
                        "[routing] provider {provider_name} returned quota_exhausted; retrying another provider"
                    );
                }
                continue;
            }
            TerminalSignalDisposition::ProlongedSilenceFail
            | TerminalSignalDisposition::InteractiveFail => {
                let terminal_reason = terminal_signal_reason(
                    &result.terminal_signal,
                    result.terminal_reason.as_deref(),
                )
                .expect("typed failure signal must have terminal reason");
                supervise_captured_child_invocations(
                    &env.state,
                    invocation_row_id,
                    &result.captured_child_invocations,
                    Some(terminal_reason),
                );
                if let executor::SessionCaptureMethod::Failed(reason) =
                    &result.session_capture.method
                {
                    eprintln!("[session-capture] {reason}");
                }
                env.state
                    .update_session_capture(
                        invocation_row_id,
                        result.session_capture.session_id.as_deref(),
                        result.session_capture.method.db_value(),
                    )
                    .unwrap_or_else(|e| {
                        eprintln!("Warning: Failed to update session capture: {e}")
                    });
                agent_runtime_services
                    .invocation_lifecycle_service
                    .finalize_invocation(InvocationLifecycleFinalizeRequest {
                        state: &env.state,
                        invocation_row_id,
                        success: false,
                        exit_code: result.exit_code,
                        error_category: terminal_signal_error_category(
                            &result.terminal_signal,
                            terminal_reason,
                        ),
                        terminal_reason: Some(terminal_reason),
                    })
                    .map(|_| ())
                    .unwrap_or_else(|e| eprintln!("Warning: Failed to finalize invocation: {e}"));
                guard.mark_finalized();
                emit_failure_result_envelope(FailureResultEnvelopeInput {
                    state: &env.state,
                    invocation_id: &invocation.id,
                    provider_name,
                    provider_session_id: zero_turn_provider_session_id.as_deref(),
                    exit_code: result.exit_code,
                    error_category: Some(terminal_reason),
                    terminal_reason: Some(terminal_reason),
                });
                eprintln!("{}", result.stderr);
                return Ok(result.exit_code);
            }
            TerminalSignalDisposition::InteractiveClean
            | TerminalSignalDisposition::NotApplicable => {
                supervise_captured_child_invocations(
                    &env.state,
                    invocation_row_id,
                    &result.captured_child_invocations,
                    result.terminal_reason.as_deref(),
                );
            }
        }

        if let executor::SessionCaptureMethod::Failed(reason) = &result.session_capture.method {
            eprintln!("[session-capture] {reason}");
        }

        env.state
            .update_session_capture(
                invocation_row_id,
                result.session_capture.session_id.as_deref(),
                result.session_capture.method.db_value(),
            )
            .unwrap_or_else(|e| eprintln!("Warning: Failed to update session capture: {e}"));

        let success = execution_succeeded(result.exit_code);

        let error_category = balanced_result_error_category(
            agent_runtime_services,
            &result,
            all_models,
            working_dir,
        );
        let quota_exhausted = error_category_is_quota_exhausted(error_category.as_deref());

        if let Err(err) = env
            .state
            .record_returned_artifacts(invocation_row_id, &result.returned_artifacts)
        {
            eprintln!("Error: Failed to record returned artifacts: {err}");
            agent_runtime_services
                .invocation_lifecycle_service
                .finalize_invocation(InvocationLifecycleFinalizeRequest {
                    state: &env.state,
                    invocation_row_id,
                    success: false,
                    exit_code: 1,
                    error_category: Some("returned_artifacts"),
                    terminal_reason: Some("returned_artifacts_persist_failed"),
                })
                .map(|_| ())
                .unwrap_or_else(|e| eprintln!("Warning: Failed to finalize invocation: {e}"));
            emit_failure_result_envelope(FailureResultEnvelopeInput {
                state: &env.state,
                invocation_id: &invocation.id,
                provider_name,
                provider_session_id: zero_turn_provider_session_id.as_deref(),
                exit_code: 1,
                error_category: Some("returned_artifacts"),
                terminal_reason: Some("returned_artifacts_persist_failed"),
            });
            guard.mark_finalized();
            return Ok(1);
        }

        emit_unknown_diagnostic_if_settled_unknown(
            &env.state,
            provider_name,
            provider_index,
            &result,
            error_category.as_deref(),
            "no_retry",
        );

        agent_runtime_services
            .invocation_lifecycle_service
            .finalize_invocation(InvocationLifecycleFinalizeRequest {
                state: &env.state,
                invocation_row_id,
                success,
                exit_code: result.exit_code,
                error_category: error_category.as_deref(),
                terminal_reason: result.terminal_reason.as_deref(),
            })
            .map(|_| ())
            .unwrap_or_else(|e| eprintln!("Warning: Failed to finalize invocation: {e}"));
        guard.mark_finalized();

        if success {
            let ingest_effective_cwd = effective_spawn_cwd(working_dir)?;
            let emitted = ingest_and_emit_session_id_resume_aware(
                agent_runtime_services,
                SessionIngestRequest {
                    state: &env.state,
                    sessions_cfg: &env.sessions_cfg,
                    providers_cfg: Some(&env.providers_cfg),
                    provider_name,
                    invocation_row_id,
                    invocation_uuid: &invocation.id,
                    effective_cwd: Some(&ingest_effective_cwd),
                    mode: ResumeIngestMode::Unpinned {
                        capture_method: "turn_script",
                    },
                },
            );
            if !emitted && let Some(session_id) = result.session_capture.session_id.as_deref() {
                emit_known_session_id(
                    &env.state,
                    invocation_row_id,
                    &invocation.id,
                    session_id,
                    result.session_capture.method.db_value(),
                );
            }
        }

        // Bump calls_since_refresh for this provider (account). Errors here are
        // non-fatal — missing a tick just slightly skews the next projection.
        env.state
            .increment_calls_since_refresh(provider_name)
            .unwrap_or_else(|e| eprintln!("Warning: Failed to bump quota tick: {e}"));

        if success {
            let _ = std::io::stdout().write_all(&result.stdout);
            emit_result_envelope(
                &invocation.id,
                success,
                result.exit_code,
                error_category.as_deref(),
                result.terminal_reason.as_deref(),
                None,
            );
            return Ok(result.exit_code);
        }
        if quota_exhausted {
            if attempts < max_attempts {
                eprintln!(
                    "[routing] provider {provider_name} returned quota_exhausted; retrying another provider"
                );
            }
            continue;
        }

        emit_failure_result_envelope(FailureResultEnvelopeInput {
            state: &env.state,
            invocation_id: &invocation.id,
            provider_name,
            provider_session_id: zero_turn_provider_session_id.as_deref(),
            exit_code: result.exit_code,
            error_category: error_category.as_deref(),
            terminal_reason: result.terminal_reason.as_deref(),
        });
        eprintln!("{}", result.stderr);
        if let Some(ref cat) = error_category {
            eprintln!("[diagnostics: {cat}]");
        }
        return Ok(result.exit_code);
    }
}

fn supervise_captured_child_invocations(
    state: &StateDb,
    parent_invocation_id: i64,
    captured: &[executor::CapturedChildInvocation],
    parent_terminal_reason: Option<&str>,
) {
    let supervisor_reason = format_supervisor_reason(parent_terminal_reason);

    for child in captured {
        let Some(row) = inspected_captured_child_row(state, child) else {
            continue;
        };
        if !captured_child_matches_parent(&row, parent_invocation_id, child) {
            continue;
        }
        finalize_captured_child_invocation(state, &row, child, &supervisor_reason);
    }
}

fn emit_captured_child_marker_lines(captured: &[executor::CapturedChildInvocation]) {
    for child in captured {
        eprintln!("{}", child.raw_marker_line);
    }
}

fn inspected_captured_child_row(
    state: &StateDb,
    child: &executor::CapturedChildInvocation,
) -> Option<InvocationRecord> {
    match lookup_captured_child_row(state, child) {
        Ok(row) => row,
        Err(err) => {
            emit_captured_child_inspection_warning(child, &err);
            None
        }
    }
}

fn lookup_captured_child_row(
    state: &StateDb,
    child: &executor::CapturedChildInvocation,
) -> Result<Option<InvocationRecord>, String> {
    state.get_invocation_by_uuid(&child.composite_id.id)
}

fn emit_captured_child_inspection_warning(child: &executor::CapturedChildInvocation, err: &str) {
    eprintln!(
        "Warning: Failed to inspect captured child invocation {}: {err}",
        child.composite_id.id
    );
}

fn finalize_captured_child_invocation(
    state: &StateDb,
    row: &InvocationRecord,
    child: &executor::CapturedChildInvocation,
    supervisor_reason: &str,
) {
    if let Err(err) = state.finalize_invocation(row.id, false, -1, None, Some(supervisor_reason)) {
        emit_captured_child_finalize_warning(child, &err);
    }
}

fn emit_captured_child_finalize_warning(child: &executor::CapturedChildInvocation, err: &str) {
    eprintln!(
        "Warning: Failed to finalize captured child invocation {}: {err}",
        child.composite_id.id
    );
}

fn format_supervisor_reason(parent_terminal_reason: Option<&str>) -> String {
    let observed_reason = parent_terminal_reason.unwrap_or("unknown_exit");
    format!("supervisor_observed_{observed_reason}")
}

fn captured_child_matches_parent(
    row: &InvocationRecord,
    parent_invocation_id: i64,
    child: &executor::CapturedChildInvocation,
) -> bool {
    row.status == InvocationStatus::Running
        && row.parent_invocation_id == Some(parent_invocation_id)
        && row.provider_name.as_deref() == Some(child.composite_id.source.as_str())
}

fn resolve_parent_invocation_id(state: &StateDb) -> Option<i64> {
    let composite = parse_parent_invocation_env()?;
    let record = lookup_parent_invocation_record(state, &composite)?;
    if parent_invocation_source_matches(&record, &composite) {
        Some(record.id)
    } else {
        None
    }
}

fn parse_parent_invocation_env() -> Option<CompositeInvocationId> {
    let raw = read_parent_invocation_env()?;
    CompositeInvocationId::parse_env_value(&raw).ok()
}

fn read_parent_invocation_env() -> Option<String> {
    std::env::var("OULIPOLY_PARENT_INVOCATION").ok()
}

fn lookup_parent_invocation_record(
    state: &StateDb,
    composite: &CompositeInvocationId,
) -> Option<InvocationRecord> {
    state.get_invocation_by_uuid(&composite.id).ok().flatten()
}

fn parent_invocation_source_matches(
    record: &InvocationRecord,
    composite: &CompositeInvocationId,
) -> bool {
    record.provider_name.as_deref() == Some(composite.source.as_str())
}

// ---
// Component: diagnostics-execution
// Declared roles: orchestration, mapper, formatter, accessor
// ---

fn run_diagnostics(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    provider_output: &str,
    exit_code: i32,
    models: &HashMap<String, ModelConfig>,
    working_dir: Option<&Path>,
) -> Option<String> {
    let context = diagnostics_context(models)?;
    render_diagnostics_result(run_diagnostics_service(
        agent_runtime_services,
        context,
        provider_output,
        exit_code,
        working_dir,
    ))
}

struct DiagnosticsContext {
    diag_model: ModelConfig,
    provider: ProviderConfig,
    prompt_mode: PromptMode,
}

struct DiagnosticsDependencies {
    diag_model: ModelConfig,
    providers_cfg: ProvidersConfig,
}

fn diagnostics_context(models: &HashMap<String, ModelConfig>) -> Option<DiagnosticsContext> {
    diagnostics_context_from_dependencies(load_diagnostics_dependencies(models)?)
}

fn load_diagnostics_dependencies(
    models: &HashMap<String, ModelConfig>,
) -> Option<DiagnosticsDependencies> {
    let app_config = load_app_config();
    let diag_model_name = app_config.diagnostics_model?;
    let diag_model = models.get(&diag_model_name)?.clone();
    let providers_path = default_config_root().join("providers.toml");
    let providers_cfg = ProvidersConfig::load(&providers_path).unwrap_or_default();
    Some(DiagnosticsDependencies {
        diag_model,
        providers_cfg,
    })
}

fn diagnostics_context_from_dependencies(
    dependencies: DiagnosticsDependencies,
) -> Option<DiagnosticsContext> {
    let (provider, prompt_mode) = effective_provider_for_model_provider(
        &dependencies.diag_model,
        0,
        &dependencies.providers_cfg,
    )
    .ok()?;
    Some(DiagnosticsContext {
        diag_model: dependencies.diag_model,
        provider,
        prompt_mode,
    })
}

fn run_diagnostics_service(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    context: DiagnosticsContext,
    provider_output: &str,
    exit_code: i32,
    working_dir: Option<&Path>,
) -> Result<oulipoly_runtime::diagnostics::Diagnosis, String> {
    agent_runtime_services
        .diagnostics_service
        .diagnose(DiagnosticsServiceRequest::DiagnoseError {
            diagnostics_model: context.diag_model,
            effective_provider: context.provider,
            provider_index: 0,
            prompt_mode: context.prompt_mode,
            exit_code,
            stderr: provider_output.to_string(),
            working_dir: working_dir.map(Path::to_path_buf),
        })
        .map_err(|err| err.to_string())
        .and_then(diagnostics_output_diagnosis)
}

fn diagnostics_output_diagnosis(
    output: DiagnosticsServiceOutput,
) -> Result<oulipoly_runtime::diagnostics::Diagnosis, String> {
    match output {
        DiagnosticsServiceOutput::Diagnosis { diagnosis } => Ok(diagnosis),
        DiagnosticsServiceOutput::ExhaustionClassification { .. } => {
            Err("diagnostics service returned exhaustion classification".to_string())
        }
    }
}

fn render_diagnostics_result(
    diagnosis: Result<oulipoly_runtime::diagnostics::Diagnosis, String>,
) -> Option<String> {
    match diagnosis {
        Ok(diagnosis) => {
            emit_diagnostics_success(&diagnosis);
            Some(diagnostics_category_name(&diagnosis))
        }
        Err(e) => {
            emit_diagnostics_failure(&e);
            None
        }
    }
}

fn emit_diagnostics_success(diagnosis: &oulipoly_runtime::diagnostics::Diagnosis) {
    eprintln!(
        "[diagnostics] {}: {}",
        diagnosis.category.as_str(),
        diagnosis.summary
    );
}

fn diagnostics_category_name(diagnosis: &oulipoly_runtime::diagnostics::Diagnosis) -> String {
    diagnosis.category.as_str().to_string()
}

fn emit_diagnostics_failure(error: &str) {
    eprintln!("[diagnostics] Failed to diagnose: {error}");
}

// ---
// Component: db-migration-backfill
// Declared roles: orchestration, parser, formatter, predicate, validator, accessor, mapper, filter
// ---

fn run_migrate_db() -> Result<i32, String> {
    let state = StateDb::open_default()?;
    let report = state.backfill_session_chains()?;
    render_session_chain_backfill_report(&report);
    let compaction_report = run_compaction_backfill(&state)?;
    render_compaction_backfill_report(&compaction_report);
    Ok(0)
}

fn run_migrate(rebuild: bool) -> Result<i32, String> {
    validate_migrate_rebuild_flag(rebuild)?;
    run_migrate_rebuild()
}

fn run_migrate_rebuild() -> Result<i32, String> {
    let Some(plan) = migrate_rebuild_plan()? else {
        return Ok(0);
    };
    execute_migrate_rebuild(&plan)?;
    let fresh = StateDb::open(&plan.db_path)?;
    drop(fresh);
    render_migrate_rebuild_report(&plan);
    Ok(0)
}

fn unique_backup_dir(root: &Path) -> Result<PathBuf, String> {
    let base = backup_dir_base_name();
    first_available_backup_dir(backup_dir_candidates(root, &base))
        .ok_or_else(|| format_backup_dir_exhausted_error(root))
}

fn backup_dir_candidates(root: &Path, base: &str) -> Vec<PathBuf> {
    (0..1000)
        .map(|suffix| root.join(backup_dir_candidate_name(base, suffix)))
        .collect()
}

fn first_available_backup_dir(candidates: Vec<PathBuf>) -> Option<PathBuf> {
    candidates
        .into_iter()
        .find(|candidate| unused_path(candidate))
}

fn unused_path(path: &Path) -> bool {
    !path.exists()
}

fn render_session_chain_backfill_report(report: &oulipoly_state::BackfillReport) {
    println!(
        "session chain backfill: chains={} segments={} skipped_existing={}",
        report.chains_inserted, report.segments_inserted, report.skipped_existing
    );
}

fn render_compaction_backfill_report(report: &CompactionBackfillReport) {
    println!(
        "compaction backfill: {} turns flagged across {} sessions",
        report.turns_flagged, report.sessions_processed
    );
}

fn validate_migrate_rebuild_flag(rebuild: bool) -> Result<(), String> {
    if rebuild {
        Ok(())
    } else {
        Err("missing required flag: --rebuild".to_string())
    }
}

struct MigrateRebuildPlan {
    db_path: PathBuf,
    backup_dir: PathBuf,
    sidecars: Vec<PathBuf>,
}

fn migrate_rebuild_plan() -> Result<Option<MigrateRebuildPlan>, String> {
    let db_path = default_state_db_path()?;
    if missing_state_db(&db_path) {
        render_missing_state_db_rebuild_message(&db_path);
        return Ok(None);
    }
    let backup_root = prepare_migrate_backup_root(&db_path)?;
    Ok(Some(migrate_rebuild_plan_from_paths(
        db_path,
        &backup_root,
    )?))
}

fn default_state_db_path() -> Result<PathBuf, String> {
    StateDb::default_path()
}

fn missing_state_db(db_path: &Path) -> bool {
    !db_path.exists()
}

fn render_missing_state_db_rebuild_message(db_path: &Path) {
    println!("no state.db to rebuild at {}", db_path.display());
}

fn prepare_migrate_backup_root(db_path: &Path) -> Result<PathBuf, String> {
    let data_dir = state_db_parent_dir(db_path)?;
    let backup_root = data_dir.join("state-backups");
    create_backup_root_dir(&backup_root)?;
    Ok(backup_root)
}

fn state_db_parent_dir(db_path: &Path) -> Result<&Path, String> {
    db_path
        .parent()
        .ok_or_else(|| format!("state DB path has no parent: {}", db_path.display()))
}

fn create_backup_root_dir(backup_root: &Path) -> Result<(), String> {
    fs::create_dir_all(backup_root).map_err(format_backup_root_create_error)
}

fn format_backup_root_create_error(error: std::io::Error) -> String {
    format!("failed to create backup directory: {error}")
}

fn migrate_rebuild_plan_from_paths(
    db_path: PathBuf,
    backup_root: &Path,
) -> Result<MigrateRebuildPlan, String> {
    Ok(MigrateRebuildPlan {
        backup_dir: unique_backup_dir(backup_root)?,
        sidecars: db_sidecar_paths(&db_path),
        db_path,
    })
}

fn db_sidecar_paths(db_path: &Path) -> Vec<PathBuf> {
    vec![
        db_path.to_path_buf(),
        PathBuf::from(format!("{}-wal", db_path.display())),
        PathBuf::from(format!("{}-shm", db_path.display())),
    ]
}

fn execute_migrate_rebuild(plan: &MigrateRebuildPlan) -> Result<(), String> {
    create_backup_dir(&plan.backup_dir)?;
    backup_rebuild_sidecars(&plan.sidecars, &plan.backup_dir)?;
    remove_live_sidecars(&plan.sidecars)
}

fn create_backup_dir(backup_dir: &Path) -> Result<(), String> {
    fs::create_dir(backup_dir).map_err(|e| {
        format!(
            "failed to create backup directory {}: {e}",
            backup_dir.display()
        )
    })
}

fn backup_rebuild_sidecars(sidecars: &[PathBuf], backup_dir: &Path) -> Result<(), String> {
    for source in sidecars {
        if source.exists() {
            backup_rebuild_sidecar(source, backup_dir)?;
        }
    }
    Ok(())
}

fn backup_rebuild_sidecar(source: &Path, backup_dir: &Path) -> Result<(), String> {
    let file_name = backup_source_file_name(source)?;
    copy_rebuild_sidecar(source, &backup_sidecar_destination(backup_dir, file_name))?;
    Ok(())
}

fn backup_source_file_name(source: &Path) -> Result<&std::ffi::OsStr, String> {
    source
        .file_name()
        .ok_or_else(|| format_backup_source_missing_file_name_error(source))
}

fn format_backup_source_missing_file_name_error(source: &Path) -> String {
    format!("backup source has no file name: {}", source.display())
}

fn backup_sidecar_destination(backup_dir: &Path, file_name: &std::ffi::OsStr) -> PathBuf {
    backup_dir.join(file_name)
}

fn copy_rebuild_sidecar(source: &Path, destination: &Path) -> Result<(), String> {
    fs::copy(source, destination)
        .map(|_| ())
        .map_err(|e| format_rebuild_sidecar_copy_error(source, destination, e))
}

fn format_rebuild_sidecar_copy_error(
    source: &Path,
    destination: &Path,
    error: std::io::Error,
) -> String {
    let backup_dir = destination.parent().unwrap_or(destination);
    format!(
        "failed to back up {} to {}: {error}",
        source.display(),
        backup_dir.display()
    )
}

fn remove_live_sidecars(sidecars: &[PathBuf]) -> Result<(), String> {
    for source in sidecars {
        if source.exists() {
            fs::remove_file(source)
                .map_err(|e| format!("failed to remove live {}: {e}", source.display()))?;
        }
    }
    Ok(())
}

fn render_migrate_rebuild_report(plan: &MigrateRebuildPlan) {
    println!("backup: {}", plan.backup_dir.display());
    println!("fresh state DB: {}", plan.db_path.display());
    println!(
        "historical state was not preserved in the live DB; backup is at {}",
        plan.backup_dir.display()
    );
}

fn backup_dir_base_name() -> String {
    format_backup_dir_base_name(&backup_dir_timestamp(), backup_dir_process_id())
}

fn backup_dir_timestamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%S%.fZ").to_string()
}

fn backup_dir_process_id() -> u32 {
    std::process::id()
}

fn format_backup_dir_base_name(stamp: &str, pid: u32) -> String {
    format!("{stamp}-pid{pid}")
}

fn backup_dir_candidate_name(base: &str, suffix: usize) -> String {
    if suffix == 0 {
        base.to_string()
    } else {
        format!("{base}-{suffix}")
    }
}

fn format_backup_dir_exhausted_error(root: &Path) -> String {
    format!(
        "failed to allocate unique backup directory under {}",
        root.display()
    )
}

#[derive(Debug, Clone, PartialEq)]
struct ConfigMigrationReport {
    providers_touched: usize,
    model_files_rewritten: usize,
    moved_blocks: Vec<String>,
}

// ---
// Component: config-migration
// Declared roles: orchestration, parser, mapper, validator, formatter, filter, accessor, predicate
// ---

fn run_migrate_config(models_dir_override: Option<&Path>) -> Result<i32, String> {
    let (models_dir, providers_path) = migrate_config_paths(models_dir_override);
    let report = migrate_config_files(&models_dir, &providers_path)?;
    render_config_migration_report(&report);
    Ok(0)
}

fn migrate_config_paths(models_dir_override: Option<&Path>) -> (PathBuf, PathBuf) {
    let models_dir = models_dir_override
        .map(Path::to_path_buf)
        .unwrap_or_else(default_models_dir);
    let config_root = models_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let providers_path = config_root.join("providers.toml");
    (models_dir, providers_path)
}

fn render_config_migration_report(report: &ConfigMigrationReport) {
    println!(
        "migrate-config: providers_touched={} model_files_rewritten={}",
        report.providers_touched, report.model_files_rewritten
    );
    for moved in &report.moved_blocks {
        println!("  moved {moved}");
    }
}

fn migrate_config_files(
    models_dir: &Path,
    providers_path: &Path,
) -> Result<ConfigMigrationReport, String> {
    let mut providers_root = read_optional_toml_table(providers_path)?;
    let mut moved_blocks = Vec::new();
    let mut rewritten = 0usize;

    for path in model_toml_paths(models_dir)? {
        rewritten += migrate_model_config_file(&path, &mut providers_root, &mut moved_blocks)?;
    }

    if let Some(config_root) = providers_path.parent() {
        backfill_session_storage_from_sessions(
            &mut providers_root,
            &config_root.join("sessions.toml"),
            &mut moved_blocks,
        )?;
    }

    let providers_touched = count_runtime_provider_tables(&providers_root);
    write_changed_providers_toml(providers_path, &providers_root)?;
    oulipoly_config::migrate_legacy_session_storage_file(providers_path)?;

    Ok(ConfigMigrationReport {
        providers_touched,
        model_files_rewritten: rewritten,
        moved_blocks,
    })
}

fn read_optional_toml_table(path: &Path) -> Result<toml::Table, String> {
    if path.exists() {
        read_toml_table(path)
    } else {
        Ok(toml::Table::new())
    }
}

fn read_toml_table(path: &Path) -> Result<toml::Table, String> {
    parse_toml_table(path, &read_toml_text(path)?)
}

fn read_toml_text(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format_toml_read_error(path, e))
}

fn format_toml_read_error(path: &Path, error: std::io::Error) -> String {
    format!("Failed to read {}: {error}", path.display())
}

fn parse_toml_table(path: &Path, text: &str) -> Result<toml::Table, String> {
    text.parse::<toml::Table>()
        .map_err(|e| format_toml_parse_error(path, e))
}

fn format_toml_parse_error(path: &Path, error: toml::de::Error) -> String {
    format!("TOML parse error in {}: {error}", path.display())
}

fn model_toml_paths(models_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = if path_exists(models_dir) {
        toml_paths_from_dir_entries(read_model_dir_entries(models_dir)?)
    } else {
        Vec::new()
    };
    sort_paths(&mut paths);
    Ok(paths)
}

fn path_exists(path: &Path) -> bool {
    path.exists()
}

fn read_model_dir_entries(models_dir: &Path) -> Result<fs::ReadDir, String> {
    std::fs::read_dir(models_dir).map_err(|e| format_model_dir_read_error(models_dir, e))
}

fn format_model_dir_read_error(models_dir: &Path, error: std::io::Error) -> String {
    format!("Failed to read {}: {error}", models_dir.display())
}

fn toml_paths_from_dir_entries(entries: fs::ReadDir) -> Vec<PathBuf> {
    entries
        .filter_map(read_dir_entry_path)
        .filter(|path| is_toml_path(path))
        .collect()
}

fn read_dir_entry_path(entry: Result<fs::DirEntry, std::io::Error>) -> Option<PathBuf> {
    entry.ok().map(|entry| entry.path())
}

fn sort_paths(paths: &mut [PathBuf]) {
    paths.sort();
}

fn is_toml_path(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "toml")
}

fn migrate_model_config_file(
    path: &Path,
    providers_root: &mut toml::Table,
    moved_blocks: &mut Vec<String>,
) -> Result<usize, String> {
    let mut table = read_toml_table(path)?;
    let before = serialize_toml_table(path, &table)?;
    let changed = migrate_model_config_table(path, &mut table, providers_root, moved_blocks)?;
    let after = serialize_toml_table(path, &table)?;
    if changed && after != before {
        write_text_file(path, after)?;
        Ok(1)
    } else {
        Ok(0)
    }
}

fn serialize_toml_table(path: &Path, table: &toml::Table) -> Result<String, String> {
    toml::to_string_pretty(table)
        .map_err(|e| format!("Failed to serialize {}: {e}", path.display()))
}

fn migrate_model_config_table(
    path: &Path,
    table: &mut toml::Table,
    providers_root: &mut toml::Table,
    moved_blocks: &mut Vec<String>,
) -> Result<bool, String> {
    let mut changed = false;
    let global_prompt_mode = take_global_prompt_mode(table);
    changed |= removed_global_prompt_mode(&global_prompt_mode);

    if has_old_top_level_command(table) {
        let provider_table = old_top_level_provider_table(table)?;
        let migrated = migrate_provider_table(
            provider_table,
            global_prompt_mode,
            providers_root,
            path,
            moved_blocks,
        )?;
        table.insert("providers".to_string(), toml::Value::Array(vec![migrated]));
        return Ok(true);
    }

    changed |= migrate_provider_array(
        path,
        table,
        global_prompt_mode,
        providers_root,
        moved_blocks,
    )?;
    Ok(changed)
}

fn take_global_prompt_mode(table: &mut toml::Table) -> Option<toml::Value> {
    table.remove("prompt_mode")
}

fn removed_global_prompt_mode(global_prompt_mode: &Option<toml::Value>) -> bool {
    global_prompt_mode.is_some()
}

fn has_old_top_level_command(table: &toml::Table) -> bool {
    table.contains_key("command")
}

fn migrate_provider_array(
    path: &Path,
    table: &mut toml::Table,
    global_prompt_mode: Option<toml::Value>,
    providers_root: &mut toml::Table,
    moved_blocks: &mut Vec<String>,
) -> Result<bool, String> {
    let Some(toml::Value::Array(providers)) = table.get_mut("providers") else {
        return Ok(false);
    };
    let mut changed = false;
    for provider in providers.iter_mut() {
        changed |= migrate_provider_array_entry(
            provider,
            global_prompt_mode.clone(),
            providers_root,
            path,
            moved_blocks,
        )?;
    }
    Ok(changed)
}

fn migrate_provider_array_entry(
    provider: &mut toml::Value,
    global_prompt_mode: Option<toml::Value>,
    providers_root: &mut toml::Table,
    path: &Path,
    moved_blocks: &mut Vec<String>,
) -> Result<bool, String> {
    let migrated = migrate_provider_table(
        provider.clone(),
        global_prompt_mode,
        providers_root,
        path,
        moved_blocks,
    )?;
    if migrated != *provider {
        *provider = migrated;
        Ok(true)
    } else {
        Ok(false)
    }
}

fn count_runtime_provider_tables(providers_root: &toml::Table) -> usize {
    providers_root
        .iter()
        .filter(|(_, value)| {
            value
                .as_table()
                .is_some_and(|table| table.contains_key("command"))
        })
        .count()
}

fn write_changed_providers_toml(
    providers_path: &Path,
    providers_root: &toml::Table,
) -> Result<(), String> {
    let providers_text = serialize_toml_table(providers_path, providers_root)?;
    let current = read_optional_text_file(providers_path)?;
    if providers_text != current {
        ensure_parent_dir(providers_path)?;
        write_text_file(providers_path, providers_text)?;
    }
    Ok(())
}

fn read_optional_text_file(path: &Path) -> Result<String, String> {
    if path.exists() {
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read {}: {e}", path.display()))
    } else {
        Ok(String::new())
    }
}

fn ensure_parent_dir(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    }
    Ok(())
}

fn write_text_file(path: &Path, text: String) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("Failed to write {}: {e}", path.display()))
}

fn old_top_level_provider_table(table: &mut toml::Table) -> Result<toml::Value, String> {
    let provider = take_old_top_level_provider_fields(table);
    validate_old_top_level_provider(&provider)?;
    Ok(toml::Value::Table(provider))
}

fn take_old_top_level_provider_fields(table: &mut toml::Table) -> toml::Table {
    let mut provider = toml::Table::new();
    for key in [
        "command",
        "args",
        "interactive_args",
        "resume",
        "session_capture",
        "session_storage",
        "resume_acceptance",
    ] {
        if let Some(value) = table.remove(key) {
            provider.insert(key.to_string(), value);
        }
    }
    provider
}

fn validate_old_top_level_provider(provider: &toml::Table) -> Result<(), String> {
    if !provider.contains_key("command") {
        return Err("old model provider is missing command".to_string());
    }
    Ok(())
}

fn migrate_provider_table(
    provider_value: toml::Value,
    global_prompt_mode: Option<toml::Value>,
    providers_root: &mut toml::Table,
    path: &Path,
    moved_blocks: &mut Vec<String>,
) -> Result<toml::Value, String> {
    let mut draft = provider_migration_draft(provider_value, path)?;
    if should_keep_model_only_provider(
        draft.has_runtime_blocks,
        draft.provider_name.as_deref(),
        providers_root,
    ) {
        return Ok(toml::Value::Table(draft.original_provider));
    }
    let provider_name = validate_migrated_provider_name(draft.provider_name, path)?;
    let runtime_parts = provider_runtime_parts(
        draft.command.as_ref(),
        draft.model_args,
        draft.model_interactive_args,
    );
    let prompt_mode = take_provider_prompt_mode(&mut draft.provider, global_prompt_mode);
    let blocks = take_provider_runtime_blocks(&mut draft.provider);
    let runtime = runtime_provider_table(providers_root, &provider_name)?;
    apply_runtime_provider_migration(RuntimeProviderMigration {
        runtime,
        provider_name: &provider_name,
        path,
        has_runtime_blocks: draft.has_runtime_blocks,
        prompt_mode,
        runtime_parts: &runtime_parts,
        blocks,
        moved_blocks,
    })?;

    Ok(reduced_provider_value(
        provider_name,
        runtime_parts.model_args,
        runtime_parts.model_interactive_args,
    ))
}

struct ProviderMigrationDraft {
    provider: toml::Table,
    original_provider: toml::Table,
    has_runtime_blocks: bool,
    command: Option<String>,
    model_args: Vec<String>,
    model_interactive_args: Option<Vec<String>>,
    provider_name: Option<String>,
}

fn provider_migration_draft(
    provider_value: toml::Value,
    path: &Path,
) -> Result<ProviderMigrationDraft, String> {
    let mut provider = provider_table_from_value(provider_value, path)?;
    let original_provider = provider.clone();
    let has_runtime_blocks = provider_has_runtime_blocks(&provider);
    let command = take_provider_command(&mut provider, path)?;
    let model_args = take_string_array(&mut provider, "args")?;
    let model_interactive_args = take_optional_string_array(&mut provider, "interactive_args")?;
    let provider_name =
        derive_migrated_provider_name(&mut provider, command.as_deref(), &model_args);
    Ok(ProviderMigrationDraft {
        provider,
        original_provider,
        has_runtime_blocks,
        command,
        model_args,
        model_interactive_args,
        provider_name,
    })
}

struct ProviderRuntimeParts {
    runtime_command: Option<String>,
    command_runtime_args: Vec<String>,
    runtime_args: Vec<String>,
    model_args: Vec<String>,
    runtime_interactive_args: Option<Vec<String>>,
    model_interactive_args: Option<Vec<String>>,
}

fn provider_runtime_parts(
    command: Option<&String>,
    model_args: Vec<String>,
    model_interactive_args: Option<Vec<String>>,
) -> ProviderRuntimeParts {
    let command_parts = split_optional_command(command.map(String::as_str));
    let runtime_command = runtime_command_from_parts(command, &command_parts);
    let command_runtime_args = command_runtime_args_from_parts(&command_parts);
    let (runtime_args, model_args) =
        combine_command_runtime_args(command_runtime_args.clone(), model_args);
    let (runtime_interactive_args, model_interactive_args) =
        partition_optional_model_specific_args(model_interactive_args);
    ProviderRuntimeParts {
        runtime_command,
        command_runtime_args,
        runtime_args,
        model_args,
        runtime_interactive_args,
        model_interactive_args,
    }
}

fn combine_command_runtime_args(
    command_runtime_args: Vec<String>,
    model_args: Vec<String>,
) -> (Vec<String>, Vec<String>) {
    let (runtime_args, model_args) = partition_model_specific_args(model_args);
    if command_runtime_args.is_empty() {
        return (runtime_args, model_args);
    }
    let mut combined = command_runtime_args;
    combined.extend(runtime_args);
    (combined, model_args)
}

fn partition_optional_model_specific_args(
    args: Option<Vec<String>>,
) -> (Option<Vec<String>>, Option<Vec<String>>) {
    args.map(partition_model_specific_args)
        .map(|(runtime, model)| (Some(runtime), Some(model)))
        .unwrap_or((None, None))
}

struct ProviderRuntimeBlocks {
    resume: Option<toml::Value>,
    session_capture: Option<toml::Value>,
    session_storage: Option<toml::Value>,
    resume_acceptance: Option<toml::Value>,
}

fn take_provider_prompt_mode(
    provider: &mut toml::Table,
    global_prompt_mode: Option<toml::Value>,
) -> toml::Value {
    provider
        .remove("prompt_mode")
        .or(global_prompt_mode)
        .unwrap_or_else(|| toml::Value::String("stdin".to_string()))
}

fn take_provider_runtime_blocks(provider: &mut toml::Table) -> ProviderRuntimeBlocks {
    ProviderRuntimeBlocks {
        resume: provider.remove("resume"),
        session_capture: provider.remove("session_capture"),
        session_storage: provider.remove("session_storage"),
        resume_acceptance: provider.remove("resume_acceptance"),
    }
}

struct RuntimeProviderMigration<'a> {
    runtime: &'a mut toml::Table,
    provider_name: &'a str,
    path: &'a Path,
    has_runtime_blocks: bool,
    prompt_mode: toml::Value,
    runtime_parts: &'a ProviderRuntimeParts,
    blocks: ProviderRuntimeBlocks,
    moved_blocks: &'a mut Vec<String>,
}

fn apply_runtime_provider_migration(migration: RuntimeProviderMigration<'_>) -> Result<(), String> {
    apply_runtime_command(
        migration.runtime,
        migration.runtime_parts,
        migration.provider_name,
        migration.path,
    )?;
    apply_runtime_args(
        migration.runtime,
        migration.has_runtime_blocks,
        migration.runtime_parts,
        migration.provider_name,
        migration.path,
    )?;
    apply_runtime_interactive_args(
        migration.runtime,
        migration.has_runtime_blocks,
        migration.runtime_parts,
        migration.provider_name,
        migration.path,
    )?;
    if migration.has_runtime_blocks {
        set_or_conflict(
            migration.runtime,
            "prompt_mode",
            migration.prompt_mode,
            migration.provider_name,
            migration.path,
        )?;
    }
    move_provider_runtime_blocks(
        migration.runtime,
        migration.blocks,
        migration.provider_name,
        migration.path,
        migration.moved_blocks,
    )
}

fn apply_runtime_command(
    runtime: &mut toml::Table,
    runtime_parts: &ProviderRuntimeParts,
    provider_name: &str,
    path: &Path,
) -> Result<(), String> {
    if let Some(runtime_command) = &runtime_parts.runtime_command {
        set_or_repair_empty_array(
            runtime,
            "command",
            toml::Value::String(runtime_command.clone()),
            provider_name,
            path,
        )?;
    }
    Ok(())
}

fn apply_runtime_args(
    runtime: &mut toml::Table,
    has_runtime_blocks: bool,
    runtime_parts: &ProviderRuntimeParts,
    provider_name: &str,
    path: &Path,
) -> Result<(), String> {
    if has_runtime_blocks || !runtime_parts.runtime_args.is_empty() {
        set_or_repair_empty_array(
            runtime,
            "args",
            string_array_value(runtime_parts.runtime_args.clone()),
            provider_name,
            path,
        )?;
    }
    Ok(())
}

fn apply_runtime_interactive_args(
    runtime: &mut toml::Table,
    has_runtime_blocks: bool,
    runtime_parts: &ProviderRuntimeParts,
    provider_name: &str,
    path: &Path,
) -> Result<(), String> {
    if has_runtime_blocks || runtime_parts_has_interactive_args(runtime_parts) {
        set_or_repair_empty_array(
            runtime,
            "interactive_args",
            string_array_value(combined_runtime_interactive_args(runtime_parts)),
            provider_name,
            path,
        )?;
    }
    Ok(())
}

fn runtime_parts_has_interactive_args(runtime_parts: &ProviderRuntimeParts) -> bool {
    runtime_parts
        .runtime_interactive_args
        .as_ref()
        .is_some_and(|args| !args.is_empty())
}

fn combined_runtime_interactive_args(runtime_parts: &ProviderRuntimeParts) -> Vec<String> {
    let mut combined = runtime_parts.command_runtime_args.clone();
    if let Some(runtime_interactive_args) = &runtime_parts.runtime_interactive_args {
        combined.extend(runtime_interactive_args.clone());
    }
    combined
}

fn move_provider_runtime_blocks(
    runtime: &mut toml::Table,
    blocks: ProviderRuntimeBlocks,
    provider_name: &str,
    path: &Path,
    moved_blocks: &mut Vec<String>,
) -> Result<(), String> {
    for (key, value) in provider_runtime_block_entries(blocks) {
        set_or_conflict(runtime, key, value, provider_name, path)?;
        moved_blocks.push(format_moved_runtime_block(path, key, provider_name));
    }
    Ok(())
}

fn provider_runtime_block_entries(
    blocks: ProviderRuntimeBlocks,
) -> Vec<(&'static str, toml::Value)> {
    [
        ("resume", blocks.resume),
        ("session_capture", blocks.session_capture),
        ("session_storage", blocks.session_storage),
        ("resume_acceptance", blocks.resume_acceptance),
    ]
    .into_iter()
    .filter_map(|(key, value)| value.map(|value| (key, value)))
    .collect()
}

fn format_moved_runtime_block(path: &Path, key: &str, provider_name: &str) -> String {
    format!(
        "{}.{} -> providers.toml[{provider_name}]",
        path.display(),
        key
    )
}

fn provider_table_from_value(
    provider_value: toml::Value,
    path: &Path,
) -> Result<toml::Table, String> {
    provider_value
        .as_table()
        .cloned()
        .ok_or_else(|| format!("provider entry in {} is not a table", path.display()))
}

fn provider_has_runtime_blocks(provider: &toml::Table) -> bool {
    provider.contains_key("command")
        || provider.contains_key("resume")
        || provider.contains_key("session_capture")
        || provider.contains_key("session_storage")
        || provider.contains_key("resume_acceptance")
        || provider.contains_key("prompt_mode")
}

fn take_provider_command(
    provider: &mut toml::Table,
    path: &Path,
) -> Result<Option<String>, String> {
    provider
        .remove("command")
        .map(|value| {
            value.as_str().map(ToString::to_string).ok_or_else(|| {
                format!(
                    "command in old per-provider config in {} must be a string",
                    path.display()
                )
            })
        })
        .transpose()
}

fn derive_migrated_provider_name(
    provider: &mut toml::Table,
    command: Option<&str>,
    model_args: &[String],
) -> Option<String> {
    provider
        .remove("name")
        .and_then(|value| value.as_str().map(ToString::to_string))
        .or_else(|| command.map(|command| derive_migration_provider_name(command, model_args)))
}

fn should_keep_model_only_provider(
    has_runtime_blocks: bool,
    provider_name: Option<&str>,
    providers_root: &toml::Table,
) -> bool {
    !has_runtime_blocks
        && provider_name
            .and_then(|name| providers_root.get(name))
            .is_none()
}

fn validate_migrated_provider_name(
    provider_name: Option<String>,
    path: &Path,
) -> Result<String, String> {
    provider_name.ok_or_else(|| {
        format!(
            "Old per-provider config in {} is missing command; run `agents migrate-config` after adding it.",
            path.display()
        )
    })
}

fn split_optional_command(command: Option<&str>) -> Vec<String> {
    command.map(executor::cli::shell_split).unwrap_or_default()
}

fn runtime_command_from_parts(
    command: Option<&String>,
    command_parts: &[String],
) -> Option<String> {
    command.map(|command| {
        command_parts
            .first()
            .cloned()
            .unwrap_or_else(|| command.clone())
    })
}

fn command_runtime_args_from_parts(command_parts: &[String]) -> Vec<String> {
    command_parts.iter().skip(1).cloned().collect()
}

fn runtime_provider_table<'a>(
    providers_root: &'a mut toml::Table,
    provider_name: &str,
) -> Result<&'a mut toml::Table, String> {
    let runtime = providers_root
        .entry(provider_name.to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    runtime
        .as_table_mut()
        .ok_or_else(|| format!("providers.toml entry [{provider_name}] is not a table"))
}

fn reduced_provider_value(
    provider_name: String,
    model_args: Vec<String>,
    model_interactive_args: Option<Vec<String>>,
) -> toml::Value {
    let mut reduced = toml::Table::new();
    reduced.insert("name".to_string(), toml::Value::String(provider_name));
    reduced.insert("args".to_string(), string_array_value(model_args));
    if let Some(interactive_args) = model_interactive_args {
        reduced.insert(
            "interactive_args".to_string(),
            string_array_value(interactive_args),
        );
    }
    toml::Value::Table(reduced)
}

fn backfill_session_storage_from_sessions(
    providers_root: &mut toml::Table,
    sessions_path: &Path,
    moved_blocks: &mut Vec<String>,
) -> Result<(), String> {
    if !sessions_path.exists() {
        return Ok(());
    }
    let sessions = read_toml_table(sessions_path)?;

    for (provider_name, entry) in sessions {
        let Some(storage) = session_storage_from_entry(&entry) else {
            continue;
        };
        let Some(provider) = providers_root
            .get_mut(&provider_name)
            .and_then(toml::Value::as_table_mut)
        else {
            continue;
        };
        if provider.contains_key("session_storage") {
            continue;
        }
        provider.insert("session_storage".to_string(), toml::Value::Table(storage));
        moved_blocks.push(format!(
            "{}[{provider_name}].turn_script -> providers.toml[{provider_name}].session_storage",
            sessions_path.display()
        ));
    }
    Ok(())
}

fn session_storage_from_entry(entry: &toml::Value) -> Option<toml::Table> {
    entry
        .as_table()
        .and_then(|table| table.get("turn_script"))
        .and_then(toml::Value::as_str)
        .and_then(storage_from_turn_script)
}

fn storage_from_turn_script(turn_script: &str) -> Option<toml::Table> {
    let (adapter, storage_root) = turn_script_storage_parts(turn_script)?;
    let adapter_name = Path::new(&adapter).file_name()?.to_str()?;
    let adapter = turn_script_storage_adapter(adapter_name)?;
    Some(storage_table_from_turn_script(&storage_root, adapter))
}

fn turn_script_storage_parts(turn_script: &str) -> Option<(String, String)> {
    let parts = executor::cli::shell_split(turn_script);
    Some((parts.first()?.clone(), parts.get(1)?.clone()))
}

struct TurnScriptStorageAdapter {
    cwd_adapter: &'static str,
    transcript_adapter: &'static str,
    storage_type: &'static str,
}

fn turn_script_storage_adapter(adapter_name: &str) -> Option<TurnScriptStorageAdapter> {
    match adapter_name {
        "claude-code-turns" => Some(TurnScriptStorageAdapter {
            cwd_adapter: "claude-code-cwd",
            transcript_adapter: "claude-code-locate-transcript",
            storage_type: "claude_code",
        }),
        "codex-turns" => Some(TurnScriptStorageAdapter {
            cwd_adapter: "codex-cwd",
            transcript_adapter: "codex-locate-transcript",
            storage_type: "codex_session",
        }),
        _ => None,
    }
}

fn storage_table_from_turn_script(
    storage_root: &str,
    adapter: TurnScriptStorageAdapter,
) -> toml::Table {
    let storage_root = shell_word_arg(storage_root);
    let mut storage = toml::Table::new();
    storage.insert(
        "kind".to_string(),
        toml::Value::String("script".to_string()),
    );
    storage.insert(
        "cwd_script".to_string(),
        toml::Value::String(format!("{} {storage_root}", adapter.cwd_adapter)),
    );
    storage.insert(
        "transcript_script".to_string(),
        toml::Value::String(format!("{} {storage_root}", adapter.transcript_adapter)),
    );
    storage.insert(
        "storage_type".to_string(),
        toml::Value::String(adapter.storage_type.to_string()),
    );
    storage
}

fn shell_word_arg(input: &str) -> String {
    if input
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | '~'))
    {
        return input.to_string();
    }
    format!("'{}'", input.replace('\'', r#"'\''"#))
}

fn set_or_conflict(
    table: &mut toml::Table,
    key: &str,
    value: toml::Value,
    provider_name: &str,
    path: &Path,
) -> Result<(), String> {
    if let Some(existing) = table.get(key) {
        validate_no_toml_conflict(existing, &value, key, provider_name, path)?;
        return Ok(());
    }
    table.insert(key.to_string(), value);
    Ok(())
}

fn validate_no_toml_conflict(
    existing: &toml::Value,
    value: &toml::Value,
    key: &str,
    provider_name: &str,
    path: &Path,
) -> Result<(), String> {
    if existing != value {
        Err(format_toml_conflict_error(
            existing,
            value,
            key,
            provider_name,
            path,
        ))
    } else {
        Ok(())
    }
}

fn format_toml_conflict_error(
    existing: &toml::Value,
    value: &toml::Value,
    key: &str,
    provider_name: &str,
    path: &Path,
) -> String {
    format!(
        "conflicting {key} for provider {provider_name} while migrating {}: existing providers.toml value {existing:?}, model TOML value {value:?}",
        path.display()
    )
}

fn set_or_repair_empty_array(
    table: &mut toml::Table,
    key: &str,
    value: toml::Value,
    provider_name: &str,
    path: &Path,
) -> Result<(), String> {
    if should_repair_empty_array(table.get(key), &value) {
        table.insert(key.to_string(), value);
        return Ok(());
    }
    set_or_conflict(table, key, value, provider_name, path)
}

fn should_repair_empty_array(existing: Option<&toml::Value>, value: &toml::Value) -> bool {
    matches!(existing, Some(toml::Value::Array(existing)) if existing.is_empty())
        && !matches!(value, toml::Value::Array(value) if value.is_empty())
}

fn take_string_array(table: &mut toml::Table, key: &str) -> Result<Vec<String>, String> {
    take_optional_string_array(table, key).map(|value| value.unwrap_or_default())
}

fn take_optional_string_array(
    table: &mut toml::Table,
    key: &str,
) -> Result<Option<Vec<String>>, String> {
    let Some(value) = table.remove(key) else {
        return Ok(None);
    };
    value
        .as_array()
        .ok_or_else(|| format!("{key} must be an array of strings"))?
        .iter()
        .map(|item| {
            item.as_str()
                .map(ToString::to_string)
                .ok_or_else(|| format!("{key} must be an array of strings"))
        })
        .collect::<Result<Vec<_>, String>>()
        .map(Some)
}

fn string_array_value(values: Vec<String>) -> toml::Value {
    toml::Value::Array(values.into_iter().map(toml::Value::String).collect())
}

fn derive_migration_provider_name(command: &str, args: &[String]) -> String {
    let command_parts = split_migration_command(command);
    let Some(command) = command_parts.first() else {
        return command.to_string();
    };
    oulipoly_config::derive_provider_name(command, &migration_provider_args(&command_parts, args))
}

fn split_migration_command(command: &str) -> Vec<String> {
    executor::cli::shell_split(command)
}

fn migration_provider_args(command_parts: &[String], args: &[String]) -> Vec<String> {
    let mut derived_args = command_parts.iter().skip(1).cloned().collect::<Vec<_>>();
    derived_args.extend(args.iter().cloned());
    derived_args
}

fn partition_model_specific_args(args: Vec<String>) -> (Vec<String>, Vec<String>) {
    let mut runtime = Vec::new();
    let mut model_specific = Vec::new();
    let mut iter = args.into_iter().peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--model" | "-m" => {
                model_specific.push(arg);
                if let Some(value) = iter.next() {
                    model_specific.push(value);
                }
            }
            "-c" => {
                if let Some(value) = iter.next() {
                    if value
                        .split_once('=')
                        .is_some_and(|(key, _)| key.starts_with("model_"))
                    {
                        model_specific.push(arg);
                        model_specific.push(value);
                    } else {
                        runtime.push(arg);
                        runtime.push(value);
                    }
                } else {
                    runtime.push(arg);
                }
            }
            _ => runtime.push(arg),
        }
    }
    (runtime, model_specific)
}

// ---
// Component: db-migration-backfill
// Declared roles: orchestration, parser, formatter, predicate, validator, accessor, mapper, filter
// ---

fn run_resume_list(uuid: &str) -> Result<i32, String> {
    validate_resume_list_uuid(uuid)?;
    render_resume_list(uuid, &load_resume_previews(uuid)?);
    Ok(0)
}

fn validate_resume_list_uuid(uuid: &str) -> Result<(), String> {
    Uuid::parse_str(uuid)
        .map(|_| ())
        .map_err(|e| format!("invalid session UUID: {uuid}: {e}"))
}

fn load_resume_previews(uuid: &str) -> Result<Vec<oulipoly_state::ChainPreview>, String> {
    let state = StateDb::open_default()?;
    state
        .resume_previews(uuid)
        .map_err(|e| format!("Failed to list resume chains: {e}"))
}

fn render_resume_list(uuid: &str, previews: &[oulipoly_state::ChainPreview]) {
    if resume_preview_list_is_empty(previews) {
        render_empty_resume_list(uuid);
        return;
    }
    render_resume_preview_lines(previews);
}

fn resume_preview_list_is_empty(previews: &[oulipoly_state::ChainPreview]) -> bool {
    previews.is_empty()
}

fn render_empty_resume_list(uuid: &str) {
    println!("No chains found for {uuid}");
}

fn render_resume_preview_lines(previews: &[oulipoly_state::ChainPreview]) {
    for preview in previews {
        println!("{}", format_resume_list_line(preview));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompactionBackfillReport {
    turns_flagged: u64,
    sessions_processed: u64,
}

fn run_compaction_backfill(state: &StateDb) -> Result<CompactionBackfillReport, String> {
    let mut report = empty_compaction_backfill_report();
    for (provider_name, session_id) in distinct_compaction_chain_segments(state)? {
        let flagged = backfill_compaction_session(state, &provider_name, &session_id)?;
        accumulate_compaction_backfill(&mut report, flagged);
        render_compaction_backfill_session(&provider_name, &session_id, flagged);
    }
    Ok(report)
}

fn empty_compaction_backfill_report() -> CompactionBackfillReport {
    CompactionBackfillReport {
        turns_flagged: 0,
        sessions_processed: 0,
    }
}

fn distinct_compaction_chain_segments(state: &StateDb) -> Result<Vec<(String, String)>, String> {
    state.distinct_chain_segments()
}

fn backfill_compaction_session(
    state: &StateDb,
    provider_name: &str,
    session_id: &str,
) -> Result<u64, String> {
    // AGE-160 Phase 8: rustfmt requires wrapping the pre-AGE-160 single-line chain.
    let evidence = state
        .compact_summary_evidence(session_id)
        .map_err(|e| e.to_string())?;
    flag_compaction_boundaries_from_evidence(state, provider_name, &evidence)
}

fn flag_compaction_boundaries_from_evidence(
    state: &StateDb,
    provider_name: &str,
    evidence: &oulipoly_state::CompactSummaryEvidence,
) -> Result<u64, String> {
    let mut flagged = 0u64;
    for turn_uuid in &evidence.compact_turn_uuids {
        if state.flag_compaction_boundary(provider_name, &evidence.session_id, turn_uuid)? {
            flagged += 1;
        }
    }
    Ok(flagged)
}

fn accumulate_compaction_backfill(report: &mut CompactionBackfillReport, flagged: u64) {
    report.turns_flagged += flagged;
    report.sessions_processed += 1;
}

fn render_compaction_backfill_session(provider_name: &str, session_id: &str, flagged: u64) {
    println!(
        "compaction backfill session: provider={} session_id={} flagged={}",
        provider_name, session_id, flagged
    );
}

// AGE-160 drift discovery: compaction-backfill helper has been dead since pre-AGE-160
// baseline (commit 8922b652). Follow-up cleanup tracked in DECISIONS.md § AGE-160 -
// Drift-discovery disposition.
#[allow(dead_code)]
struct CompactionBackfillEnvironment {
    sessions_cfg: oulipoly_config::SessionsConfig,
    models: HashMap<String, ModelConfig>,
}

// AGE-160 drift discovery: compaction-backfill helper has been dead since pre-AGE-160
// baseline (commit 8922b652). Follow-up cleanup tracked in DECISIONS.md § AGE-160 -
// Drift-discovery disposition.
#[allow(dead_code)]
fn load_compaction_backfill_environment() -> Result<CompactionBackfillEnvironment, String> {
    Ok(CompactionBackfillEnvironment {
        sessions_cfg: load_compaction_sessions_config()?,
        models: load_compaction_models()?,
    })
}

fn load_compaction_sessions_config() -> Result<oulipoly_config::SessionsConfig, String> {
    let sessions_path = default_config_root().join("sessions.toml");
    oulipoly_config::SessionsConfig::load(&sessions_path)
        .map_err(|e| format!("Failed to load {}: {e}", sessions_path.display()))
}

fn load_compaction_models() -> Result<HashMap<String, ModelConfig>, String> {
    let models_dir = default_models_dir();
    if models_dir.is_dir() {
        Ok(load_models(&models_dir, None)?)
    } else {
        Ok(HashMap::new())
    }
}

// AGE-160 drift discovery: compaction-backfill helper has been dead since pre-AGE-160
// baseline (commit 8922b652). Follow-up cleanup tracked in DECISIONS.md § AGE-160 -
// Drift-discovery disposition.
#[allow(dead_code)]
fn locate_compaction_backfill_source(
    provider_name: &str,
    session_id: &str,
    sessions_cfg: &oulipoly_config::SessionsConfig,
    models: &HashMap<String, ModelConfig>,
) -> Option<PathBuf> {
    if let Some(path) = existing_session_transcript_path(sessions_cfg, provider_name, session_id) {
        return Some(path);
    }

    existing_storage_transcript_path(provider_name, session_id, models)
}

fn existing_session_transcript_path(
    sessions_cfg: &oulipoly_config::SessionsConfig,
    provider_name: &str,
    session_id: &str,
) -> Option<PathBuf> {
    let path =
        oulipoly_runtime::sessions::locate_transcript(sessions_cfg, provider_name, session_id)
            .ok()
            .flatten()?;
    existing_path(path)
}

fn existing_storage_transcript_path(
    provider_name: &str,
    session_id: &str,
    models: &HashMap<String, ModelConfig>,
) -> Option<PathBuf> {
    existing_path(storage_transcript_path(
        matching_storage_providers(provider_name, models),
        session_id,
    )?)
}

fn matching_storage_providers<'a>(
    provider_name: &str,
    models: &'a HashMap<String, ModelConfig>,
) -> Vec<&'a ProviderConfig> {
    models
        .values()
        .flat_map(|model| model.providers.iter())
        .filter(|provider| provider.name == provider_name)
        .collect()
}

fn storage_transcript_path(providers: Vec<&ProviderConfig>, session_id: &str) -> Option<PathBuf> {
    providers.into_iter().find_map(|provider| {
        oulipoly_runtime::migration::find_claude_source_from_storage(provider, session_id)
    })
}

fn existing_path(path: PathBuf) -> Option<PathBuf> {
    if path.exists() { Some(path) } else { None }
}

// AGE-160 drift discovery: compaction-backfill helper has been dead since pre-AGE-160
// baseline (commit 8922b652). Follow-up cleanup tracked in DECISIONS.md § AGE-160 -
// Drift-discovery disposition.
#[allow(dead_code)]
fn flag_compaction_boundaries_from_jsonl(
    state: &StateDb,
    provider_name: &str,
    session_id: &str,
    path: &Path,
) -> Result<u64, String> {
    let mut flagged = 0u64;
    for line in read_compaction_jsonl_lines(path)? {
        let Some(turn_id) = compact_summary_turn_id(&line) else {
            continue;
        };
        if flag_compaction_boundary(state, provider_name, session_id, &turn_id)? {
            flagged += 1;
        }
    }
    Ok(flagged)
}

fn read_compaction_jsonl_lines(path: &Path) -> Result<Vec<String>, String> {
    let file = open_compaction_source(path)?;
    collect_compaction_jsonl_lines(path, std::io::BufReader::new(file).lines())
}

fn collect_compaction_jsonl_lines<I>(path: &Path, lines: I) -> Result<Vec<String>, String>
where
    I: Iterator<Item = Result<String, std::io::Error>>,
{
    lines
        .map(|line| line.map_err(|e| format_compaction_source_line_error(path, e)))
        .collect()
}

fn open_compaction_source(path: &Path) -> Result<std::fs::File, String> {
    std::fs::File::open(path)
        .map_err(|e| format!("Failed to open compaction source {}: {e}", path.display()))
}

fn format_compaction_source_line_error(path: &Path, error: std::io::Error) -> String {
    format!(
        "Failed to read compaction source line from {}: {error}",
        path.display()
    )
}

fn flag_compaction_boundary(
    state: &StateDb,
    provider_name: &str,
    session_id: &str,
    turn_id: &str,
) -> Result<bool, String> {
    state.flag_compaction_boundary(provider_name, session_id, turn_id)
}

fn compact_summary_turn_id(line: &str) -> Option<String> {
    let obj = parse_compaction_json_line(line)?;
    compact_summary_turn_uuid(&obj)
}

fn parse_compaction_json_line(line: &str) -> Option<serde_json::Value> {
    serde_json::from_str::<serde_json::Value>(line).ok()
}

fn is_compact_summary_json(obj: &serde_json::Value) -> bool {
    obj.get("isCompactSummary")
        .and_then(|value| value.as_bool())
        == Some(true)
}

fn compact_summary_turn_uuid(obj: &serde_json::Value) -> Option<String> {
    if !is_compact_summary_json(obj) {
        return None;
    }
    raw_compact_summary_uuid(obj).map(string_from_str)
}

fn raw_compact_summary_uuid(obj: &serde_json::Value) -> Option<&str> {
    obj.get("uuid").and_then(|value| value.as_str())
}

fn string_from_str(value: &str) -> String {
    value.to_string()
}

fn format_resume_list_line(preview: &oulipoly_state::ChainPreview) -> String {
    format!(
        "chain_id={} last_used_at={} active_provider={} active_session_id={} turn_count={} recent_turns_count={}",
        preview.chain_id,
        preview.last_used_at.to_rfc3339(),
        preview.active_provider,
        preview.active_session_id,
        preview.turn_count,
        preview.recent_turns.len()
    )
}

fn normalize_resume_list_args<I, S>(args: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args = args.into_iter().map(Into::into).collect::<Vec<String>>();
    if legacy_resume_list_args(&args) {
        normalized_resume_list_args(args)
    } else {
        args
    }
}

fn legacy_resume_list_args(args: &[String]) -> bool {
    args.len() >= 4
        && args.get(1).is_some_and(|arg| arg == "resume")
        && args.get(2).is_some_and(|arg| arg == "--list")
}

fn normalized_resume_list_args(args: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::with_capacity(args.len() - 1);
    normalized.push(args[0].clone());
    normalized.push("resume-list".to_string());
    normalized.push(args[3].clone());
    normalized.extend(args.into_iter().skip(4));
    normalized
}

fn main() -> ExitCode {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    if std::env::args().len() == 1 {
        agent_runner_lib::run_tauri();
        return ExitCode::SUCCESS;
    }

    let cli = Cli::parse_from(normalize_resume_list_args(std::env::args()));

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
    use oulipoly_state::InvocationStatus;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::{Mutex, OnceLock};

    const TRACE_UUID: &str = "11111111-1111-1111-1111-111111111111";
    const REPL_MODEL: &str = "fixture-model";

    trait ResumeSessionField {
        fn as_optional_str(&self) -> Option<&str>;
    }

    impl ResumeSessionField for String {
        fn as_optional_str(&self) -> Option<&str> {
            Some(self.as_str())
        }
    }

    impl ResumeSessionField for Option<String> {
        fn as_optional_str(&self) -> Option<&str> {
            self.as_deref()
        }
    }

    fn resume_session_field_as_deref(field: &impl ResumeSessionField) -> Option<&str> {
        field.as_optional_str()
    }

    fn parse_resume_subcommand<const N: usize>(argv: [&str; N]) -> Subcommands {
        Cli::try_parse_from(argv)
            .unwrap()
            .command
            .expect("resume argv should produce a subcommand")
    }

    fn assert_resume_debug_contains_option_field(
        command: Subcommands,
        field_name: &str,
        expected: &str,
    ) {
        match &command {
            Subcommands::Resume { .. } => {}
            _ => panic!("expected resume subcommand"),
        }

        let rendered = format!("{command:?}");
        let expected_fragment = format!("{field_name}: Some(\"{expected}\")");
        assert!(
            rendered.contains(&expected_fragment),
            "expected `{expected_fragment}` in parsed Resume variant: {rendered}"
        );
    }

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn cwd_lock() -> &'static Mutex<()> {
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

    fn production_source() -> &'static str {
        include_str!("main.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production source precedes tests")
    }

    fn production_block_after(start: &str) -> &'static str {
        let source = production_source();
        let start_idx = source
            .find(start)
            .unwrap_or_else(|| panic!("missing {start}"));
        let open_idx = source[start_idx..]
            .find('{')
            .map(|idx| start_idx + idx)
            .unwrap_or_else(|| panic!("missing opening brace after {start}"));
        let mut depth = 1usize;
        let mut idx = open_idx + 1;
        let bytes = source.as_bytes();

        while idx < bytes.len() {
            match bytes[idx] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &source[open_idx + 1..idx];
                    }
                }
                _ => {}
            }
            idx += 1;
        }

        panic!("missing closing brace after {start}");
    }

    #[test]
    fn age153_typed_signal_precedence_runs_before_legacy_diagnostics_in_headless_paths() {
        let balanced = production_block_after("fn run_with_balancing(");
        let signal_idx = balanced
            .find("apply_terminal_signal_outcome")
            .expect("run_with_balancing must consume typed signal");
        let diagnostics_idx = balanced
            .find("balanced_result_error_category")
            .expect("run_with_balancing diagnostics fallback");
        assert!(
            signal_idx < diagnostics_idx,
            "typed signal must precede balanced legacy diagnostics"
        );

        let resume = production_block_after("fn run_resume(");
        let signal_idx = resume
            .find("apply_terminal_signal_outcome")
            .expect("run_resume must consume typed signal");
        let diagnostics_idx = resume
            .find("resume_result_error_category")
            .expect("run_resume diagnostics fallback");
        assert!(
            signal_idx < diagnostics_idx,
            "typed signal must precede resume legacy diagnostics"
        );
    }

    #[test]
    fn age153_interactive_signal_precedence_and_clean_no_marker_are_declared() {
        let repl = production_block_after("fn run_repl(");
        assert!(repl.contains("InteractiveExecutionResult") || repl.contains("terminal_signal"));
        assert!(
            repl.contains("TerminalSignalDisposition::InteractiveFail")
                && repl.contains("TerminalSignalDisposition::InteractiveClean"),
            "interactive path must distinguish non-clean marker emission from clean no-marker"
        );
        assert!(
            !repl.contains("execute_with_bounded_silence"),
            "run_repl inherited-stdio path must not add bounded-silence supervision"
        );
    }

    #[test]
    fn age153_marker_emitting_typed_dispositions_mark_guard_after_explicit_finalize() {
        for (function_name, disposition) in [
            ("fn run_with_balancing(", "QuotaExhaustedRetry"),
            ("fn run_with_balancing(", "ProlongedSilenceFail"),
            ("fn run_resume(", "QuotaExhaustedRetry"),
            ("fn run_resume(", "ProlongedSilenceFail"),
            ("fn run_repl(", "InteractiveFail"),
        ] {
            assert_typed_signal_disposition_marks_guard_after_finalize(function_name, disposition);
        }
    }

    fn assert_typed_signal_disposition_marks_guard_after_finalize(
        function_name: &str,
        disposition: &str,
    ) {
        let body = production_block_after(function_name);
        let disposition_token = format!("TerminalSignalDisposition::{disposition}");
        let disposition_idx = body
            .find(&disposition_token)
            .unwrap_or_else(|| panic!("{function_name} must handle {disposition_token}"));
        let branch = disposition_branch_source(&body[disposition_idx..]);
        let finalize_idx = branch
            .find("finalize_invocation")
            .unwrap_or_else(|| panic!("{disposition_token} must explicitly finalize invocation"));
        let mark_idx = branch
            .find("guard.mark_finalized()")
            .unwrap_or_else(|| panic!("{disposition_token} must mark FinalizerGuard finalized"));
        assert!(
            finalize_idx < mark_idx,
            "{disposition_token} must mark the guard only after explicit finalization"
        );
        assert!(
            !branch.contains("finalize_invocation_from_guard"),
            "{disposition_token} must not route typed-signal handling through FinalizerGuard::drop"
        );
    }

    fn disposition_branch_source(source: &str) -> &str {
        if let Some(arrow_idx) = source.find("=>") {
            let after_arrow = &source[arrow_idx + "=>".len()..];
            return match after_arrow.find("TerminalSignalDisposition::") {
                Some(next_idx) => &source[..arrow_idx + "=>".len() + next_idx],
                None => source,
            };
        }

        let after_first = &source["TerminalSignalDisposition::".len()..];
        match after_first.find("TerminalSignalDisposition::") {
            Some(next_idx) => &source[..next_idx + "TerminalSignalDisposition::".len()],
            None => source,
        }
    }

    fn providers_cfg_with_storage(names: &[&str]) -> ProvidersConfig {
        let mut cfg = ProvidersConfig::default();
        for name in names {
            cfg.entries.insert(
                (*name).to_string(),
                oulipoly_config::ProviderEntry {
                    command: Some((*name).to_string()),
                    session_storage: Some(oulipoly_config::SessionStorage::ClaudeCode {
                        projects_dir: PathBuf::from(format!("/tmp/{name}/projects")),
                    }),
                    resume: Some(oulipoly_config::ResumeStrategy {
                        kind: oulipoly_config::ResumeKind::Flag,
                        flag: Some("--resume".to_string()),
                        subcommand: None,
                    }),
                    ..oulipoly_config::ProviderEntry::default()
                },
            );
        }
        cfg
    }

    fn providers_cfg_with_cwd_script(
        provider_name: &str,
        cwd_script: impl Into<String>,
    ) -> ProvidersConfig {
        let mut cfg = ProvidersConfig::default();
        cfg.entries.insert(
            provider_name.to_string(),
            oulipoly_config::ProviderEntry {
                command: Some(provider_name.to_string()),
                session_storage: Some(oulipoly_config::SessionStorage::Script {
                    cwd_script: cwd_script.into(),
                    transcript_script: None,
                    storage_type: None,
                }),
                resume: Some(oulipoly_config::ResumeStrategy {
                    kind: oulipoly_config::ResumeKind::Flag,
                    flag: Some("--resume".to_string()),
                    subcommand: None,
                }),
                ..oulipoly_config::ProviderEntry::default()
            },
        );
        cfg
    }

    fn imported_chain_state(db_path: &Path, provider_name: &str, session_id: &str) -> StateDb {
        let state = StateDb::open(db_path).unwrap();
        let started_at = chrono::DateTime::parse_from_rfc3339("2026-04-17T08:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        state
            .mint_imported_chain_if_absent(provider_name, session_id, &started_at, "<unknown>")
            .unwrap();
        state
    }

    fn toml_array_strings(table: &toml::Table, key: &str) -> Vec<String> {
        table
            .get(key)
            .and_then(toml::Value::as_array)
            .unwrap_or_else(|| panic!("missing array key {key} in {table:?}"))
            .iter()
            .map(|value| value.as_str().unwrap().to_string())
            .collect()
    }

    fn migrated_model_provider(path: &Path) -> toml::Table {
        let table = std::fs::read_to_string(path)
            .unwrap()
            .parse::<toml::Table>()
            .unwrap();
        table["providers"]
            .as_array()
            .unwrap()
            .first()
            .unwrap()
            .as_table()
            .unwrap()
            .clone()
    }

    fn migrated_runtime_provider(path: &Path, provider: &str) -> toml::Table {
        let table = std::fs::read_to_string(path)
            .unwrap()
            .parse::<toml::Table>()
            .unwrap();
        table[provider].as_table().unwrap().clone()
    }

    // Characterization test for AGE-8 — pins current behavior of CLI adapter helpers in this inline test section.
    fn with_deleted_current_dir(test: impl FnOnce()) {
        let _guard = cwd_lock().lock().unwrap();
        let original = std::env::current_dir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        drop(dir);

        let result = catch_unwind(AssertUnwindSafe(test));

        std::env::set_current_dir(original).unwrap();
        if let Err(payload) = result {
            std::panic::resume_unwind(payload);
        }
    }

    // Characterization test for AGE-8 — pins current behavior of parse_inputs CLI adapter.
    #[test]
    fn parse_inputs_collects_repeated_keys_and_rejects_missing_separator() {
        let parsed = parse_inputs(&[
            "size=large".to_string(),
            "style=flat".to_string(),
            "size=small".to_string(),
            "empty=".to_string(),
        ])
        .unwrap();

        assert_eq!(parsed["size"], vec!["large", "small"]);
        assert_eq!(parsed["style"], vec!["flat"]);
        assert_eq!(parsed["empty"], vec![""]);

        let err = parse_inputs(&["not-key-value".to_string()]).unwrap_err();
        assert_eq!(
            err,
            "Invalid input format 'not-key-value': expected KEY=VALUE"
        );
    }

    #[test]
    fn format_agent_prompt_with_inputs_appends_sorted_json_contract_block() {
        let agent = AgentConfig {
            name: "linear-operator".to_string(),
            description: String::new(),
            model: "claude-opus".to_string(),
            output_format: String::new(),
            instructions: "# Linear Operator\n\nDo the task.".to_string(),
        };
        let mut inputs = HashMap::new();
        inputs.insert("task".to_string(), vec!["create".to_string()]);
        inputs.insert(
            "label".to_string(),
            vec!["bug".to_string(), "runtime".to_string()],
        );

        let prompt =
            format_agent_prompt_with_inputs(&agent, "User prompt".to_string(), &inputs).unwrap();

        assert!(prompt.starts_with("# Linear Operator\n\nDo the task.\n\nUser prompt"));
        assert!(prompt.contains("## Operator Inputs\n\n```json\n"));
        assert!(
            prompt.contains(
                "\"label\": [\n    \"bug\",\n    \"runtime\"\n  ],\n  \"task\": [\n    \"create\"\n  ]"
            ),
            "{prompt}"
        );
        assert!(prompt.ends_with("\n```"));
    }

    // Characterization test for AGE-8 — pins current behavior of resolve_models_dir CLI adapter.
    #[test]
    fn resolve_models_dir_prefers_explicit_override() {
        let cli = Cli::try_parse_from([
            "oulipoly-agent-runner",
            "--models-dir",
            "/tmp/age8-models",
            "--model",
            "fixture",
            "prompt",
        ])
        .unwrap();

        assert_eq!(resolve_models_dir(&cli), PathBuf::from("/tmp/age8-models"));
    }

    // Characterization test for AGE-8 — pins current behavior of effective_spawn_cwd CLI adapter.
    #[test]
    fn effective_spawn_cwd_keeps_absolute_paths_and_absolutizes_relative_paths() {
        let _guard = cwd_lock().lock().unwrap();
        let cwd = std::env::current_dir().unwrap();

        assert_eq!(
            effective_spawn_cwd(Some(Path::new("/tmp/age8-project"))).unwrap(),
            PathBuf::from("/tmp/age8-project")
        );
        assert_eq!(
            effective_spawn_cwd(Some(Path::new("relative-project"))).unwrap(),
            cwd.join("relative-project")
        );
        assert_eq!(effective_spawn_cwd(None).unwrap(), cwd);
    }

    // Characterization test for AGE-8 — pins current behavior of effective_spawn_cwd error handling.
    #[cfg(unix)]
    #[test]
    fn effective_spawn_cwd_reports_current_dir_resolution_errors() {
        with_deleted_current_dir(|| {
            let err = effective_spawn_cwd(Some(Path::new("relative-project"))).unwrap_err();
            assert!(
                err.starts_with("Failed to resolve current directory:"),
                "{err}"
            );
        });
    }

    #[test]
    fn effective_resume_spawn_cwd_prefers_workspace_from_cwd_script() {
        let dir = tempfile::tempdir().unwrap();
        let provider_name = "provider6";
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
        let workspace = dir.path().join("workspace");
        let caller_cwd = dir.path().join("caller");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&caller_cwd).unwrap();

        let state = imported_chain_state(&dir.path().join("state.db"), provider_name, session_id);
        let models = oulipoly_state::ModelStore::new();
        let providers_cfg = providers_cfg_with_cwd_script(
            provider_name,
            format!(
                "printf '{{\"found\":true,\"cwd\":\"{}\"}}\\n'",
                workspace.display()
            ),
        );
        let sessions_cfg = oulipoly_config::SessionsConfig::default();

        let cwd = effective_resume_spawn_cwd(
            &state,
            &models,
            &providers_cfg,
            &sessions_cfg,
            session_id,
            Some(&caller_cwd),
        )
        .unwrap();

        assert_eq!(cwd, workspace);
    }

    #[test]
    fn effective_resume_spawn_cwd_falls_back_when_cwd_script_reports_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let provider_name = "provider6";
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
        let fallback = dir.path().join("caller");
        std::fs::create_dir_all(&fallback).unwrap();

        let state = imported_chain_state(&dir.path().join("state.db"), provider_name, session_id);
        let models = oulipoly_state::ModelStore::new();
        let providers_cfg =
            providers_cfg_with_cwd_script(provider_name, "printf '{\"found\":false}\\n'");
        let sessions_cfg = oulipoly_config::SessionsConfig::default();

        let cwd = effective_resume_spawn_cwd(
            &state,
            &models,
            &providers_cfg,
            &sessions_cfg,
            session_id,
            Some(&fallback),
        )
        .unwrap();

        assert_eq!(cwd, fallback);
    }

    #[test]
    fn effective_resume_spawn_cwd_falls_back_when_cwd_script_returns_malformed_json() {
        let dir = tempfile::tempdir().unwrap();
        let provider_name = "provider6";
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
        let fallback = dir.path().join("caller");
        std::fs::create_dir_all(&fallback).unwrap();

        let state = imported_chain_state(&dir.path().join("state.db"), provider_name, session_id);
        let models = oulipoly_state::ModelStore::new();
        let providers_cfg = providers_cfg_with_cwd_script(provider_name, "printf 'not-json\\n'");
        let sessions_cfg = oulipoly_config::SessionsConfig::default();

        let cwd = effective_resume_spawn_cwd(
            &state,
            &models,
            &providers_cfg,
            &sessions_cfg,
            session_id,
            Some(&fallback),
        )
        .unwrap();

        assert_eq!(cwd, fallback);
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
    fn parser_accepts_long_new() {
        let cli = Cli::try_parse_from(["oulipoly-agent-runner", "--new"]).unwrap();

        assert!(cli.new);
        assert!(cli.command.is_none());
        assert!(cli.agent.is_none());
        assert!(cli.prompt_args.is_empty());
        assert!(cli.model.is_none());
        assert!(cli.resume.is_none());
    }

    #[test]
    fn parser_accepts_short_n() {
        let cli = Cli::try_parse_from(["oulipoly-agent-runner", "-n"]).unwrap();

        assert!(cli.new);
        assert!(cli.command.is_none());
        assert!(cli.agent.is_none());
        assert!(cli.prompt_args.is_empty());
        assert!(cli.model.is_none());
        assert!(cli.resume.is_none());
    }

    #[test]
    fn parser_rejects_resume_then_new() {
        let err = match Cli::try_parse_from([
            "oulipoly-agent-runner",
            "--resume",
            "5169694d-de0f-40d1-890c-6e28e55bab27",
            "--new",
        ]) {
            Ok(_) => panic!("--resume and --new should conflict"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
        let rendered = err.to_string();
        assert!(rendered.contains("--resume"), "{rendered}");
        assert!(rendered.contains("--new"), "{rendered}");
    }

    #[test]
    fn parser_rejects_new_then_resume() {
        let err = match Cli::try_parse_from([
            "oulipoly-agent-runner",
            "--new",
            "--resume",
            "5169694d-de0f-40d1-890c-6e28e55bab27",
        ]) {
            Ok(_) => panic!("--new and --resume should conflict"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
        let rendered = err.to_string();
        assert!(rendered.contains("--new"), "{rendered}");
        assert!(rendered.contains("--resume"), "{rendered}");
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
                rotate_provider: _,
                project,
                models_dir,
            }) => {
                assert_eq!(model.as_deref(), Some(REPL_MODEL));
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
                assert_eq!(model.as_deref(), Some(REPL_MODEL));
                assert_eq!(resume.as_deref(), Some(session_id));
            }
            _ => panic!("expected repl subcommand"),
        }
    }

    #[test]
    fn resume_subcommand_accepts_positional_chain_id() {
        let chain_id = "7ec82d7d-5f83-4be7-8868-b1ce3c9c3123";
        let command = parse_resume_subcommand(["oulipoly-agent-runner", "resume", chain_id]);

        // Keep this layout-tolerant: Phase 6c may use flat fields or a flattened target Args struct.
        assert_resume_debug_contains_option_field(command, "chain_id", chain_id);
    }

    #[test]
    fn resume_subcommand_accepts_session_id_flag() {
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
        let command = parse_resume_subcommand([
            "oulipoly-agent-runner",
            "resume",
            "--session-id",
            session_id,
        ]);

        assert_resume_debug_contains_option_field(command, "session_id", session_id);
    }

    #[test]
    fn resume_subcommand_rejects_both_positional_and_session_id() {
        let err = match Cli::try_parse_from([
            "oulipoly-agent-runner",
            "resume",
            "7ec82d7d-5f83-4be7-8868-b1ce3c9c3123",
            "--session-id",
            "5169694d-de0f-40d1-890c-6e28e55bab27",
        ]) {
            Ok(_) => panic!("expected clap to reject both resume target forms"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn resume_subcommand_rejects_no_target() {
        let err = match Cli::try_parse_from(["oulipoly-agent-runner", "resume"]) {
            Ok(_) => panic!("expected clap to reject resume without a target"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn resume_subcommand_dispatch_routes_positional_to_run_resume_arg() {
        let chain_id = "7ec82d7d-5f83-4be7-8868-b1ce3c9c3123";
        let command = parse_resume_subcommand(["oulipoly-agent-runner", "resume", chain_id]);

        // Parser-level proxy for dispatch: the field selected by the Resume arm must hold
        // the positional target unchanged so Phase 6c can pass it directly to run_resume.
        assert_resume_debug_contains_option_field(command, "chain_id", chain_id);
    }

    #[test]
    fn resume_subcommand_parses_answer_file_and_project() {
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
        let cli = Cli::try_parse_from([
            "oulipoly-agent-runner",
            "resume",
            "-m",
            REPL_MODEL,
            "--session-id",
            session_id,
            "-f",
            "/tmp/answer.md",
            "-p",
            "/tmp/project",
            "--models-dir",
            "/tmp/models",
        ])
        .unwrap();

        match cli.command {
            Some(Subcommands::Resume {
                model,
                session_id: parsed_session,
                rotate_provider: _,
                prompt,
                file,
                project,
                models_dir,
                ..
            }) => {
                assert_eq!(model.as_deref(), Some(REPL_MODEL));
                assert_eq!(
                    resume_session_field_as_deref(&parsed_session),
                    Some(session_id)
                );
                assert_eq!(prompt, None);
                assert_eq!(file, Some(PathBuf::from("/tmp/answer.md")));
                assert_eq!(project, Some(PathBuf::from("/tmp/project")));
                assert_eq!(models_dir, Some(PathBuf::from("/tmp/models")));
            }
            _ => panic!("expected resume subcommand"),
        }
    }

    #[test]
    fn resume_subcommand_parses_inline_prompt() {
        let cli = Cli::try_parse_from([
            "oulipoly-agent-runner",
            "resume",
            "-m",
            REPL_MODEL,
            "--session-id",
            "5169694d-de0f-40d1-890c-6e28e55bab27",
            "--prompt",
            "answer text",
        ])
        .unwrap();

        match cli.command {
            Some(Subcommands::Resume { prompt, file, .. }) => {
                assert_eq!(prompt.as_deref(), Some("answer text"));
                assert_eq!(file, None);
            }
            _ => panic!("expected resume subcommand"),
        }
    }

    #[test]
    fn resume_subcommand_rejects_prompt_and_file_together() {
        let err = match Cli::try_parse_from([
            "oulipoly-agent-runner",
            "resume",
            "-m",
            REPL_MODEL,
            "--session-id",
            "5169694d-de0f-40d1-890c-6e28e55bab27",
            "--prompt",
            "answer text",
            "-f",
            "/tmp/answer.md",
        ]) {
            Ok(_) => panic!("expected clap to reject --prompt with --file"),
            Err(err) => err,
        };

        let rendered = err.to_string();
        assert!(
            rendered.contains("--prompt") || rendered.contains("--file"),
            "{rendered}"
        );
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
                assert_eq!(model.as_deref(), Some(REPL_MODEL));
                assert_eq!(resume.as_deref(), Some(session_id));
            }
            _ => panic!("expected repl subcommand"),
        }
    }

    #[test]
    fn repl_subcommand_allows_missing_model_with_resume() {
        let cli = Cli::try_parse_from([
            "oulipoly-agent-runner",
            "repl",
            "--resume",
            "5169694d-de0f-40d1-890c-6e28e55bab27",
        ])
        .unwrap();

        match cli.command {
            Some(Subcommands::Repl { model, resume, .. }) => {
                assert_eq!(model, None);
                assert_eq!(
                    resume.as_deref(),
                    Some("5169694d-de0f-40d1-890c-6e28e55bab27")
                );
            }
            _ => panic!("expected repl subcommand"),
        }
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
    fn resume_short_line_helper_emits_for_non_tty_stderr() {
        assert!(should_emit_resume_short_line(false));
    }

    #[test]
    fn resume_short_line_helper_also_emits_for_tty_stderr() {
        // The short [resume] -> <provider> line is unconditional per
        // proposal §5: V10 (observable selection) wins over V15
        // (caller-controlled surface) for this single line. The test
        // pins the "always-on" guarantee against future drift.
        assert!(should_emit_resume_short_line(true));
    }

    #[test]
    fn diagnostic_input_includes_stdout_when_provider_reports_errors_there() {
        let stdout = br#"{"api_error_status":429,"result":"You've hit your limit"}"#;

        assert_eq!(
            diagnostic_input("", stdout),
            r#"{"api_error_status":429,"result":"You've hit your limit"}"#
        );
        assert_eq!(
            diagnostic_input("stderr line", stdout),
            "stderr line\n{\"api_error_status\":429,\"result\":\"You've hit your limit\"}"
        );
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

    // RISK: FinalizerGuard panic/drop fallback could leave terminal_reason null while setting guard_drop error_category (proposal §test-intent "FinalizerGuard panic-path characterization", assumption A4)
    // LEVEL: unit
    // SOURCE: contracts/nes-250-contract.md § Test catalog § Finalize cascade (T-FINAL-GUARD)
    #[test]
    fn finalizer_guard_drop_finalizes_failed_row_during_panic_unwind() {
        // CHARACTERIZATION: T-FINAL-GUARD writes error_category=guard_drop and terminal_reason=guard_drop.
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
        assert_eq!(row.terminal_reason.as_deref(), Some("guard_drop"));
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

    // risk: CLI surface; level: unit; source: proposal §11.1 CLI surface / A8.
    #[test]
    fn top_level_resume_parse_allows_missing_model_and_rotate_provider_flag() {
        let cli = Cli::try_parse_from([
            "oulipoly-agent-runner",
            "--resume",
            "5169694d-de0f-40d1-890c-6e28e55bab27",
            "--rotate-provider",
            "claude2",
            "continue",
        ])
        .unwrap();

        assert!(cli.command.is_none());
        assert_eq!(cli.model, None);
        assert_eq!(
            cli.resume.as_deref(),
            Some("5169694d-de0f-40d1-890c-6e28e55bab27")
        );
        assert_eq!(cli.rotate_provider.as_deref(), Some("claude2"));
    }

    // risk: CLI surface; level: unit; source: proposal §11.1 CLI surface / A5, A8.
    #[test]
    fn resume_list_user_syntax_rewrites_to_hidden_subcommand() {
        let argv = normalize_resume_list_args([
            "oulipoly-agent-runner",
            "resume",
            "--list",
            "5169694d-de0f-40d1-890c-6e28e55bab27",
        ]);

        let cli = Cli::try_parse_from(argv).unwrap();

        match cli.command {
            Some(Subcommands::ResumeList { uuid }) => {
                assert_eq!(uuid, "5169694d-de0f-40d1-890c-6e28e55bab27");
            }
            other => panic!("expected hidden resume-list variant, got {other:?}"),
        }
    }

    // risk: CLI surface; level: unit; source: proposal §11.1 CLI surface / A5, A8.
    #[test]
    fn resume_list_line_includes_required_chain_fields() {
        let ts = chrono::DateTime::parse_from_rfc3339("2026-04-17T08:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let preview = oulipoly_state::ChainPreview {
            chain_id: "5169694d-de0f-40d1-890c-6e28e55bab27".to_string(),
            last_used_at: ts,
            active_provider: "claude".to_string(),
            active_session_id: "dd116a3c-6819-42b1-b3d2-f512331eb5ec".to_string(),
            turn_count: 42,
            recent_turns: vec![oulipoly_state::TurnPreview {
                role: "assistant".to_string(),
                timestamp: ts,
                snippet: None,
            }],
        };

        let line = format_resume_list_line(&preview);

        assert!(line.contains("chain_id=5169694d-de0f-40d1-890c-6e28e55bab27"));
        assert!(line.contains("last_used_at=2026-04-17T08:00:00+00:00"));
        assert!(line.contains("active_provider=claude"));
        assert!(line.contains("active_session_id=dd116a3c-6819-42b1-b3d2-f512331eb5ec"));
        assert!(line.contains("turn_count=42"));
        assert!(line.contains("recent_turns_count=1"));
    }

    #[test]
    fn migration_target_pool_when_model_none_is_all_storage_providers() {
        let resolved = oulipoly_state::ResolvedResume {
            chain_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string(),
            model_name: None,
            model: None,
            active_provider: "claude".to_string(),
            active_session_id: "5169694d-de0f-40d1-890c-6e28e55bab27".to_string(),
        };
        let providers_cfg = providers_cfg_with_storage(&["claude", "claude2", "claude3"]);

        let pool = resume_migration_pool(&resolved, &providers_cfg);
        let names = pool
            .providers
            .iter()
            .map(|provider| provider.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["claude", "claude2", "claude3"]);
    }

    #[test]
    fn migration_target_pool_when_model_set_is_model_pool() {
        let model = ModelConfig::from_toml_with_name(
            "claude-opus",
            r#"
[[providers]]
name = "claude"
args = ["--model", "opus"]

[[providers]]
name = "claude2"
args = ["--model", "opus"]
"#,
            None,
        )
        .unwrap();
        let resolved = oulipoly_state::ResolvedResume {
            chain_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string(),
            model_name: Some("claude-opus".to_string()),
            model: Some(model),
            active_provider: "claude".to_string(),
            active_session_id: "5169694d-de0f-40d1-890c-6e28e55bab27".to_string(),
        };
        let providers_cfg = providers_cfg_with_storage(&["claude", "claude2", "claude3"]);

        let pool = resume_migration_pool(&resolved, &providers_cfg);
        let names = pool
            .providers
            .iter()
            .map(|provider| provider.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["claude", "claude2"]);
    }

    #[test]
    fn migrate_config_lifts_per_provider_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let providers_path = dir.path().join("providers.toml");
        std::fs::write(
            models_dir.join("claude-opus.toml"),
            r#"
prompt_mode = "stdin"

[[providers]]
name = "claude2"
command = "env -u CLAUDECODE claude2"
args = ["-p", "--model", "opus", "--output-format", "json"]
interactive_args = ["--model", "opus"]

[providers.resume]
kind = "flag"
flag = "--resume"

[providers.session_capture]
kind = "forced_flag_verified"
flag = "--session-id"

[providers.session_storage]
kind = "claude_code"
projects_dir = "/tmp/claude2/projects"

[providers.resume_acceptance]
accepted_output_patterns = ["\"session_id\":\"{session_id}\""]
"#,
        )
        .unwrap();

        let report = migrate_config_files(&models_dir, &providers_path).unwrap();

        assert_eq!(report.model_files_rewritten, 1);
        let model = std::fs::read_to_string(models_dir.join("claude-opus.toml")).unwrap();
        assert!(model.contains("name = \"claude2\""), "{model}");
        assert!(model.contains("\"--model\""), "{model}");
        assert!(model.contains("\"opus\""), "{model}");
        assert!(!model.contains("\"--output-format\""), "{model}");
        assert!(!model.contains("command ="), "{model}");
        assert!(!model.contains("session_storage"), "{model}");

        let providers = std::fs::read_to_string(&providers_path).unwrap();
        assert!(providers.contains("[claude2]"), "{providers}");
        assert!(providers.contains("command = \"env\""), "{providers}");
        assert!(providers.contains("\"-u\""), "{providers}");
        assert!(providers.contains("\"CLAUDECODE\""), "{providers}");
        assert!(providers.contains("\"claude2\""), "{providers}");
        assert!(providers.contains("[claude2.resume]"), "{providers}");
        assert!(
            providers.contains("[claude2.session_storage]"),
            "{providers}"
        );
        assert!(providers.contains("\"--output-format\""), "{providers}");
    }

    #[test]
    fn migrate_config_backfills_session_storage_from_turn_scripts() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let providers_path = dir.path().join("providers.toml");
        let sessions_path = dir.path().join("sessions.toml");
        std::fs::write(
            models_dir.join("claude-opus.toml"),
            r#"
[[providers]]
name = "claude"
args = ["--model", "opus"]
"#,
        )
        .unwrap();
        std::fs::write(
            &providers_path,
            r#"
[claude]
command = "claude"
args = ["-p"]
"#,
        )
        .unwrap();
        std::fs::write(
            &sessions_path,
            r#"
[claude]
turn_script = "claude-code-turns ~/.claude/projects"
"#,
        )
        .unwrap();

        let report = migrate_config_files(&models_dir, &providers_path).unwrap();

        assert_eq!(report.model_files_rewritten, 0);
        assert!(
            report
                .moved_blocks
                .iter()
                .any(|block| block.contains("sessions.toml[claude].turn_script")),
            "{:?}",
            report.moved_blocks
        );
        let runtime = migrated_runtime_provider(&providers_path, "claude");
        let storage = runtime
            .get("session_storage")
            .and_then(toml::Value::as_table)
            .unwrap();
        assert_eq!(
            storage.get("kind").and_then(toml::Value::as_str),
            Some("script")
        );
        assert_eq!(
            storage.get("cwd_script").and_then(toml::Value::as_str),
            Some("claude-code-cwd ~/.claude/projects")
        );
        assert_eq!(
            storage
                .get("transcript_script")
                .and_then(toml::Value::as_str),
            Some("claude-code-locate-transcript ~/.claude/projects")
        );
        assert_eq!(
            storage.get("storage_type").and_then(toml::Value::as_str),
            Some("claude_code")
        );
    }

    #[test]
    fn migrate_config_keeps_model_only_interactive_args_out_of_provider_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let providers_path = dir.path().join("providers.toml");
        let model_path = models_dir.join("claude-haiku.toml");
        std::fs::write(
            &providers_path,
            r#"
[claude]
command = "claude"
args = ["-p", "--output-format", "json"]
interactive_args = ["--dangerously-skip-permissions"]
"#,
        )
        .unwrap();
        std::fs::write(
            &model_path,
            r#"
[[providers]]
name = "claude"
args = ["--model", "haiku"]
interactive_args = ["--model", "haiku"]
"#,
        )
        .unwrap();

        let report = migrate_config_files(&models_dir, &providers_path).unwrap();

        assert_eq!(report.model_files_rewritten, 0);
        let runtime = migrated_runtime_provider(&providers_path, "claude");
        assert_eq!(
            toml_array_strings(&runtime, "interactive_args"),
            ["--dangerously-skip-permissions"]
        );
        let model = migrated_model_provider(&model_path);
        assert_eq!(
            toml_array_strings(&model, "interactive_args"),
            ["--model", "haiku"]
        );
    }

    #[test]
    fn migrate_config_lifts_runtime_args_strips_model_flags() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let providers_path = dir.path().join("providers.toml");
        let model_path = models_dir.join("claude-opus.toml");
        std::fs::write(
            &model_path,
            r#"
[[providers]]
name = "claude"
command = "env"
args = ["-u", "CLAUDECODE", "claude", "-p", "--model", "opus", "--dangerously-skip-permissions"]
"#,
        )
        .unwrap();

        migrate_config_files(&models_dir, &providers_path).unwrap();

        let runtime = migrated_runtime_provider(&providers_path, "claude");
        assert_eq!(
            toml_array_strings(&runtime, "args"),
            [
                "-u",
                "CLAUDECODE",
                "claude",
                "-p",
                "--dangerously-skip-permissions"
            ]
        );
        let model = migrated_model_provider(&model_path);
        assert_eq!(toml_array_strings(&model, "args"), ["--model", "opus"]);
    }

    #[test]
    fn migrate_config_strips_dash_m_pairs() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let providers_path = dir.path().join("providers.toml");
        let model_path = models_dir.join("gpt-high.toml");
        std::fs::write(
            &model_path,
            r#"
[[providers]]
name = "codex"
command = "codex"
args = ["exec", "--dangerously-bypass-approvals-and-sandbox", "-m", "gpt-5.5"]
"#,
        )
        .unwrap();

        migrate_config_files(&models_dir, &providers_path).unwrap();

        let runtime = migrated_runtime_provider(&providers_path, "codex");
        assert_eq!(
            toml_array_strings(&runtime, "args"),
            ["exec", "--dangerously-bypass-approvals-and-sandbox"]
        );
        let model = migrated_model_provider(&model_path);
        assert_eq!(toml_array_strings(&model, "args"), ["-m", "gpt-5.5"]);
    }

    #[test]
    fn migrate_config_strips_model_prefixed_c_keys() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let providers_path = dir.path().join("providers.toml");
        let model_path = models_dir.join("gpt-high.toml");
        std::fs::write(
            &model_path,
            r#"
[[providers]]
name = "codex"
command = "codex"
args = ["exec", "-c", "model_reasoning_effort=high", "-c", "sandbox=workspace-write"]
"#,
        )
        .unwrap();

        migrate_config_files(&models_dir, &providers_path).unwrap();

        let runtime = migrated_runtime_provider(&providers_path, "codex");
        assert_eq!(
            toml_array_strings(&runtime, "args"),
            ["exec", "-c", "sandbox=workspace-write"]
        );
        let model = migrated_model_provider(&model_path);
        assert_eq!(
            toml_array_strings(&model, "args"),
            ["-c", "model_reasoning_effort=high"]
        );
    }

    #[test]
    fn migrate_config_strips_interactive_args_same_filter() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let providers_path = dir.path().join("providers.toml");
        let model_path = models_dir.join("gpt-high.toml");
        std::fs::write(
            &model_path,
            r#"
[[providers]]
name = "codex"
command = "codex"
args = ["exec", "-m", "gpt-5.5"]
interactive_args = ["exec", "-c", "model_reasoning_effort=high", "-c", "sandbox=workspace-write"]
"#,
        )
        .unwrap();

        migrate_config_files(&models_dir, &providers_path).unwrap();

        let runtime = migrated_runtime_provider(&providers_path, "codex");
        assert_eq!(toml_array_strings(&runtime, "args"), ["exec"]);
        assert_eq!(
            toml_array_strings(&runtime, "interactive_args"),
            ["exec", "-c", "sandbox=workspace-write"]
        );
        let model = migrated_model_provider(&model_path);
        assert_eq!(toml_array_strings(&model, "args"), ["-m", "gpt-5.5"]);
        assert_eq!(
            toml_array_strings(&model, "interactive_args"),
            ["-c", "model_reasoning_effort=high"]
        );
    }

    #[test]
    fn migrate_config_aborts_on_conflicting_runtime_args() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let providers_path = dir.path().join("providers.toml");
        for (name, env_name) in [("a", "CLAUDECODE"), ("b", "CLAUDE_CONFIG_DIR")] {
            std::fs::write(
                models_dir.join(format!("{name}.toml")),
                format!(
                    r#"
[[providers]]
name = "claude"
command = "env"
args = ["-u", "{env_name}", "claude", "-p", "--model", "opus"]
"#
                ),
            )
            .unwrap();
        }

        let err = migrate_config_files(&models_dir, &providers_path).unwrap_err();

        assert!(
            err.contains("conflicting args for provider claude"),
            "{err}"
        );
    }

    #[test]
    fn migrate_config_idempotent_after_proper_lift() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let providers_path = dir.path().join("providers.toml");
        let model_path = models_dir.join("claude-opus.toml");
        std::fs::write(
            &model_path,
            r#"
[[providers]]
name = "claude"
command = "env"
args = ["-u", "CLAUDECODE", "claude", "-p", "--model", "opus"]
"#,
        )
        .unwrap();

        migrate_config_files(&models_dir, &providers_path).unwrap();
        let model_after_first = std::fs::read_to_string(&model_path).unwrap();
        let providers_after_first = std::fs::read_to_string(&providers_path).unwrap();
        let second = migrate_config_files(&models_dir, &providers_path).unwrap();

        assert_eq!(second.model_files_rewritten, 0);
        assert_eq!(
            model_after_first,
            std::fs::read_to_string(&model_path).unwrap()
        );
        assert_eq!(
            providers_after_first,
            std::fs::read_to_string(&providers_path).unwrap()
        );
    }

    #[test]
    fn migrate_config_repairs_prior_empty_runtime_args() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let providers_path = dir.path().join("providers.toml");
        let model_path = models_dir.join("claude-opus.toml");
        std::fs::write(
            &providers_path,
            r#"
[claude]
command = "env"
args = []
interactive_args = []

[claude.resume]
kind = "flag"
flag = "--resume"
"#,
        )
        .unwrap();
        std::fs::write(
            &model_path,
            r#"
[[providers]]
name = "claude"
args = ["-u", "CLAUDECODE", "claude", "-p", "--model", "opus"]
interactive_args = ["-u", "CLAUDECODE", "claude", "--model", "opus"]
"#,
        )
        .unwrap();

        migrate_config_files(&models_dir, &providers_path).unwrap();

        let runtime = migrated_runtime_provider(&providers_path, "claude");
        assert_eq!(
            toml_array_strings(&runtime, "args"),
            ["-u", "CLAUDECODE", "claude", "-p"]
        );
        assert_eq!(
            toml_array_strings(&runtime, "interactive_args"),
            ["-u", "CLAUDECODE", "claude"]
        );
        let model = migrated_model_provider(&model_path);
        assert_eq!(toml_array_strings(&model, "args"), ["--model", "opus"]);
        assert_eq!(
            toml_array_strings(&model, "interactive_args"),
            ["--model", "opus"]
        );
    }

    #[test]
    fn migrate_config_aborts_on_conflicting_command() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let providers_path = dir.path().join("providers.toml");
        for (name, command) in [("a", "claude"), ("b", "claude-other")] {
            std::fs::write(
                models_dir.join(format!("{name}.toml")),
                format!(
                    r#"
[[providers]]
name = "p"
command = "{command}"
"#
                ),
            )
            .unwrap();
        }

        let err = migrate_config_files(&models_dir, &providers_path).unwrap_err();

        assert!(err.contains("conflicting command for provider p"), "{err}");
    }

    #[test]
    fn migrate_config_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let providers_path = dir.path().join("providers.toml");
        std::fs::write(
            models_dir.join("claude-opus.toml"),
            r#"
[[providers]]
name = "claude"
command = "claude"
args = ["-p", "--model", "opus"]

[providers.resume]
kind = "flag"
flag = "--resume"
"#,
        )
        .unwrap();

        let first = migrate_config_files(&models_dir, &providers_path).unwrap();
        let model_after_first =
            std::fs::read_to_string(models_dir.join("claude-opus.toml")).unwrap();
        let providers_after_first = std::fs::read_to_string(&providers_path).unwrap();
        let second = migrate_config_files(&models_dir, &providers_path).unwrap();

        assert_eq!(first.model_files_rewritten, 1);
        assert_eq!(second.model_files_rewritten, 0);
        assert_eq!(
            model_after_first,
            std::fs::read_to_string(models_dir.join("claude-opus.toml")).unwrap()
        );
        assert_eq!(
            providers_after_first,
            std::fs::read_to_string(&providers_path).unwrap()
        );
    }
}
