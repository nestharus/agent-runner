use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chrono::{DateTime, Utc};
use clap::{Args, Parser, Subcommand};
use oulipoly_agent_store::TombstoneMeta;
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::store_adapter::private_workflow;
use crate::{
    CanonicalAddress, DeleteReceipt, DeleteRequest, DeleteSelector, GcReport, GcRequest,
    GcSelector, InvocationScope, ListRequest, PublishReceipt, PublishRequest, ReadRequest,
    Scratchpad, ScratchpadAddress, ScratchpadError, ScratchpadMeta, ScratchpadName, WriteReceipt,
    WriteRequest,
};

#[derive(Debug, Parser)]
#[command(name = "agent-scratchpad")]
#[command(about = "Invocation-scoped private artifact scratchpad")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Write(WriteArgs),
    Read(ReadArgs),
    List(ListArgs),
    Delete(DeleteArgs),
    Publish(PublishArgs),
    Gc(GcArgs),
    Scope(ScopeArgs),
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
struct ScopedArgs {
    #[arg(long)]
    invocation_uuid: Option<String>,
}

#[derive(Debug, Args)]
struct WriteArgs {
    #[arg(long)]
    db: PathBuf,
    #[command(flatten)]
    scope: ScopedArgs,
    #[arg(long)]
    name: String,
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
struct ReadArgs {
    #[arg(long)]
    db: PathBuf,
    #[command(flatten)]
    scope: ScopedArgs,
    #[arg(long)]
    name: String,
    #[arg(long)]
    version: Option<u64>,
    #[arg(long)]
    out: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct ListArgs {
    #[arg(long)]
    db: PathBuf,
    #[command(flatten)]
    scope: ScopedArgs,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    include_tombstoned: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
#[group(multiple = false)]
struct DeleteSelectorArgs {
    #[arg(long)]
    version: Option<u64>,
    #[arg(long)]
    all_versions: bool,
}

#[derive(Debug, Args)]
struct DeleteArgs {
    #[arg(long)]
    db: PathBuf,
    #[command(flatten)]
    scope: ScopedArgs,
    #[arg(long)]
    name: String,
    #[command(flatten)]
    selector: DeleteSelectorArgs,
    #[arg(long)]
    actor: Option<String>,
    #[arg(long)]
    reason: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct PublishArgs {
    #[arg(long)]
    db: PathBuf,
    #[command(flatten)]
    scope: ScopedArgs,
    #[arg(long)]
    name: String,
    #[arg(long)]
    workflow_run_id: String,
    #[arg(long)]
    artifact_name: String,
    #[arg(long)]
    version: Option<u64>,
    #[arg(long = "format")]
    format_hint: Option<String>,
    #[arg(long)]
    verdict_line: Option<String>,
    #[arg(long)]
    predecessor_version: Option<u64>,
    #[arg(long)]
    json: bool,
}

struct PublishRequestFields {
    workflow_run_id: String,
    artifact_name: String,
    version: Option<u64>,
    format_hint: Option<String>,
    verdict_line: Option<String>,
    predecessor_version: Option<u64>,
}

#[derive(Debug, Args)]
#[group(required = true, multiple = false)]
struct GcSelectorArgs {
    #[arg(long)]
    invocation_uuid: Option<String>,
    #[arg(long)]
    expired_before: Option<String>,
}

#[derive(Debug, Args)]
struct GcArgs {
    #[arg(long)]
    db: PathBuf,
    #[command(flatten)]
    selector: GcSelectorArgs,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    actor: Option<String>,
    #[arg(long)]
    reason: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ScopeArgs {
    #[arg(long)]
    invocation_uuid: String,
    #[arg(long)]
    json: bool,
}

pub fn run() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            let _ = err.print();
            return ExitCode::from(64);
        }
    };

    match cli.command {
        Command::Write(args) => handle_write(args),
        Command::Read(args) => handle_read(args),
        Command::List(args) => handle_list(args),
        Command::Delete(args) => handle_delete(args),
        Command::Publish(args) => handle_publish(args),
        Command::Gc(args) => handle_gc(args),
        Command::Scope(args) => handle_scope(args),
    }
}

fn handle_write(args: WriteArgs) -> ExitCode {
    run_cli(|| {
        let scope = resolve_scope(args.scope.invocation_uuid)?;
        let name = ScratchpadName::new(args.name)?;
        let content = read_content(args.content)?;
        let scratchpad = Scratchpad::open(args.db)?;
        let receipt = scratchpad.write(write_request(
            scope,
            name,
            content,
            args.format_hint,
            args.verdict_line,
        ))?;
        write_write_output(&receipt, args.json)?;
        Ok(())
    })
}

fn handle_read(args: ReadArgs) -> ExitCode {
    run_cli(|| {
        let scope = resolve_scope(args.scope.invocation_uuid)?;
        let name_for_error = args.name.clone();
        let name = ScratchpadName::new(args.name)?;
        let scratchpad = Scratchpad::open(args.db)?;
        let record = scratchpad
            .read(read_request(scope, name, args.version))
            .map_err(|err| with_name(err, &name_for_error))?;

        write_record_content(args.out, record.content)?;
        Ok(())
    })
}

fn handle_list(args: ListArgs) -> ExitCode {
    run_cli(|| {
        let scope = resolve_scope(args.scope.invocation_uuid)?;
        let scratchpad = Scratchpad::open(args.db)?;
        let rows = scratchpad.list(list_request(scope, args.name, args.include_tombstoned)?)?;
        write_list_output(&rows, args.json)?;
        Ok(())
    })
}

fn handle_delete(args: DeleteArgs) -> ExitCode {
    run_cli(|| {
        let scope = resolve_scope(args.scope.invocation_uuid)?;
        let name_for_error = args.name.clone();
        let name = ScratchpadName::new(args.name)?;
        let selector = delete_selector(args.selector)?;
        let scratchpad = Scratchpad::open(args.db)?;
        let receipt = scratchpad
            .delete(delete_request(
                scope,
                name,
                selector,
                args.actor,
                args.reason,
            ))
            .map_err(|err| with_name(err, &name_for_error))?;

        write_delete_output(&receipt, args.json)?;
        Ok(())
    })
}

fn handle_publish(args: PublishArgs) -> ExitCode {
    run_cli(|| {
        let PublishArgs {
            db,
            scope,
            name,
            workflow_run_id,
            artifact_name,
            version,
            format_hint,
            verdict_line,
            predecessor_version,
            json,
        } = args;
        let scope = resolve_scope(scope.invocation_uuid)?;
        let name_for_error = name.clone();
        let name = ScratchpadName::new(name)?;
        let scratchpad = Scratchpad::open(&db)?;
        let receipt = scratchpad
            .publish(publish_request(
                scope,
                name,
                PublishRequestFields {
                    workflow_run_id,
                    artifact_name,
                    version,
                    format_hint,
                    verdict_line,
                    predecessor_version,
                },
            ))
            .map_err(|err| with_name(err, &name_for_error))?;

        write_publish_output(&receipt, json)?;
        Ok(())
    })
}

fn handle_gc(args: GcArgs) -> ExitCode {
    run_cli(|| {
        let selector = gc_selector(args.selector)?;
        let scratchpad = Scratchpad::open(args.db)?;
        let report = scratchpad.gc(gc_request(selector, args.dry_run, args.actor, args.reason))?;
        write_gc_output(&report, args.json)?;
        Ok(())
    })
}

fn handle_scope(args: ScopeArgs) -> ExitCode {
    run_cli(|| {
        let invocation_uuid = parse_uuid(&args.invocation_uuid)?;
        write_scope_output(invocation_uuid, args.json)?;
        Ok(())
    })
}

fn write_request(
    scope: InvocationScope,
    name: ScratchpadName,
    content: Vec<u8>,
    format_hint: Option<String>,
    verdict_line: Option<String>,
) -> WriteRequest {
    WriteRequest {
        scope,
        name,
        content,
        format_hint,
        verdict_line,
        predecessor_version: None,
    }
}

fn read_request(scope: InvocationScope, name: ScratchpadName, version: Option<u64>) -> ReadRequest {
    ReadRequest {
        scope,
        name,
        version,
    }
}

fn list_request(
    scope: InvocationScope,
    name: Option<String>,
    include_tombstoned: bool,
) -> Result<ListRequest, ScratchpadError> {
    Ok(ListRequest {
        scope,
        name: name.map(ScratchpadName::new).transpose()?,
        include_tombstoned,
    })
}

fn delete_selector(args: DeleteSelectorArgs) -> Result<DeleteSelector, ScratchpadError> {
    match (args.version, args.all_versions) {
        (Some(version), false) => Ok(DeleteSelector::Version(version)),
        (None, true) => Ok(DeleteSelector::AllVersions),
        (None, false) => Ok(DeleteSelector::Latest),
        (Some(_), true) => Err(ScratchpadError::InvalidInput(
            "--version and --all-versions are mutually exclusive".to_string(),
        )),
    }
}

fn delete_request(
    scope: InvocationScope,
    name: ScratchpadName,
    selector: DeleteSelector,
    actor: Option<String>,
    reason: Option<String>,
) -> DeleteRequest {
    DeleteRequest {
        scope,
        name,
        selector,
        actor,
        reason,
    }
}

fn publish_request(
    scope: InvocationScope,
    name: ScratchpadName,
    fields: PublishRequestFields,
) -> PublishRequest {
    PublishRequest {
        source: ScratchpadAddress {
            invocation_uuid: scope.invocation_uuid,
            name,
        },
        source_version: fields.version,
        destination: CanonicalAddress {
            workflow_run_id: fields.workflow_run_id,
            artifact_name: fields.artifact_name,
        },
        format_hint: fields.format_hint,
        verdict_line: fields.verdict_line,
        predecessor_version: fields.predecessor_version,
    }
}

fn gc_selector(args: GcSelectorArgs) -> Result<GcSelector, ScratchpadError> {
    match (args.invocation_uuid, args.expired_before) {
        (Some(invocation_uuid), None) => Ok(GcSelector::Invocation(parse_uuid(&invocation_uuid)?)),
        (None, Some(expired_before)) => Ok(GcSelector::ExpiredBefore(parse_expired_before(
            &expired_before,
        )?)),
        _ => Err(ScratchpadError::InvalidInput(
            "pass exactly one of --invocation-uuid or --expired-before".to_string(),
        )),
    }
}

fn parse_expired_before(value: &str) -> Result<DateTime<Utc>, ScratchpadError> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|err| {
            ScratchpadError::InvalidInput(format!("invalid --expired-before {value}: {err}"))
        })
}

