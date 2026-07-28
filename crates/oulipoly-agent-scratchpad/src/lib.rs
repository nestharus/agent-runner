mod application;
mod compatibility_ddl;
mod core_api;
mod error_compatibility;
mod retirement_status;
mod store_adapter;

use chrono::{DateTime, Utc};
use oulipoly_agent_store::TombstoneMeta;
use uuid::Uuid;

const SCRATCHPAD_PREFIX: &str = "scratchpad:";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScratchpadName(String);

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

pub struct Scratchpad {
    application: application::ScratchpadApplication<
        store_adapter::StoreScratchpadPersistence,
        fn() -> DateTime<Utc>,
    >,
}

pub mod cli;
