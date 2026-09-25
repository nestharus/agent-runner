//! Private pre-send F join. Only the original recipient can call the broker
//! endpoint; this helper obtains the selected adapter's typed Tail first.
use oulipoly_kernel_broker::protocol::{FreshRecipientRequest, fresh_recipient_request_at};
use oulipoly_provider::client::CancellationToken;
use oulipoly_runtime::provider_registry::ProviderRegistry;
use oulipoly_runtime::session_provider::{
    SessionProviderIdentity, SessionProviderPageCursor, SessionProviderReadPageRequest,
    SessionProviderTurnProjection, read_turn_page,
};
use oulipoly_state::mailbox::{
    FreshDeliveryReadback, FreshNativeFPreparation, FreshNativeFPrepareRequest,
};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;

pub(crate) struct PrivateNativeFInput<'a> {
    pub socket: &'a Path,
    pub registry: &'a ProviderRegistry,
    pub identity: SessionProviderIdentity,
    pub effective_cwd: &'a Path,
    pub grant: &'a FreshDeliveryReadback,
    pub delivery_request_id: &'a str,
    pub delivery_token: &'a str,
    pub runtime_generation_id: &'a str,
    pub preparation_request_id: &'a str,
    pub envelope_nonce: &'a str,
}

/// The root recipient can only offer this source after its own fresh session
/// has a resident interactive generation and a selected adapter page reader.
/// The private headless K/Q route does not currently provide either one.
pub(crate) struct OriginalRecipientNativeFSource<'a> {
    pub registry: &'a ProviderRegistry,
    pub identity: SessionProviderIdentity,
    pub effective_cwd: &'a Path,
    pub runtime_generation_id: &'a str,
    pub preparation_request_id: &'a str,
    pub envelope_nonce: &'a str,
}

/// Read back the original recipient's exact F key before considering a native
/// preparation. A lost F reply has no token here and must remain unknown.
pub(crate) fn prepare_original_recipient_native_f(
    socket: &Path,
    session_id: &str,
    delivery_request_id: &str,
    grant: &FreshDeliveryReadback,
    delivery_token: Option<&str>,
    source: Option<OriginalRecipientNativeFSource<'_>>,
) -> Result<FreshNativeFPreparation, String> {
    let read = fresh_recipient_request_at(
        socket,
        &FreshRecipientRequest::Read {
            delivery_request_id: delivery_request_id.into(),
        },
    )
    .map_err(|e| format!("native F exact delivery readback unavailable: {e}"))?;
    let current: FreshDeliveryReadback = serde_json::from_value(read["grant"].clone())
        .map_err(|_| "native F exact delivery readback absent")?;
    if &current != grant || current.session_id != session_id {
        return Err("native F original recipient grant or fresh session changed".into());
    }
    if !matches!(current.phase.as_str(), "unknown" | "submitted") {
        return Err("native F original recipient grant is no longer pending".into());
    }
    let token = delivery_token.ok_or("native F delivery token unknown after lost F reply")?;
    let source = source.ok_or(
        "native F original-root resident PTY generation and selected adapter page authority absent",
    )?;
    let prepared = prepare_private_native_f_input(PrivateNativeFInput {
        socket,
        registry: source.registry,
        identity: source.identity,
        effective_cwd: source.effective_cwd,
        grant,
        delivery_request_id,
        delivery_token: token,
        runtime_generation_id: source.runtime_generation_id,
        preparation_request_id: source.preparation_request_id,
        envelope_nonce: source.envelope_nonce,
    })?;
    let after = fresh_recipient_request_at(
        socket,
        &FreshRecipientRequest::Read {
            delivery_request_id: delivery_request_id.into(),
        },
    )
    .map_err(|e| format!("native F post-preparation delivery readback unknown: {e}"))?;
    let latest: FreshDeliveryReadback = serde_json::from_value(after["grant"].clone())
        .map_err(|_| "native F post-preparation delivery readback absent")?;
    if latest != current {
        return Err("native F original recipient grant changed after preparation".into());
    }
    Ok(prepared)
}

