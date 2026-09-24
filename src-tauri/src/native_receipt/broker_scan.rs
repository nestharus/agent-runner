//! Detached receipt page and exact State integration. This module performs a
//! real provider read without opening State in the worker. Its result is not
//! authority by itself: production dispatch is closed until a broker-owned
//! one-use grant, worker identity and physical Q bind the returned bytes.
use super::{
    OBSERVATION_MAX_RESPONSE_BYTES, OBSERVATION_MAX_SOURCE_BYTES, OBSERVATION_MAX_TURNS,
    ObservationProgress,
};
use oulipoly_provider::client::CancellationToken;
use oulipoly_runtime::provider_registry::ProviderRegistry;
use oulipoly_runtime::session_provider::{
    SessionProviderIdentity, SessionProviderReadPageRequest, SessionProviderTurnProjection,
    read_turn_page,
};
use oulipoly_state::mailbox::{MailboxDb, MailboxDeliveryObservationAnchor};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::Duration;

const PROTOCOL: &str = "receipt-scan-page-v1";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScanRequest {
    protocol: String,
    attempt_id: String,
    invocation_uuid: String,
    anchor: MailboxDeliveryObservationAnchor,
    previous_progress: Option<String>,
    model_name: String,
    cwd: PathBuf,
    config_root: PathBuf,
}

