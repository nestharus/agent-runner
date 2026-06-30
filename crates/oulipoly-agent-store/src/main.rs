use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use oulipoly_agent_store::{
    ArtifactKey, ArtifactMeta, ListFilter, PutReceipt, PutRequest, Store, StoreError,
    TombstoneReceipt, TombstoneStatus,
};
use serde::Serialize;

#[derive(Debug, Parser)]
#[command(name = "agent-store")]
#[command(about = "Versioned artifact store")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Init(InitArgs),
    Put(PutArgs),
    Get(GetArgs),
    GetMeta(GetMetaArgs),
    List(ListArgs),
    Tombstone(TombstoneArgs),
}

#[derive(Debug, Args)]
struct InitArgs {
    #[arg(long)]
    db: PathBuf,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
#[group(required = true, multiple = false)]
struct ContentInput {
    #[arg(long)]
    content_file: Option<PathBuf>,
    #[arg(long)]
    content_stdin: bool,
}

#[derive(Debug, Args)]
struct PutArgs {
    #[arg(long)]
    db: PathBuf,
    #[arg(long)]
    workflow_run_id: String,
    #[arg(long)]
    artifact_name: String,
    #[arg(long)]
    predecessor_version: Option<u64>,
    #[arg(long)]
    producer_uuid: Option<uuid::Uuid>,
    #[arg(long = "format")]
    format_hint: Option<String>,
    #[arg(long)]
    verdict_line: Option<String>,
    #[command(flatten)]
    content: ContentInput,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct GetArgs {
    #[arg(long)]
    db: PathBuf,
    #[arg(long)]
    workflow_run_id: String,
    #[arg(long)]
    artifact_name: String,
    #[arg(long)]
    version: Option<u64>,
    #[arg(long)]
    out: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct GetMetaArgs {
    #[arg(long)]
    db: PathBuf,
    #[arg(long)]
    workflow_run_id: String,
    #[arg(long)]
    artifact_name: String,
    #[arg(long)]
    version: Option<u64>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ListArgs {
    #[arg(long)]
    db: PathBuf,
    #[arg(long)]
    workflow_run_id: Option<String>,
    #[arg(long)]
    artifact_name: Option<String>,
    #[arg(long)]
    include_tombstoned: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct TombstoneArgs {
    #[arg(long)]
    db: PathBuf,
    #[arg(long)]
    workflow_run_id: String,
    #[arg(long)]
    artifact_name: String,
    #[arg(long)]
    version: u64,
    #[arg(long)]
    actor: String,
    #[arg(long)]
    reason: String,
    #[arg(long)]
    json: bool,
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            let _ = err.print();
            return ExitCode::from(64);
        }
    };

    match cli.command {
        Command::Init(args) => handle_init(args),
        Command::Put(args) => handle_put(args),
        Command::Get(args) => handle_get(args),
        Command::GetMeta(args) => handle_get_meta(args),
        Command::List(args) => handle_list(args),
        Command::Tombstone(args) => handle_tombstone(args),
    }
}

fn handle_init(args: InitArgs) -> ExitCode {
    run_cli(|| {
        let already_current = store_is_current(&args.db);
        Store::init(&args.db)?;
        write_init_output(&args.db, init_status(already_current), args.json)?;
        Ok(())
    })
}

fn handle_put(args: PutArgs) -> ExitCode {
    run_cli(|| {
        let PutArgs {
            db,
            workflow_run_id,
            artifact_name,
            predecessor_version,
            producer_uuid,
            format_hint,
            verdict_line,
            content: content_input,
            json,
        } = args;
        let content = read_content(content_input)?;
        let store = Store::open(&db)?;
        let receipt = store.put(put_request(
            workflow_run_id,
            artifact_name,
            producer_uuid,
            format_hint,
            verdict_line,
            predecessor_version,
            content,
        ))?;
        write_put_output(&receipt, json)?;
        Ok(())
    })
}

fn handle_get(args: GetArgs) -> ExitCode {
    run_cli(|| {
        let store = Store::open(&args.db)?;
        let key = artifact_key(args.workflow_run_id, args.artifact_name);
        let record = store.get(&key, args.version)?;
        write_content_output(args.out, record.content)?;
        Ok(())
    })
}

fn handle_get_meta(args: GetMetaArgs) -> ExitCode {
    run_cli(|| {
        let store = Store::open(&args.db)?;
        let key = artifact_key(args.workflow_run_id, args.artifact_name);
        let meta = store.get_meta(&key, args.version)?;
        write_meta_output(&meta, args.json)?;
        Ok(())
    })
}

fn handle_list(args: ListArgs) -> ExitCode {
    run_cli(|| {
        let store = Store::open(&args.db)?;
        let json = args.json;
        let rows = store.list(list_filter(args))?;
        write_list_output(&rows, json)?;
        Ok(())
    })
}

fn handle_tombstone(args: TombstoneArgs) -> ExitCode {
    run_cli(|| {
        let store = Store::open(&args.db)?;
        let key = artifact_key(args.workflow_run_id, args.artifact_name);
        let receipt = store.tombstone(&key, args.version, &args.actor, &args.reason)?;
        write_tombstone_output(&receipt, args.json)?;
        Ok(())
    })
}

fn store_is_current(db_path: &PathBuf) -> bool {
    Store::open(db_path).is_ok()
}

fn init_status(already_current: bool) -> &'static str {
    if already_current {
        "already_current"
    } else {
        "initialized"
    }
}

fn artifact_key(workflow_run_id: String, artifact_name: String) -> ArtifactKey {
    ArtifactKey {
        workflow_run_id,
        artifact_name,
    }
}

fn put_request(
    workflow_run_id: String,
    artifact_name: String,
    producer_uuid: Option<uuid::Uuid>,
    format_hint: Option<String>,
    verdict_line: Option<String>,
    predecessor_version: Option<u64>,
    content: Vec<u8>,
) -> PutRequest {
    PutRequest {
        key: artifact_key(workflow_run_id, artifact_name),
        producer_invocation_uuid: producer_uuid,
        format_hint,
        verdict_line,
        predecessor_version,
        content,
    }
}

fn list_filter(args: ListArgs) -> ListFilter {
    ListFilter {
        workflow_run_id: args.workflow_run_id,
        artifact_name: args.artifact_name,
        include_tombstoned: args.include_tombstoned,
    }
}

fn write_init_output(db: &Path, status: &'static str, json: bool) -> Result<(), CliError> {
    if json {
        print_json(&InitEnvelope {
            db_path: db.display().to_string(),
            schema_version: 1,
            status,
        })
    } else {
        writeln!(io::stdout(), "{status}: {}", db.display()).map_err(CliError::from)
    }
}

fn write_put_output(receipt: &PutReceipt, json: bool) -> Result<(), CliError> {
    if json {
        print_json(&PutEnvelope::from_receipt(receipt))
    } else {
        writeln!(
            io::stdout(),
            "{} {} v{} {}",
            receipt.key.workflow_run_id,
            receipt.key.artifact_name,
            receipt.version,
            receipt.sha256
        )
        .map_err(CliError::from)
    }
}

fn write_content_output(out: Option<PathBuf>, content: Vec<u8>) -> Result<(), CliError> {
    if let Some(path) = out {
        return fs::write(path, content).map_err(CliError::from);
    }

    let stdout = io::stdout();
    let mut handle = stdout.lock();
    handle.write_all(&content)?;
    handle.flush()?;
    Ok(())
}

fn write_meta_output(meta: &ArtifactMeta, json: bool) -> Result<(), CliError> {
    if json {
        print_json(&MetaEnvelope::from_meta(meta))
    } else {
        writeln!(
            io::stdout(),
            "{} {} v{} {}",
            meta.key.workflow_run_id,
            meta.key.artifact_name,
            meta.version,
            meta.sha256
        )
        .map_err(CliError::from)
    }
}

fn write_list_output(rows: &[ArtifactMeta], json: bool) -> Result<(), CliError> {
    if json {
        let envelopes: Vec<_> = rows.iter().map(MetaEnvelope::from_meta).collect();
        return print_json(&envelopes);
    }

    let mut stdout = io::stdout();
    for row in rows {
        writeln!(
            stdout,
            "{} {} v{} {}",
            row.key.workflow_run_id, row.key.artifact_name, row.version, row.sha256
        )?;
    }
    Ok(())
}

fn write_tombstone_output(receipt: &TombstoneReceipt, json: bool) -> Result<(), CliError> {
    if json {
        print_json(&TombstoneEnvelope::from_receipt(receipt))
    } else {
        writeln!(
            io::stdout(),
            "{}: {} {} v{}",
            status_text(&receipt.status),
            receipt.key.workflow_run_id,
            receipt.key.artifact_name,
            receipt.version
        )
        .map_err(CliError::from)
    }
}

#[derive(Debug)]
enum CliError {
    Store(StoreError),
    Io(io::Error),
    Json(serde_json::Error),
}

impl From<StoreError> for CliError {
    fn from(value: StoreError) -> Self {
        Self::Store(value)
    }
}

impl From<io::Error> for CliError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for CliError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

fn run_cli(op: impl FnOnce() -> Result<(), CliError>) -> ExitCode {
    match op() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{}", error_message(&err));
            ExitCode::from(error_code(&err))
        }
    }
}