fn gc_request(
    selector: GcSelector,
    dry_run: bool,
    actor: Option<String>,
    reason: Option<String>,
) -> GcRequest {
    GcRequest {
        selector,
        dry_run,
        actor,
        reason,
    }
}

fn write_write_output(receipt: &WriteReceipt, json: bool) -> Result<(), ScratchpadError> {
    if json {
        print_json(&WriteEnvelope::from_receipt(receipt))
    } else {
        writeln!(
            io::stdout(),
            "{} v{} {}",
            receipt.address.name.as_str(),
            receipt.version,
            receipt.sha256
        )?;
        Ok(())
    }
}

fn write_record_content(out: Option<PathBuf>, content: Vec<u8>) -> Result<(), ScratchpadError> {
    if let Some(path) = out {
        return write_file(&path, content);
    }

    let stdout = io::stdout();
    let mut handle = stdout.lock();
    handle.write_all(&content)?;
    handle.flush()?;
    Ok(())
}

fn write_list_output(rows: &[ScratchpadMeta], json: bool) -> Result<(), ScratchpadError> {
    if json {
        let envelopes: Vec<_> = rows.iter().map(MetaEnvelope::from_meta).collect();
        return print_json(&envelopes);
    }

    let mut stdout = io::stdout();
    for row in rows {
        writeln!(
            stdout,
            "{} v{} {}",
            row.name.as_str(),
            row.version,
            row.sha256
        )?;
    }
    Ok(())
}