impl ScanRequest {
    pub(crate) fn digest(&self) -> Result<String, String> {
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScanPage {
    provider_instance_id: String,
    settings_id: String,
    session_id: String,
    reader_identity: String,
    snapshot_id: String,
    page_index: u64,
    page_start_sequence: u64,
    page_turn_count: u64,
    snapshot_complete: bool,
    next_page_token: Option<String>,
    resume_token: Option<String>,
    matching_turn_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScanResult {
    protocol: String,
    request_sha256: String,
    page: ScanPage,
}

/// Read-only preparation from the exact pending submission. The broker must
/// eventually own this selection; no caller-supplied path or attempt string
/// may become a production work grant merely by calling this function.
pub(crate) fn prepare_request(
    db: &MailboxDb,
    attempt_id: &str,
) -> Result<Option<ScanRequest>, String> {
    let Some(anchor) = db.delivery_observation_anchor(attempt_id)? else {
        return Ok(None);
    };
    let Some(invocation_uuid) = db.pending_receipt_scan_owner(attempt_id, &anchor)? else {
        return Ok(None);
    };
    if anchor.resume_token.is_none() {
        return Ok(None);
    }
    let Some(runtime) = db
        .wake_session_reader()
        .session_metadata(&anchor.provider_session_id)?
    else {
        return Ok(None);
    };
    if runtime.mode != "headless"
        || runtime.provider_name.as_deref() != Some(anchor.provider_name.as_str())
    {
        return Err("receipt runtime identity changed".into());
    }
    let model_name = runtime.model_name.ok_or("receipt runtime model absent")?;
    let cwd = PathBuf::from(runtime.effective_cwd.ok_or("receipt runtime cwd absent")?);
    let config_root = PathBuf::from(
        runtime
            .models_dir
            .ok_or("receipt runtime config root absent")?,
    );
    if !cwd.is_absolute() || !config_root.is_absolute() {
        return Err("receipt runtime paths must be absolute".into());
    }
    Ok(Some(ScanRequest {
        protocol: PROTOCOL.into(),
        attempt_id: attempt_id.into(),
        invocation_uuid,
        anchor,
        previous_progress: db.delivery_observation_progress(attempt_id)?,
        model_name,
        cwd,
        config_root,
    }))
}

/// This is the actual provider page read used by a future broker-owned worker.
/// It has no State write or negative-delivery path. Transport failure, timeout,
/// cancellation and provider error all leave the submitted attempt pending.
pub(crate) fn scan_with_registry(
    request: &ScanRequest,
    registry: &ProviderRegistry,
    cancellation: &CancellationToken,
    timeout: Duration,
) -> Result<ScanResult, String> {
    if request.protocol != PROTOCOL
        || !request.cwd.is_absolute()
        || !request.config_root.is_absolute()
    {
        return Err("invalid receipt scan request".into());
    }
    let anchor = &request.anchor;
    let endpoint = registry
        .preflight_account(&anchor.provider_name)
        .map_err(|e| e.to_string())?;
    if endpoint.account_name() != anchor.provider_name
        || format!("{}-instance", endpoint.capabilities().provider_id)
            != anchor.provider_instance_id
        || endpoint.settings_id().map_err(|e| e.to_string())? != anchor.settings_id
    {
        return Err("receipt endpoint identity changed".into());
    }
    let reader_identity = endpoint.client().pinned_executable_identity_sha256()?;
    let mut progress: ObservationProgress = request
        .previous_progress
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|e| format!("invalid receipt checkpoint: {e}"))?
        .unwrap_or_default();
    if progress.receipt_policy != 1
        || progress.reader_identity.as_deref() != Some(reader_identity.as_str())
    {
        progress = ObservationProgress {
            receipt_policy: 1,
            reader_identity: Some(reader_identity.clone()),
            ..Default::default()
        };
    }
    if progress.complete {
        // A completed zero-match snapshot advances from its opaque tail. A
        // completed positive checkpoint needs only State CAS, not another read.
        if progress.matching_turns != 0 {
            return Err("completed receipt checkpoint awaits integration".into());
        }
        progress.complete = false;
        progress.snapshot_id = None;
        progress.page_token = None;
        progress.page_index = 0;
        progress.turn_sequence = 0;
    }
    let page = read_turn_page(SessionProviderReadPageRequest {
        registry,
        identity: SessionProviderIdentity {
            model_name: request.model_name.clone(),
            provider_name: anchor.provider_name.clone(),
            provider_instance_id: Some(anchor.provider_instance_id.clone()),
            settings_id: anchor.settings_id.clone(),
        },
        session_id: &anchor.provider_session_id,
        effective_cwd: Some(&request.cwd),
        projection: SessionProviderTurnProjection::UserObservation,
        expected_delivery_nonce: Some(&request.attempt_id),
        cursor: progress.cursor(anchor)?,
        expected_page_index: progress.page_index,
        expected_turn_sequence: progress.turn_sequence,
        max_turns: OBSERVATION_MAX_TURNS,
        max_response_bytes: OBSERVATION_MAX_RESPONSE_BYTES,
        max_source_bytes: OBSERVATION_MAX_SOURCE_BYTES,
        max_inline_body_bytes: 0,
        cancellation,
        timeout,
    })
    .map_err(|e| e.to_string())?;
    if page.provider_instance_id != anchor.provider_instance_id
        || page.settings_id != anchor.settings_id
        || page.session_id != anchor.provider_session_id
        || page.projection != SessionProviderTurnProjection::UserObservation
        || page.page_index != progress.page_index
        || page.page_start_sequence != progress.turn_sequence
        || progress
            .snapshot_id
            .as_ref()
            .is_some_and(|id| id != &page.snapshot_id)
    {
        return Err("receipt page identity/position mismatch".into());
    }
    let matching_turn_ids = page
        .turns
        .iter()
        .filter(|turn| {
            turn.role == "user"
                && turn.canonical_text_sha256.as_deref() == Some(anchor.expected_sha256.as_str())
        })
        .map(|turn| turn.turn_id.clone())
        .collect();
    Ok(ScanResult {
        protocol: PROTOCOL.into(),
        request_sha256: request.digest()?,
        page: ScanPage {
            provider_instance_id: page.provider_instance_id,
            settings_id: page.settings_id,
            session_id: page.session_id,
            reader_identity,
            snapshot_id: page.snapshot_id,
            page_index: page.page_index,
            page_start_sequence: page.page_start_sequence,
            page_turn_count: page.page_turn_count,
            snapshot_complete: page.snapshot_complete,
            next_page_token: page.next_page_token,
            resume_token: page.resume_token,
            matching_turn_ids,
        },
    })
}

/// Returns true only for one exact confirmed turn. Physical Q and a worker
/// success receipt are prerequisites for the future broker caller, never
/// inferred from this value or from a page containing no match.
pub(crate) fn integrate_result(
    db: &MailboxDb,
    request: &ScanRequest,
    result: &ScanResult,
) -> Result<bool, String> {
    if request.protocol != PROTOCOL
        || result.protocol != PROTOCOL
        || result.request_sha256 != request.digest()?
        || db
            .pending_receipt_scan_owner(&request.attempt_id, &request.anchor)?
            .as_deref()
            != Some(&request.invocation_uuid)
    {
        return Err("receipt result has no exact pending request".into());
    }
    let page = &result.page;
    if page.provider_instance_id != request.anchor.provider_instance_id
        || page.settings_id != request.anchor.settings_id
        || page.session_id != request.anchor.provider_session_id
        || page.matching_turn_ids.len() as u64 > page.page_turn_count
        || page
            .matching_turn_ids
            .iter()
            .any(|id| id.is_empty() || id.len() > 1024)
    {
        return Err("receipt result page identity or turn count changed".into());
    }
    let mut progress: ObservationProgress = request
        .previous_progress
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|e| format!("invalid receipt checkpoint: {e}"))?
        .unwrap_or_default();
    if progress.receipt_policy != 1
        || progress.reader_identity.as_deref() != Some(page.reader_identity.as_str())
    {
        progress = ObservationProgress {
            receipt_policy: 1,
            reader_identity: Some(page.reader_identity.clone()),
            ..Default::default()
        };
    }
    if progress.complete {
        if progress.matching_turns != 0 {
            return Err("completed receipt checkpoint awaits integration".into());
        }
        progress.complete = false;
        progress.snapshot_id = None;
        progress.page_token = None;
        progress.page_index = 0;
        progress.turn_sequence = 0;
    }
    if page.page_index != progress.page_index
        || page.page_start_sequence != progress.turn_sequence
        || progress
            .snapshot_id
            .as_ref()
            .is_some_and(|id| id != &page.snapshot_id)
    {
        return Err("receipt result checkpoint position changed".into());
    }
    progress.matching_turns = progress
        .matching_turns
        .saturating_add(page.matching_turn_ids.len() as u64);
    if progress.matching_turn_id.is_none() {
        progress.matching_turn_id = page.matching_turn_ids.first().cloned();
    }
    progress.complete = page.snapshot_complete;
    if page.snapshot_complete {
        progress.after_token = Some(
            page.resume_token
                .clone()
                .ok_or("receipt result resume token missing")?,
        );
        progress.snapshot_id = None;
        progress.page_token = None;
    } else {
        progress.page_index = page
            .page_index
            .checked_add(1)
            .ok_or("receipt page overflow")?;
        progress.turn_sequence = page
            .page_start_sequence
            .checked_add(page.page_turn_count)
            .ok_or("receipt sequence overflow")?;
        progress.snapshot_id = Some(page.snapshot_id.clone());
        progress.page_token = Some(
            page.next_page_token
                .clone()
                .ok_or("receipt result next page token missing")?,
        );
    }
    let next = serde_json::to_string(&progress).map_err(|e| e.to_string())?;
    let cwd = request.cwd.to_str().ok_or("receipt cwd is not UTF-8")?;
    let config_root = request
        .config_root
        .to_str()
        .ok_or("receipt config root is not UTF-8")?;
    if !db.advance_delivery_observation_progress_exact(
        &request.attempt_id,
        &request.invocation_uuid,
        &request.anchor,
        request.previous_progress.as_deref(),
        &next,
        &request.model_name,
        cwd,
        config_root,
    )? {
        return Err("receipt result checkpoint CAS lost; attempt remains pending".into());
    }
    if progress.complete && progress.matching_turns == 1 {
        db.confirm_native_delivery_receipt(
            &request.attempt_id,
            &request.invocation_uuid,
            &request.anchor,
            &next,
            progress
                .matching_turn_id
                .as_deref()
                .ok_or("receipt match id absent")?,
        )
    } else {
        Ok(false)
    }
}

/// Recover the narrow crash window after a successful page checkpoint but
/// before its positive receipt publication. The checkpoint is the retained
/// result of an already integrated page; this never authorizes a new scan or
/// asserts physical Q. The eventual broker must verify its original worker
/// result and Q before first calling integrate_result.
pub(crate) fn finish_completed_checkpoint(
    db: &MailboxDb,
    request: &ScanRequest,
) -> Result<bool, String> {
    if db
        .pending_receipt_scan_owner(&request.attempt_id, &request.anchor)?
        .as_deref()
        != Some(&request.invocation_uuid)
    {
        return Ok(false);
    }
    let Some(checkpoint) = db.delivery_observation_progress(&request.attempt_id)? else {
        return Ok(false);
    };
    if request.previous_progress.as_deref() != Some(checkpoint.as_str()) {
        return Ok(false);
    }
    let progress: ObservationProgress = serde_json::from_str(&checkpoint)
        .map_err(|e| format!("invalid retained receipt checkpoint: {e}"))?;
    if !progress.complete
        || progress.matching_turns != 1
        || progress.receipt_policy != 1
        || progress
            .reader_identity
            .as_deref()
            .is_none_or(str::is_empty)
    {
        return Ok(false);
    }
    db.confirm_native_delivery_receipt(
        &request.attempt_id,
        &request.invocation_uuid,
        &request.anchor,
        &checkpoint,
        progress
            .matching_turn_id
            .as_deref()
            .ok_or("receipt match id absent")?,
    )
}
