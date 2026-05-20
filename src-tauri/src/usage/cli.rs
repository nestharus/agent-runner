use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Clone, Debug, Parser)]
#[command(
    name = "oulipoly-agent-runner",
    about = "LLM agent runner with load balancing",
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Subcommands>,

    /// Print live usage for model-pool provider accounts.
    #[arg(
        long,
        conflicts_with_all = [
            "model",
            "agent",
            "prompt_args",
            "file",
            "agent_file",
            "new",
            "resume",
            "rotate_provider",
            "input"
        ]
    )]
    pub(crate) usage: bool,

    /// Agent name (from agents directory)
    pub(crate) agent: Option<String>,

    /// Prompt text (remaining arguments joined)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub(crate) prompt_args: Vec<String>,

    /// Execute a model directly (no agent)
    #[arg(short, long)]
    pub(crate) model: Option<String>,

    /// Resume an existing session by UUID at the top level. Routes by
    /// prompt presence: a prompt (positional, `--file`, or piped stdin)
    /// dispatches to non-interactive headless mode; no prompt drops into
    /// the provider's interactive REPL. Equivalent to `repl --resume`
    /// (no prompt) or `resume --session-id <sid>` (with prompt), but
    /// unified at the top level.
    #[arg(long = "resume")]
    pub(crate) resume: Option<String>,

    /// Launch a new sessionless provider-family REPL using default_provider.
    #[arg(short = 'n', long = "new", conflicts_with = "resume")]
    pub(crate) new: bool,

    /// Rotate the active chain segment's provider. With a value, reroute to
    /// the named provider; without a value, auto-rotate to the next working
    /// candidate. Omit to honor the bound provider.
    #[arg(
        long = "rotate-provider",
        value_name = "TARGET",
        num_args = 0..=1,
        default_missing_value = "",
    )]
    pub(crate) rotate_provider: Option<String>,

    /// Path to an agent .md file
    #[arg(short = 'a', long = "agent-file")]
    pub(crate) agent_file: Option<PathBuf>,

    /// Read prompt from file
    #[arg(short, long)]
    pub(crate) file: Option<PathBuf>,

    /// Working directory
    #[arg(short = 'p', long = "project")]
    pub(crate) project: Option<PathBuf>,

    /// Models directory (default: ~/.config/oulipoly-agent-runner/models/)
    #[arg(long)]
    pub(crate) models_dir: Option<PathBuf>,

    /// Agents directory
    #[arg(long)]
    pub(crate) agents_dir: Option<PathBuf>,

    /// Pass model inputs as key=value pairs (repeatable)
    #[arg(id = "input", short = 'i', long = "input", value_name = "KEY=VALUE")]
    pub(crate) inputs: Vec<String>,
}

#[derive(Clone, Debug, Subcommand)]
pub(crate) enum Subcommands {
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

        /// Rotate the active chain segment's provider. With a value, reroute
        /// to the named provider; without a value, auto-rotate to the next
        /// working candidate. Omit to honor the bound provider.
        #[arg(
            long = "rotate-provider",
            value_name = "TARGET",
            num_args = 0..=1,
            default_missing_value = "",
        )]
        rotate_provider: Option<String>,

        /// Working directory for the wrapped CLI
        #[arg(short = 'p', long = "project")]
        project: Option<PathBuf>,

        /// Override models directory
        #[arg(long = "models-dir")]
        models_dir: Option<PathBuf>,
    },
    /// Resume a provider session non-interactively with an answer payload.
    #[command(group = clap::ArgGroup::new("resume_target").args(["session_id", "chain_id"]).required(true).multiple(false))]
    Resume {
        /// Model id whose provider pool must include the session owner.
        #[arg(short, long)]
        model: Option<String>,

        /// Provider session UUID to resume.
        #[arg(long = "session-id")]
        session_id: Option<String>,

        /// Provider session chain UUID to resume.
        #[arg(value_name = "CHAIN_ID")]
        chain_id: Option<String>,

        /// Rotate the active chain segment's provider. With a value, reroute
        /// to the named provider; without a value, auto-rotate to the next
        /// working candidate. Omit to honor the bound provider.
        #[arg(
            long = "rotate-provider",
            value_name = "TARGET",
            num_args = 0..=1,
            default_missing_value = "",
        )]
        rotate_provider: Option<String>,

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
    /// Recover from an unusable state DB by backing it up and creating a fresh one.
    Migrate {
        /// Back up state.db and sidecars, then create a fresh current schema DB.
        #[arg(long)]
        rebuild: bool,
    },
    /// Move runtime provider config from model TOMLs into providers.toml. Idempotent - safe to re-run if a previous run left empty args.
    MigrateConfig {
        /// Override models directory.
        #[arg(long = "models-dir")]
        models_dir: Option<PathBuf>,
    },
}

#[derive(Clone, Debug, Subcommand)]
pub(crate) enum SessionSubcommands {
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
