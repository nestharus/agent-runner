use agent_runner_lib::balancer;
use agent_runner_lib::config::{
    AgentConfig, AgentConfigRepository, FilesystemAgentConfigRepository,
    FilesystemModelConfigRepository, FilesystemProviderConfigSource,
    FilesystemSessionsConfigSource, ModelConfig, ModelConfigRepository, PromptMode, ProviderConfig,
    ProvidersConfig, load_models,
};
use agent_runner_lib::diagnostics;
use agent_runner_lib::executor;
use agent_runner_lib::process::{OsProcessRunner, ProcessRunner};
use agent_runner_lib::schema_probe::{self, ProbeError};
use agent_runner_lib::session_export::{
    ExportError, ExportSessionMetadata, read_canonical_transcript,
    resolve_export_session_metadata_with_deps,
};
use agent_runner_lib::session_lock::{LockError, SessionLock};
use agent_runner_lib::session_metadata::{MetadataError, locate_session_metadata};
use agent_runner_lib::session_replace::{self, ReplaceError};
use agent_runner_lib::state::{CompositeInvocationId, InvocationStart, ReadOnlyOpenError, StateDb};
use agent_runner_lib::trace::{TraceOptions, render_ascii_trace, trace_invocation_with_sessions};

use clap::{Parser, Subcommand};
use std::collections::HashMap;
use std::io::{BufRead, IsTerminal, Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use uuid::Uuid;

const DEFAULT_PAUSE_HANDSHAKE_TTL_MS: u64 = 60_000;
const MAX_PAUSE_HANDSHAKE_TTL_MS: u64 = 600_000;

struct CliBalanceEffects<'a> {
    providers_cfg: &'a agent_runner_lib::config::ProvidersConfig,
    sessions_cfg: &'a agent_runner_lib::config::SessionsConfig,
    in_flight: &'a agent_runner_lib::quota::InFlight,
    state: &'a StateDb,
    runner: &'a dyn ProcessRunner,
}

impl balancer::BalanceEffects for CliBalanceEffects<'_> {
    fn refresh_quota_if_stale(&self, provider_name: &str) {
        if agent_runner_lib::quota::is_stale(self.state, provider_name) {
            let _ = agent_runner_lib::quota::refresh_provider(
                provider_name,
                self.providers_cfg,
                self.in_flight,
                self.state,
                self.runner,
            );
        }
    }

    fn scan_provider_sessions(&self, provider_name: &str) {
        let _ = agent_runner_lib::sessions::scan_provider_with_runner(
            provider_name,
            self.sessions_cfg,
            self.state,
            self.state,
            self.runner,
        );
    }
}

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

    /// Resume an existing session by UUID at the top level. Routes by
    /// prompt presence: a prompt (positional, `--file`, or piped stdin)
    /// dispatches to non-interactive headless mode; no prompt drops into
    /// the provider's interactive REPL. Equivalent to `repl --resume`
    /// (no prompt) or `resume --session-id <sid>` (with prompt), but
    /// unified at the top level.
    #[arg(long = "resume")]
    resume: Option<String>,

    /// Manually migrate the active chain segment to the named provider.
    #[arg(long = "migrate")]
    migrate: Option<String>,

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
        /// Model id to launch interactively. Optional when --resume can infer
        /// a model or fall through to the provider CLI's default model.
        model: Option<String>,

        /// Resume an existing session by full UUID
        #[arg(long = "resume")]
        resume: Option<String>,

        /// Manually migrate the active chain segment to the named provider.
        #[arg(long = "migrate")]
        migrate: Option<String>,

        /// Working directory for the wrapped CLI
        #[arg(short = 'p', long = "project")]
        project: Option<PathBuf>,

        /// Override models directory
        #[arg(long = "models-dir")]
        models_dir: Option<PathBuf>,
    },
    /// Resume a provider session non-interactively with an answer payload.
    Resume {
        /// Model id whose provider pool must include the session owner.
        #[arg(short, long)]
        model: Option<String>,

        /// Provider session UUID to resume.
        #[arg(long = "session-id")]
        session_id: String,

        /// Manually migrate the active chain segment to the named provider.
        #[arg(long = "migrate")]
        migrate: Option<String>,

        /// Inline answer payload. Use --file for larger payloads.
        #[arg(long = "prompt", conflicts_with = "file")]
        prompt: Option<String>,

        /// Read answer payload from file.
        #[arg(short, long, conflicts_with = "prompt")]
        file: Option<PathBuf>,

        /// Working directory for the wrapped CLI.
        #[arg(short = 'p', long = "project")]
        project: Option<PathBuf>,

        /// Override models directory.
        #[arg(long = "models-dir")]
        models_dir: Option<PathBuf>,
    },
    /// Inspect and coordinate session control-plane operations.
    Session {
        #[command(subcommand)]
        command: SessionSubcommands,
    },
    /// Hidden normalized form for `resume --list <UUID>`.
    #[command(hide = true, name = "resume-list")]
    ResumeList { uuid: String },
    /// Run chain-table backfill explicitly.
    MigrateDb,
    /// Move runtime provider config from model TOMLs into providers.toml. Idempotent - safe to re-run if a previous run left empty args.
    MigrateConfig {
        /// Override models directory.
        #[arg(long = "models-dir")]
        models_dir: Option<PathBuf>,
    },
}

