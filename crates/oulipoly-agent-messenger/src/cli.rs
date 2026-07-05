//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - mapper
//! - orchestration
//! - parser
//! - validator
//!
//! Command-line adapter for the agent-messenger library API.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-agent-messenger/src/cli.rs
//!     role: adapter
//!     Translates:
//!       - clap command-line argument contract
//!       - OULIPOLY_PARENT_INVOCATION and OULIPOLY_RETURN_CHANNEL environment contract
//!       - agent-messenger library request/receipt contract
//!       - stdout/stderr/stdin and filesystem byte I/O contract
//!       - JSON CLI output contract
//! ```

use crate::formatter::RECEIPT_SCHEMA_VERSION;
use crate::model::{
    ListReturnedRequest, ReturnRequest, ReturnSource, ReturnedArtifact, ReturnedArtifactMeta,
    ReturnedArtifactRecord, ShowReturnedRequest,
};
use crate::validator::{
    require_parent_invocation_env, require_parent_invocation_id, require_return_channel_env,
};
use crate::{MessengerError, ReturnName, list_returned, return_artifact, show_returned};
use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(name = "agent-messenger")]
#[command(about = "Return artifacts from child agents to their caller")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Return(ReturnArgs),
    ListReturned(ListReturnedArgs),
    Show(ShowArgs),
    Version(VersionArgs),
}

