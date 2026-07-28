mod application;
mod compatibility_ddl;
mod retirement_status;
mod store_adapter;

use std::error::Error;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chrono::{DateTime, Utc};
use oulipoly_agent_store::{Store, TombstoneMeta};
use uuid::Uuid;

use application::ScratchpadApplication;
use compatibility_ddl::install_store_aliases;
use store_adapter::StoreScratchpadPersistence;

const SCRATCHPAD_PREFIX: &str = "scratchpad:";

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
    application: ScratchpadApplication<StoreScratchpadPersistence, fn() -> DateTime<Utc>>,
}

fn system_current_utc() -> DateTime<Utc> {
    Utc::now()
}

impl Scratchpad {
    pub fn open(db_path: impl AsRef<Path>) -> Result<Self, ScratchpadError> {
        let db_path = db_path.as_ref();
        let store = Store::open(db_path)?;
        install_store_aliases(db_path)?;
        let persistence = StoreScratchpadPersistence::new(store);
        let application =
            ScratchpadApplication::new(persistence, system_current_utc as fn() -> DateTime<Utc>);
        Ok(Self { application })
    }

    pub fn write(&self, req: WriteRequest) -> Result<WriteReceipt, ScratchpadError> {
        self.application.write(req)
    }

    pub fn read(&self, req: ReadRequest) -> Result<ScratchpadRecord, ScratchpadError> {
        self.application.read(req)
    }

    pub fn list(&self, req: ListRequest) -> Result<Vec<ScratchpadMeta>, ScratchpadError> {
        self.application.list(req)
    }

    pub fn delete(&self, req: DeleteRequest) -> Result<DeleteReceipt, ScratchpadError> {
        self.application.delete(req)
    }

    pub fn publish(&self, req: PublishRequest) -> Result<PublishReceipt, ScratchpadError> {
        self.application.publish(req)
    }

    pub fn gc(&self, req: GcRequest) -> Result<GcReport, ScratchpadError> {
        self.application.gc(req)
    }
}

pub mod cli;