fn write_delete_output(receipt: &DeleteReceipt, json: bool) -> Result<(), ScratchpadError> {
    if json {
        print_json(&DeleteEnvelope::from_receipt(receipt))
    } else {
        writeln!(
            io::stdout(),
            "{} tombstoned={} already_tombstoned={}",
            receipt.address.name.as_str(),
            receipt.tombstoned_versions.len(),
            receipt.already_tombstoned_versions.len()
        )?;
        Ok(())
    }
}

fn write_publish_output(receipt: &PublishReceipt, json: bool) -> Result<(), ScratchpadError> {
    if json {
        print_json(&PublishEnvelope::from_receipt(receipt))
    } else {
        writeln!(
            io::stdout(),
            "{} -> {} {} v{} {}",
            receipt.source.name.as_str(),
            receipt.destination.workflow_run_id,
            receipt.destination.artifact_name,
            receipt.destination_version,
            receipt.destination_sha256
        )?;
        Ok(())
    }
}

fn write_gc_output(report: &GcReport, json: bool) -> Result<(), ScratchpadError> {
    if json {
        print_json(&GcEnvelope::from_report(report))
    } else {
        writeln!(
            io::stdout(),
            "gc dry_run={} tombstoned={}",
            report.dry_run,
            report.tombstoned_rows.len()
        )?;
        Ok(())
    }
}

