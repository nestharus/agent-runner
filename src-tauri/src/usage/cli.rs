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
            "fresh_continuation_request",
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

    /// Execute an evidence-bound fresh continuation after the named resume.
    #[arg(
        long = "fresh-continuation-request",
        value_name = "PATH",
        requires = "resume",
        conflicts_with = "rotate_provider"
    )]
    pub(crate) fresh_continuation_request: Option<PathBuf>,

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
        conflicts_with_all = ["resume", "rotate_provider"]
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
        /// a model or when default_provider selects a fresh provider-family REPL.
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
    /// Inspect DB-independent flight-recorder artifacts without starting runtime services.
    Diagnostics {
        #[command(subcommand)]
        command: DiagnosticsSubcommands,
    },
    /// Inspect or cancel one exact detached-maintenance job locally.
    Maintenance {
        #[command(subcommand)]
        command: MaintenanceSubcommands,
    },
    /// Hidden normalized form for `resume --list <UUID>`.
    #[command(hide = true, name = "resume-list")]
    ResumeList { uuid: String },
    /// Inspect or settle an admitted completed turn without provider execution.
    CompletedTurn {
        #[arg(long)]
        invocation: Option<String>,
        /// Continue the bounded recovery listing after this invocation row ID.
        #[arg(long, requires = "epoch", conflicts_with_all = ["invocation", "settle", "output"])]
        after_id: Option<i64>,
        /// Epoch returned by the first page; restart after this pass if requested.
        #[arg(long, requires = "after_id")]
        epoch: Option<i64>,
        #[arg(long)]
        settle: bool,
        /// Explicitly replay retained stdout bytes; never asserts prior delivery.
        #[arg(long)]
        output: bool,
    },
    /// Linux, opt-in: start one fresh native OpenCode ACP v2 root from a
    /// JSON request file whose `env` is the root's whole environment, or
    /// recover one for `cancel` or `continue-attached` (never a new root
    /// incarnation). Needs `oulipoly-root-supervisor` built next to this
    /// binary. Stdout: JSON lines; stdin lines go to the root's owner. See
    /// `native_root` docs for the exit statuses.
    #[cfg(target_os = "linux")]
    #[command(name = "native-root")]
    NativeRoot {
        /// The fresh root's request file (JSON).
        #[arg(long, required_unless_present = "recover", conflicts_with = "recover")]
        request: Option<PathBuf>,
        /// The recovery request file (JSON): `store`, `purpose`, `env`.
        #[arg(long)]
        recover: Option<PathBuf>,
    },
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
pub(crate) enum DiagnosticsSubcommands {
    /// Show recent recorder failures and their coalesced presentation groups.
    Recent {
        /// Maximum number of raw recent failures to return.
        #[arg(long)]
        limit: std::num::NonZeroUsize,

        /// Emit structured JSON instead of a human-readable report.
        #[arg(long)]
        json: bool,

        /// Opaque continuation from the previous diagnostics recent page.
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Trace all retained recorder events for one diagnostic ID.
    Trace {
        diagnostic_id: String,

        /// Emit structured JSON instead of a human-readable report.
        #[arg(long)]
        json: bool,

        /// Opaque continuation from the previous diagnostics trace page.
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Query bounded longitudinal metrics across event-store rotations.
    Metrics {
        /// Time window ending now, in minutes.
        #[arg(long, default_value = "60")]
        minutes: std::num::NonZeroU64,

        /// Emit structured JSON instead of a human-readable report.
        #[arg(long)]
        json: bool,
    },
    /// Read bounded event-store exemplars for one trace UUID.
    EventTrace {
        trace_id: String,

        /// Emit structured JSON instead of a human-readable report.
        #[arg(long)]
        json: bool,
    },
    /// Read the direct evidence and checkpoint for one exact maintenance job.
    Maintenance {
        #[arg(long)]
        kind: String,

        #[arg(long)]
        partition: String,

        /// Emit structured JSON instead of a human-readable report.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Clone, Debug, Subcommand)]
pub(crate) enum MaintenanceSubcommands {
    /// Inspect one exact maintenance job and its offline evidence.
    Status {
        #[arg(long)]
        kind: String,

        #[arg(long)]
        partition: String,

        #[arg(long)]
        json: bool,
    },
    /// Request cancellation of one exact admitted opportunity epoch.
    Cancel {
        #[arg(long)]
        kind: String,

        #[arg(long)]
        partition: String,

        #[arg(long)]
        epoch: i64,

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

#[cfg(test)]
mod observation_rearm_cli_tests {
    use super::*;

    #[test]
    fn completed_turn_cursor_requires_epoch_and_cannot_settle_a_page() {
        assert!(Cli::try_parse_from(["runner", "completed-turn", "--after-id", "100"]).is_err());
        assert!(Cli::try_parse_from(["runner", "completed-turn", "--epoch", "1"]).is_err());
        assert!(
            Cli::try_parse_from([
                "runner",
                "completed-turn",
                "--after-id",
                "100",
                "--epoch",
                "1",
                "--settle",
            ])
            .is_err()
        );
        let cli = Cli::try_parse_from([
            "runner",
            "completed-turn",
            "--after-id",
            "100",
            "--epoch",
            "1",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Subcommands::CompletedTurn {
                invocation: None,
                after_id: Some(100),
                epoch: Some(1),
                settle: false,
                output: false,
            })
        ));
    }
}

#[cfg(test)]
mod offline_diagnostics_cli_tests {
    use super::*;

    #[test]
    fn parses_recent_with_positive_limit_and_json() {
        let cli =
            Cli::try_parse_from(["runner", "diagnostics", "recent", "--limit", "17", "--json"])
                .unwrap();

        assert!(matches!(
            cli.command,
            Some(Subcommands::Diagnostics {
                command: DiagnosticsSubcommands::Recent {
                    limit,
                    json: true,
                    cursor: None,
                },
            }) if limit.get() == 17
        ));
    }

    #[test]
    fn recent_rejects_zero_limit() {
        assert!(Cli::try_parse_from(["runner", "diagnostics", "recent", "--limit", "0"]).is_err());
    }

    #[test]
    fn parses_trace_diagnostic_id_and_json() {
        let cli =
            Cli::try_parse_from(["runner", "diagnostics", "trace", "diagnostic-17", "--json"])
                .unwrap();

        assert!(matches!(
            cli.command,
            Some(Subcommands::Diagnostics {
                command: DiagnosticsSubcommands::Trace {
                    diagnostic_id,
                    json: true,
                    cursor: None,
                },
            }) if diagnostic_id == "diagnostic-17"
        ));
    }

    #[test]
    fn parses_metrics_and_event_trace() {
        let metrics = Cli::try_parse_from([
            "runner",
            "diagnostics",
            "metrics",
            "--minutes",
            "15",
            "--json",
        ])
        .unwrap();
        assert!(matches!(
            metrics.command,
            Some(Subcommands::Diagnostics {
                command: DiagnosticsSubcommands::Metrics { minutes, json: true },
            }) if minutes.get() == 15
        ));
        assert!(
            Cli::try_parse_from(["runner", "diagnostics", "metrics", "--minutes", "0",]).is_err()
        );

        let trace = Cli::try_parse_from([
            "runner",
            "diagnostics",
            "event-trace",
            "11111111-1111-4111-8111-111111111111",
        ])
        .unwrap();
        assert!(matches!(
            trace.command,
            Some(Subcommands::Diagnostics {
                command: DiagnosticsSubcommands::EventTrace { trace_id, json: false },
            }) if trace_id == "11111111-1111-4111-8111-111111111111"
        ));
    }

    #[test]
    fn parses_exact_maintenance_offline_reader_and_epoch_fenced_cancel() {
        let diagnostics = Cli::try_parse_from([
            "runner",
            "diagnostics",
            "maintenance",
            "--kind",
            "event_retirement",
            "--partition",
            "aa/bb",
            "--json",
        ])
        .unwrap();
        assert!(matches!(
            diagnostics.command,
            Some(Subcommands::Diagnostics {
                command: DiagnosticsSubcommands::Maintenance {
                    kind,
                    partition,
                    json: true,
                },
            }) if kind == "event_retirement" && partition == "aa/bb"
        ));

        let cancel = Cli::try_parse_from([
            "runner",
            "maintenance",
            "cancel",
            "--kind",
            "event_retirement",
            "--partition",
            "aa/bb",
            "--epoch",
            "19",
        ])
        .unwrap();
        assert!(matches!(
            cancel.command,
            Some(Subcommands::Maintenance {
                command: MaintenanceSubcommands::Cancel {
                    kind,
                    partition,
                    epoch: 19,
                    json: false,
                },
            }) if kind == "event_retirement" && partition == "aa/bb"
        ));
    }
}
