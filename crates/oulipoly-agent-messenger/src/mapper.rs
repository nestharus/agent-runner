//! ## Declared roles
//!
//! - mapper
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-agent-messenger/src/mapper.rs
//!     role: adapter
//!     Translates:
//!       - oulipoly-agent-store artifact request and record contract
//!       - oulipoly-agent-scratchpad read request and record contract
//!       - agent-messenger returned-artifact receipt contract
//!       - agent-messenger returned-artifact address contract
//! ```

use crate::MessengerError;
use crate::address::{invocation_from_return_workflow, parse_version_id, return_lookup_key};
use crate::formatter::{RECEIPT_SCHEMA_VERSION, return_workflow, version_id};
use crate::model::{
    ReturnLookup, ReturnPayload, ReturnRequest, ReturnedArtifact, ReturnedArtifactMeta,
    ReturnedArtifactRecord, ReturnedArtifactSource, ShowReturnedRequest, StoreAddress,
};
use oulipoly_agent_scratchpad::{InvocationScope, ReadRequest, ScratchpadName, ScratchpadRecord};
use oulipoly_agent_store::{ArtifactMeta, ArtifactRecord, ListFilter, PutReceipt, PutRequest};
use std::path::PathBuf;
use uuid::Uuid;

pub(crate) fn inline_return_payload(
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

pub(crate) fn scratchpad_read_request(
    invocation_uuid: Uuid,
    name: ScratchpadName,
    version: Option<u64>,
) -> ReadRequest {
    ReadRequest {
        scope: InvocationScope { invocation_uuid },
        name,
        version,
    }
}

pub(crate) fn scratchpad_return_payload(
    requested_name: &ScratchpadName,
    request_format_hint: Option<String>,
    request_verdict_line: Option<String>,
    record: ScratchpadRecord,
) -> ReturnPayload {
    ReturnPayload {
        source: ReturnedArtifactSource::Scratchpad {
            name: requested_name.as_str().to_string(),
            version: record.meta.version,
        },
        format_hint: request_format_hint.or(record.meta.format_hint),
        verdict_line: request_verdict_line.or(record.meta.verdict_line),
        content: record.content,
    }
}

pub(crate) fn return_put_request(req: &ReturnRequest, payload: ReturnPayload) -> PutRequest {
    PutRequest {
        key: return_put_key(req),
        producer_invocation_uuid: Some(req.invocation_uuid),
        format_hint: payload.format_hint,
        verdict_line: payload.verdict_line,
        predecessor_version: None,
        content: payload.content,
    }
}

fn return_put_key(req: &ReturnRequest) -> oulipoly_agent_store::ArtifactKey {
    return_lookup_key(req.invocation_uuid, req.name.as_str().to_string())
}

pub(crate) fn returned_list_filter(
    invocation_uuid: Uuid,
    name: Option<crate::ReturnName>,
) -> ListFilter {
    ListFilter {
        workflow_run_id: Some(return_workflow(invocation_uuid)),
        artifact_name: name.map(|name| name.as_str().to_string()),
        include_tombstoned: false,
    }
}

pub(crate) fn return_lookup(req: ShowReturnedRequest) -> Result<ReturnLookup, MessengerError> {
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
            name.as_str().to_string(),
            version,
        )),
    }
}

fn return_lookup_from_version_id(
    db_path: PathBuf,
    version_id: &str,
) -> Result<ReturnLookup, MessengerError> {
    let (invocation_uuid, name, version) = parse_version_id(version_id)?;
    Ok(return_lookup_from_address(
        db_path,
        invocation_uuid,
        name,
        Some(version),
    ))
}

fn return_lookup_from_address(
    db_path: PathBuf,
    invocation_uuid: Uuid,
    name: String,
    version: Option<u64>,
) -> ReturnLookup {
    ReturnLookup {
        db_path,
        key: return_lookup_key(invocation_uuid, name),
        version,
    }
}

pub(crate) fn returned_from_put(
    receipt: PutReceipt,
    invocation_uuid: Uuid,
    source: ReturnedArtifactSource,
) -> ReturnedArtifact {
    ReturnedArtifact {
        schema_version: RECEIPT_SCHEMA_VERSION,
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

pub(crate) fn metas_from_store(
    rows: Vec<ArtifactMeta>,
) -> Result<Vec<ReturnedArtifactMeta>, MessengerError> {
    rows.into_iter().map(meta_from_store).collect()
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

pub(crate) fn record_from_store(
    record: ArtifactRecord,
) -> Result<ReturnedArtifactRecord, MessengerError> {
    Ok(ReturnedArtifactRecord {
        meta: meta_from_store(record.meta)?,
        content: record.content,
    })
}
