//! ## Declared roles
//!
//! `parser`, `validator`, `mapper`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/usage/cli.rs
//!     role: adapter
//!     Translates:
//!       - clap-derive-Parser-Subcommand to public AgentsCli command surface
//! ```

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
            "pin_provider",
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

    /// Caller-stable token for idempotent durable submission of a resume payload.
    #[arg(long = "submission-token", requires = "resume")]
    pub(crate) submission_token: Option<String>,

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

    /// Force a fresh run to use the named provider account if it is valid and eligible.
    #[arg(
        long = "pin-provider",
        value_name = "TARGET",
        conflicts_with_all = ["resume", "new", "rotate_provider"]
    )]
    pub(crate) pin_provider: Option<String>,

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
    /// Resume a provider session non-interactively. If no answer payload is
    /// supplied, the provider receives native resume args without a prompt so
    /// deferred resume markers can continue.
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

        /// Caller-stable token for idempotent durable submission of the payload.
        #[arg(long = "submission-token")]
        submission_token: Option<String>,

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
    /// Receive durable notifications from external agent workloads.
    Notify {
        #[command(subcommand)]
        command: NotifySubcommands,
    },
    /// Inspect queued session mailbox notifications.
    Mailbox {
        #[command(subcommand)]
        command: MailboxSubcommands,
    },
    /// Hidden normalized form for `resume --list <UUID>`.
    #[command(hide = true, name = "resume-list")]
    ResumeList { uuid: String },
    /// Run chain-table backfill explicitly.
    MigrateDb,
    /// Run the session ownership migration harness.
    MigrateSessionOwnership {
        /// Run against copied DBs only.
        #[arg(long = "dry-run")]
        dry_run: bool,

        /// Apply the migration to the live state DB.
        #[arg(long = "apply")]
        apply: bool,

        /// Roll back a live migration from its durable preimage table.
        #[arg(long = "rollback")]
        rollback: bool,

        /// Run the model corrective variant for the selected mode.
        #[arg(long = "corrective")]
        corrective: bool,

        /// Directory where copied DBs and report are written.
        #[arg(long = "scratch-dir")]
        scratch_dir: Option<PathBuf>,

        /// Directory where live-apply backups and reports are written.
        #[arg(long = "backup-dir")]
        backup_dir: Option<PathBuf>,

        /// Required acknowledgement before mutating the live DB.
        #[arg(long = "confirm-mutate-live-db")]
        confirm_mutate_live_db: bool,

        /// Skip external provider contract proof.
        #[arg(long = "skip-provider-proof")]
        skip_provider_proof: bool,

        /// Required acknowledgement when provider proof is skipped.
        #[arg(long = "confirm-skip-provider-proof")]
        confirm_skip_provider_proof: bool,

        /// Override state DB path.
        #[arg(long = "state-db")]
        state_db: Option<PathBuf>,

        /// Override models directory.
        #[arg(long = "models-dir")]
        models_dir: Option<PathBuf>,
    },
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
pub(crate) enum NotifySubcommands {
    /// Queue an agent-bash completion notification for the owning session.
    AgentBashComplete {
        /// Diagnostic producer caller PID; not used for owner identity.
        #[arg(long = "caller-ppid")]
        caller_ppid: u32,

        /// Stable workload handle for idempotent enqueue.
        #[arg(long)]
        handle: String,

        /// Spooler state directory.
        #[arg(long = "state-dir")]
        state_dir: PathBuf,

        /// Path to meta.json containing caller_chain.
        #[arg(long)]
        meta: PathBuf,

        /// Path to retained workload log.
        #[arg(long)]
        log: PathBuf,

        /// Path to workload rc file.
        #[arg(long)]
        rc: PathBuf,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Clone, Debug, Subcommand)]