fn write_scope_output(invocation_uuid: Uuid, json: bool) -> Result<(), ScratchpadError> {
    if json {
        print_json(&ScopeEnvelope {
            invocation_uuid: invocation_uuid.to_string(),
            workflow_run_id: private_workflow(invocation_uuid),
        })
    } else {
        writeln!(io::stdout(), "{}", private_workflow(invocation_uuid))?;
        Ok(())
    }
}

fn run_cli(op: impl FnOnce() -> Result<(), ScratchpadError>) -> ExitCode {
    match op() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(error_code(&err))
        }
    }
}

fn error_code(err: &ScratchpadError) -> u8 {
    match err {
        ScratchpadError::InvalidInput(_)
        | ScratchpadError::MissingInvocationScope
        | ScratchpadError::InvalidInvocationScope(_) => 64,
        ScratchpadError::NotFound | ScratchpadError::NotFoundNamed(_) => 65,
        ScratchpadError::Collision => 66,
        ScratchpadError::Serialization(_) => 70,
        ScratchpadError::Database(_)
        | ScratchpadError::MigrationRequired
        | ScratchpadError::IncompatibleSchema
        | ScratchpadError::MetadataDecode(_) => 73,
        ScratchpadError::Io(_) => 74,
    }
}

fn resolve_scope(explicit: Option<String>) -> Result<InvocationScope, ScratchpadError> {
    if let Some(value) = explicit {
        return scope_from_uuid_text(&value);
    }

    scope_from_parent_env()
}

fn scope_from_uuid_text(value: &str) -> Result<InvocationScope, ScratchpadError> {
    Ok(InvocationScope {
        invocation_uuid: parse_uuid(value)?,
    })
}

fn scope_from_parent_env() -> Result<InvocationScope, ScratchpadError> {
    let env_value = parent_invocation_env()?;
    let id = parent_invocation_id(&env_value)?;
    scope_from_uuid_text(&id)
}

fn parent_invocation_env() -> Result<String, ScratchpadError> {
    let env_value = std::env::var("OULIPOLY_PARENT_INVOCATION")
        .map_err(|_| ScratchpadError::MissingInvocationScope)?;
    require_parent_invocation_env(&env_value)?;
    Ok(env_value)
}

fn require_parent_invocation_env(value: &str) -> Result<(), ScratchpadError> {
    if value.trim().is_empty() {
        return Err(ScratchpadError::MissingInvocationScope);
    }
    Ok(())
}

fn parent_invocation_id(env_value: &str) -> Result<String, ScratchpadError> {
    let value: Value = serde_json::from_str(env_value).map_err(|err| {
        ScratchpadError::InvalidInvocationScope(format!(
            "OULIPOLY_PARENT_INVOCATION is not valid JSON: {err}"
        ))
    })?;
    value
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            ScratchpadError::InvalidInvocationScope(
                "OULIPOLY_PARENT_INVOCATION is missing id".to_string(),
            )
        })
}

