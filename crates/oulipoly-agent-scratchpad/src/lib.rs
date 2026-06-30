use std::error::Error;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chrono::{DateTime, TimeDelta, Utc};
use oulipoly_agent_store::{
    ArtifactKey, ArtifactMeta, ArtifactRecord, ListFilter, PutReceipt, PutRequest, Store,
    StoreError, TombstoneMeta, TombstoneReceipt, TombstoneStatus,
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

struct DeleteDefaults {
    actor: String,
    reason: String,
}

#[derive(Default)]
struct DeleteSummary {
    tombstoned_versions: Vec<u64>,
    already_tombstoned_versions: Vec<u64>,
    tombstoned_at: Option<DateTime<Utc>>,
}

struct GcDefaults {
    actor: String,
    reason: String,
}

#[derive(Default)]
struct GcSummary {
    tombstoned_rows: Vec<ScratchpadAddress>,
    already_tombstoned_rows: Vec<ScratchpadAddress>,
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
        let invocation_uuid = req.scope.invocation_uuid;
        let name = req.name;
        let receipt = self.store.put(scratchpad_put_request(
            key,
            invocation_uuid,
            req.format_hint,
            req.verdict_line,
            req.predecessor_version,
            req.content,
        ))?;
        Ok(write_receipt_from_put(receipt, invocation_uuid, name))
    }

    pub fn read(&self, req: ReadRequest) -> Result<ScratchpadRecord, ScratchpadError> {
        let key = private_key(req.scope.invocation_uuid, &req.name);
        let record = self.store.get(&key, req.version)?;
        record_from_store(record)
    }

    pub fn list(&self, req: ListRequest) -> Result<Vec<ScratchpadMeta>, ScratchpadError> {
        self.store
            .list(scratchpad_list_filter(req))?
            .into_iter()
            .map(meta_from_store)
            .collect()
    }

    pub fn delete(&self, req: DeleteRequest) -> Result<DeleteReceipt, ScratchpadError> {
        let invocation_uuid = req.scope.invocation_uuid;
        let key = private_key(invocation_uuid, &req.name);
        let name = req.name;
        let selector = req.selector;
        let defaults = delete_defaults(req.actor, req.reason);
        let versions = self.delete_versions(&key, &selector)?;
        require_delete_versions(&selector, &versions)?;
        let summary =
            self.tombstone_delete_versions(&key, &versions, &defaults.actor, &defaults.reason)?;
        Ok(delete_receipt(
            invocation_uuid,
            name,
            selector,
            defaults,
            summary,
        ))
    }

    pub fn publish(&self, req: PublishRequest) -> Result<PublishReceipt, ScratchpadError> {
        validate_canonical_destination(&req.destination)?;
        let source_key = private_key(req.source.invocation_uuid, &req.source.name);
        let source = self.store.get(&source_key, req.source_version)?;
        let destination = req.destination;
        let producer_invocation_uuid = req.source.invocation_uuid;
        let ArtifactRecord {
            meta: source_meta,
            content,
        } = source;
        let destination_receipt = self.store.put(publish_put_request(
            &destination,
            req.source.invocation_uuid,
            req.format_hint,
            req.verdict_line,
            req.predecessor_version,
            &source_meta,
            content,
        ))?;

        Ok(publish_receipt(
            req.source,
            source_meta,
            destination,
            producer_invocation_uuid,
            destination_receipt,
        ))
    }

    pub fn gc(&self, req: GcRequest) -> Result<GcReport, ScratchpadError> {
        let evaluated_at = Utc::now();
        let selector = req.selector;
        let dry_run = req.dry_run;
        let defaults = gc_defaults(req.actor, req.reason, &selector);
        let candidates = self.gc_candidates(&selector)?;
        let summary =
            self.tombstone_gc_candidates(candidates, dry_run, &defaults.actor, &defaults.reason)?;

        Ok(gc_report(
            selector,
            dry_run,
            defaults,
            summary,
            evaluated_at,
        ))
    }

    fn delete_versions(
        &self,
        key: &ArtifactKey,
        selector: &DeleteSelector,
    ) -> Result<Vec<u64>, ScratchpadError> {
        match selector {
            DeleteSelector::Latest => self.latest_delete_version(key),
            DeleteSelector::Version(version) => self.specific_delete_version(key, *version),
            DeleteSelector::AllVersions => self.all_delete_versions(key),
        }
    }

    fn gc_candidates(&self, selector: &GcSelector) -> Result<Vec<ScratchpadMeta>, ScratchpadError> {
        match selector {
            GcSelector::Invocation(invocation_uuid) => {
                self.invocation_gc_candidates(*invocation_uuid)
            }
            GcSelector::ExpiredBefore(cutoff) => self.expired_gc_candidates(cutoff),
        }
    }

    fn latest_delete_version(&self, key: &ArtifactKey) -> Result<Vec<u64>, ScratchpadError> {
        Ok(vec![self.store.get_meta(key, None)?.version])
    }

    fn specific_delete_version(
        &self,
        key: &ArtifactKey,
        version: u64,
    ) -> Result<Vec<u64>, ScratchpadError> {
        self.store.get_meta(key, Some(version))?;
        Ok(vec![version])
    }

    fn all_delete_versions(&self, key: &ArtifactKey) -> Result<Vec<u64>, ScratchpadError> {
        Ok(self
            .store
            .list(ListFilter {
                workflow_run_id: Some(key.workflow_run_id.clone()),
                artifact_name: Some(key.artifact_name.clone()),
                include_tombstoned: false,
            })?
            .into_iter()
            .map(|meta| meta.version)
            .collect())
    }

    fn tombstone_delete_versions(
        &self,
        key: &ArtifactKey,
        versions: &[u64],
        actor: &str,
        reason: &str,
    ) -> Result<DeleteSummary, ScratchpadError> {
        let mut summary = DeleteSummary::default();
        for version in versions {
            let receipt = self.store.tombstone(key, *version, actor, reason)?;
            record_delete_tombstone(&mut summary, *version, &receipt);
        }
        Ok(summary)
    }

    fn invocation_gc_candidates(
        &self,
        invocation_uuid: Uuid,
    ) -> Result<Vec<ScratchpadMeta>, ScratchpadError> {
        self.store
            .list(ListFilter {
                workflow_run_id: Some(private_workflow(invocation_uuid)),
                artifact_name: None,
                include_tombstoned: false,
            })?
            .into_iter()
            .map(meta_from_store)
            .collect()
    }

    fn expired_gc_candidates(
        &self,
        cutoff: &DateTime<Utc>,
    ) -> Result<Vec<ScratchpadMeta>, ScratchpadError> {
        let ttl = TimeDelta::days(TTL_DAYS);
        self.store
            .list(ListFilter {
                workflow_run_id: None,
                artifact_name: None,
                include_tombstoned: false,
            })?
            .into_iter()
            .filter(is_scratchpad_meta)
            .filter(|meta| is_expired_meta(meta, ttl, cutoff))
            .map(meta_from_store)
            .collect()
    }

    fn tombstone_gc_candidates(
        &self,
        candidates: Vec<ScratchpadMeta>,
        dry_run: bool,
        actor: &str,
        reason: &str,
    ) -> Result<GcSummary, ScratchpadError> {
        let mut summary = GcSummary::default();
        for meta in candidates {
            self.record_gc_tombstone(&mut summary, meta, dry_run, actor, reason)?;
        }
        Ok(summary)
    }

    fn record_gc_tombstone(
        &self,
        summary: &mut GcSummary,
        meta: ScratchpadMeta,
        dry_run: bool,
        actor: &str,
        reason: &str,
    ) -> Result<(), ScratchpadError> {
        let address = meta.address.clone();
        if dry_run {
            summary.tombstoned_rows.push(address);
            return Ok(());
        }

        let key = private_key(address.invocation_uuid, &address.name);
        let receipt = self.store.tombstone(&key, meta.version, actor, reason)?;
        record_gc_tombstone_status(summary, address, &receipt.status);
        Ok(())
    }
}