#[derive(Debug, Args)]
struct ReturnArgs {
    #[arg(long)]
    db: PathBuf,
    #[arg(long)]
    invocation_uuid: Option<String>,
    #[arg(long)]
    name: String,
    #[arg(long)]
    scratchpad: Option<String>,
    #[arg(long)]
    scratchpad_version: Option<u64>,
    #[arg(long)]
    body: Option<String>,
    #[arg(long)]
    content_file: Option<PathBuf>,
    #[arg(long)]
    content_stdin: bool,
    #[arg(long = "format")]
    format_hint: Option<String>,
    #[arg(long)]
    verdict_line: Option<String>,
    #[arg(long)]
    return_channel: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ListReturnedArgs {
    #[arg(long)]
    db: PathBuf,
    #[arg(long)]
    invocation_uuid: Option<String>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ShowArgs {
    #[arg(long)]
    db: PathBuf,
    #[arg(long)]
    version_id: Option<String>,
    #[arg(long)]
    invocation_uuid: Option<String>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    version: Option<u64>,
    #[arg(long)]
    out: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct VersionArgs {
    #[arg(long)]
    json: bool,
}

struct ReturnSourceArgs {
    scratchpad: Option<String>,
    scratchpad_version: Option<u64>,
    body: Option<String>,
    content_file: Option<PathBuf>,
    content_stdin: bool,
}

enum ShowAddressArgs {
    VersionId(String),
    Name(String),
}

pub fn run() -> i32 {
    run_parsed_cli(parse_cli())
}

fn parse_cli() -> Result<Cli, clap::Error> {
    Cli::try_parse()
}

fn run_parsed_cli(parsed: Result<Cli, clap::Error>) -> i32 {
    match parsed {
        Ok(cli) => dispatch_command(cli.command),
        Err(err) => cli_parse_error_code(err),
    }
}

fn cli_parse_error_code(err: clap::Error) -> i32 {
    let _ = err.print();
    64
}

fn dispatch_command(command: Command) -> i32 {
    match command {
        Command::Return(args) => handle_return(args),
        Command::ListReturned(args) => handle_list_returned(args),
        Command::Show(args) => handle_show(args),
        Command::Version(args) => handle_version(args),
    }
}

fn handle_return(args: ReturnArgs) -> i32 {
    run_cli(handle_return_result(args))
}

fn handle_return_result(args: ReturnArgs) -> Result<(), MessengerError> {
    let json = args.json;
    let receipt = return_artifact(return_request(args)?)?;
    write_return_output(&receipt, json)
}

fn handle_list_returned(args: ListReturnedArgs) -> i32 {
    run_cli(handle_list_returned_result(args))
}

fn handle_list_returned_result(args: ListReturnedArgs) -> Result<(), MessengerError> {
    let json = args.json;
    let rows = list_returned(list_returned_request(args)?)?;
    write_list_output(&rows, json)
}

fn handle_show(args: ShowArgs) -> i32 {
    run_cli(handle_show_result(args))
}

fn handle_show_result(args: ShowArgs) -> Result<(), MessengerError> {
    let out = args.out.clone();
    let record = show_returned(show_request(args)?)?;
    write_record_content(out, record)
}

fn handle_version(args: VersionArgs) -> i32 {
    run_cli(write_version_output(args.json))
}

fn return_request(args: ReturnArgs) -> Result<ReturnRequest, MessengerError> {
    let ReturnArgs {
        db,
        invocation_uuid,
        name,
        scratchpad,
        scratchpad_version,
        body,
        content_file,
        content_stdin,
        format_hint,
        verdict_line,
        return_channel,
        json: _,
    } = args;
    let invocation_uuid = resolve_invocation_uuid(invocation_uuid)?;
    let source = resolve_return_source(ReturnSourceArgs {
        scratchpad,
        scratchpad_version,
        body,
        content_file,
        content_stdin,
    })?;
    let name = ReturnName::new(name)?;
    let return_channel = resolve_return_channel(return_channel)?;
    Ok(ReturnRequest {
        db_path: db,
        invocation_uuid,
        name,
        source,
        format_hint,
        verdict_line,
        return_channel: Some(return_channel),
    })
}

fn list_returned_request(args: ListReturnedArgs) -> Result<ListReturnedRequest, MessengerError> {
    Ok(ListReturnedRequest {
        db_path: args.db,
        invocation_uuid: resolve_invocation_uuid(args.invocation_uuid)?,
        name: args.name.map(ReturnName::new).transpose()?,
    })
}

fn show_request(args: ShowArgs) -> Result<ShowReturnedRequest, MessengerError> {
    let ShowArgs {
        db,
        version_id,
        invocation_uuid,
        name,
        version,
        out: _,
    } = args;
    let address = show_address_args(version_id, name)?;
    show_request_from_address(db, invocation_uuid, version, address)
}

fn show_address_args(
    version_id: Option<String>,
    name: Option<String>,
) -> Result<ShowAddressArgs, MessengerError> {
    match (version_id, name) {
        (Some(version_id), None) => Ok(ShowAddressArgs::VersionId(version_id)),
        (None, Some(name)) => Ok(ShowAddressArgs::Name(name)),
        _ => Err(MessengerError::InvalidInput(
            "pass exactly one of --version-id or --name".to_string(),
        )),
    }
}

fn show_request_from_address(
    db_path: PathBuf,
    invocation_uuid: Option<String>,
    version: Option<u64>,
    address: ShowAddressArgs,
) -> Result<ShowReturnedRequest, MessengerError> {
    match address {
        ShowAddressArgs::VersionId(version_id) => Ok(ShowReturnedRequest::VersionId {
            db_path,
            version_id,
        }),
        ShowAddressArgs::Name(name) => Ok(ShowReturnedRequest::Address {
            db_path,
            invocation_uuid: resolve_invocation_uuid(invocation_uuid)?,
            name: ReturnName::new(name)?,
            version,
        }),
    }
}

fn write_return_output(receipt: &ReturnedArtifact, json: bool) -> Result<(), MessengerError> {
    if json {
        return print_json(receipt);
    }
    writeln!(io::stdout(), "{} {}", receipt.version_id, receipt.sha256)?;
    Ok(())
}

fn write_list_output(rows: &[ReturnedArtifactMeta], json: bool) -> Result<(), MessengerError> {
    if json {
        return print_json(&rows);
    }
    write_list_rows(rows)
}

fn write_list_rows(rows: &[ReturnedArtifactMeta]) -> Result<(), MessengerError> {
    let mut stdout = io::stdout();
    for row in rows {
        writeln!(
            stdout,
            "{} v{} {}",
            row.name, row.store_address.version, row.sha256
        )?;
    }
    Ok(())
}

fn write_record_content(
    out: Option<PathBuf>,
    record: ReturnedArtifactRecord,
) -> Result<(), MessengerError> {
    if let Some(path) = out {
        return fs::write(path, record.content).map_err(MessengerError::from);
    }
    write_stdout_bytes(&record.content)
}

fn write_stdout_bytes(content: &[u8]) -> Result<(), MessengerError> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    handle.write_all(content)?;
    handle.flush()?;
    Ok(())
}

#[derive(Serialize)]
struct VersionEnvelope<'a> {
    package: &'a str,
    version: &'a str,
    receipt_schema_version: u32,
}

fn version_envelope() -> VersionEnvelope<'static> {
    VersionEnvelope {
        package: "oulipoly-agent-messenger",
        version: env!("CARGO_PKG_VERSION"),
        receipt_schema_version: RECEIPT_SCHEMA_VERSION,
    }
}

fn write_version_output(json: bool) -> Result<(), MessengerError> {
    if json {
        return print_json(&version_envelope());
    }
    writeln!(
        io::stdout(),
        "oulipoly-agent-messenger {}",
        env!("CARGO_PKG_VERSION")
    )?;
    Ok(())
}

fn resolve_return_source(args: ReturnSourceArgs) -> Result<ReturnSource, MessengerError> {
    validate_return_source_args(&args)?;
    selected_return_source(args)
}

fn validate_return_source_args(args: &ReturnSourceArgs) -> Result<(), MessengerError> {
    match (&args.scratchpad, inline_source_count(args)) {
        (Some(_), 1..) => Err(MessengerError::InvalidInput(
            "--scratchpad cannot be combined with inline content sources".to_string(),
        )),
        (None, 0) => Err(MessengerError::InvalidInput(
            "pass --scratchpad or exactly one of --body, --content-file, --content-stdin"
                .to_string(),
        )),
        (None, 2..) => Err(MessengerError::InvalidInput(
            "--body, --content-file, and --content-stdin are mutually exclusive".to_string(),
        )),
        _ => Ok(()),
    }
}