fn parse_uuid(value: &str) -> Result<Uuid, ScratchpadError> {
    Uuid::parse_str(value).map_err(|err| {
        ScratchpadError::InvalidInvocationScope(format!("{value}: invalid UUID: {err}"))
    })
}

fn with_name(err: ScratchpadError, name: &str) -> ScratchpadError {
    match err {
        ScratchpadError::NotFound => ScratchpadError::NotFoundNamed(name.to_string()),
        other => other,
    }
}

fn read_content(input: ContentInput) -> Result<Vec<u8>, ScratchpadError> {
    if let Some(path) = input.content_file {
        return read_file(&path);
    }

    let mut content = Vec::new();
    io::stdin().read_to_end(&mut content)?;
    Ok(content)
}

fn read_file(path: &Path) -> Result<Vec<u8>, ScratchpadError> {
    fs::read(path).map_err(|err| {
        ScratchpadError::Io(io::Error::new(
            err.kind(),
            format!("read {}: {err}", path.display()),
        ))
    })
}

fn write_file(path: &Path, content: Vec<u8>) -> Result<(), ScratchpadError> {
    fs::write(path, content).map_err(|err| {
        ScratchpadError::Io(io::Error::new(
            err.kind(),
            format!("write {}: {err}", path.display()),
        ))
    })
}

fn print_json(value: &impl Serialize) -> Result<(), ScratchpadError> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    serde_json::to_writer(&mut handle, value)?;
    writeln!(handle)?;
    Ok(())
}

#[derive(Serialize)]
struct ScopeEnvelope {
    invocation_uuid: String,
    workflow_run_id: String,
}

#[derive(Serialize)]
struct AddressEnvelope {
    invocation_uuid: String,
    name: String,
}

impl AddressEnvelope {
    fn from_address(address: &ScratchpadAddress) -> Self {
        Self {
            invocation_uuid: address.invocation_uuid.to_string(),
            name: address.name.as_str().to_string(),
        }
    }
}

#[derive(Serialize)]
struct CanonicalAddressEnvelope {
    workflow_run_id: String,
    artifact_name: String,
}

impl CanonicalAddressEnvelope {
    fn from_address(address: &CanonicalAddress) -> Self {
        Self {
            workflow_run_id: address.workflow_run_id.clone(),
            artifact_name: address.artifact_name.clone(),
        }
    }
}

#[derive(Serialize)]
struct TombstoneEnvelope {
    tombstoned_at: String,
    actor: String,
    reason: String,
}

impl TombstoneEnvelope {
    fn from_meta(meta: &TombstoneMeta) -> Self {
        Self {
            tombstoned_at: meta.tombstoned_at.to_rfc3339(),
            actor: meta.actor.clone(),
            reason: meta.reason.clone(),
        }
    }
}

#[derive(Serialize)]
struct WriteEnvelope {
    address: AddressEnvelope,
    invocation_uuid: String,
    name: String,
    version: u64,
    producer_invocation_uuid: Option<String>,
    sha256: String,
    content_len: u64,
    format_hint: Option<String>,
    verdict_line: Option<String>,
    predecessor_version: Option<u64>,
    created_at: String,
}