fn scratchpad_put_request(
    key: ArtifactKey,
    invocation_uuid: Uuid,
    format_hint: Option<String>,
    verdict_line: Option<String>,
    predecessor_version: Option<u64>,
    content: Vec<u8>,
) -> PutRequest {
    PutRequest {
        key,
        producer_invocation_uuid: Some(invocation_uuid),
        format_hint,
        verdict_line,
        predecessor_version,
        content,
    }
}

fn scratchpad_list_filter(req: ListRequest) -> ListFilter {
    ListFilter {
        workflow_run_id: Some(private_workflow(req.scope.invocation_uuid)),
        artifact_name: req.name.as_ref().map(|name| name.as_str().to_string()),
        include_tombstoned: req.include_tombstoned,
    }
}

fn delete_defaults(actor: Option<String>, reason: Option<String>) -> DeleteDefaults {
    DeleteDefaults {
        actor: actor.unwrap_or_else(|| DEFAULT_DELETE_ACTOR.to_string()),
        reason: reason.unwrap_or_else(|| DEFAULT_DELETE_REASON.to_string()),
    }
}

fn require_delete_versions(
    selector: &DeleteSelector,
    versions: &[u64],
) -> Result<(), ScratchpadError> {
    if matches!(selector, DeleteSelector::Latest) && versions.is_empty() {
        return Err(ScratchpadError::NotFound);
    }
    Ok(())
}

fn record_delete_tombstone(summary: &mut DeleteSummary, version: u64, receipt: &TombstoneReceipt) {
    summary.tombstoned_at = Some(receipt.tombstone.tombstoned_at);
    match receipt.status {
        TombstoneStatus::Tombstoned => summary.tombstoned_versions.push(version),
        TombstoneStatus::AlreadyTombstoned => summary.already_tombstoned_versions.push(version),
    }
}

