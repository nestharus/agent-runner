use chrono::{DateTime, Utc};
use oulipoly_agent_store::{
    ArtifactKey, ArtifactMeta, ArtifactRecord, ListFilter, PutReceipt, PutRequest, Store,
    StoreError, TombstoneMeta, TombstoneReceipt, TombstoneStatus,
};
use uuid::Uuid;

use crate::application::{
    self, CanonicalPublicationDraft, PrivateAppendOutcome, PrivateRecordData,
    PrivateRetirementOutcome, PrivateTombstone, PrivateVersionDraft, PrivateVersionMeta,
    PrivateVersionTarget, PrivateVisibility, PublicationAppendOutcome, ScratchpadPersistence,
};
use crate::retirement_status::RetirementStatus;
use crate::{SCRATCHPAD_PREFIX, ScratchpadAddress, ScratchpadError, ScratchpadName};

pub(super) struct StoreScratchpadPersistence {
    store: Store,
}

impl StoreScratchpadPersistence {
    pub(super) fn new(store: Store) -> Self {
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

pub(super) fn private_workflow(invocation_uuid: Uuid) -> String {
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

pub(super) fn map_store_retirement_status(status: &TombstoneStatus) -> RetirementStatus {
    match status {
        TombstoneStatus::Tombstoned => RetirementStatus::Retired,
        TombstoneStatus::AlreadyTombstoned => RetirementStatus::AlreadyRetired,
    }
}

fn map_store_retirement_outcome(receipt: TombstoneReceipt) -> PrivateRetirementOutcome {
    PrivateRetirementOutcome {
        status: map_store_retirement_status(&receipt.status),
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
