use chrono::{DateTime, Utc};
use clap::{Args, Parser, Subcommand};
use oulipoly_agent_scratchpad::{InvocationScope, ReadRequest, Scratchpad, ScratchpadName};
use oulipoly_agent_store::{
    ArtifactKey, ArtifactMeta, ArtifactRecord, ListFilter, PutReceipt, PutRequest, Store,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReturnName(String);

impl ReturnName {
    pub fn new(value: impl Into<String>) -> Result<Self, MessengerError> {
        let value = value.into();
        if value.is_empty() {
            return Err(MessengerError::InvalidInput(
                "return name must not be empty".to_string(),
            ));
        }
        for prefix in ["scratchpad:", "return:"] {
            if value.starts_with(prefix) {
                return Err(MessengerError::InvalidInput(format!(
                    "return name must not start with reserved prefix {prefix}"
                )));
            }
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub struct ReturnRequest {
    pub db_path: PathBuf,
    pub invocation_uuid: Uuid,
    pub name: ReturnName,
    pub source: ReturnSource,
    pub format_hint: Option<String>,
    pub verdict_line: Option<String>,
    pub return_channel: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub enum ReturnSource {
    Scratchpad {
        name: ScratchpadName,
        version: Option<u64>,
    },
    InlineBytes(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreAddress {
    pub workflow_run_id: String,
    pub artifact_name: String,
    pub version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReturnedArtifactSource {
    Scratchpad { name: String, version: u64 },
    InlineBytes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReturnedArtifact {
    pub schema_version: u32,
    pub version_id: String,
    pub name: String,
    pub store_address: StoreAddress,
    pub sha256: String,
    pub content_len: u64,
    pub format_hint: Option<String>,
    pub verdict_line: Option<String>,
    pub source: ReturnedArtifactSource,
    pub producer_invocation_uuid: Uuid,
    pub returned_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReturnedArtifactMeta {
    pub version_id: String,
    pub name: String,
    pub store_address: StoreAddress,
    pub sha256: String,
    pub content_len: u64,
    pub format_hint: Option<String>,
    pub verdict_line: Option<String>,
    pub source: ReturnedArtifactSource,
    pub producer_invocation_uuid: Uuid,
    pub returned_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReturnedArtifactRecord {
    pub meta: ReturnedArtifactMeta,
    pub content: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReturnedArtifactRef {
    pub version_id: String,
    pub name: String,
    pub store_address: StoreAddress,
    pub sha256: String,
    pub content_len: u64,
    pub format_hint: Option<String>,
    pub verdict_line: Option<String>,
    pub source: ReturnedArtifactSource,
    pub producer_invocation_uuid: Uuid,
    pub returned_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ListReturnedRequest {
    pub db_path: PathBuf,
    pub invocation_uuid: Uuid,
    pub name: Option<ReturnName>,
}

#[derive(Debug, Clone)]
pub enum ShowReturnedRequest {
    VersionId {
        db_path: PathBuf,
        version_id: String,
    },
    Address {
        db_path: PathBuf,
        invocation_uuid: Uuid,
        name: ReturnName,
        version: Option<u64>,
    },
}

struct ReturnPayload {
    content: Vec<u8>,
    format_hint: Option<String>,
    verdict_line: Option<String>,
    source: ReturnedArtifactSource,
}

struct ReturnLookup {
    db_path: PathBuf,
    key: ArtifactKey,
    version: Option<u64>,
}

#[derive(Debug)]
pub enum MessengerError {
    InvalidInput(String),
    MissingInvocationScope,
    InvalidInvocationScope(String),
    MissingReturnChannel,
    InvalidReturnChannel(String),
    NotFound,
    Collision,
    Io(std::io::Error),
    Database(rusqlite::Error),
    MigrationRequired,
    IncompatibleSchema(String),
    Serialization(serde_json::Error),
    MetadataDecode(String),
    Scratchpad(oulipoly_agent_scratchpad::ScratchpadError),
    Store(oulipoly_agent_store::StoreError),
}

impl fmt::Display for MessengerError {
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
            Self::MissingReturnChannel => write!(
                f,
                "missing return channel: pass --return-channel or set OULIPOLY_RETURN_CHANNEL"
            ),
            Self::InvalidReturnChannel(message) => write!(f, "invalid return channel: {message}"),
            Self::NotFound => write!(f, "returned artifact not found"),
            Self::Collision => write!(f, "backing store collision"),
            Self::Io(err) => write!(f, "io error: {err}"),
            Self::Database(err) => write!(f, "database error: {err}"),
            Self::MigrationRequired => write!(f, "database schema migration required"),
            Self::IncompatibleSchema(message) => {
                write!(f, "incompatible database schema: {message}")
            }
            Self::Serialization(err) => write!(f, "json serialization error: {err}"),
            Self::MetadataDecode(message) => write!(f, "metadata decode error: {message}"),
            Self::Scratchpad(err) => write!(f, "{err}"),
            Self::Store(err) => write!(f, "{err}"),
        }
    }
}

impl Error for MessengerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Database(err) => Some(err),
            Self::Serialization(err) => Some(err),
            Self::Scratchpad(err) => Some(err),
            Self::Store(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for MessengerError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<rusqlite::Error> for MessengerError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Database(value)
    }
}

impl From<serde_json::Error> for MessengerError {
    fn from(value: serde_json::Error) -> Self {
        Self::Serialization(value)
    }
}

impl From<oulipoly_agent_store::StoreError> for MessengerError {
    fn from(value: oulipoly_agent_store::StoreError) -> Self {
        match value {
            oulipoly_agent_store::StoreError::InvalidInput(message) => Self::InvalidInput(message),
            oulipoly_agent_store::StoreError::NotFound => Self::NotFound,
            oulipoly_agent_store::StoreError::Collision => Self::Collision,
            oulipoly_agent_store::StoreError::Io(err) => Self::Io(err),
            oulipoly_agent_store::StoreError::Database(err) => Self::Database(err),
            oulipoly_agent_store::StoreError::MigrationRequired => Self::MigrationRequired,
            oulipoly_agent_store::StoreError::IncompatibleSchema(version) => {
                Self::IncompatibleSchema(format!("version {version}"))
            }
        }
    }
}

impl From<oulipoly_agent_scratchpad::ScratchpadError> for MessengerError {
    fn from(value: oulipoly_agent_scratchpad::ScratchpadError) -> Self {
        match value {
            oulipoly_agent_scratchpad::ScratchpadError::InvalidInput(message) => {
                Self::InvalidInput(message)
            }
            oulipoly_agent_scratchpad::ScratchpadError::MissingInvocationScope => {
                Self::MissingInvocationScope
            }
            oulipoly_agent_scratchpad::ScratchpadError::InvalidInvocationScope(message) => {
                Self::InvalidInvocationScope(message)
            }
            oulipoly_agent_scratchpad::ScratchpadError::NotFound
            | oulipoly_agent_scratchpad::ScratchpadError::NotFoundNamed(_) => Self::NotFound,
            oulipoly_agent_scratchpad::ScratchpadError::Collision => Self::Collision,
            oulipoly_agent_scratchpad::ScratchpadError::Io(err) => Self::Io(err),
            oulipoly_agent_scratchpad::ScratchpadError::Database(err) => Self::Database(err),
            oulipoly_agent_scratchpad::ScratchpadError::MigrationRequired => {
                Self::MigrationRequired
            }
            oulipoly_agent_scratchpad::ScratchpadError::IncompatibleSchema => {
                Self::IncompatibleSchema("scratchpad schema is incompatible".to_string())
            }
            oulipoly_agent_scratchpad::ScratchpadError::Serialization(err) => {
                Self::Serialization(err)
            }
            oulipoly_agent_scratchpad::ScratchpadError::MetadataDecode(message) => {
                Self::MetadataDecode(message)
            }
        }
    }
}

pub fn return_artifact(req: ReturnRequest) -> Result<ReturnedArtifact, MessengerError> {
    let payload = materialize_return_source(&req)?;
    let receipt = put_return_payload(&req, payload)?;
    let returned = returned_from_put(receipt.receipt, req.invocation_uuid, receipt.source);
    if let Some(path) = req.return_channel {
        append_return_channel(&path, &returned)?;
    }
    Ok(returned)
}

pub fn list_returned(
    req: ListReturnedRequest,
) -> Result<Vec<ReturnedArtifactMeta>, MessengerError> {
    let store = Store::open(&req.db_path)?;
    store
        .list(returned_list_filter(req.invocation_uuid, req.name))?
        .into_iter()
        .map(meta_from_store)
        .collect()
}

pub fn show_returned(req: ShowReturnedRequest) -> Result<ReturnedArtifactRecord, MessengerError> {
    let lookup = return_lookup(req)?;
    let store = Store::open(lookup.db_path)?;
    let record = store.get(&lookup.key, lookup.version)?;
    record_from_store(record)
}

pub fn append_return_channel(
    path: &Path,
    receipt: &ReturnedArtifact,
) -> Result<(), MessengerError> {
    let line = return_channel_line(receipt)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(&line)?;
    file.flush()?;
    Ok(())
}

struct StoredReturnPayload {
    receipt: PutReceipt,
    source: ReturnedArtifactSource,
}

fn materialize_return_source(req: &ReturnRequest) -> Result<ReturnPayload, MessengerError> {
    match &req.source {
        ReturnSource::Scratchpad { name, version } => {
            scratchpad_return_payload(req, name, *version)
        }
        ReturnSource::InlineBytes(bytes) => Ok(inline_return_payload(
            bytes.clone(),
            req.format_hint.clone(),
            req.verdict_line.clone(),
        )),
    }
}

fn scratchpad_return_payload(
    req: &ReturnRequest,
    name: &ScratchpadName,
    version: Option<u64>,
) -> Result<ReturnPayload, MessengerError> {
    let record = read_return_scratchpad(&req.db_path, req.invocation_uuid, name.clone(), version)?;
    Ok(ReturnPayload {
        source: ReturnedArtifactSource::Scratchpad {
            name: name.as_str().to_string(),
            version: record.meta.version,
        },
        format_hint: req.format_hint.clone().or(record.meta.format_hint),
        verdict_line: req.verdict_line.clone().or(record.meta.verdict_line),
        content: record.content,
    })
}

fn read_return_scratchpad(
    db_path: &Path,
    invocation_uuid: Uuid,
    name: ScratchpadName,
    version: Option<u64>,
) -> Result<oulipoly_agent_scratchpad::ScratchpadRecord, MessengerError> {
    let scratchpad = Scratchpad::open(db_path)?;
    scratchpad
        .read(ReadRequest {
            scope: InvocationScope { invocation_uuid },
            name,
            version,
        })
        .map_err(MessengerError::from)
}

fn inline_return_payload(
    content: Vec<u8>,
    format_hint: Option<String>,
    verdict_line: Option<String>,
) -> ReturnPayload {
    ReturnPayload {
        content,
        format_hint,
        verdict_line,
        source: ReturnedArtifactSource::InlineBytes,
    }
}

fn put_return_payload(
    req: &ReturnRequest,
    payload: ReturnPayload,
) -> Result<StoredReturnPayload, MessengerError> {
    let source = payload.source.clone();
    let store = Store::open(&req.db_path)?;
    let receipt = store.put(return_put_request(req, payload))?;
    Ok(StoredReturnPayload { receipt, source })
}

fn return_put_request(req: &ReturnRequest, payload: ReturnPayload) -> PutRequest {
    PutRequest {
        key: ArtifactKey {
            workflow_run_id: return_workflow(req.invocation_uuid),
            artifact_name: req.name.as_str().to_string(),
        },
        producer_invocation_uuid: Some(req.invocation_uuid),
        format_hint: payload.format_hint,
        verdict_line: payload.verdict_line,
        predecessor_version: None,
        content: payload.content,
    }
}

fn returned_list_filter(invocation_uuid: Uuid, name: Option<ReturnName>) -> ListFilter {
    ListFilter {
        workflow_run_id: Some(return_workflow(invocation_uuid)),
        artifact_name: name.map(|name| name.as_str().to_string()),
        include_tombstoned: false,
    }
}

fn return_lookup(req: ShowReturnedRequest) -> Result<ReturnLookup, MessengerError> {
    match req {
        ShowReturnedRequest::VersionId {
            db_path,
            version_id,
        } => return_lookup_from_version_id(db_path, &version_id),
        ShowReturnedRequest::Address {
            db_path,
            invocation_uuid,
            name,
            version,
        } => Ok(return_lookup_from_address(
            db_path,
            invocation_uuid,
            name,
            version,
        )),
    }
}

fn return_lookup_from_version_id(
    db_path: PathBuf,
    version_id: &str,
) -> Result<ReturnLookup, MessengerError> {
    let (invocation_uuid, name, version) = parse_version_id(version_id)?;
    Ok(ReturnLookup {
        db_path,
        key: ArtifactKey {
            workflow_run_id: return_workflow(invocation_uuid),
            artifact_name: name,
        },
        version: Some(version),
    })
}

fn return_lookup_from_address(
    db_path: PathBuf,
    invocation_uuid: Uuid,
    name: ReturnName,
    version: Option<u64>,
) -> ReturnLookup {
    ReturnLookup {
        db_path,
        key: ArtifactKey {
            workflow_run_id: return_workflow(invocation_uuid),
            artifact_name: name.as_str().to_string(),
        },
        version,
    }
}

fn return_channel_line(receipt: &ReturnedArtifact) -> Result<Vec<u8>, MessengerError> {
    let mut line = serde_json::to_vec(receipt)?;
    line.push(b'\n');
    Ok(line)
}

fn return_workflow(invocation_uuid: Uuid) -> String {
    format!("return:{invocation_uuid}")
}

fn version_id(invocation_uuid: Uuid, name: &str, version: u64) -> String {
    format!(
        "store://return/{}/{}/{}",
        invocation_uuid,
        percent_encode(name),
        version
    )
}

fn returned_from_put(
    receipt: PutReceipt,
    invocation_uuid: Uuid,
    source: ReturnedArtifactSource,
) -> ReturnedArtifact {
    ReturnedArtifact {
        schema_version: 1,
        version_id: version_id(invocation_uuid, &receipt.key.artifact_name, receipt.version),
        name: receipt.key.artifact_name.clone(),
        store_address: StoreAddress {
            workflow_run_id: receipt.key.workflow_run_id,
            artifact_name: receipt.key.artifact_name,
            version: receipt.version,
        },
        sha256: receipt.sha256,
        content_len: receipt.content_len,
        format_hint: receipt.format_hint,
        verdict_line: receipt.verdict_line,
        source,
        producer_invocation_uuid: invocation_uuid,
        returned_at: receipt.created_at,
    }
}

fn meta_from_store(meta: ArtifactMeta) -> Result<ReturnedArtifactMeta, MessengerError> {
    let invocation_uuid = invocation_from_return_workflow(&meta.key.workflow_run_id)?;
    Ok(ReturnedArtifactMeta {
        version_id: version_id(invocation_uuid, &meta.key.artifact_name, meta.version),
        name: meta.key.artifact_name.clone(),
        store_address: StoreAddress {
            workflow_run_id: meta.key.workflow_run_id,
            artifact_name: meta.key.artifact_name,
            version: meta.version,
        },
        sha256: meta.sha256,
        content_len: meta.content_len,
        format_hint: meta.format_hint,
        verdict_line: meta.verdict_line,
        // The artifact store preserves content metadata but not the return source.
        // Channel receipts and StateDb rows carry exact source information.
        source: ReturnedArtifactSource::InlineBytes,
        producer_invocation_uuid: meta.producer_invocation_uuid.unwrap_or(invocation_uuid),
        returned_at: meta.created_at,
    })
}

fn record_from_store(record: ArtifactRecord) -> Result<ReturnedArtifactRecord, MessengerError> {
    Ok(ReturnedArtifactRecord {
        meta: meta_from_store(record.meta)?,
        content: record.content,
    })
}

fn invocation_from_return_workflow(workflow_run_id: &str) -> Result<Uuid, MessengerError> {
    let value = workflow_run_id.strip_prefix("return:").ok_or_else(|| {
        MessengerError::InvalidInput("version_id is not in return namespace".to_string())
    })?;
    Uuid::parse_str(value).map_err(|err| {
        MessengerError::InvalidInput(format!("invalid return invocation UUID {value}: {err}"))
    })
}

fn parse_version_id(value: &str) -> Result<(Uuid, String, u64), MessengerError> {
    let rest = value.strip_prefix("store://return/").ok_or_else(|| {
        MessengerError::InvalidInput("version_id must start with store://return/".to_string())
    })?;
    let (uuid_text, rest) = rest
        .split_once('/')
        .ok_or_else(|| MessengerError::InvalidInput("version_id is missing name".to_string()))?;
    let (name_text, version_text) = rest
        .rsplit_once('/')
        .ok_or_else(|| MessengerError::InvalidInput("version_id is missing version".to_string()))?;
    let invocation_uuid = Uuid::parse_str(uuid_text).map_err(|err| {
        MessengerError::InvalidInput(format!("invalid version_id invocation UUID: {err}"))
    })?;
    let name = percent_decode(name_text)?;
    let version = version_text.parse::<u64>().map_err(|err| {
        MessengerError::InvalidInput(format!("invalid version_id version: {err}"))
    })?;
    if version == 0 {
        return Err(MessengerError::InvalidInput(
            "version_id version must be greater than zero".to_string(),
        ));
    }
    Ok((invocation_uuid, name, version))
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn percent_decode(value: &str) -> Result<String, MessengerError> {
    let mut bytes = Vec::new();
    let input = value.as_bytes();
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'%' {
            if index + 2 >= input.len() {
                return Err(MessengerError::InvalidInput(
                    "invalid percent escape in version_id".to_string(),
                ));
            }
            let hex = std::str::from_utf8(&input[index + 1..index + 3]).map_err(|err| {
                MessengerError::InvalidInput(format!("invalid percent escape in version_id: {err}"))
            })?;
            let byte = u8::from_str_radix(hex, 16).map_err(|err| {
                MessengerError::InvalidInput(format!("invalid percent escape in version_id: {err}"))
            })?;
            bytes.push(byte);
            index += 3;
        } else {
            bytes.push(input[index]);
            index += 1;
        }
    }
    String::from_utf8(bytes)
        .map_err(|err| MessengerError::InvalidInput(format!("version_id name is not UTF-8: {err}")))
}

pub mod cli {
    use super::*;

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

    pub fn run() -> i32 {
        let cli = match Cli::try_parse() {
            Ok(cli) => cli,
            Err(err) => {
                let _ = err.print();
                return 64;
            }
        };
        match cli.command {
            Command::Return(args) => handle_return(args),
            Command::ListReturned(args) => handle_list_returned(args),
            Command::Show(args) => handle_show(args),
            Command::Version(args) => handle_version(args),
        }
    }

    fn handle_return(args: ReturnArgs) -> i32 {
        run_cli(|| {
            let invocation_uuid = resolve_invocation_uuid(args.invocation_uuid.clone())?;
            let source = resolve_return_source(&args)?;
            let name = ReturnName::new(args.name)?;
            let return_channel = resolve_return_channel(args.return_channel)?;
            let json = args.json;
            let receipt = return_artifact(return_request(
                args.db,
                invocation_uuid,
                name,
                source,
                args.format_hint,
                args.verdict_line,
                return_channel,
            ))?;
            write_return_output(&receipt, json)?;
            Ok(())
        })
    }

    fn handle_list_returned(args: ListReturnedArgs) -> i32 {
        run_cli(|| {
            let invocation_uuid = resolve_invocation_uuid(args.invocation_uuid)?;
            let rows = list_returned(ListReturnedRequest {
                db_path: args.db,
                invocation_uuid,
                name: args.name.map(ReturnName::new).transpose()?,
            })?;
            write_list_output(&rows, args.json)?;
            Ok(())
        })
    }

    fn handle_show(args: ShowArgs) -> i32 {
        run_cli(|| {
            let out = args.out.clone();
            let req = show_request(args)?;
            let record = show_returned(req)?;
            write_record_content(out, record.content)?;
            Ok(())
        })
    }

    fn handle_version(args: VersionArgs) -> i32 {
        run_cli(|| {
            write_version_output(args.json)?;
            Ok(())
        })
    }

    fn return_request(
        db_path: PathBuf,
        invocation_uuid: Uuid,
        name: ReturnName,
        source: ReturnSource,
        format_hint: Option<String>,
        verdict_line: Option<String>,
        return_channel: PathBuf,
    ) -> ReturnRequest {
        ReturnRequest {
            db_path,
            invocation_uuid,
            name,
            source,
            format_hint,
            verdict_line,
            return_channel: Some(return_channel),
        }
    }

    fn show_request(args: ShowArgs) -> Result<ShowReturnedRequest, MessengerError> {
        match (args.version_id, args.name) {
            (Some(version_id), None) => Ok(ShowReturnedRequest::VersionId {
                db_path: args.db,
                version_id,
            }),
            (None, Some(name)) => Ok(ShowReturnedRequest::Address {
                db_path: args.db,
                invocation_uuid: resolve_invocation_uuid(args.invocation_uuid)?,
                name: ReturnName::new(name)?,
                version: args.version,
            }),
            _ => Err(MessengerError::InvalidInput(
                "pass exactly one of --version-id or --name".to_string(),
            )),
        }
    }

    fn write_return_output(receipt: &ReturnedArtifact, json: bool) -> Result<(), MessengerError> {
        if json {
            print_json(receipt)
        } else {
            writeln!(io::stdout(), "{} {}", receipt.version_id, receipt.sha256)?;
            Ok(())
        }
    }

    fn write_list_output(rows: &[ReturnedArtifactMeta], json: bool) -> Result<(), MessengerError> {
        if json {
            return print_json(&rows);
        }

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

    fn write_record_content(out: Option<PathBuf>, content: Vec<u8>) -> Result<(), MessengerError> {
        if let Some(path) = out {
            return fs::write(path, content).map_err(MessengerError::from);
        }

        let stdout = io::stdout();
        let mut handle = stdout.lock();
        handle.write_all(&content)?;
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
            receipt_schema_version: 1,
        }
    }

    fn write_version_output(json: bool) -> Result<(), MessengerError> {
        if json {
            print_json(&version_envelope())
        } else {
            writeln!(
                io::stdout(),
                "oulipoly-agent-messenger {}",
                env!("CARGO_PKG_VERSION")
            )?;
            Ok(())
        }
    }

    fn resolve_return_source(args: &ReturnArgs) -> Result<ReturnSource, MessengerError> {
        match (&args.scratchpad, inline_source_count(args)) {
            (Some(_), 1..) => Err(MessengerError::InvalidInput(
                "--scratchpad cannot be combined with inline content sources".to_string(),
            )),
            (Some(name), 0) => scratchpad_source(name, args.scratchpad_version),
            (None, 1) => inline_source(args),
            (None, 0) => Err(MessengerError::InvalidInput(
                "pass --scratchpad or exactly one of --body, --content-file, --content-stdin"
                    .to_string(),
            )),
            (None, _) => Err(MessengerError::InvalidInput(
                "--body, --content-file, and --content-stdin are mutually exclusive".to_string(),
            )),
        }
    }

    fn inline_source_count(args: &ReturnArgs) -> usize {
        usize::from(args.body.is_some())
            + usize::from(args.content_file.is_some())
            + usize::from(args.content_stdin)
    }

    fn scratchpad_source(name: &str, version: Option<u64>) -> Result<ReturnSource, MessengerError> {
        Ok(ReturnSource::Scratchpad {
            name: ScratchpadName::new(name.to_string())?,
            version,
        })
    }

    fn inline_source(args: &ReturnArgs) -> Result<ReturnSource, MessengerError> {
        Ok(ReturnSource::InlineBytes(inline_bytes(args)?))
    }

    fn inline_bytes(args: &ReturnArgs) -> Result<Vec<u8>, MessengerError> {
        if let Some(body) = &args.body {
            return Ok(body.as_bytes().to_vec());
        }
        if let Some(path) = &args.content_file {
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
        if let Some(value) = explicit {
            return parse_uuid(&value);
        }
        let env_value = parent_invocation_env()?;
        let id = parent_invocation_id(&env_value)?;
        parse_uuid(&id)
    }

    fn resolve_return_channel(explicit: Option<PathBuf>) -> Result<PathBuf, MessengerError> {
        if let Some(path) = explicit {
            return Ok(path);
        }
        return_channel_from_env()
    }

    fn parent_invocation_env() -> Result<String, MessengerError> {
        let env_value = std::env::var("OULIPOLY_PARENT_INVOCATION")
            .map_err(|_| MessengerError::MissingInvocationScope)?;
        require_parent_invocation_env(&env_value)?;
        Ok(env_value)
    }

    fn require_parent_invocation_env(value: &str) -> Result<(), MessengerError> {
        if value.trim().is_empty() {
            return Err(MessengerError::MissingInvocationScope);
        }
        Ok(())
    }

    fn parent_invocation_id(env_value: &str) -> Result<String, MessengerError> {
        let value: Value = serde_json::from_str(env_value).map_err(|err| {
            MessengerError::InvalidInvocationScope(format!(
                "OULIPOLY_PARENT_INVOCATION is not valid JSON: {err}"
            ))
        })?;
        value
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                MessengerError::InvalidInvocationScope(
                    "OULIPOLY_PARENT_INVOCATION is missing id".to_string(),
                )
            })
    }

    fn return_channel_from_env() -> Result<PathBuf, MessengerError> {
        let value = std::env::var("OULIPOLY_RETURN_CHANNEL")
            .map_err(|_| MessengerError::MissingReturnChannel)?;
        require_return_channel_env(&value)?;
        Ok(PathBuf::from(value))
    }

    fn require_return_channel_env(value: &str) -> Result<(), MessengerError> {
        if value.trim().is_empty() {
            return Err(MessengerError::MissingReturnChannel);
        }
        Ok(())
    }

    fn parse_uuid(value: &str) -> Result<Uuid, MessengerError> {
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

    fn run_cli(op: impl FnOnce() -> Result<(), MessengerError>) -> i32 {
        match op() {
            Ok(()) => 0,
            Err(err) => {
                eprintln!("{err}");
                error_code(&err)
            }
        }
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
}

#[cfg(test)]
mod tests {
    use super::MessengerError;
    use oulipoly_agent_store::StoreError;

    #[test]
    fn store_incompatible_schema_version_is_preserved() {
        let err = MessengerError::from(StoreError::IncompatibleSchema("2".to_string()));

        assert!(matches!(
            err,
            MessengerError::IncompatibleSchema(ref message) if message == "version 2"
        ));
        assert_eq!(err.to_string(), "incompatible database schema: version 2");
    }
}