#[derive(Clone, Debug, Subcommand)]
enum SessionSubcommands {
    /// Locate transcript and workspace metadata for a session.
    Locate {
        /// Provider session UUID to locate.
        session_id: String,

        /// Emit JSON. Accepted for symmetry; locate always emits JSON.
        #[arg(long)]
        json: bool,
    },
    /// Inspect the default state database schema and supported session features.
    SchemaProbe,
    /// Export a provider session as canonical JSONL.
    Export {
        session_id: String,
        #[arg(long, default_value = "canonical-jsonl")]
        format: String,
    },
    /// Acquire an advisory pause lease for a resolved session.
    PauseHandshake {
        session_id: String,
        #[arg(long)]
        ttl_ms: Option<u64>,
    },
    /// Release a previously acquired advisory pause lease.
    ResumeHandshake {
        session_id: String,
        #[arg(long)]
        token: String,
    },
    /// Replace a provider transcript from canonical JSONL.
    ImportReplace {
        session_id: String,
        #[arg(long = "from-file")]
        from_file: Option<PathBuf>,
        #[arg(long = "preimage-sha256")]
        preimage_sha256: Option<String>,
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

fn resolve_resume_answer(prompt: Option<&str>, file: Option<&Path>) -> Result<String, String> {
    if let Some(path) = file {
        return std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read answer file: {e}"));
    }
    if let Some(prompt) = prompt {
        return Ok(prompt.to_string());
    }
    if std::io::stdin().is_terminal() {
        return Err(
            "No answer payload provided. Pass --prompt, --file, or pipe to stdin.".to_string(),
        );
    }
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| format!("Failed to read stdin: {e}"))?;
    if input.trim().is_empty() {
        return Err("Empty answer payload from stdin.".to_string());
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
    if let Err(err) = session_replace::recover_pending_replaces() {
        eprintln!("{}", err.to_json());
        return Ok(err.exit_code());
    }

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
                migrate,
                project,
                models_dir,
            } => run_repl(
                model.as_deref(),
                resume.as_deref(),
                migrate.as_deref(),
                project.as_deref(),
                models_dir.as_deref(),
            ),
            Subcommands::Resume {
                model,
                session_id,
                migrate,
                prompt,
                file,
                project,
                models_dir,
            } => run_resume(
                model.as_deref(),
                &session_id,
                migrate.as_deref(),
                prompt.as_deref(),
                file.as_deref(),
                project.as_deref(),
                models_dir.as_deref(),
            ),
            Subcommands::Session { command } => match command {
                SessionSubcommands::Locate { session_id, json } => {
                    run_session_locate(&session_id, json)
                }
                SessionSubcommands::SchemaProbe => run_session_schema_probe(),
                SessionSubcommands::Export { session_id, format } => {
                    run_session_export(&session_id, &format)
                }
                SessionSubcommands::PauseHandshake { session_id, ttl_ms } => {
                    run_pause_handshake(&session_id, ttl_ms)
                }
                SessionSubcommands::ResumeHandshake { session_id, token } => {
                    run_resume_handshake(&session_id, &token)
                }
                SessionSubcommands::ImportReplace {
                    session_id,
                    from_file,
                    preimage_sha256,
                } => run_session_import_replace(
                    &session_id,
                    from_file.as_deref(),
                    preimage_sha256.as_deref(),
                ),
            },
            Subcommands::ResumeList { uuid } => run_resume_list(&uuid),
            Subcommands::MigrateDb => run_migrate_db(),
            Subcommands::MigrateConfig { models_dir } => run_migrate_config(models_dir.as_deref()),
        };
    }

    // Top-level --resume unifies REPL and headless paths. A prompt source
    // (--file, positional args, or piped stdin) dispatches to headless;
    // no prompt dispatches to the provider's interactive REPL. Subcommand
    // forms (`repl --resume`, `resume`) still work unchanged.
    if let Some(ref session_id) = cli.resume {
        if cli.agent_file.is_some() {
            return Err("--resume is incompatible with --agent-file.".to_string());
        }

        let prompt_text = collect_positional_prompt(&cli, true);
        let stdin_prompt =
            if prompt_text.is_none() && cli.file.is_none() && !std::io::stdin().is_terminal() {
                let mut input = String::new();
                std::io::stdin()
                    .read_to_string(&mut input)
                    .map_err(|e| format!("Failed to read stdin: {e}"))?;
                if input.trim().is_empty() {
                    None
                } else {
                    Some(input)
                }
            } else {
                None
            };

        let has_positional_prompt = prompt_text.is_some();
        let has_file = cli.file.is_some();
        let has_prompt = has_positional_prompt || has_file || stdin_prompt.is_some();

        if has_prompt {
            let prompt_text = prompt_text.as_deref().or(stdin_prompt.as_deref());
            return run_resume(
                cli.model.as_deref(),
                session_id,
                cli.migrate.as_deref(),
                prompt_text,
                cli.file.as_deref(),
                cli.project.as_deref(),
                cli.models_dir.as_deref(),
            );
        } else {
            return run_repl(
                cli.model.as_deref(),
                Some(session_id),
                cli.migrate.as_deref(),
                cli.project.as_deref(),
                cli.models_dir.as_deref(),
            );
        }
    }

    let models_dir = resolve_models_dir(&cli);
    let model_repo = FilesystemModelConfigRepository::new(models_dir.clone());
    let models = model_repo.load_models()?;
    let extra_inputs = parse_inputs(&cli.inputs)?;

    let working_dir = cli.project.clone();

    // Direct model execution (--model)
    if let Some(ref model_name) = cli.model {
        let model = models
            .get(model_name)
            .ok_or_else(|| format!("Unknown model: {model_name}"))?;

        let prompt = if let Some(ref agent_path) = cli.agent_file {
            let agent_repo = FilesystemAgentConfigRepository::new(
                cli.agents_dir
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("agents")),
            );
            let agent = agent_repo.load_agent_file(agent_path)?;
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

fn run_session_schema_probe() -> Result<i32, String> {
    match schema_probe::run_schema_probe() {
        Ok(report) if report.state_db.exists && !report.state_db.compatible => {
            write_json_error(
                "schema-incompatible",
                &format!(
                    "state database schema is incompatible: {}",
                    report.state_db.path.display()
                ),
            )?;
            Ok(14)
        }
        Ok(report) => {
            let json = serde_json::to_string(&report)
                .map_err(|e| format!("Failed to serialize schema probe report: {e}"))?;
            println!("{json}");
            Ok(0)
        }
        Err(error) => {
            write_json_error("operational-error", &probe_error_message(error))?;
            Ok(1)
        }
    }
}

fn run_session_import_replace(
    session_id: &str,
    from_file: Option<&Path>,
    preimage_sha256: Option<&str>,
) -> Result<i32, String> {
    if Uuid::try_parse(session_id).is_err() {
        let err = ReplaceError::InvalidSessionId {
            input: session_id.to_string(),
        };
        eprintln!("{}", err.to_json());
        return Ok(err.exit_code());
    }
    if let Some(hash) = preimage_sha256
        && (hash.len() != 64 || !hash.chars().all(|ch| ch.is_ascii_hexdigit()))
    {
        let err = ReplaceError::InvalidArgument {
            message: "preimage sha256 must be 64 hex characters".to_string(),
        };
        eprintln!("{}", err.to_json());
        return Ok(err.exit_code());
    }
    match session_replace::run_import_replace(session_id, from_file, preimage_sha256) {
        Ok(receipt) => {
            let json = serde_json::to_string(&receipt)
                .map_err(|e| format!("Failed to serialize replace receipt: {e}"))?;
            println!("{json}");
            Ok(0)
        }
        Err(err) => {
            eprintln!("{}", err.to_json());
            Ok(err.exit_code())
        }
    }
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
    let value = serde_json::json!({
        "error": {
            "code": code,
            "message": message,
        }
    });
    let json = serde_json::to_string(&value)
        .map_err(|e| format!("Failed to serialize schema probe error: {e}"))?;
    eprintln!("{json}");
    Ok(())
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

fn run_session_locate(session_id: &str, _json: bool) -> Result<i32, String> {
    if Uuid::parse_str(session_id).is_err() {
        emit_metadata_error(&MetadataError::InvalidSessionId {
            input: session_id.to_string(),
        });
        return Ok(2);
    }

    let state = match StateDb::open_default() {
        Ok(state) => state,
        Err(message) => {
            emit_metadata_error(&MetadataError::Operational { message });
            return Ok(1);
        }
    };

    let config_root = dirs::config_dir()
        .map(|d| d.join("oulipoly-agent-runner"))
        .unwrap_or_else(|| PathBuf::from("."));
    let model_repo = FilesystemModelConfigRepository::new(default_models_dir());
    let provider_source = FilesystemProviderConfigSource::new(config_root.join("providers.toml"));
    let sessions_source = FilesystemSessionsConfigSource::new(config_root.join("sessions.toml"));
    let runner = OsProcessRunner;

    match locate_session_metadata(
        &state,
        &model_repo,
        &provider_source,
        &sessions_source,
        &runner,
        session_id,
    ) {
        Ok(metadata) => match serde_json::to_string(&metadata) {
            Ok(json) => {
                println!("{json}");
                Ok(0)
            }
            Err(err) => {
                emit_metadata_error(&MetadataError::Operational {
                    message: format!("failed to serialize session metadata: {err}"),
                });
                Ok(1)
            }
        },
        Err(err) => {
            let code = metadata_error_exit_code(&err);
            emit_metadata_error(&err);
            Ok(code)
        }
    }
}

fn run_session_export(session_id: &str, format: &str) -> Result<i32, String> {
    if format != "canonical-jsonl" {
        emit_export_json_error(
            "invalid-format",
            &format!("unsupported export format {format}; expected canonical-jsonl"),
        );
        return Ok(2);
    }

    if Uuid::parse_str(session_id).is_err() {
        let err = ExportError::InvalidSessionId {
            input: session_id.to_string(),
        };
        emit_export_error(&err);
        return Ok(export_error_exit_code(&err));
    }

    let metadata = match resolve_export_session_metadata(session_id) {
        Ok(metadata) => metadata,
        Err(err) => {
            emit_export_error(&err);
            return Ok(export_error_exit_code(&err));
        }
    };

    let records = match read_canonical_transcript(&metadata) {
        Ok(records) => records,
        Err(err) => {
            emit_export_error(&err);
            return Ok(export_error_exit_code(&err));
        }
    };

    let mut output = String::new();
    for record in records {
        let line = serde_json::to_string(&record).map_err(|e| {
            format!(
                "Failed to serialize canonical export for session {}: {e}",
                metadata.session_id
            )
        })?;
        output.push_str(&line);
        output.push('\n');
    }
    print!("{output}");
    Ok(0)
}

fn resolve_export_session_metadata(session_id: &str) -> Result<ExportSessionMetadata, ExportError> {
    let state = StateDb::open_default().map_err(|message| ExportError::Operational { message })?;
    let config_root = dirs::config_dir()
        .map(|d| d.join("oulipoly-agent-runner"))
        .unwrap_or_else(|| PathBuf::from("."));
    let model_repo = FilesystemModelConfigRepository::new(default_models_dir());
    let provider_source = FilesystemProviderConfigSource::new(config_root.join("providers.toml"));
    let sessions_source = FilesystemSessionsConfigSource::new(config_root.join("sessions.toml"));
    let runner = OsProcessRunner;
    resolve_export_session_metadata_with_deps(
        &state,
        &model_repo,
        &provider_source,
        &sessions_source,
        &runner,
        session_id,
    )
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
    let payload = serde_json::json!({
        "error": {
            "code": metadata_error_code(err),
            "message": metadata_error_message(err),
        }
    });
    eprintln!("{payload}");
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
    let payload = serde_json::json!({
        "error": {
            "code": code,
            "message": message,
        }
    });
    eprintln!("{payload}");
}

fn resolve_agent(cli: &Cli) -> Result<AgentConfig, String> {
    let agents_dir = cli.agents_dir.clone().unwrap_or_else(|| {
        dirs::config_dir()
            .map(|d| d.join("oulipoly-agent-runner").join("agents"))
            .unwrap_or_else(|| PathBuf::from("agents"))
    });
    let agent_repo = FilesystemAgentConfigRepository::new(agents_dir);

    if let Some(ref path) = cli.agent_file {
        return agent_repo.load_agent_file(path);
    }

    if let Some(ref name) = cli.agent {
        let agents = agent_repo.load_agents()?;
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

fn ingest_and_emit_session_id(
    state: &StateDb,
    sessions_cfg: &agent_runner_lib::config::SessionsConfig,
    provider_name: &str,
    invocation_row_id: i64,
    invocation_uuid: &str,
    capture_method: &str,
) -> bool {
    let invocation = match state.get_invocation_by_uuid(invocation_uuid) {
        Ok(Some(row)) => row,
        Ok(None) => {
            eprintln!("Warning: Could not resolve invocation {invocation_uuid} for session ingest");
            return false;
        }
        Err(err) => {
            eprintln!(
                "Warning: Failed to load invocation {invocation_uuid} for session ingest: {err}"
            );
            return false;
        }
    };
    let Some(finished_at) = invocation.finished_at else {
        eprintln!("Warning: Invocation {invocation_uuid} was not finalized before session ingest");
        return false;
    };

    let report = agent_runner_lib::sessions::scan_provider(provider_name, sessions_cfg, state);
    for err in report.errors {
        eprintln!("Warning: Session ingest failed for {provider_name}: {err}");
    }

    let session_id = match state.find_session_for_invocation_window(
        provider_name,
        &invocation.created_at,
        &finished_at,
    ) {
        Ok(Some(session_id)) => session_id,
        Ok(None) => return false,
        Err(err) => {
            eprintln!("Warning: Failed to resolve session for invocation {invocation_uuid}: {err}");
            return false;
        }
    };

    emit_known_session_id(
        state,
        invocation_row_id,
        invocation_uuid,
        session_id.as_str(),
        capture_method,
    )
}

fn emit_known_session_id(
    state: &StateDb,
    invocation_row_id: i64,
    invocation_uuid: &str,
    session_id: &str,
    capture_method: &str,
) -> bool {
    if let Err(err) =
        state.update_session_capture(invocation_row_id, Some(session_id), capture_method)
    {
        eprintln!("Warning: Failed to update invocation session_id: {err}");
        return false;
    }
    if let Err(err) = state.mint_chain_for_invocation_session(invocation_row_id) {
        eprintln!("Warning: Failed to mint session chain: {err}");
    }
    let payload = serde_json::json!({
        "id": invocation_uuid,
        "session_id": session_id,
    });
    eprintln!("OULIPOLY_SESSION={payload}");
    true
}

/// The short `[resume] -> <provider>` line is always emitted regardless of
/// TTY (per proposal §5: V10 wins over V15 here — even at a terminal, the
/// runner's selection must be visible). Factored as a helper so the
/// "always-on" semantic has an explicit, unit-testable surface that mirrors
/// `should_emit_invocation_line`.
fn should_emit_resume_short_line(_is_terminal: bool) -> bool {
    true
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

fn format_resume_error(err: agent_runner_lib::state::ResumeError) -> String {
    use agent_runner_lib::state::ResumeError;
    match err {
        ResumeError::InvalidUuid { input } => format!("invalid session UUID: {input}"),
        ResumeError::NoChainFound { input } => format!(
            "No session found matching {input}. Check that session ingestion is configured and that the provider still has resumable local state."
        ),
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
    resolved: &agent_runner_lib::state::ResolvedResume,
    providers_cfg: &ProvidersConfig,
) -> Result<ResumeExecutionTarget, agent_runner_lib::state::ResumeError> {
    if let Some(model) = resolved.model.as_ref() {
        let provider_index = model
            .providers
            .iter()
            .position(|provider| provider.name == resolved.active_provider)
            .ok_or_else(
                || agent_runner_lib::state::ResumeError::ProviderModelMismatch {
                    model_name: model.name.clone(),
                    active_provider: resolved.active_provider.clone(),
                    suggestions: Vec::new(),
                },
            )?;
        let (provider, prompt_mode) = providers_cfg
            .effective_provider(&model.providers[provider_index])
            .map_err(|message| agent_runner_lib::state::ResumeError::Db { message })?;
        Ok(ResumeExecutionTarget {
            model: Some(model.clone()),
            provider_index,
            provider,
            prompt_mode,
        })
    } else {
        let (provider, prompt_mode) = providers_cfg
            .runtime_provider_with_mode(&resolved.active_provider)
            .map_err(|message| agent_runner_lib::state::ResumeError::Db { message })?;
        let provider_index =
            provider_index_in_providers_cfg(providers_cfg, &resolved.active_provider);
        Ok(ResumeExecutionTarget {
            model: None,
            provider_index,
            provider,
            prompt_mode,
        })
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

fn run_pause_handshake(session_id: &str, ttl_ms: Option<u64>) -> Result<i32, String> {
    if Uuid::parse_str(session_id).is_err() {
        return Ok(emit_json_error(
            2,
            "invalid-session-id",
            format!("invalid session UUID: {session_id}"),
        ));
    }

    let ttl_ms = ttl_ms.unwrap_or(DEFAULT_PAUSE_HANDSHAKE_TTL_MS);
    if ttl_ms > MAX_PAUSE_HANDSHAKE_TTL_MS {
        return Ok(emit_json_error(
            2,
            "invalid-ttl",
            format!("ttl-ms must be at most {MAX_PAUSE_HANDSHAKE_TTL_MS}"),
        ));
    }

    let state = match StateDb::open_default() {
        Ok(state) => state,
        Err(message) => return Ok(emit_json_error(1, "operational-error", message)),
    };
    let models = match load_models(&default_models_dir()) {
        Ok(models) => models,
        Err(message) => return Ok(emit_json_error(1, "operational-error", message)),
    };
    let resolved = match state.resolve_resume(&models, session_id, None) {
        Ok(resolved) => resolved,
        Err(err) => return Ok(emit_resume_resolution_error(err)),
    };
    let lock_dir = match default_lock_dir() {
        Ok(lock_dir) => lock_dir,
        Err(message) => return Ok(emit_json_error(1, "operational-error", message)),
    };
    let lock = match SessionLock::new(&lock_dir) {
        Ok(lock) => lock,
        Err(err) => {
            return Ok(emit_json_error(
                1,
                "operational-error",
                format!("failed to open locks: {err}"),
            ));
        }
    };
    match lock.acquire(
        &resolved.active_session_id,
        &resolved.active_provider,
        std::time::Duration::from_millis(ttl_ms),
    ) {
        Ok(lease) => {
            let payload = serde_json::json!({
                "session_id": lease.session_id,
                "chain_id": resolved.chain_id,
                "provider_name": lease.provider_name,
                "token": lease.token,
                "expires_at": lease.expires_at,
                "lock_path": lease.lock_path,
            });
            println!(
                "{}",
                serde_json::to_string(&payload)
                    .map_err(|err| format!("failed to encode pause receipt: {err}"))?
            );
            Ok(0)
        }
        Err(err) => Ok(emit_lock_error(err)),
    }
}

fn run_resume_handshake(session_id: &str, token: &str) -> Result<i32, String> {
    if Uuid::parse_str(session_id).is_err() {
        return Ok(emit_json_error(
            2,
            "invalid-session-id",
            format!("invalid session UUID: {session_id}"),
        ));
    }

    if let Err(message) = StateDb::open_default() {
        return Ok(emit_json_error(1, "operational-error", message));
    }
    let lock_dir = match default_lock_dir() {
        Ok(lock_dir) => lock_dir,
        Err(message) => return Ok(emit_json_error(1, "operational-error", message)),
    };
    let lock = match SessionLock::new(&lock_dir) {
        Ok(lock) => lock,
        Err(err) => {
            return Ok(emit_json_error(
                1,
                "operational-error",
                format!("failed to open locks: {err}"),
            ));
        }
    };
    match lock.release(session_id, token) {
        Ok(receipt) => {
            println!(
                "{}",
                serde_json::to_string(&receipt)
                    .map_err(|err| format!("failed to encode resume receipt: {err}"))?
            );
            Ok(0)
        }
        Err(err) => Ok(emit_lock_error(err)),
    }
}

fn default_lock_dir() -> Result<PathBuf, String> {
    dirs::data_dir()
        .map(|dir| dir.join("oulipoly-agent-runner").join("locks"))
        .ok_or_else(|| "Could not determine data directory".to_string())
}

fn emit_resume_resolution_error(err: agent_runner_lib::state::ResumeError) -> i32 {
    use agent_runner_lib::state::ResumeError;
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
    let payload = serde_json::json!({
        "error": {
            "code": error_code,
            "message": message.into(),
        }
    });
    let _ = writeln!(std::io::stderr(), "{payload}");
    code
}

fn effective_model_for_execution(
    model: &ModelConfig,
    provider_index: usize,
    providers_cfg: &ProvidersConfig,
) -> Result<(ProviderConfig, PromptMode), String> {
    providers_cfg.effective_provider(&model.providers[provider_index])
}

fn resume_migration_pool(
    resolved: &agent_runner_lib::state::ResolvedResume,
    providers_cfg: &ProvidersConfig,
) -> ModelConfig {
    if let Some(model) = resolved.model.as_ref() {
        let mut effective = model.clone();
        effective.providers = model
            .providers
            .iter()
            .filter_map(|provider| providers_cfg.effective_provider(provider).ok().map(|p| p.0))
            .collect();
        return effective;
    }

    let mut names = providers_cfg.entries.keys().cloned().collect::<Vec<_>>();
    names.sort();
    let mut providers = Vec::new();
    for name in names {
        let is_candidate = name == resolved.active_provider
            || providers_cfg
                .get(&name)
                .is_some_and(|entry| entry.session_storage.is_some());
        if is_candidate && let Ok((provider, _)) = providers_cfg.runtime_provider_with_mode(&name) {
            providers.push(provider);
        }
    }
    ModelConfig {
        name: "<provider-default>".to_string(),
        prompt_mode: PromptMode::Stdin,
        providers,
        inputs: Vec::new(),
    }
}

fn run_repl(
    model_name: Option<&str>,
    resume: Option<&str>,
    manual_migrate: Option<&str>,
    working_dir: Option<&Path>,
    models_dir_override: Option<&Path>,
) -> Result<i32, String> {
    let state = StateDb::open_default()?;
    let models_dir = models_dir_override
        .map(Path::to_path_buf)
        .unwrap_or_else(default_models_dir);
    let models = load_models(&models_dir)?;
    let config_root = dirs::config_dir()
        .map(|d| d.join("oulipoly-agent-runner"))
        .unwrap_or_else(|| PathBuf::from("."));
    let providers_path = config_root.join("providers.toml");
    let sessions_path = config_root.join("sessions.toml");
    let providers_cfg =
        agent_runner_lib::config::ProvidersConfig::load(&providers_path).unwrap_or_default();
    let sessions_cfg =
        agent_runner_lib::config::SessionsConfig::load(&sessions_path).unwrap_or_default();
    let mut resolved_resume = if let Some(session_id) = resume {
        Some(
            match state.resolve_resume(&models, session_id, model_name) {
                Ok(resolved) => resolved,
                Err(agent_runner_lib::state::ResumeError::ProviderModelMismatch {
                    active_provider,
                    ..
                }) => {
                    return Err(resume_model_pool_mismatch_message(
                        &models,
                        model_name.unwrap_or("<unknown>"),
                        session_id,
                        &active_provider,
                    ));
                }
                Err(err) => return Err(format_resume_error(err)),
            },
        )
    } else {
        None
    };
    let mut fallback_target = match resolved_resume.as_ref() {
        Some(resolved) => {
            Some(resume_execution_target(resolved, &providers_cfg).map_err(format_resume_error)?)
        }
        None => None,
    };
    let direct_model = if fallback_target.is_none() {
        let model_name =
            model_name.ok_or_else(|| "model is required unless --resume is present".to_string())?;
        Some(
            models
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
        });

    let in_flight = agent_runner_lib::quota::InFlight::new();
    let runner = OsProcessRunner;
    let effects = CliBalanceEffects {
        providers_cfg: &providers_cfg,
        sessions_cfg: &sessions_cfg,
        in_flight: &in_flight,
        state: &state,
        runner: &runner,
    };

    let parent_invocation_id = resolve_parent_invocation_id(&state);
    let stderr_is_terminal = std::io::stderr().is_terminal();
    let (provider_index, provider, resume_session_id) = if let Some(resolved) =
        resolved_resume.as_mut()
    {
        let selected_provider = &resolved.active_provider;
        if should_emit_resume_short_line(stderr_is_terminal) {
            eprintln!("[resume] -> {selected_provider}");
        }
        let migration_model = resume_migration_pool(resolved, &providers_cfg);
        if let Ok(balancer::MigrationDecision::Migrate {
            target_provider_index,
            reason,
        }) = balancer::decide_migration(&state, &migration_model, resolved, manual_migrate)
        {
            let mut stderr = std::io::stderr();
            match agent_runner_lib::migration::migrate_chain_segment(
                &state,
                &sessions_cfg,
                &migration_model,
                resolved,
                target_provider_index,
                reason,
                &mut stderr,
            ) {
                Ok(migrated) => {
                    resolved.active_provider = migrated.target_provider.clone();
                    resolved.active_session_id = migrated.target_session_id.clone();
                    fallback_target = Some(
                        resume_execution_target(resolved, &providers_cfg)
                            .map_err(format_resume_error)?,
                    );
                }
                Err(err) => {
                    eprintln!("migration failed: {err:?}");
                    return Ok(1);
                }
            }
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
        let provider_index = balancer::select_provider(&model, &state, Some(&effects));
        let (provider, _) = effective_model_for_execution(&model, provider_index, &providers_cfg)?;
        (provider_index, provider, None)
    };
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
    let invocation_row_id = state.start_invocation(&InvocationStart {
        invocation_uuid: invocation.id.clone(),
        model_name: invocation_model_name,
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

    let resume_payload = resume_session_id.as_deref().map(|session_id| {
        let strategy = provider
            .resume
            .as_ref()
            .expect("resumable provider must have a resume strategy");
        executor::cli::ResumePayload {
            session_id,
            strategy,
            target_jsonl_path: None,
        }
    });

    match executor::cli::execute_interactive(
        &provider,
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
            if exit_code == 0 {
                let emitted = ingest_and_emit_session_id(
                    &state,
                    &sessions_cfg,
                    &provider.name,
                    invocation_row_id,
                    &invocation.id,
                    if resume.is_some() {
                        "resumed"
                    } else {
                        "turn_script"
                    },
                );
                if !emitted && let Some(session_id) = resume {
                    emit_known_session_id(
                        &state,
                        invocation_row_id,
                        &invocation.id,
                        session_id,
                        "resumed",
                    );
                }
            }
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

fn run_resume(
    model_name: Option<&str>,
    session_id: &str,
    manual_migrate: Option<&str>,
    prompt: Option<&str>,
    file: Option<&Path>,
    working_dir: Option<&Path>,
    models_dir_override: Option<&Path>,
) -> Result<i32, String> {
    if Uuid::parse_str(session_id).is_err() {
        eprintln!("invalid session UUID: {session_id}");
        return Ok(1);
    }

    let answer = resolve_resume_answer(prompt, file)?;
    let state = StateDb::open_default()?;
    let models_dir = models_dir_override
        .map(Path::to_path_buf)
        .unwrap_or_else(default_models_dir);
    let models = load_models(&models_dir)?;
    let config_root = dirs::config_dir()
        .map(|d| d.join("oulipoly-agent-runner"))
        .unwrap_or_else(|| PathBuf::from("."));
    let providers_path = config_root.join("providers.toml");
    let sessions_path = config_root.join("sessions.toml");
    let providers_cfg =
        agent_runner_lib::config::ProvidersConfig::load(&providers_path).unwrap_or_default();
    let sessions_cfg =
        agent_runner_lib::config::SessionsConfig::load(&sessions_path).unwrap_or_default();

    let stderr_is_terminal = std::io::stderr().is_terminal();
    let mut resolved = match state.resolve_resume(&models, session_id, model_name) {
        Ok(resolved) => resolved,
        Err(agent_runner_lib::state::ResumeError::ProviderModelMismatch {
            active_provider,
            ..
        }) => {
            eprintln!(
                "{}",
                resume_model_pool_mismatch_message(
                    &models,
                    model_name.unwrap_or("<unknown>"),
                    session_id,
                    &active_provider,
                )
            );
            return Ok(1);
        }
        Err(err) => {
            eprintln!("{}", format_resume_error(err));
            return Ok(1);
        }
    };
    let mut target = match resume_execution_target(&resolved, &providers_cfg) {
        Ok(target) => target,
        Err(err) => {
            eprintln!("{}", format_resume_error(err));
            return Ok(1);
        }
    };
    let selected_provider = &resolved.active_provider;
    if should_emit_resume_short_line(stderr_is_terminal) {
        eprintln!("[resume] -> {selected_provider}");
    }
    let migration_model = resume_migration_pool(&resolved, &providers_cfg);
    if let Ok(balancer::MigrationDecision::Migrate {
        target_provider_index,
        reason,
    }) = balancer::decide_migration(&state, &migration_model, &resolved, manual_migrate)
    {
        let mut stderr = std::io::stderr();
        match agent_runner_lib::migration::migrate_chain_segment(
            &state,
            &sessions_cfg,
            &migration_model,
            &resolved,
            target_provider_index,
            reason,
            &mut stderr,
        ) {
            Ok(migrated) => {
                resolved.active_provider = migrated.target_provider.clone();
                resolved.active_session_id = migrated.target_session_id.clone();
                target = match resume_execution_target(&resolved, &providers_cfg) {
                    Ok(target) => target,
                    Err(err) => {
                        eprintln!("{}", format_resume_error(err));
                        return Ok(1);
                    }
                };
            }
            Err(err) => {
                eprintln!("migration failed: {err:?}");
                return Ok(1);
            }
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

    let parent_invocation_id = resolve_parent_invocation_id(&state);
    let invocation = CompositeInvocationId {
        source: provider.name.clone(),
        id: Uuid::new_v4().to_string(),
    };
    let invocation_model_name = resolved
        .model_name
        .clone()
        .unwrap_or_else(|| "<unknown>".to_string());
    let invocation_row_id = state.start_invocation(&InvocationStart {
        invocation_uuid: invocation.id.clone(),
        model_name: invocation_model_name,
        provider_name: provider.name.clone(),
        provider_index,
        parent_invocation_id,
    })?;
    let mut guard = FinalizerGuard::new(&state, invocation_row_id);
    state.update_session_capture(invocation_row_id, Some(session_id), "resumed")?;

    let invocation_env = serde_json::to_string(&invocation)
        .map_err(|e| format!("Failed to serialize invocation id: {e}"))?;
    eprintln!("{}", invocation.stderr_line());

    let result = match executor::cli::execute_resume(
        &provider,
        provider_index,
        target.prompt_mode,
        &answer,
        working_dir,
        Some(&invocation_env),
        executor::cli::ResumePayload {
            session_id: &resolved.active_session_id,
            strategy,
            target_jsonl_path: None,
        },
    ) {
        Ok(result) => result,
        Err(spawn_err) => {
            state.finalize_invocation(
                invocation_row_id,
                false,
                1,
                Some("spawn_error"),
                Some(&spawn_err),
            )?;
            guard.mark_finalized();
            return Ok(1);
        }
    };

    if let Some(acceptance) = &result.resume_acceptance {
        state.update_resume_acceptance(
            invocation_row_id,
            acceptance.status.db_value(),
            acceptance.evidence.as_deref(),
        )?;
    }

    let success = result.exit_code == 0;
    let error_category = if !success {
        run_diagnostics(&result.stderr, result.exit_code, &models, working_dir)
    } else {
        None
    };
    state.finalize_invocation(
        invocation_row_id,
        success,
        result.exit_code,
        error_category.as_deref(),
        if success { None } else { Some(&result.stderr) },
    )?;
    guard.mark_finalized();

    if success {
        let emitted = ingest_and_emit_session_id(
            &state,
            &sessions_cfg,
            &provider.name,
            invocation_row_id,
            &invocation.id,
            "resumed",
        );
        if !emitted {
            emit_known_session_id(
                &state,
                invocation_row_id,
                &invocation.id,
                session_id,
                "resumed",
            );
        }
        let _ = std::io::stdout().write_all(&result.stdout);
    } else {
        eprintln!("{}", result.stderr);
        if let Some(ref cat) = error_category {
            eprintln!("[diagnostics: {cat}]");
        }
    }
    Ok(result.exit_code)
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
    let runner = OsProcessRunner;
    let effects = CliBalanceEffects {
        providers_cfg: &providers_cfg,
        sessions_cfg: &sessions_cfg,
        in_flight: &in_flight,
        state: &state,
        runner: &runner,
    };
    // Resolve parent invocation BEFORE provider selection so the provider
    // selection itself can be attributed to a parent context if needed
    // (matches contract `tmp/01-pr-a-contract.md` lifecycle ordering).
    let parent_invocation_id = resolve_parent_invocation_id(&state);
    let provider_index = balancer::select_provider(model, &state, Some(&effects));
    let (provider, prompt_mode) =
        effective_model_for_execution(model, provider_index, &providers_cfg)?;
    let provider_name = &provider.name;
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

    let result = match executor::execute_effective_with_inputs_and_env(
        executor::cli::EffectiveExecuteRequest {
            model,
            provider: &provider,
            provider_index,
            prompt_mode,
            prompt,
            working_dir,
            extra_inputs,
            parent_invocation_env: Some(&invocation_env),
        },
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
    if error_category.as_deref() == Some(diagnostics::ErrorCategory::QuotaExhausted.as_str()) {
        state
            .mark_exhausted(provider_name)
            .unwrap_or_else(|e| eprintln!("Warning: Failed to mark provider exhausted: {e}"));
    }

    state
        .finalize_invocation(
            invocation_row_id,
            success,
            result.exit_code,
            error_category.as_deref(),
            if success { None } else { Some(&result.stderr) },
        )
        .unwrap_or_else(|e| eprintln!("Warning: Failed to finalize invocation: {e}"));

    if success {
        let emitted = ingest_and_emit_session_id(
            &state,
            &sessions_cfg,
            provider_name,
            invocation_row_id,
            &invocation.id,
            "turn_script",
        );
        if !emitted && let Some(session_id) = result.session_capture.session_id.as_deref() {
            emit_known_session_id(
                &state,
                invocation_row_id,
                &invocation.id,
                session_id,
                result.session_capture.method.db_value(),
            );
        }
    }

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

fn run_migrate_db() -> Result<i32, String> {
    let state = StateDb::open_default()?;
    let report = state.backfill_session_chains()?;
    println!(
        "session chain backfill: chains={} segments={} skipped_existing={}",
        report.chains_inserted, report.segments_inserted, report.skipped_existing
    );
    let compaction_report = run_compaction_backfill(&state)?;
    println!(
        "compaction backfill: {} turns flagged across {} sessions",
        compaction_report.turns_flagged, compaction_report.sessions_processed
    );
    Ok(0)
}

#[derive(Debug, Clone, PartialEq)]
struct ConfigMigrationReport {
    providers_touched: usize,
    model_files_rewritten: usize,
    moved_blocks: Vec<String>,
}

fn run_migrate_config(models_dir_override: Option<&Path>) -> Result<i32, String> {
    let models_dir = models_dir_override
        .map(Path::to_path_buf)
        .unwrap_or_else(default_models_dir);
    let config_root = models_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let providers_path = config_root.join("providers.toml");
    let report = migrate_config_files(&models_dir, &providers_path)?;
    println!(
        "migrate-config: providers_touched={} model_files_rewritten={}",
        report.providers_touched, report.model_files_rewritten
    );
    for moved in &report.moved_blocks {
        println!("  moved {moved}");
    }
    Ok(0)
}

fn migrate_config_files(
    models_dir: &Path,
    providers_path: &Path,
) -> Result<ConfigMigrationReport, String> {
    let mut providers_root = if providers_path.exists() {
        std::fs::read_to_string(providers_path)
            .map_err(|e| format!("Failed to read {}: {e}", providers_path.display()))?
            .parse::<toml::Table>()
            .map_err(|e| format!("TOML parse error in {}: {e}", providers_path.display()))?
    } else {
        toml::Table::new()
    };
    let mut moved_blocks = Vec::new();
    let mut rewritten = 0usize;

    let mut model_paths = if models_dir.exists() {
        std::fs::read_dir(models_dir)
            .map_err(|e| format!("Failed to read {}: {e}", models_dir.display()))?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    model_paths.sort();

    for path in model_paths {
        let original = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
        let mut table = original
            .parse::<toml::Table>()
            .map_err(|e| format!("TOML parse error in {}: {e}", path.display()))?;
        let before = toml::to_string_pretty(&table)
            .map_err(|e| format!("Failed to serialize {}: {e}", path.display()))?;

        let mut changed = false;
        let global_prompt_mode = table.remove("prompt_mode");
        changed |= global_prompt_mode.is_some();

        if table.contains_key("command") {
            let provider_table = old_top_level_provider_table(&mut table)?;
            let migrated = migrate_provider_table(
                provider_table,
                global_prompt_mode.clone(),
                &mut providers_root,
                &path,
                &mut moved_blocks,
            )?;
            table.insert("providers".to_string(), toml::Value::Array(vec![migrated]));
            changed = true;
        } else if let Some(toml::Value::Array(providers)) = table.get_mut("providers") {
            for provider in providers.iter_mut() {
                let migrated = migrate_provider_table(
                    provider.clone(),
                    global_prompt_mode.clone(),
                    &mut providers_root,
                    &path,
                    &mut moved_blocks,
                )?;
                if migrated != *provider {
                    *provider = migrated;
                    changed = true;
                }
            }
        }

        let after = toml::to_string_pretty(&table)
            .map_err(|e| format!("Failed to serialize {}: {e}", path.display()))?;
        if changed && after != before {
            std::fs::write(&path, after)
                .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;
            rewritten += 1;
        }
    }

    let providers_text = toml::to_string_pretty(&providers_root)
        .map_err(|e| format!("Failed to serialize {}: {e}", providers_path.display()))?;
    let current = if providers_path.exists() {
        std::fs::read_to_string(providers_path)
            .map_err(|e| format!("Failed to read {}: {e}", providers_path.display()))?
    } else {
        String::new()
    };
    let providers_touched = providers_root
        .iter()
        .filter(|(_, value)| {
            value
                .as_table()
                .is_some_and(|table| table.contains_key("command"))
        })
        .count();
    if providers_text != current {
        if let Some(parent) = providers_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
        }
        std::fs::write(providers_path, providers_text)
            .map_err(|e| format!("Failed to write {}: {e}", providers_path.display()))?;
    }

    Ok(ConfigMigrationReport {
        providers_touched,
        model_files_rewritten: rewritten,
        moved_blocks,
    })
}

fn old_top_level_provider_table(table: &mut toml::Table) -> Result<toml::Value, String> {
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
    if !provider.contains_key("command") {
        return Err("old model provider is missing command".to_string());
    }
    Ok(toml::Value::Table(provider))
}

fn migrate_provider_table(
    provider_value: toml::Value,
    global_prompt_mode: Option<toml::Value>,
    providers_root: &mut toml::Table,
    path: &Path,
    moved_blocks: &mut Vec<String>,
) -> Result<toml::Value, String> {
    let mut provider = provider_value
        .as_table()
        .cloned()
        .ok_or_else(|| format!("provider entry in {} is not a table", path.display()))?;
    let original_provider = provider.clone();
    let has_runtime_blocks = provider.contains_key("command")
        || provider.contains_key("resume")
        || provider.contains_key("session_capture")
        || provider.contains_key("session_storage")
        || provider.contains_key("resume_acceptance")
        || provider.contains_key("prompt_mode");

    let command = provider
        .remove("command")
        .map(|value| {
            value.as_str().map(ToString::to_string).ok_or_else(|| {
                format!(
                    "command in old per-provider config in {} must be a string",
                    path.display()
                )
            })
        })
        .transpose()?;
    let model_args = take_string_array(&mut provider, "args")?;
    let model_interactive_args = take_optional_string_array(&mut provider, "interactive_args")?;
    let provider_name = provider
        .remove("name")
        .and_then(|value| value.as_str().map(ToString::to_string))
        .or_else(|| {
            command
                .as_deref()
                .map(|command| derive_migration_provider_name(command, &model_args))
        });
    if !has_runtime_blocks
        && provider_name
            .as_ref()
            .and_then(|name| providers_root.get(name))
            .is_none()
    {
        return Ok(toml::Value::Table(original_provider));
    }
    let provider_name = provider_name.ok_or_else(|| {
        format!(
            "Old per-provider config in {} is missing command; run `agents migrate-config` after adding it.",
            path.display()
        )
    })?;

    let command_parts = command
        .as_deref()
        .map(executor::cli::shell_split)
        .unwrap_or_default();
    let runtime_command = command.as_ref().map(|command| {
        command_parts
            .first()
            .cloned()
            .unwrap_or_else(|| command.clone())
    });
    let command_runtime_args = command_parts.iter().skip(1).cloned().collect::<Vec<_>>();
    let (mut runtime_args, model_args) = partition_model_specific_args(model_args);
    let had_interactive_args = model_interactive_args.is_some();
    let (runtime_interactive_args, model_interactive_args) = model_interactive_args
        .map(partition_model_specific_args)
        .map(|(runtime, model)| (Some(runtime), Some(model)))
        .unwrap_or((None, None));
    if !command_runtime_args.is_empty() {
        let mut combined = command_runtime_args.clone();
        combined.extend(runtime_args);
        runtime_args = combined;
    }

    let prompt_mode = provider
        .remove("prompt_mode")
        .or(global_prompt_mode)
        .unwrap_or_else(|| toml::Value::String("stdin".to_string()));
    let resume = provider.remove("resume");
    let session_capture = provider.remove("session_capture");
    let session_storage = provider.remove("session_storage");
    let resume_acceptance = provider.remove("resume_acceptance");

    let runtime = providers_root
        .entry(provider_name.clone())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let runtime = runtime
        .as_table_mut()
        .ok_or_else(|| format!("providers.toml entry [{provider_name}] is not a table"))?;
    if let Some(runtime_command) = runtime_command {
        set_or_repair_empty_array(
            runtime,
            "command",
            toml::Value::String(runtime_command),
            &provider_name,
            path,
        )?;
    }
    if has_runtime_blocks || !runtime_args.is_empty() {
        set_or_repair_empty_array(
            runtime,
            "args",
            string_array_value(runtime_args),
            &provider_name,
            path,
        )?;
    }
    if had_interactive_args
        || runtime_interactive_args
            .as_ref()
            .is_some_and(|args| !args.is_empty())
    {
        let mut combined = command_runtime_args;
        if let Some(runtime_interactive_args) = runtime_interactive_args {
            combined.extend(runtime_interactive_args);
        }
        set_or_repair_empty_array(
            runtime,
            "interactive_args",
            string_array_value(combined),
            &provider_name,
            path,
        )?;
    }
    if has_runtime_blocks {
        set_or_conflict(runtime, "prompt_mode", prompt_mode, &provider_name, path)?;
    }
    for (key, value) in [
        ("resume", resume),
        ("session_capture", session_capture),
        ("session_storage", session_storage),
        ("resume_acceptance", resume_acceptance),
    ] {
        if let Some(value) = value {
            set_or_conflict(runtime, key, value, &provider_name, path)?;
            moved_blocks.push(format!(
                "{}.{} -> providers.toml[{provider_name}]",
                path.display(),
                key
            ));
        }
    }

    let mut reduced = toml::Table::new();
    reduced.insert("name".to_string(), toml::Value::String(provider_name));
    reduced.insert("args".to_string(), string_array_value(model_args));
    if let Some(interactive_args) = model_interactive_args {
        reduced.insert(
            "interactive_args".to_string(),
            string_array_value(interactive_args),
        );
    }
    Ok(toml::Value::Table(reduced))
}

fn set_or_conflict(
    table: &mut toml::Table,
    key: &str,
    value: toml::Value,
    provider_name: &str,
    path: &Path,
) -> Result<(), String> {
    if let Some(existing) = table.get(key) {
        if existing != &value {
            return Err(format!(
                "conflicting {key} for provider {provider_name} while migrating {}: existing providers.toml value {existing:?}, model TOML value {value:?}",
                path.display()
            ));
        }
        return Ok(());
    }
    table.insert(key.to_string(), value);
    Ok(())
}

fn set_or_repair_empty_array(
    table: &mut toml::Table,
    key: &str,
    value: toml::Value,
    provider_name: &str,
    path: &Path,
) -> Result<(), String> {
    if matches!(table.get(key), Some(toml::Value::Array(existing)) if existing.is_empty())
        && !matches!(&value, toml::Value::Array(value) if value.is_empty())
    {
        table.insert(key.to_string(), value);
        return Ok(());
    }
    set_or_conflict(table, key, value, provider_name, path)
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
    let command_parts = executor::cli::shell_split(command);
    let Some(command) = command_parts.first() else {
        return command.to_string();
    };
    let mut derived_args = command_parts.iter().skip(1).cloned().collect::<Vec<_>>();
    derived_args.extend(args.iter().cloned());
    agent_runner_lib::config::derive_provider_name(command, &derived_args)
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

fn run_resume_list(uuid: &str) -> Result<i32, String> {
    Uuid::parse_str(uuid).map_err(|e| format!("invalid session UUID: {uuid}: {e}"))?;
    let state = StateDb::open_default()?;
    let previews = state
        .resume_previews(uuid)
        .map_err(|e| format!("Failed to list resume chains: {e}"))?;
    if previews.is_empty() {
        println!("No chains found for {uuid}");
        return Ok(0);
    }
    for preview in previews {
        println!("{}", format_resume_list_line(&preview));
    }
    Ok(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompactionBackfillReport {
    turns_flagged: u64,
    sessions_processed: u64,
}

fn run_compaction_backfill(state: &StateDb) -> Result<CompactionBackfillReport, String> {
    let config_root = dirs::config_dir()
        .map(|d| d.join("oulipoly-agent-runner"))
        .unwrap_or_else(|| PathBuf::from("."));
    let sessions_path = config_root.join("sessions.toml");
    let sessions_cfg = agent_runner_lib::config::SessionsConfig::load(&sessions_path)
        .map_err(|e| format!("Failed to load {}: {e}", sessions_path.display()))?;
    let models_dir = default_models_dir();
    let models = if models_dir.is_dir() {
        load_models(&models_dir)?
    } else {
        HashMap::new()
    };

    let mut report = CompactionBackfillReport {
        turns_flagged: 0,
        sessions_processed: 0,
    };
    for (provider_name, session_id) in state.distinct_chain_segments()? {
        let Some(path) =
            locate_compaction_backfill_source(&provider_name, &session_id, &sessions_cfg, &models)
        else {
            continue;
        };
        let flagged =
            flag_compaction_boundaries_from_jsonl(state, &provider_name, &session_id, &path)?;
        report.turns_flagged += flagged;
        report.sessions_processed += 1;
        println!(
            "compaction backfill session: provider={} session_id={} flagged={}",
            provider_name, session_id, flagged
        );
    }
    Ok(report)
}

fn locate_compaction_backfill_source(
    provider_name: &str,
    session_id: &str,
    sessions_cfg: &agent_runner_lib::config::SessionsConfig,
    models: &HashMap<String, ModelConfig>,
) -> Option<PathBuf> {
    if let Ok(Some(path)) =
        agent_runner_lib::sessions::locate_transcript(sessions_cfg, provider_name, session_id)
        && path.exists()
    {
        return Some(path);
    }

    models
        .values()
        .flat_map(|model| model.providers.iter())
        .filter(|provider| provider.name == provider_name)
        .find_map(|provider| {
            agent_runner_lib::migration::find_claude_source_from_storage(provider, session_id)
        })
        .filter(|path| path.exists())
}

fn flag_compaction_boundaries_from_jsonl(
    state: &StateDb,
    provider_name: &str,
    session_id: &str,
    path: &Path,
) -> Result<u64, String> {
    let file = std::fs::File::open(path)
        .map_err(|e| format!("Failed to open compaction source {}: {e}", path.display()))?;
    let mut flagged = 0u64;
    for line in std::io::BufReader::new(file).lines() {
        let line = line.map_err(|e| {
            format!(
                "Failed to read compaction source line from {}: {e}",
                path.display()
            )
        })?;
        let Ok(obj) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if obj
            .get("isCompactSummary")
            .and_then(|value| value.as_bool())
            != Some(true)
        {
            continue;
        }
        let Some(turn_id) = obj.get("uuid").and_then(|value| value.as_str()) else {
            continue;
        };
        if state.flag_compaction_boundary(provider_name, session_id, turn_id)? {
            flagged += 1;
        }
    }
    Ok(flagged)
}

fn format_resume_list_line(preview: &agent_runner_lib::state::ChainPreview) -> String {
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
    if args.len() >= 4
        && args.get(1).is_some_and(|arg| arg == "resume")
        && args.get(2).is_some_and(|arg| arg == "--list")
    {
        let mut normalized = Vec::with_capacity(args.len() - 1);
        normalized.push(args[0].clone());
        normalized.push("resume-list".to_string());
        normalized.push(args[3].clone());
        normalized.extend(args.into_iter().skip(4));
        normalized
    } else {
        args
    }
}

fn main() -> ExitCode {
    if std::env::args().len() <= 1 {
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

    fn providers_cfg_with_storage(names: &[&str]) -> ProvidersConfig {
        let mut cfg = ProvidersConfig::default();
        for name in names {
            cfg.entries.insert(
                (*name).to_string(),
                agent_runner_lib::config::ProviderEntry {
                    command: Some((*name).to_string()),
                    session_storage: Some(agent_runner_lib::config::SessionStorage::ClaudeCode {
                        projects_dir: PathBuf::from(format!("/tmp/{name}/projects")),
                    }),
                    resume: Some(agent_runner_lib::config::ResumeStrategy {
                        kind: agent_runner_lib::config::ResumeKind::Flag,
                        flag: Some("--resume".to_string()),
                        subcommand: None,
                    }),
                    ..agent_runner_lib::config::ProviderEntry::default()
                },
            );
        }
        cfg
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
                migrate: _,
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
                migrate: _,
                prompt,
                file,
                project,
                models_dir,
            }) => {
                assert_eq!(model.as_deref(), Some(REPL_MODEL));
                assert_eq!(parsed_session, session_id);
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

    // risk: CLI surface; level: unit; source: proposal §11.1 CLI surface / A8.
    #[test]
    fn top_level_resume_parse_allows_missing_model_and_migrate_flag() {
        let cli = Cli::try_parse_from([
            "oulipoly-agent-runner",
            "--resume",
            "5169694d-de0f-40d1-890c-6e28e55bab27",
            "--migrate",
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
        assert_eq!(cli.migrate.as_deref(), Some("claude2"));
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
        let preview = agent_runner_lib::state::ChainPreview {
            chain_id: "5169694d-de0f-40d1-890c-6e28e55bab27".to_string(),
            last_used_at: ts,
            active_provider: "claude".to_string(),
            active_session_id: "dd116a3c-6819-42b1-b3d2-f512331eb5ec".to_string(),
            turn_count: 42,
            recent_turns: vec![agent_runner_lib::state::TurnPreview {
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
        let resolved = agent_runner_lib::state::ResolvedResume {
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
        let model = ModelConfig::from_toml(
            "claude-opus",
            r#"
[[providers]]
name = "claude"
args = ["--model", "opus"]

[[providers]]
name = "claude2"
args = ["--model", "opus"]
"#,
        )
        .unwrap();
        let resolved = agent_runner_lib::state::ResolvedResume {
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

    // risk: Schema migration and backfill; level: particular-integration; source: proposal §11.1 Schema migration and backfill / A5, A6.
    #[test]
    fn migrate_db_compaction_backfill_idempotent_on_second_run() {
        let dir = tempfile::tempdir().unwrap();
        let db = StateDb::open(&dir.path().join("state.db")).unwrap();
        let ts = chrono::DateTime::parse_from_rfc3339("2026-04-17T08:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        db.ingest_session_turn("claude", "session-a", "turn-1", &ts, "assistant", "")
            .unwrap();
        let jsonl = dir.path().join("session-a.jsonl");
        std::fs::write(
            &jsonl,
            r#"{"uuid":"turn-1","sessionId":"session-a","timestamp":"2026-04-17T08:00:00Z","type":"assistant","isCompactSummary":true}"#,
        )
        .unwrap();

        let first =
            flag_compaction_boundaries_from_jsonl(&db, "claude", "session-a", &jsonl).unwrap();
        let second =
            flag_compaction_boundaries_from_jsonl(&db, "claude", "session-a", &jsonl).unwrap();

        assert_eq!(first, 1);
        assert_eq!(second, 0);
        assert!(
            db.latest_compaction_boundary("claude", "session-a")
                .unwrap()
                .is_some()
        );
    }
}
