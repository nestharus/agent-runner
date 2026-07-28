//! ## Declared roles
//!
//! `orchestration`, `validator`, `predicate`, `mapper`, `formatter`.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::retirement_status::{
    DeleteStatusReduction, GcStatusReduction, RetirementStatus, map_gc_dry_run_addresses,
    partition_delete_version, partition_gc_address, project_last_delete_tombstoned_at,
};
use crate::{
    CanonicalAddress, DeleteReceipt, DeleteRequest, DeleteSelector, GcReport, GcRequest,
    GcSelector, InvocationScope, ListRequest, PublishReceipt, PublishRequest, ReadRequest,
    ScratchpadAddress, ScratchpadError, ScratchpadMeta, ScratchpadName, ScratchpadRecord,
    WriteReceipt, WriteRequest,
};

const DEFAULT_DELETE_ACTOR: &str = "agent-scratchpad";
const DEFAULT_DELETE_REASON: &str = "scratchpad delete";
const DEFAULT_GC_ACTOR: &str = "agent-scratchpad-gc";
const DEFAULT_GC_INVOCATION_REASON: &str = "scratchpad gc invocation";
const DEFAULT_GC_EXPIRED_REASON: &str = "scratchpad gc expired";
pub(super) trait ScratchpadPersistence {
    fn append_private_version(
        &self,
        draft: PrivateVersionDraft,
    ) -> Result<PrivateAppendOutcome, ScratchpadError>;

    fn load_active_private_record(
        &self,
        address: &ScratchpadAddress,
        version: Option<u64>,
    ) -> Result<PrivateRecordData, ScratchpadError>;

    fn acquire_newest_active_target(
        &self,
        address: &ScratchpadAddress,
    ) -> Result<PrivateVersionTarget, ScratchpadError>;

    fn acquire_existing_target(
        &self,
        address: &ScratchpadAddress,
        version: u64,
    ) -> Result<PrivateVersionTarget, ScratchpadError>;

    fn enumerate_private_versions(
        &self,
        invocation_uuid: Uuid,
        name: Option<&ScratchpadName>,
        visibility: PrivateVisibility,
    ) -> Result<Vec<PrivateVersionMeta>, ScratchpadError>;

    fn collect_cleanup_eligible_versions(
        &self,
        eligible_by_age: &mut dyn FnMut(DateTime<Utc>) -> bool,
    ) -> Result<Vec<PrivateVersionMeta>, ScratchpadError>;

    fn retire_private_version(
        &self,
        target: &PrivateVersionTarget,
        actor: &str,
        reason: &str,
    ) -> Result<PrivateRetirementOutcome, ScratchpadError>;