fn inline_source_count(args: &ReturnSourceArgs) -> usize {
    usize::from(args.body.is_some())
        + usize::from(args.content_file.is_some())
        + usize::from(args.content_stdin)
}

fn selected_return_source(args: ReturnSourceArgs) -> Result<ReturnSource, MessengerError> {
    if let Some(name) = args.scratchpad {
        return scratchpad_source(name, args.scratchpad_version);
    }
    inline_source(args)
}

fn scratchpad_source(name: String, version: Option<u64>) -> Result<ReturnSource, MessengerError> {
    Ok(ReturnSource::Scratchpad {
        name: oulipoly_agent_scratchpad::ScratchpadName::new(name)?,
        version,
    })
}

fn inline_source(args: ReturnSourceArgs) -> Result<ReturnSource, MessengerError> {
    Ok(ReturnSource::InlineBytes(inline_bytes(args)?))
}

fn inline_bytes(args: ReturnSourceArgs) -> Result<Vec<u8>, MessengerError> {
    if let Some(body) = args.body {
        return Ok(body.into_bytes());
    }
    if let Some(path) = args.content_file {
        return fs::read(path).map_err(MessengerError::from);
    }
    read_stdin_bytes()
}

fn read_stdin_bytes() -> Result<Vec<u8>, MessengerError> {
    let mut bytes = Vec::new();
    io::stdin().read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn resolve_invocation_uuid(explicit: Option<String>) -> Result<Uuid, MessengerError> {
    let value = invocation_uuid_text(explicit)?;
    parse_invocation_uuid(&value)
}

fn invocation_uuid_text(explicit: Option<String>) -> Result<String, MessengerError> {
    if let Some(value) = explicit {
        return Ok(value);
    }
    parent_invocation_id_from_env()
}

fn parent_invocation_id_from_env() -> Result<String, MessengerError> {
    let env_value = parent_invocation_env()?;
    parent_invocation_id(&env_value)
}

fn parent_invocation_env() -> Result<String, MessengerError> {
    let env_value = std::env::var("OULIPOLY_PARENT_INVOCATION")
        .map_err(|_| MessengerError::MissingInvocationScope)?;
    require_parent_invocation_env(&env_value)?;
    Ok(env_value)
}

fn parent_invocation_id(env_value: &str) -> Result<String, MessengerError> {
    let value = parse_parent_invocation_json(env_value)?;
    require_parent_invocation_id(value.get("id").and_then(Value::as_str))
}

fn parse_parent_invocation_json(env_value: &str) -> Result<Value, MessengerError> {
    serde_json::from_str(env_value).map_err(|err| {
        MessengerError::InvalidInvocationScope(format!(
            "OULIPOLY_PARENT_INVOCATION is not valid JSON: {err}"
        ))
    })
}

fn resolve_return_channel(explicit: Option<PathBuf>) -> Result<PathBuf, MessengerError> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    return_channel_from_env()
}

fn return_channel_from_env() -> Result<PathBuf, MessengerError> {
    let value = return_channel_env()?;
    Ok(PathBuf::from(value))
}

fn return_channel_env() -> Result<String, MessengerError> {
    let value = std::env::var("OULIPOLY_RETURN_CHANNEL")
        .map_err(|_| MessengerError::MissingReturnChannel)?;
    require_return_channel_env(&value)?;
    Ok(value)
}

fn parse_invocation_uuid(value: &str) -> Result<Uuid, MessengerError> {
    Uuid::parse_str(value).map_err(|err| {
        MessengerError::InvalidInvocationScope(format!("{value}: invalid UUID: {err}"))
    })
}

fn print_json(value: &impl Serialize) -> Result<(), MessengerError> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    serde_json::to_writer(&mut handle, value)?;
    handle.write_all(b"\n")?;
    Ok(())
}

fn run_cli(result: Result<(), MessengerError>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(err) => cli_error_exit_code(err),
    }
}

fn cli_error_exit_code(err: MessengerError) -> i32 {
    report_cli_error(&err);
    error_code(&err)
}

fn report_cli_error(err: &MessengerError) {
    eprintln!("{err}");
}

fn error_code(err: &MessengerError) -> i32 {
    match err {
        MessengerError::InvalidInput(_)
        | MessengerError::MissingInvocationScope
        | MessengerError::InvalidInvocationScope(_)
        | MessengerError::MissingReturnChannel
        | MessengerError::InvalidReturnChannel(_) => 64,
        MessengerError::NotFound => 65,
        MessengerError::Collision => 66,
        MessengerError::Serialization(_) => 70,
        MessengerError::Database(_)
        | MessengerError::MigrationRequired
        | MessengerError::IncompatibleSchema(_)
        | MessengerError::MetadataDecode(_) => 73,
        MessengerError::Io(_) => 74,
        MessengerError::Scratchpad(_) | MessengerError::Store(_) => 73,
    }
}
