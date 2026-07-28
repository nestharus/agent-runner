mod application;
mod retirement_status;

use std::error::Error;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chrono::{DateTime, Utc};
use oulipoly_agent_store::{
    ArtifactKey, ArtifactMeta, ArtifactRecord, ListFilter, PutReceipt, PutRequest, Store,
    StoreError, TombstoneMeta, TombstoneReceipt, TombstoneStatus,
};
use uuid::Uuid;

use application::{
    CanonicalPublicationDraft, PrivateAppendOutcome, PrivateRecordData, PrivateRetirementOutcome,
    PrivateTombstone, PrivateVersionDraft, PrivateVersionMeta, PrivateVersionTarget,
    PrivateVisibility, PublicationAppendOutcome, ScratchpadApplication, ScratchpadPersistence,
};
use retirement_status::RetirementStatus;

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
    application: ScratchpadApplication<StoreScratchpadPersistence, fn() -> DateTime<Utc>>,
}

struct StoreScratchpadPersistence {
    store: Store,
}

impl StoreScratchpadPersistence {
    fn new(store: Store) -> Self {
        Self { store }
    }
}

impl ScratchpadPersistence for StoreScratchpadPersistence {
    fn append_private_version(
        &self,
        draft: PrivateVersionDraft,
    ) -> Result<PrivateAppendOutcome, ScratchpadError> {
        let receipt = self.store.put(map_private_append_request(draft))?;
        Ok(map_store_private_append_outcome(receipt))
    }

    fn load_active_private_record(
        &self,
        address: &ScratchpadAddress,
        version: Option<u64>,
    ) -> Result<PrivateRecordData, ScratchpadError> {
        let record = self.store.get(&private_key(address), version)?;
        decode_private_store_record(record)
    }

    fn acquire_newest_active_target(
        &self,
        address: &ScratchpadAddress,
    ) -> Result<PrivateVersionTarget, ScratchpadError> {
        let meta = self.store.get_meta(&private_key(address), None)?;
        let private_meta = decode_private_store_meta(meta)?;
        Ok(application::map_private_meta_to_target(private_meta))
    }

    fn acquire_existing_target(
        &self,
        address: &ScratchpadAddress,
        version: u64,
    ) -> Result<PrivateVersionTarget, ScratchpadError> {
        let meta = self.store.get_meta(&private_key(address), Some(version))?;
        let private_meta = decode_private_store_meta(meta)?;
        Ok(application::map_private_meta_to_target(private_meta))
    }

    fn enumerate_private_versions(
        &self,
        invocation_uuid: Uuid,
        name: Option<&ScratchpadName>,
        visibility: PrivateVisibility,
    ) -> Result<Vec<PrivateVersionMeta>, ScratchpadError> {
        let rows = self
            .store
            .list(map_scoped_private_filter(invocation_uuid, name, visibility))?;
        decode_private_store_metas(rows)
    }

    fn collect_cleanup_eligible_versions(
        &self,
        eligible_by_age: &mut dyn FnMut(DateTime<Utc>) -> bool,
    ) -> Result<Vec<PrivateVersionMeta>, ScratchpadError> {
        let rows = load_ordered_active_store_metadata(&self.store)?;
        let rows = filter_private_age_eligible_store_metadata(rows, eligible_by_age);
        decode_private_store_metas(rows)
    }

    fn retire_private_version(
        &self,
        target: &PrivateVersionTarget,
        actor: &str,
        reason: &str,
    ) -> Result<PrivateRetirementOutcome, ScratchpadError> {
        let receipt =
            self.store
                .tombstone(&private_key(&target.address), target.version, actor, reason)?;
        Ok(map_store_retirement_outcome(receipt))
    }