    fn append_canonical_publication(
        &self,
        draft: CanonicalPublicationDraft,
    ) -> Result<PublicationAppendOutcome, ScratchpadError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PrivateVersionDraft {
    pub(super) address: ScratchpadAddress,
    pub(super) content: Vec<u8>,
    pub(super) producer_invocation_uuid: Uuid,
    pub(super) format_hint: Option<String>,
    pub(super) verdict_line: Option<String>,
    pub(super) predecessor_version: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PrivateAppendOutcome {
    pub(super) version: u64,
    pub(super) producer_invocation_uuid: Option<Uuid>,
    pub(super) sha256: String,
    pub(super) content_len: u64,
    pub(super) format_hint: Option<String>,
    pub(super) verdict_line: Option<String>,
    pub(super) predecessor_version: Option<u64>,
    pub(super) created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PrivateTombstone {
    pub(super) tombstoned_at: DateTime<Utc>,
    pub(super) actor: String,
    pub(super) reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PrivateVersionMeta {
    pub(super) address: ScratchpadAddress,
    pub(super) version: u64,
    pub(super) sha256: String,
    pub(super) content_len: u64,
    pub(super) producer_invocation_uuid: Option<Uuid>,
    pub(super) format_hint: Option<String>,
    pub(super) verdict_line: Option<String>,
    pub(super) predecessor_version: Option<u64>,
    pub(super) created_at: DateTime<Utc>,
    pub(super) tombstone: Option<PrivateTombstone>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PrivateRecordData {
    pub(super) meta: PrivateVersionMeta,
    pub(super) content: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PrivateVersionTarget {
    pub(super) address: ScratchpadAddress,
    pub(super) version: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PrivateVisibility {
    ActiveOnly,
    IncludeTombstoned,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PrivateRetirementOutcome {
    pub(super) status: RetirementStatus,
    pub(super) tombstoned_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CanonicalPublicationDraft {
    pub(super) destination: CanonicalAddress,
    pub(super) content: Vec<u8>,
    pub(super) producer_invocation_uuid: Uuid,
    pub(super) format_hint: Option<String>,
    pub(super) verdict_line: Option<String>,
    pub(super) predecessor_version: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PublicationAppendOutcome {
    pub(super) version: u64,
    pub(super) sha256: String,
    pub(super) content_len: u64,
    pub(super) format_hint: Option<String>,
    pub(super) verdict_line: Option<String>,
    pub(super) predecessor_version: Option<u64>,
    pub(super) created_at: DateTime<Utc>,
}

pub(super) struct ScratchpadApplication<P, N> {
    persistence: P,
    observe_current_utc: N,
}

impl<P, N> ScratchpadApplication<P, N>
where
    P: ScratchpadPersistence,
    N: Fn() -> DateTime<Utc>,
{
    pub(super) fn new(persistence: P, observe_current_utc: N) -> Self {
        Self {
            persistence,
            observe_current_utc,
        }
    }

    pub(super) fn write(&self, req: WriteRequest) -> Result<WriteReceipt, ScratchpadError> {
        let (address, draft) = map_private_version_draft(req);
        let outcome = self.persistence.append_private_version(draft)?;
        Ok(format_write_receipt(address, outcome))
    }

    pub(super) fn read(&self, req: ReadRequest) -> Result<ScratchpadRecord, ScratchpadError> {
        let ReadRequest {
            scope,
            name,
            version,
        } = req;
        let address = map_private_address(scope, name);
        let data = self
            .persistence
            .load_active_private_record(&address, version)?;
        Ok(format_scratchpad_record(data))
    }

    pub(super) fn list(&self, req: ListRequest) -> Result<Vec<ScratchpadMeta>, ScratchpadError> {
        let ListRequest {
            scope,
            name,
            include_tombstoned,
        } = req;
        let visibility = map_list_visibility(include_tombstoned);
        let rows = self.persistence.enumerate_private_versions(
            scope.invocation_uuid,
            name.as_ref(),
            visibility,
        )?;
        Ok(format_scratchpad_metas(rows))
    }

    pub(super) fn delete(&self, req: DeleteRequest) -> Result<DeleteReceipt, ScratchpadError> {
        let DeleteRequest {
            scope,
            name,
            selector,
            actor,
            reason,
        } = req;
        let address = map_private_address(scope, name);
        let (actor, reason) = map_delete_defaults(actor, reason);
        let targets = self.acquire_delete_targets(&address, &selector)?;
        let reduction = self.retire_delete_targets(&targets, &actor, &reason)?;
        Ok(format_delete_receipt(
            address, selector, actor, reason, reduction,
        ))
    }

    pub(super) fn publish(&self, req: PublishRequest) -> Result<PublishReceipt, ScratchpadError> {
        validate_canonical_destination(&req.destination)?;
        let source = self
            .persistence
            .load_active_private_record(&req.source, req.source_version)?;
        let draft = map_publication_draft(&req, &source);
        let outcome = self.persistence.append_canonical_publication(draft)?;
        Ok(format_publish_receipt(req, source, outcome))
    }

    pub(super) fn gc(&self, req: GcRequest) -> Result<GcReport, ScratchpadError> {
        let evaluated_at = (self.observe_current_utc)();
        let GcRequest {
            selector,
            dry_run,
            actor,
            reason,
        } = req;
        let (actor, reason) = map_gc_defaults(actor, reason, &selector);
        let targets = self.acquire_gc_candidates(&selector)?;
        let reduction = if dry_run {
            map_gc_dry_run_addresses(targets.into_iter().map(|target| target.address).collect())
        } else {
            self.retire_gc_targets(&targets, &actor, &reason)?
        };
        Ok(format_gc_report(
            selector,
            dry_run,
            actor,
            reason,
            reduction,
            evaluated_at,
        ))
    }

    fn acquire_delete_targets(
        &self,
        address: &ScratchpadAddress,
        selector: &DeleteSelector,
    ) -> Result<Vec<PrivateVersionTarget>, ScratchpadError> {
        match selector {
            DeleteSelector::Latest => self
                .persistence
                .acquire_newest_active_target(address)
                .map(|target| vec![target]),
            DeleteSelector::Version(version) => self
                .persistence
                .acquire_existing_target(address, *version)
                .map(|target| vec![target]),
            DeleteSelector::AllVersions => self
                .persistence
                .enumerate_private_versions(
                    address.invocation_uuid,
                    Some(&address.name),
                    PrivateVisibility::ActiveOnly,
                )
                .map(map_private_meta_to_targets),
        }
    }

    fn retire_delete_targets(
        &self,
        targets: &[PrivateVersionTarget],
        actor: &str,
        reason: &str,
    ) -> Result<DeleteStatusReduction, ScratchpadError> {
        let mut reduction = DeleteStatusReduction::default();
        for target in targets {
            let outcome = self
                .persistence
                .retire_private_version(target, actor, reason)?;
            partition_delete_version(&mut reduction, target.version, outcome.status);
            project_last_delete_tombstoned_at(&mut reduction, outcome.tombstoned_at);
        }
        Ok(reduction)
    }

    fn acquire_gc_candidates(
        &self,
        selector: &GcSelector,
    ) -> Result<Vec<PrivateVersionTarget>, ScratchpadError> {
        let rows = match selector {
            GcSelector::Invocation(invocation_uuid) => {
                self.persistence.enumerate_private_versions(
                    *invocation_uuid,
                    None,
                    PrivateVisibility::ActiveOnly,
                )?
            }
            GcSelector::ExpiredBefore(cutoff) => {
                let mut eligible_by_age = |created_at| is_expired_at(created_at, *cutoff);
                self.persistence
                    .collect_cleanup_eligible_versions(&mut eligible_by_age)?
            }
        };
        Ok(map_private_meta_to_targets(rows))
    }

    fn retire_gc_targets(
        &self,
        targets: &[PrivateVersionTarget],
        actor: &str,
        reason: &str,
    ) -> Result<GcStatusReduction, ScratchpadError> {
        let mut reduction = GcStatusReduction::default();
        for target in targets {
            let outcome = self
                .persistence
                .retire_private_version(target, actor, reason)?;
            partition_gc_address(&mut reduction, target.address.clone(), outcome.status);
        }
        Ok(reduction)
    }
}

fn map_private_address(scope: InvocationScope, name: ScratchpadName) -> ScratchpadAddress {
    ScratchpadAddress {
        invocation_uuid: scope.invocation_uuid,
        name,
    }
}

fn map_private_version_draft(req: WriteRequest) -> (ScratchpadAddress, PrivateVersionDraft) {
    let WriteRequest {
        scope,
        name,
        content,
        format_hint,
        verdict_line,
        predecessor_version,
    } = req;
    let producer_invocation_uuid = scope.invocation_uuid;
    let address = map_private_address(scope, name);
    let draft = PrivateVersionDraft {
        address: address.clone(),
        content,
        producer_invocation_uuid,
        format_hint,
        verdict_line,
        predecessor_version,
    };
    (address, draft)
}

fn map_list_visibility(include_tombstoned: bool) -> PrivateVisibility {
    if include_tombstoned {
        PrivateVisibility::IncludeTombstoned
    } else {
        PrivateVisibility::ActiveOnly
    }
}

fn map_delete_defaults(actor: Option<String>, reason: Option<String>) -> (String, String) {
    (
        actor.unwrap_or_else(|| DEFAULT_DELETE_ACTOR.to_string()),
        reason.unwrap_or_else(|| DEFAULT_DELETE_REASON.to_string()),
    )
}

fn map_gc_defaults(
    actor: Option<String>,
    reason: Option<String>,
    selector: &GcSelector,
) -> (String, String) {
    let reason = reason.unwrap_or_else(|| match selector {
        GcSelector::Invocation(_) => DEFAULT_GC_INVOCATION_REASON.to_string(),
        GcSelector::ExpiredBefore(_) => DEFAULT_GC_EXPIRED_REASON.to_string(),
    });
    (
        actor.unwrap_or_else(|| DEFAULT_GC_ACTOR.to_string()),
        reason,
    )
}

pub(super) fn map_private_meta_to_target(meta: PrivateVersionMeta) -> PrivateVersionTarget {
    PrivateVersionTarget {
        address: meta.address,
        version: meta.version,
    }
}

fn map_private_meta_to_targets(rows: Vec<PrivateVersionMeta>) -> Vec<PrivateVersionTarget> {
    rows.into_iter().map(map_private_meta_to_target).collect()
}

fn map_publication_draft(
    req: &PublishRequest,
    source: &PrivateRecordData,
) -> CanonicalPublicationDraft {
    CanonicalPublicationDraft {
        destination: req.destination.clone(),
        content: source.content.clone(),
        producer_invocation_uuid: req.source.invocation_uuid,
        format_hint: req
            .format_hint
            .clone()
            .or_else(|| source.meta.format_hint.clone()),
        verdict_line: req
            .verdict_line
            .clone()
            .or_else(|| source.meta.verdict_line.clone()),
        predecessor_version: req.predecessor_version,
    }
}

fn validate_canonical_destination(destination: &CanonicalAddress) -> Result<(), ScratchpadError> {
    if destination
        .workflow_run_id
        .starts_with(crate::SCRATCHPAD_PREFIX)
    {
        return Err(ScratchpadError::InvalidInput(format!(
            "canonical workflow_run_id must not start with reserved prefix {}",
            crate::SCRATCHPAD_PREFIX
        )));
    }
    Ok(())
}

fn is_expired_at(created_at: DateTime<Utc>, cutoff: DateTime<Utc>) -> bool {
    created_at + std::time::Duration::from_secs(7 * 24 * 60 * 60) <= cutoff
}

fn format_scratchpad_meta(meta: PrivateVersionMeta) -> ScratchpadMeta {
    let PrivateVersionMeta {
        address,
        version,
        sha256,
        content_len,
        producer_invocation_uuid,
        format_hint,
        verdict_line,
        predecessor_version,
        created_at,
        tombstone,
    } = meta;
    ScratchpadMeta {
        invocation_uuid: address.invocation_uuid,
        name: address.name.clone(),
        address,
        version,
        sha256,
        content_len,
        producer_invocation_uuid,
        format_hint,
        verdict_line,
        predecessor_version,
        created_at,
        tombstone: tombstone.map(Into::into),
    }
}

fn format_scratchpad_metas(rows: Vec<PrivateVersionMeta>) -> Vec<ScratchpadMeta> {
    rows.into_iter().map(format_scratchpad_meta).collect()
}

fn format_scratchpad_record(data: PrivateRecordData) -> ScratchpadRecord {
    ScratchpadRecord {
        meta: format_scratchpad_meta(data.meta),
        content: data.content,
    }
}

fn format_write_receipt(address: ScratchpadAddress, outcome: PrivateAppendOutcome) -> WriteReceipt {
    WriteReceipt {
        address,
        version: outcome.version,
        producer_invocation_uuid: outcome.producer_invocation_uuid,
        sha256: outcome.sha256,
        content_len: outcome.content_len,
        format_hint: outcome.format_hint,
        verdict_line: outcome.verdict_line,
        predecessor_version: outcome.predecessor_version,
        created_at: outcome.created_at,
    }
}

fn format_delete_receipt(
    address: ScratchpadAddress,
    selector: DeleteSelector,
    actor: String,
    reason: String,
    reduction: DeleteStatusReduction,
) -> DeleteReceipt {
    DeleteReceipt {
        address,
        selector,
        tombstoned_versions: reduction.tombstoned_versions,
        already_tombstoned_versions: reduction.already_tombstoned_versions,
        actor,
        reason,
        tombstoned_at: reduction.last_tombstoned_at,
    }
}

fn format_publish_receipt(
    req: PublishRequest,
    source: PrivateRecordData,
    outcome: PublicationAppendOutcome,
) -> PublishReceipt {
    PublishReceipt {
        source: req.source,
        source_version: source.meta.version,
        source_sha256: source.meta.sha256,
        destination: req.destination,
        destination_version: outcome.version,
        destination_sha256: outcome.sha256,
        content_len: outcome.content_len,
        producer_invocation_uuid: source.meta.address.invocation_uuid,
        format_hint: outcome.format_hint,
        verdict_line: outcome.verdict_line,
        predecessor_version: outcome.predecessor_version,
        created_at: outcome.created_at,
    }
}

fn format_gc_report(
    selector: GcSelector,
    dry_run: bool,
    actor: String,
    reason: String,
    reduction: GcStatusReduction,
    evaluated_at: DateTime<Utc>,
) -> GcReport {
    GcReport {
        selector,
        dry_run,
        tombstoned_rows: reduction.tombstoned_rows,
        already_tombstoned_rows: reduction.already_tombstoned_rows,
        actor,
        reason,
        evaluated_at,
    }
}

#[cfg(test)]
mod tests;