fn delete_receipt(
    invocation_uuid: Uuid,
    name: ScratchpadName,
    selector: DeleteSelector,
    defaults: DeleteDefaults,
    summary: DeleteSummary,
) -> DeleteReceipt {
    DeleteReceipt {
        address: ScratchpadAddress {
            invocation_uuid,
            name,
        },
        selector,
        tombstoned_versions: summary.tombstoned_versions,
        already_tombstoned_versions: summary.already_tombstoned_versions,
        actor: defaults.actor,
        reason: defaults.reason,
        tombstoned_at: summary.tombstoned_at,
    }
}

fn validate_canonical_destination(destination: &CanonicalAddress) -> Result<(), ScratchpadError> {
    if destination.workflow_run_id.starts_with(SCRATCHPAD_PREFIX) {
        return Err(ScratchpadError::InvalidInput(format!(
            "canonical workflow_run_id must not start with reserved prefix {SCRATCHPAD_PREFIX}"
        )));
    }
    Ok(())
}

fn publish_put_request(
    destination: &CanonicalAddress,
    invocation_uuid: Uuid,
    format_hint: Option<String>,
    verdict_line: Option<String>,
    predecessor_version: Option<u64>,
    source_meta: &ArtifactMeta,
    content: Vec<u8>,
) -> PutRequest {
    PutRequest {
        key: ArtifactKey {
            workflow_run_id: destination.workflow_run_id.clone(),
            artifact_name: destination.artifact_name.clone(),
        },
        producer_invocation_uuid: Some(invocation_uuid),
        format_hint: format_hint.or_else(|| source_meta.format_hint.clone()),
        verdict_line: verdict_line.or_else(|| source_meta.verdict_line.clone()),
        predecessor_version,
        content,
    }
}

fn publish_receipt(
    source: ScratchpadAddress,
    source_meta: ArtifactMeta,
    destination: CanonicalAddress,
    producer_invocation_uuid: Uuid,
    destination_receipt: PutReceipt,
) -> PublishReceipt {
    PublishReceipt {
        source,
        source_version: source_meta.version,
        source_sha256: source_meta.sha256,
        destination,
        destination_version: destination_receipt.version,
        destination_sha256: destination_receipt.sha256,
        content_len: destination_receipt.content_len,
        producer_invocation_uuid,
        format_hint: destination_receipt.format_hint,
        verdict_line: destination_receipt.verdict_line,
        predecessor_version: destination_receipt.predecessor_version,
        created_at: destination_receipt.created_at,
    }
}

fn gc_defaults(actor: Option<String>, reason: Option<String>, selector: &GcSelector) -> GcDefaults {
    GcDefaults {
        actor: actor.unwrap_or_else(|| DEFAULT_GC_ACTOR.to_string()),
        reason: reason.unwrap_or_else(|| gc_default_reason(selector)),
    }
}

fn gc_default_reason(selector: &GcSelector) -> String {
    match selector {
        GcSelector::Invocation(_) => DEFAULT_GC_INVOCATION_REASON.to_string(),
        GcSelector::ExpiredBefore(_) => DEFAULT_GC_EXPIRED_REASON.to_string(),
    }
}

fn is_scratchpad_meta(meta: &ArtifactMeta) -> bool {
    meta.key.workflow_run_id.starts_with(SCRATCHPAD_PREFIX)
}

fn is_expired_meta(meta: &ArtifactMeta, ttl: TimeDelta, cutoff: &DateTime<Utc>) -> bool {
    meta.created_at + ttl <= *cutoff
}

fn record_gc_tombstone_status(
    summary: &mut GcSummary,
    address: ScratchpadAddress,
    status: &TombstoneStatus,
) {
    match status {
        TombstoneStatus::Tombstoned => summary.tombstoned_rows.push(address),
        TombstoneStatus::AlreadyTombstoned => summary.already_tombstoned_rows.push(address),
    }
}

fn gc_report(
    selector: GcSelector,
    dry_run: bool,
    defaults: GcDefaults,
    summary: GcSummary,
    evaluated_at: DateTime<Utc>,
) -> GcReport {
    GcReport {
        selector,
        dry_run,
        tombstoned_rows: summary.tombstoned_rows,
        already_tombstoned_rows: summary.already_tombstoned_rows,
        actor: defaults.actor,
        reason: defaults.reason,
        evaluated_at,
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
            let report =
                scratchpad.gc(gc_request(selector, args.dry_run, args.actor, args.reason))?;
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

    fn read_request(
        scope: InvocationScope,
        name: ScratchpadName,
        version: Option<u64>,
    ) -> ReadRequest {
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
            (Some(invocation_uuid), None) => {
                Ok(GcSelector::Invocation(parse_uuid(&invocation_uuid)?))
            }
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