    fn append_canonical_publication(
        &self,
        draft: CanonicalPublicationDraft,
    ) -> Result<PublicationAppendOutcome, ScratchpadError> {
        let receipt = self.store.put(map_canonical_put_request(draft))?;
        Ok(map_store_publication_outcome(receipt))
    }
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

fn private_workflow(invocation_uuid: Uuid) -> String {
    format!("{SCRATCHPAD_PREFIX}{invocation_uuid}")
}

fn private_key(address: &ScratchpadAddress) -> ArtifactKey {
    ArtifactKey {
        workflow_run_id: private_workflow(address.invocation_uuid),
        artifact_name: address.name.as_str().to_string(),
    }
}

fn map_private_append_request(draft: PrivateVersionDraft) -> PutRequest {
    PutRequest {
        key: private_key(&draft.address),
        producer_invocation_uuid: Some(draft.producer_invocation_uuid),
        format_hint: draft.format_hint,
        verdict_line: draft.verdict_line,
        predecessor_version: draft.predecessor_version,
        content: draft.content,
    }
}

fn map_store_private_append_outcome(receipt: PutReceipt) -> PrivateAppendOutcome {
    PrivateAppendOutcome {
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

fn map_scoped_private_filter(
    invocation_uuid: Uuid,
    name: Option<&ScratchpadName>,
    visibility: PrivateVisibility,
) -> ListFilter {
    ListFilter {
        workflow_run_id: Some(private_workflow(invocation_uuid)),
        artifact_name: name.map(|name| name.as_str().to_string()),
        include_tombstoned: matches!(visibility, PrivateVisibility::IncludeTombstoned),
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

fn validate_private_name(value: String) -> Result<ScratchpadName, ScratchpadError> {
    ScratchpadName::new(value)
}

fn map_store_tombstone(tombstone: TombstoneMeta) -> PrivateTombstone {
    PrivateTombstone {
        tombstoned_at: tombstone.tombstoned_at,
        actor: tombstone.actor,
        reason: tombstone.reason,
    }
}

fn map_store_meta_to_private_version(
    meta: ArtifactMeta,
    invocation_uuid: Uuid,
    name: ScratchpadName,
) -> PrivateVersionMeta {
    PrivateVersionMeta {
        address: ScratchpadAddress {
            invocation_uuid,
            name,
        },
        version: meta.version,
        sha256: meta.sha256,
        content_len: meta.content_len,
        producer_invocation_uuid: meta.producer_invocation_uuid,
        format_hint: meta.format_hint,
        verdict_line: meta.verdict_line,
        predecessor_version: meta.predecessor_version,
        created_at: meta.created_at,
        tombstone: meta.tombstone.map(map_store_tombstone),
    }
}

fn decode_private_store_meta(meta: ArtifactMeta) -> Result<PrivateVersionMeta, ScratchpadError> {
    let invocation_uuid = parse_private_workflow(&meta.key.workflow_run_id)?;
    let name = validate_private_name(meta.key.artifact_name.clone())?;
    Ok(map_store_meta_to_private_version(
        meta,
        invocation_uuid,
        name,
    ))
}

fn decode_private_store_metas(
    rows: Vec<ArtifactMeta>,
) -> Result<Vec<PrivateVersionMeta>, ScratchpadError> {
    rows.into_iter().map(decode_private_store_meta).collect()
}

fn decode_private_store_record(
    record: ArtifactRecord,
) -> Result<PrivateRecordData, ScratchpadError> {
    let ArtifactRecord { meta, content } = record;
    let meta = decode_private_store_meta(meta)?;
    Ok(map_private_record_data(meta, content))
}

fn map_private_record_data(meta: PrivateVersionMeta, content: Vec<u8>) -> PrivateRecordData {
    PrivateRecordData { meta, content }
}

fn load_ordered_active_store_metadata(store: &Store) -> Result<Vec<ArtifactMeta>, ScratchpadError> {
    Ok(store.list(ListFilter {
        workflow_run_id: None,
        artifact_name: None,
        include_tombstoned: false,
    })?)
}

fn filter_private_age_eligible_store_metadata(
    rows: Vec<ArtifactMeta>,
    eligible_by_age: &mut dyn FnMut(DateTime<Utc>) -> bool,
) -> Vec<ArtifactMeta> {
    rows.into_iter()
        .filter(|meta| {
            meta.key.workflow_run_id.starts_with(SCRATCHPAD_PREFIX)
                && eligible_by_age(meta.created_at)
        })
        .collect()
}

fn map_store_retirement_status(status: &TombstoneStatus) -> RetirementStatus {
    match status {
        TombstoneStatus::Tombstoned => RetirementStatus::Retired,
        TombstoneStatus::AlreadyTombstoned => RetirementStatus::AlreadyRetired,
    }
}

fn map_store_retirement_outcome(receipt: TombstoneReceipt) -> PrivateRetirementOutcome {
    PrivateRetirementOutcome {
        status: crate::map_store_retirement_status(&receipt.status),
        tombstoned_at: receipt.tombstone.tombstoned_at,
    }
}

fn map_canonical_put_request(draft: CanonicalPublicationDraft) -> PutRequest {
    PutRequest {
        key: ArtifactKey {
            workflow_run_id: draft.destination.workflow_run_id,
            artifact_name: draft.destination.artifact_name,
        },
        producer_invocation_uuid: Some(draft.producer_invocation_uuid),
        format_hint: draft.format_hint,
        verdict_line: draft.verdict_line,
        predecessor_version: draft.predecessor_version,
        content: draft.content,
    }
}

fn map_store_publication_outcome(receipt: PutReceipt) -> PublicationAppendOutcome {
    PublicationAppendOutcome {
        version: receipt.version,
        sha256: receipt.sha256,
        content_len: receipt.content_len,
        format_hint: receipt.format_hint,
        verdict_line: receipt.verdict_line,
        predecessor_version: receipt.predecessor_version,
        created_at: receipt.created_at,
    }
}

impl From<PrivateTombstone> for TombstoneMeta {
    fn from(value: PrivateTombstone) -> Self {
        Self {
            tombstoned_at: value.tombstoned_at,
            actor: value.actor,
            reason: value.reason,
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

pub mod cli;