pub(crate) enum MailboxSubcommands {
    /// List queued mailbox notifications for a session.
    List {
        /// Provider session id.
        #[arg(long = "session-id")]
        session_id: String,

        /// Include delivered rows.
        #[arg(long)]
        all: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Search notification metadata for a session.
    Search {
        /// Provider session id.
        #[arg(long = "session-id")]
        session_id: String,

        /// Case-insensitive text matched against sequence, kind, handle, payload, and paths.
        query: String,

        /// Include delivered rows.
        #[arg(long)]
        all: bool,

        /// Maximum matching rows.
        #[arg(long, default_value = "100")]
        limit: usize,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Show one notification and optionally read its retained artifacts.
    #[command(group = clap::ArgGroup::new("mailbox_item").args(["seq", "handle"]).required(true).multiple(false))]
    Show {
        /// Provider session id.
        #[arg(long = "session-id")]
        session_id: String,

        /// Mailbox sequence number.
        #[arg(long)]
        seq: Option<i64>,

        /// Agent-bash workload handle.
        #[arg(long)]
        handle: Option<String>,

        /// Include bounded meta, log, and rc file contents.
        #[arg(long = "include-artifacts")]
        include_artifacts: bool,

        /// Maximum bytes read from each artifact.
        #[arg(long = "max-bytes", default_value = "65536")]
        max_bytes: usize,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Show pause state and pending sequence bounds for a session.
    Status {
        /// Provider session id.
        #[arg(long = "session-id")]
        session_id: String,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Pause proactive notification delivery while continuing to enqueue rows.
    Pause {
        /// Provider session id.
        #[arg(long = "session-id")]
        session_id: String,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Resume proactive notification delivery for a session.
    Resume {
        /// Provider session id.
        #[arg(long = "session-id")]
        session_id: String,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Acknowledge pending notifications in an inclusive sequence range.
    Ack {
        /// Provider session id.
        #[arg(long = "session-id")]
        session_id: String,

        /// First sequence to acknowledge.
        #[arg(long = "from-seq")]
        from_seq: i64,

        /// Last sequence to acknowledge; newer rows remain pending.
        #[arg(long = "to-seq")]
        to_seq: i64,

        /// Audit label stored with the acknowledged rows.
        #[arg(long = "delivered-by", default_value = "manual-mailbox-ack")]
        delivered_by: String,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Move delivered notification payloads from inline SQLite rows into the immutable payload store.
    CompactDelivered {
        /// Maximum delivered rows to compact in this invocation.
        #[arg(long, default_value_t = 1000)]
        limit: usize,

        /// Apply compaction. Without this flag the command only reports eligible rows.
        #[arg(long)]
        apply: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Clone, Debug, Subcommand)]
pub(crate) enum SessionSubcommands {
    /// List resumable provider sessions known to the local state DB.
    List {
        /// Emit structured JSON instead of a human-readable table.
        #[arg(long)]
        json: bool,
    },
    /// Discover and register existing provider-native sessions.
    Import {
        /// Limit import to one provider account or model name.
        #[arg(long)]
        provider: Option<String>,

        /// Maximum provider-native sessions to enumerate per provider.
        #[arg(long)]
        limit: Option<u64>,

        /// Only enumerate provider-native sessions updated since this Unix millisecond timestamp.
        #[arg(long = "since-unix-ms")]
        since_unix_ms: Option<u64>,

        /// Read and ingest provider-native turns for imported sessions when supported.
        #[arg(long = "backfill-turns")]
        backfill_turns: bool,

        /// Emit structured JSON instead of a human-readable report.
        #[arg(long)]
        json: bool,
    },
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
    /// Resolve a live OS PID to its verified sidecar session identity.
    OfPid {
        pid: u32,
        #[arg(long)]
        json: bool,
    },
    /// Return whether a live OS PID is sidecar-recorded and identity-verified.
    Alive {
        pid: u32,
        #[arg(long)]
        json: bool,
    },
    /// Resolve a live OS PID and print its invocation subtree.
    Subtree {
        pid: u32,
        #[arg(long)]
        json: bool,
        #[arg(long, default_value = "64")]
        max_depth: usize,
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