fn error_message(err: &CliError) -> String {
    match err {
        CliError::Store(err) => err.to_string(),
        CliError::Io(err) => format!("io error: {err}"),
        CliError::Json(err) => format!("json error: {err}"),
    }
}

fn error_code(err: &CliError) -> u8 {
    match err {
        CliError::Store(StoreError::NotFound) => 65,
        CliError::Store(StoreError::Collision) => 66,
        CliError::Store(StoreError::Io(_)) | CliError::Io(_) => 74,
        CliError::Store(
            StoreError::Database(_)
            | StoreError::MigrationRequired
            | StoreError::IncompatibleSchema(_),
        ) => 73,
        CliError::Json(_) => 70,
        CliError::Store(StoreError::InvalidInput(_)) => 64,
    }
}

fn read_content(input: ContentInput) -> Result<Vec<u8>, CliError> {
    if let Some(path) = input.content_file {
        return Ok(fs::read(path)?);
    }

    let mut content = Vec::new();
    io::stdin().read_to_end(&mut content)?;
    Ok(content)
}

fn print_json(value: &impl Serialize) -> Result<(), CliError> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    serde_json::to_writer(&mut handle, value)?;
    writeln!(handle)?;
    Ok(())
}

#[derive(Serialize)]
struct InitEnvelope<'a> {
    db_path: String,
    schema_version: u64,
    status: &'a str,
}