impl WriteEnvelope {
    fn from_receipt(receipt: &WriteReceipt) -> Self {
        Self {
            address: AddressEnvelope::from_address(&receipt.address),
            invocation_uuid: receipt.address.invocation_uuid.to_string(),
            name: receipt.address.name.as_str().to_string(),
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
    address: AddressEnvelope,
    invocation_uuid: String,
    name: String,
    version: u64,
    sha256: String,
    content_len: u64,
    producer_invocation_uuid: Option<String>,
    format_hint: Option<String>,
    verdict_line: Option<String>,
    predecessor_version: Option<u64>,
    created_at: String,
    tombstone: Option<TombstoneEnvelope>,
}

impl MetaEnvelope {
    fn from_meta(meta: &ScratchpadMeta) -> Self {
        Self {
            address: AddressEnvelope::from_address(&meta.address),
            invocation_uuid: meta.invocation_uuid.to_string(),
            name: meta.name.as_str().to_string(),
            version: meta.version,
            sha256: meta.sha256.clone(),
            content_len: meta.content_len,
            producer_invocation_uuid: meta.producer_invocation_uuid.map(|uuid| uuid.to_string()),
            format_hint: meta.format_hint.clone(),
            verdict_line: meta.verdict_line.clone(),
            predecessor_version: meta.predecessor_version,
            created_at: meta.created_at.to_rfc3339(),
            tombstone: meta.tombstone.as_ref().map(TombstoneEnvelope::from_meta),
        }
    }
}

#[derive(Serialize)]
struct DeleteEnvelope {
    address: AddressEnvelope,
    selector: String,
    tombstoned_versions: Vec<u64>,
    already_tombstoned_versions: Vec<u64>,
    actor: String,
    reason: String,
    tombstoned_at: Option<String>,
}

impl DeleteEnvelope {
    fn from_receipt(receipt: &DeleteReceipt) -> Self {
        Self {
            address: AddressEnvelope::from_address(&receipt.address),
            selector: selector_name(&receipt.selector),
            tombstoned_versions: receipt.tombstoned_versions.clone(),
            already_tombstoned_versions: receipt.already_tombstoned_versions.clone(),
            actor: receipt.actor.clone(),
            reason: receipt.reason.clone(),
            tombstoned_at: receipt.tombstoned_at.map(|value| value.to_rfc3339()),
        }
    }
}

#[derive(Serialize)]
struct PublishEnvelope {
    source: AddressEnvelope,
    source_version: u64,
    source_sha256: String,
    destination: CanonicalAddressEnvelope,
    destination_version: u64,
    destination_sha256: String,
    content_len: u64,
    producer_invocation_uuid: String,
    format_hint: Option<String>,
    verdict_line: Option<String>,
    predecessor_version: Option<u64>,
    created_at: String,
}

impl PublishEnvelope {
    fn from_receipt(receipt: &PublishReceipt) -> Self {
        Self {
            source: AddressEnvelope::from_address(&receipt.source),
            source_version: receipt.source_version,
            source_sha256: receipt.source_sha256.clone(),
            destination: CanonicalAddressEnvelope::from_address(&receipt.destination),
            destination_version: receipt.destination_version,
            destination_sha256: receipt.destination_sha256.clone(),
            content_len: receipt.content_len,
            producer_invocation_uuid: receipt.producer_invocation_uuid.to_string(),
            format_hint: receipt.format_hint.clone(),
            verdict_line: receipt.verdict_line.clone(),
            predecessor_version: receipt.predecessor_version,
            created_at: receipt.created_at.to_rfc3339(),
        }
    }
}

#[derive(Serialize)]
struct GcEnvelope {
    selector: String,
    dry_run: bool,
    tombstoned_rows: Vec<AddressEnvelope>,
    already_tombstoned_rows: Vec<AddressEnvelope>,
    actor: String,
    reason: String,
    evaluated_at: String,
}

impl GcEnvelope {
    fn from_report(report: &GcReport) -> Self {
        Self {
            selector: gc_selector_name(&report.selector),
            dry_run: report.dry_run,
            tombstoned_rows: report
                .tombstoned_rows
                .iter()
                .map(AddressEnvelope::from_address)
                .collect(),
            already_tombstoned_rows: report
                .already_tombstoned_rows
                .iter()
                .map(AddressEnvelope::from_address)
                .collect(),
            actor: report.actor.clone(),
            reason: report.reason.clone(),
            evaluated_at: report.evaluated_at.to_rfc3339(),
        }
    }
}

fn selector_name(selector: &DeleteSelector) -> String {
    match selector {
        DeleteSelector::Latest => "latest".to_string(),
        DeleteSelector::Version(version) => format!("version:{version}"),
        DeleteSelector::AllVersions => "all_versions".to_string(),
    }
}

fn gc_selector_name(selector: &GcSelector) -> String {
    match selector {
        GcSelector::Invocation(uuid) => format!("invocation:{uuid}"),
        GcSelector::ExpiredBefore(cutoff) => format!("expired_before:{}", cutoff.to_rfc3339()),
    }
}