/// No PTY control call or ACK follows this preparation. A caller must keep the
/// returned immutable record and later use separate native receipt authority.
pub(crate) fn prepare_private_native_f_input(
    input: PrivateNativeFInput<'_>,
) -> Result<FreshNativeFPreparation, String> {
    if !matches!(input.grant.phase.as_str(), "unknown" | "submitted")
        || input.grant.grant_id.is_empty()
        || input.identity.provider_name.is_empty()
    {
        return Err("native F grant or provider identity unavailable".into());
    }
    let endpoint = input
        .registry
        .preflight_account(&input.identity.provider_name)
        .map_err(|e| e.to_string())?;
    let instance = format!("{}-instance", endpoint.capabilities().provider_id);
    let settings = endpoint.settings_id().map_err(|e| e.to_string())?;
    if endpoint.account_name() != input.identity.provider_name
        || input.identity.provider_instance_id.as_deref() != Some(instance.as_str())
        || input.identity.settings_id != settings
        || !endpoint.capabilities().capabilities.session
        || !endpoint.capabilities().capabilities.session_turn_pages_v1
    {
        return Err("native F selected adapter identity or page capability unavailable".into());
    }
    let cancellation = CancellationToken::new();
    let page = read_turn_page(SessionProviderReadPageRequest {
        registry: input.registry,
        identity: input.identity.clone(),
        session_id: &input.grant.session_id,
        effective_cwd: Some(input.effective_cwd),
        projection: SessionProviderTurnProjection::UserObservation,
        expected_delivery_nonce: Some(input.envelope_nonce),
        cursor: SessionProviderPageCursor::Tail,
        expected_page_index: 0,
        expected_turn_sequence: 0,
        max_turns: 64,
        max_response_bytes: 256 * 1024,
        max_source_bytes: 4 * 1024 * 1024,
        max_inline_body_bytes: 16 * 1024,
        cancellation: &cancellation,
        timeout: Duration::from_secs(5),
    })
    .map_err(|e| e.to_string())?;
    if page.provider_instance_id != instance
        || page.settings_id != settings
        || page.session_id != input.grant.session_id
        || !page.snapshot_complete
        || !page.turns.is_empty()
    {
        return Err("native F pre-send Tail page identity or completion changed".into());
    }
    let tail = page
        .resume_token
        .filter(|token| !token.is_empty())
        .ok_or("native F pre-send Tail anchor missing")?;
    let prior = fresh_recipient_request_at(
        input.socket,
        &FreshRecipientRequest::ReadNativeFPreparation {
            preparation_request_id: input.preparation_request_id.into(),
        },
    )
    .map_err(|e| e.to_string())?;
    if prior["kind"] != "native_f_preparation_readback" {
        return Err("native F preparation readback kind changed".into());
    }
    if !prior["preparation"].is_null() {
        let record: FreshNativeFPreparation =
            serde_json::from_value(prior["preparation"].clone()).map_err(|e| e.to_string())?;
        if record.preparation_request_id != input.preparation_request_id
            || record.grant_id != input.grant.grant_id
            || record.delivery_request_id != input.delivery_request_id
            || record.delivery_token_sha256
                != format!("{:x}", Sha256::digest(input.delivery_token.as_bytes()))
            || record.runtime_generation_id != input.runtime_generation_id
            || record.envelope_nonce != input.envelope_nonce
            || record.provider_account != input.identity.provider_name
            || Some(record.provider_instance_id.as_str())
                != input.identity.provider_instance_id.as_deref()
            || record.settings_id != input.identity.settings_id
            || record.session_id != input.grant.session_id
            || record.seq != input.grant.seq
            || record.source_id != input.grant.source_id
            || record.attempt_id != input.grant.attempt_id
            || record.lane_id != input.grant.lane_id
            || record.source_generation != input.grant.source_generation
            || record.root_id != input.grant.root_id
            || record.owner_generation != input.grant.owner_generation
            || record.payload_sha256 != input.grant.payload_sha256
            || record.payload_byte_len != input.grant.payload_byte_len
            || record.provider_session_id != input.grant.session_id
            || record.tail_resume_token != tail
            || record.envelope_sha256
                != format!("{:x}", Sha256::digest(record.envelope_text.as_bytes()))
        {
            return Err("native F preparation readback conflicts with exact request".into());
        }
        return Ok(record);
    }
    let request = FreshNativeFPrepareRequest {
        preparation_request_id: input.preparation_request_id.into(),
        delivery_request_id: input.delivery_request_id.into(),
        grant_id: input.grant.grant_id.clone(),
        delivery_token: input.delivery_token.into(),
        runtime_generation_id: input.runtime_generation_id.into(),
        provider_instance_id: instance.clone(),
        settings_id: settings.into(),
        envelope_nonce: input.envelope_nonce.into(),
        tail_resume_token: tail.clone(),
    };
    let response = fresh_recipient_request_at(
        input.socket,
        &FreshRecipientRequest::PrepareNativeF {
            preparation: request,
        },
    )
    .map_err(|e| e.to_string())?;
    if response["kind"] != "native_f_preparation" {
        return Err("native F preparation reply kind changed".into());
    }
    let record: FreshNativeFPreparation =
        serde_json::from_value(response["preparation"].clone()).map_err(|e| e.to_string())?;
    if record.preparation_request_id != input.preparation_request_id
        || record.delivery_request_id != input.delivery_request_id
        || record.delivery_token_sha256
            != format!("{:x}", Sha256::digest(input.delivery_token.as_bytes()))
        || record.grant_id != input.grant.grant_id
        || record.session_id != input.grant.session_id
        || record.provider_session_id != input.grant.session_id
        || record.seq != input.grant.seq
        || record.source_id != input.grant.source_id
        || record.attempt_id != input.grant.attempt_id
        || record.lane_id != input.grant.lane_id
        || record.source_generation != input.grant.source_generation
        || record.root_id != input.grant.root_id
        || record.owner_generation != input.grant.owner_generation
        || record.payload_sha256 != input.grant.payload_sha256
        || record.payload_byte_len != input.grant.payload_byte_len
        || record.provider_account != input.identity.provider_name
        || record.provider_instance_id != instance
        || record.settings_id != settings
        || record.runtime_generation_id != input.runtime_generation_id
        || record.envelope_nonce != input.envelope_nonce
        || record.tail_resume_token != tail
        || record.envelope_sha256
            != format!("{:x}", Sha256::digest(record.envelope_text.as_bytes()))
    {
        return Err("native F preparation reply changed selected F or endpoint".into());
    }
    Ok(record)
}