#[derive(Serialize)]
struct PutEnvelope {
    workflow_run_id: String,
    artifact_name: String,
    version: u64,
    producer_invocation_uuid: Option<String>,
    sha256: String,
    content_len: u64,
    format_hint: Option<String>,
    verdict_line: Option<String>,
    predecessor_version: Option<u64>,
    created_at: String,
}

impl PutEnvelope {
    fn from_receipt(receipt: &PutReceipt) -> Self {
        Self {
            workflow_run_id: receipt.key.workflow_run_id.clone(),
            artifact_name: receipt.key.artifact_name.clone(),
            version: receipt.version,
            producer_invocation_uuid: receipt
                .producer_invocation_uuid
                .map(|uuid| uuid.to_string()),
            sha256: receipt.sha256.clone(),
            content_len: receipt.content_len,
            format_hint: receipt.format_hint.clone(),
            verdict_line: receipt.verdict_line.clone(),
            predecessor_version: receipt.predecessor_version,
            created_at: receipt.created_at.to_rfc3339(),
        }
    }
}

#[derive(Serialize)]
struct MetaEnvelope {
    workflow_run_id: String,
    artifact_name: String,
    version: u64,
    producer_invocation_uuid: Option<String>,
    sha256: String,
    content_len: u64,
    format_hint: Option<String>,
    verdict_line: Option<String>,
    predecessor_version: Option<u64>,
    created_at: String,
    tombstone: Option<TombstoneMetaEnvelope>,
}

impl MetaEnvelope {
    fn from_meta(meta: &ArtifactMeta) -> Self {
        Self {
            workflow_run_id: meta.key.workflow_run_id.clone(),
            artifact_name: meta.key.artifact_name.clone(),
            version: meta.version,
            producer_invocation_uuid: meta.producer_invocation_uuid.map(|uuid| uuid.to_string()),
            sha256: meta.sha256.clone(),
            content_len: meta.content_len,
            format_hint: meta.format_hint.clone(),
            verdict_line: meta.verdict_line.clone(),
            predecessor_version: meta.predecessor_version,
            created_at: meta.created_at.to_rfc3339(),
            tombstone: meta
                .tombstone
                .as_ref()
                .map(|tombstone| TombstoneMetaEnvelope {
                    tombstoned_at: tombstone.tombstoned_at.to_rfc3339(),
                    actor: tombstone.actor.clone(),
                    reason: tombstone.reason.clone(),
                }),
        }
    }
}

#[derive(Serialize)]
struct TombstoneMetaEnvelope {
    tombstoned_at: String,
    actor: String,
    reason: String,
}

#[derive(Serialize)]
struct TombstoneEnvelope {
    workflow_run_id: String,
    artifact_name: String,
    version: u64,
    tombstoned_at: String,
    actor: String,
    reason: String,
    status: &'static str,
}

impl TombstoneEnvelope {
    fn from_receipt(receipt: &TombstoneReceipt) -> Self {
        Self {
            workflow_run_id: receipt.key.workflow_run_id.clone(),
            artifact_name: receipt.key.artifact_name.clone(),
            version: receipt.version,
            tombstoned_at: receipt.tombstone.tombstoned_at.to_rfc3339(),
            actor: receipt.tombstone.actor.clone(),
            reason: receipt.tombstone.reason.clone(),
            status: status_text(&receipt.status),
        }
    }
}

fn status_text(status: &TombstoneStatus) -> &'static str {
    match status {
        TombstoneStatus::Tombstoned => "tombstoned",
        TombstoneStatus::AlreadyTombstoned => "already_tombstoned",
    }
}
