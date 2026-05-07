use std::error::Error;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chrono::{DateTime, TimeDelta, Utc};
use oulipoly_agent_store::{
    ArtifactKey, ArtifactMeta, ArtifactRecord, ListFilter, PutReceipt, PutRequest, Store,
    StoreError, TombstoneMeta, TombstoneStatus,
};
use uuid::Uuid;

const SCRATCHPAD_PREFIX: &str = "scratchpad:";
const DEFAULT_DELETE_ACTOR: &str = "agent-scratchpad";
const DEFAULT_DELETE_REASON: &str = "scratchpad delete";
const DEFAULT_GC_ACTOR: &str = "agent-scratchpad-gc";
const DEFAULT_GC_INVOCATION_REASON: &str = "scratchpad gc invocation";
const DEFAULT_GC_EXPIRED_REASON: &str = "scratchpad gc expired";
const TTL_DAYS: i64 = 7;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScratchpadName(String);

impl ScratchpadName {
    pub fn new(value: impl Into<String>) -> Result<Self, ScratchpadError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ScratchpadError::InvalidInput(
                "scratchpad name must not be empty".to_string(),
            ));
        }
        if value.starts_with(SCRATCHPAD_PREFIX) {
            return Err(ScratchpadError::InvalidInput(format!(
                "scratchpad name must not start with reserved prefix {SCRATCHPAD_PREFIX}"
            )));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvocationScope {
    pub invocation_uuid: Uuid,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScratchpadAddress {
    pub invocation_uuid: Uuid,
    pub name: ScratchpadName,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalAddress {
    pub workflow_run_id: String,
    pub artifact_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WriteRequest {
    pub scope: InvocationScope,
    pub name: ScratchpadName,
    pub content: Vec<u8>,
    pub format_hint: Option<String>,
    pub verdict_line: Option<String>,
    pub predecessor_version: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadRequest {
    pub scope: InvocationScope,
    pub name: ScratchpadName,
    pub version: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListRequest {
    pub scope: InvocationScope,
    pub name: Option<ScratchpadName>,
    pub include_tombstoned: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeleteRequest {
    pub scope: InvocationScope,
    pub name: ScratchpadName,
    pub selector: DeleteSelector,
    pub actor: Option<String>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeleteSelector {
    Latest,
    Version(u64),
    AllVersions,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishRequest {
    pub source: ScratchpadAddress,
    pub source_version: Option<u64>,
    pub destination: CanonicalAddress,
    pub format_hint: Option<String>,
    pub verdict_line: Option<String>,
    pub predecessor_version: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GcRequest {
    pub selector: GcSelector,
    pub dry_run: bool,
    pub actor: Option<String>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GcSelector {
    Invocation(Uuid),
    ExpiredBefore(DateTime<Utc>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScratchpadMeta {
    pub address: ScratchpadAddress,
    pub invocation_uuid: Uuid,
    pub name: ScratchpadName,
    pub version: u64,
    pub sha256: String,
    pub content_len: u64,
    pub producer_invocation_uuid: Option<Uuid>,
    pub format_hint: Option<String>,
    pub verdict_line: Option<String>,
    pub predecessor_version: Option<u64>,
    pub created_at: DateTime<Utc>,
    pub tombstone: Option<TombstoneMeta>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScratchpadRecord {
    pub meta: ScratchpadMeta,
    pub content: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WriteReceipt {
    pub address: ScratchpadAddress,
    pub version: u64,
    pub producer_invocation_uuid: Option<Uuid>,
    pub sha256: String,
    pub content_len: u64,
    pub format_hint: Option<String>,
    pub verdict_line: Option<String>,
    pub predecessor_version: Option<u64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeleteReceipt {
    pub address: ScratchpadAddress,
    pub selector: DeleteSelector,
    pub tombstoned_versions: Vec<u64>,
    pub already_tombstoned_versions: Vec<u64>,
    pub actor: String,
    pub reason: String,
    pub tombstoned_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishReceipt {
    pub source: ScratchpadAddress,
    pub source_version: u64,
    pub source_sha256: String,
    pub destination: CanonicalAddress,
    pub destination_version: u64,
    pub destination_sha256: String,
    pub content_len: u64,
    pub producer_invocation_uuid: Uuid,
    pub format_hint: Option<String>,
    pub verdict_line: Option<String>,
    pub predecessor_version: Option<u64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GcReport {
    pub selector: GcSelector,
    pub dry_run: bool,
    pub tombstoned_rows: Vec<ScratchpadAddress>,
    pub already_tombstoned_rows: Vec<ScratchpadAddress>,
    pub actor: String,
    pub reason: String,
    pub evaluated_at: DateTime<Utc>,
}

#[derive(Debug)]
pub enum ScratchpadError {
    InvalidInput(String),
    MissingInvocationScope,
    InvalidInvocationScope(String),
    NotFound,
    NotFoundNamed(String),
    Collision,
    Io(std::io::Error),
    Database(rusqlite::Error),
    MigrationRequired,
    IncompatibleSchema,
    Serialization(serde_json::Error),
    MetadataDecode(String),
}

impl fmt::Display for ScratchpadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(message) => write!(f, "invalid input: {message}"),
            Self::MissingInvocationScope => write!(
                f,
                "missing invocation scope: pass --invocation-uuid or set OULIPOLY_PARENT_INVOCATION"
            ),
            Self::InvalidInvocationScope(message) => {
                write!(f, "invalid invocation scope: {message}")
            }
            Self::NotFound => write!(f, "scratchpad artifact not found"),
            Self::NotFoundNamed(name) => write!(f, "scratchpad artifact not found: {name}"),
            Self::Collision => write!(f, "backing store collision"),
            Self::Io(err) => write!(f, "io error: {err}"),
            Self::Database(err) => write!(f, "database error: {err}"),
            Self::MigrationRequired => write!(f, "database schema migration required"),
            Self::IncompatibleSchema => write!(f, "incompatible database schema"),
            Self::Serialization(err) => write!(f, "json serialization error: {err}"),
            Self::MetadataDecode(message) => write!(f, "metadata decode error: {message}"),
        }
    }
}

impl Error for ScratchpadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Database(err) => Some(err),
            Self::Serialization(err) => Some(err),
            _ => None,
        }
    }
}

impl From<StoreError> for ScratchpadError {
    fn from(value: StoreError) -> Self {
        match value {
            StoreError::InvalidInput(message) => Self::InvalidInput(message),
            StoreError::NotFound => Self::NotFound,
            StoreError::Collision => Self::Collision,
            StoreError::Io(err) => Self::Io(err),
            StoreError::Database(err) => Self::Database(err),
            StoreError::MigrationRequired => Self::MigrationRequired,
            StoreError::IncompatibleSchema(_) => Self::IncompatibleSchema,
        }
    }
}

impl From<io::Error> for ScratchpadError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for ScratchpadError {
    fn from(value: serde_json::Error) -> Self {
        Self::Serialization(value)
    }
}

pub struct Scratchpad {
    store: Store,
}

impl Scratchpad {
    pub fn open(db_path: impl AsRef<Path>) -> Result<Self, ScratchpadError> {
        let db_path = db_path.as_ref();
        let store = Store::open(db_path)?;
        install_store_aliases(db_path)?;
        Ok(Self { store })
    }

    pub fn write(&self, req: WriteRequest) -> Result<WriteReceipt, ScratchpadError> {
        let key = private_key(req.scope.invocation_uuid, &req.name);
        let receipt = self.store.put(PutRequest {
            key,
            producer_invocation_uuid: Some(req.scope.invocation_uuid),
            format_hint: req.format_hint,
            verdict_line: req.verdict_line,
            predecessor_version: req.predecessor_version,
            content: req.content,
        })?;
        Ok(write_receipt_from_put(
            receipt,
            req.scope.invocation_uuid,
            req.name,
        ))
    }

    pub fn read(&self, req: ReadRequest) -> Result<ScratchpadRecord, ScratchpadError> {
        let key = private_key(req.scope.invocation_uuid, &req.name);
        let record = self.store.get(&key, req.version)?;
        record_from_store(record)
    }

    pub fn list(&self, req: ListRequest) -> Result<Vec<ScratchpadMeta>, ScratchpadError> {
        self.store
            .list(ListFilter {
                workflow_run_id: Some(private_workflow(req.scope.invocation_uuid)),
                artifact_name: req.name.as_ref().map(|name| name.as_str().to_string()),
                include_tombstoned: req.include_tombstoned,
            })?
            .into_iter()
            .map(meta_from_store)
            .collect()
    }

    pub fn delete(&self, req: DeleteRequest) -> Result<DeleteReceipt, ScratchpadError> {
        let key = private_key(req.scope.invocation_uuid, &req.name);
        let actor = req
            .actor
            .unwrap_or_else(|| DEFAULT_DELETE_ACTOR.to_string());
        let reason = req
            .reason
            .unwrap_or_else(|| DEFAULT_DELETE_REASON.to_string());
        let versions = self.delete_versions(&key, &req.selector)?;
        if matches!(req.selector, DeleteSelector::Latest) && versions.is_empty() {
            return Err(ScratchpadError::NotFound);
        }

        let mut tombstoned_versions = Vec::new();
        let mut already_tombstoned_versions = Vec::new();
        let mut tombstoned_at = None;

        for version in versions {
            let receipt = self.store.tombstone(&key, version, &actor, &reason)?;
            tombstoned_at = Some(receipt.tombstone.tombstoned_at);
            match receipt.status {
                TombstoneStatus::Tombstoned => tombstoned_versions.push(version),
                TombstoneStatus::AlreadyTombstoned => already_tombstoned_versions.push(version),
            }
        }

        Ok(DeleteReceipt {
            address: ScratchpadAddress {
                invocation_uuid: req.scope.invocation_uuid,
                name: req.name,
            },
            selector: req.selector,
            tombstoned_versions,
            already_tombstoned_versions,
            actor,
            reason,
            tombstoned_at,
        })
    }

    pub fn publish(&self, req: PublishRequest) -> Result<PublishReceipt, ScratchpadError> {
        if req
            .destination
            .workflow_run_id
            .starts_with(SCRATCHPAD_PREFIX)
        {
            return Err(ScratchpadError::InvalidInput(format!(
                "canonical workflow_run_id must not start with reserved prefix {SCRATCHPAD_PREFIX}"
            )));
        }

        let source_key = private_key(req.source.invocation_uuid, &req.source.name);
        let source = self.store.get(&source_key, req.source_version)?;
        let destination = req.destination;
        let format_hint = req.format_hint.or_else(|| source.meta.format_hint.clone());
        let verdict_line = req
            .verdict_line
            .or_else(|| source.meta.verdict_line.clone());
        let destination_receipt = self.store.put(PutRequest {
            key: ArtifactKey {
                workflow_run_id: destination.workflow_run_id.clone(),
                artifact_name: destination.artifact_name.clone(),
            },
            producer_invocation_uuid: Some(req.source.invocation_uuid),
            format_hint,
            verdict_line,
            predecessor_version: req.predecessor_version,
            content: source.content,
        })?;

        Ok(PublishReceipt {
            source: req.source.clone(),
            source_version: source.meta.version,
            source_sha256: source.meta.sha256,
            destination,
            destination_version: destination_receipt.version,
            destination_sha256: destination_receipt.sha256,
            content_len: destination_receipt.content_len,
            producer_invocation_uuid: req.source.invocation_uuid,
            format_hint: destination_receipt.format_hint,
            verdict_line: destination_receipt.verdict_line,
            predecessor_version: destination_receipt.predecessor_version,
            created_at: destination_receipt.created_at,
        })
    }

    pub fn gc(&self, req: GcRequest) -> Result<GcReport, ScratchpadError> {
        let evaluated_at = Utc::now();
        let actor = req.actor.unwrap_or_else(|| DEFAULT_GC_ACTOR.to_string());
        let reason = req.reason.unwrap_or_else(|| match req.selector {
            GcSelector::Invocation(_) => DEFAULT_GC_INVOCATION_REASON.to_string(),
            GcSelector::ExpiredBefore(_) => DEFAULT_GC_EXPIRED_REASON.to_string(),
        });
        let candidates = self.gc_candidates(&req.selector)?;
        let mut tombstoned_rows = Vec::new();
        let mut already_tombstoned_rows = Vec::new();

        for meta in candidates {
            let address = meta.address.clone();
            if req.dry_run {
                tombstoned_rows.push(address);
                continue;
            }

            let key = private_key(address.invocation_uuid, &address.name);
            let receipt = self.store.tombstone(&key, meta.version, &actor, &reason)?;
            match receipt.status {
                TombstoneStatus::Tombstoned => tombstoned_rows.push(address),
                TombstoneStatus::AlreadyTombstoned => already_tombstoned_rows.push(address),
            }
        }

        Ok(GcReport {
            selector: req.selector,
            dry_run: req.dry_run,
            tombstoned_rows,
            already_tombstoned_rows,
            actor,
            reason,
            evaluated_at,
        })
    }

    fn delete_versions(
        &self,
        key: &ArtifactKey,
        selector: &DeleteSelector,
    ) -> Result<Vec<u64>, ScratchpadError> {
        match selector {
            DeleteSelector::Latest => Ok(vec![self.store.get_meta(key, None)?.version]),
            DeleteSelector::Version(version) => {
                self.store.get_meta(key, Some(*version))?;
                Ok(vec![*version])
            }
            DeleteSelector::AllVersions => Ok(self
                .store
                .list(ListFilter {
                    workflow_run_id: Some(key.workflow_run_id.clone()),
                    artifact_name: Some(key.artifact_name.clone()),
                    include_tombstoned: false,
                })?
                .into_iter()
                .map(|meta| meta.version)
                .collect()),
        }
    }

    fn gc_candidates(&self, selector: &GcSelector) -> Result<Vec<ScratchpadMeta>, ScratchpadError> {
        match selector {
            GcSelector::Invocation(invocation_uuid) => self
                .store
                .list(ListFilter {
                    workflow_run_id: Some(private_workflow(*invocation_uuid)),
                    artifact_name: None,
                    include_tombstoned: false,
                })?
                .into_iter()
                .map(meta_from_store)
                .collect(),
            GcSelector::ExpiredBefore(cutoff) => {
                let ttl = TimeDelta::days(TTL_DAYS);
                self.store
                    .list(ListFilter {
                        workflow_run_id: None,
                        artifact_name: None,
                        include_tombstoned: false,
                    })?
                    .into_iter()
                    .filter(|meta| meta.key.workflow_run_id.starts_with(SCRATCHPAD_PREFIX))
                    .filter(|meta| meta.created_at + ttl <= *cutoff)
                    .map(meta_from_store)
                    .collect()
            }
        }
    }
}

fn install_store_aliases(db_path: &Path) -> Result<(), ScratchpadError> {
    let conn = rusqlite::Connection::open(db_path).map_err(ScratchpadError::Database)?;
    conn.execute_batch(
        r#"
        CREATE VIEW IF NOT EXISTS artifacts AS
            SELECT * FROM artifact_versions;

        CREATE TRIGGER IF NOT EXISTS artifacts_update_created_at
        INSTEAD OF UPDATE OF created_at ON artifacts
        BEGIN
            UPDATE artifact_versions
               SET created_at = NEW.created_at
             WHERE workflow_run_id = OLD.workflow_run_id
               AND artifact_name = OLD.artifact_name
               AND version = OLD.version;
        END;
        "#,
    )
    .map_err(ScratchpadError::Database)?;
    Ok(())
}

fn private_workflow(invocation_uuid: Uuid) -> String {
    format!("{SCRATCHPAD_PREFIX}{invocation_uuid}")
}

fn private_key(invocation_uuid: Uuid, name: &ScratchpadName) -> ArtifactKey {
    ArtifactKey {
        workflow_run_id: private_workflow(invocation_uuid),
        artifact_name: name.as_str().to_string(),
    }
}

fn parse_private_workflow(workflow_run_id: &str) -> Result<Uuid, ScratchpadError> {
    let raw = workflow_run_id
        .strip_prefix(SCRATCHPAD_PREFIX)
        .ok_or_else(|| {
            ScratchpadError::MetadataDecode(format!(
                "workflow_run_id {workflow_run_id:?} is not a scratchpad row"
            ))
        })?;
    Uuid::parse_str(raw).map_err(|err| {
        ScratchpadError::MetadataDecode(format!(
            "workflow_run_id {workflow_run_id:?} has invalid scratchpad UUID: {err}"
        ))
    })
}

fn meta_from_store(meta: ArtifactMeta) -> Result<ScratchpadMeta, ScratchpadError> {
    let invocation_uuid = parse_private_workflow(&meta.key.workflow_run_id)?;
    let name = ScratchpadName::new(meta.key.artifact_name)?;
    let address = ScratchpadAddress {
        invocation_uuid,
        name: name.clone(),
    };
    Ok(ScratchpadMeta {
        address,
        invocation_uuid,
        name,
        version: meta.version,
        sha256: meta.sha256,
        content_len: meta.content_len,
        producer_invocation_uuid: meta.producer_invocation_uuid,
        format_hint: meta.format_hint,
        verdict_line: meta.verdict_line,
        predecessor_version: meta.predecessor_version,
        created_at: meta.created_at,
        tombstone: meta.tombstone,
    })
}

fn record_from_store(record: ArtifactRecord) -> Result<ScratchpadRecord, ScratchpadError> {
    Ok(ScratchpadRecord {
        meta: meta_from_store(record.meta)?,
        content: record.content,
    })
}

fn write_receipt_from_put(
    receipt: PutReceipt,
    invocation_uuid: Uuid,
    name: ScratchpadName,
) -> WriteReceipt {
    WriteReceipt {
        address: ScratchpadAddress {
            invocation_uuid,
            name,
        },
        version: receipt.version,
        producer_invocation_uuid: receipt.producer_invocation_uuid,
        sha256: receipt.sha256,
        content_len: receipt.content_len,
        format_hint: receipt.format_hint,
        verdict_line: receipt.verdict_line,
        predecessor_version: receipt.predecessor_version,
        created_at: receipt.created_at,
    }
}

pub mod cli {
    use clap::{Args, Parser, Subcommand};
    use serde::Serialize;
    use serde_json::Value;

    use super::*;

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
            let receipt = scratchpad.write(WriteRequest {
                scope,
                name,
                content,
                format_hint: args.format_hint,
                verdict_line: args.verdict_line,
                predecessor_version: None,
            })?;

            if args.json {
                print_json(&WriteEnvelope::from_receipt(&receipt))?;
            } else {
                writeln!(
                    io::stdout(),
                    "{} v{} {}",
                    receipt.address.name.as_str(),
                    receipt.version,
                    receipt.sha256
                )?;
            }
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
                .read(ReadRequest {
                    scope,
                    name,
                    version: args.version,
                })
                .map_err(|err| with_name(err, &name_for_error))?;

            if let Some(path) = args.out {
                write_file(&path, record.content)?;
            } else {
                let stdout = io::stdout();
                let mut handle = stdout.lock();
                handle.write_all(&record.content)?;
                handle.flush()?;
            }
            Ok(())
        })
    }

    fn handle_list(args: ListArgs) -> ExitCode {
        run_cli(|| {
            let scope = resolve_scope(args.scope.invocation_uuid)?;
            let scratchpad = Scratchpad::open(args.db)?;
            let rows = scratchpad.list(ListRequest {
                scope,
                name: args.name.map(ScratchpadName::new).transpose()?,
                include_tombstoned: args.include_tombstoned,
            })?;

            if args.json {
                let envelopes: Vec<_> = rows.iter().map(MetaEnvelope::from_meta).collect();
                print_json(&envelopes)?;
            } else {
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
            }
            Ok(())
        })
    }

    fn handle_delete(args: DeleteArgs) -> ExitCode {
        run_cli(|| {
            let scope = resolve_scope(args.scope.invocation_uuid)?;
            let name_for_error = args.name.clone();
            let name = ScratchpadName::new(args.name)?;
            let selector = match (args.selector.version, args.selector.all_versions) {
                (Some(version), false) => DeleteSelector::Version(version),
                (None, true) => DeleteSelector::AllVersions,
                (None, false) => DeleteSelector::Latest,
                (Some(_), true) => {
                    return Err(ScratchpadError::InvalidInput(
                        "--version and --all-versions are mutually exclusive".to_string(),
                    ));
                }
            };
            let scratchpad = Scratchpad::open(args.db)?;
            let receipt = scratchpad
                .delete(DeleteRequest {
                    scope,
                    name,
                    selector,
                    actor: args.actor,
                    reason: args.reason,
                })
                .map_err(|err| with_name(err, &name_for_error))?;

            if args.json {
                print_json(&DeleteEnvelope::from_receipt(&receipt))?;
            } else {
                writeln!(
                    io::stdout(),
                    "{} tombstoned={} already_tombstoned={}",
                    receipt.address.name.as_str(),
                    receipt.tombstoned_versions.len(),
                    receipt.already_tombstoned_versions.len()
                )?;
            }
            Ok(())
        })
    }

    fn handle_publish(args: PublishArgs) -> ExitCode {
        run_cli(|| {
            let scope = resolve_scope(args.scope.invocation_uuid)?;
            let name_for_error = args.name.clone();
            let name = ScratchpadName::new(args.name)?;
            let scratchpad = Scratchpad::open(args.db)?;
            let receipt = scratchpad
                .publish(PublishRequest {
                    source: ScratchpadAddress {
                        invocation_uuid: scope.invocation_uuid,
                        name,
                    },
                    source_version: args.version,
                    destination: CanonicalAddress {
                        workflow_run_id: args.workflow_run_id,
                        artifact_name: args.artifact_name,
                    },
                    format_hint: args.format_hint,
                    verdict_line: args.verdict_line,
                    predecessor_version: args.predecessor_version,
                })
                .map_err(|err| with_name(err, &name_for_error))?;

            if args.json {
                print_json(&PublishEnvelope::from_receipt(&receipt))?;
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
            }
            Ok(())
        })
    }

    fn handle_gc(args: GcArgs) -> ExitCode {
        run_cli(|| {
            let selector = match (args.selector.invocation_uuid, args.selector.expired_before) {
                (Some(invocation_uuid), None) => {
                    GcSelector::Invocation(parse_uuid(&invocation_uuid)?)
                }
                (None, Some(expired_before)) => {
                    let cutoff = DateTime::parse_from_rfc3339(&expired_before)
                        .map_err(|err| {
                            ScratchpadError::InvalidInput(format!(
                                "invalid --expired-before {expired_before}: {err}"
                            ))
                        })?
                        .with_timezone(&Utc);
                    GcSelector::ExpiredBefore(cutoff)
                }
                _ => {
                    return Err(ScratchpadError::InvalidInput(
                        "pass exactly one of --invocation-uuid or --expired-before".to_string(),
                    ));
                }
            };
            let scratchpad = Scratchpad::open(args.db)?;
            let report = scratchpad.gc(GcRequest {
                selector,
                dry_run: args.dry_run,
                actor: args.actor,
                reason: args.reason,
            })?;

            if args.json {
                print_json(&GcEnvelope::from_report(&report))?;
            } else {
                writeln!(
                    io::stdout(),
                    "gc dry_run={} tombstoned={}",
                    report.dry_run,
                    report.tombstoned_rows.len()
                )?;
            }
            Ok(())
        })
    }

    fn handle_scope(args: ScopeArgs) -> ExitCode {
        run_cli(|| {
            let invocation_uuid = parse_uuid(&args.invocation_uuid)?;
            if args.json {
                print_json(&ScopeEnvelope {
                    invocation_uuid: invocation_uuid.to_string(),
                    workflow_run_id: private_workflow(invocation_uuid),
                })?;
            } else {
                writeln!(io::stdout(), "{}", private_workflow(invocation_uuid))?;
            }
            Ok(())
        })
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
            return Ok(InvocationScope {
                invocation_uuid: parse_uuid(&value)?,
            });
        }

        let env_value = std::env::var("OULIPOLY_PARENT_INVOCATION")
            .map_err(|_| ScratchpadError::MissingInvocationScope)?;
        if env_value.trim().is_empty() {
            return Err(ScratchpadError::MissingInvocationScope);
        }
        let value: Value = serde_json::from_str(&env_value).map_err(|err| {
            ScratchpadError::InvalidInvocationScope(format!(
                "OULIPOLY_PARENT_INVOCATION is not valid JSON: {err}"
            ))
        })?;
        let Some(id) = value.get("id").and_then(Value::as_str) else {
            return Err(ScratchpadError::InvalidInvocationScope(
                "OULIPOLY_PARENT_INVOCATION is missing id".to_string(),
            ));
        };
        Ok(InvocationScope {
            invocation_uuid: parse_uuid(id)?,
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
                producer_invocation_uuid: meta
                    .producer_invocation_uuid
                    .map(|uuid| uuid.to_string()),
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
}
