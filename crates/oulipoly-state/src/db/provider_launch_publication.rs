//! Native authority joins. Caller observations and replay hashes are not publication.
//! Declared roles: validator, accessor.
use super::provider_launch_lifecycle::*;
use super::*;
use crate::mailbox::{CompletionAuthorityFence, NativePublication};
use sha2::{Digest, Sha256};

pub(super) fn needs_publication(
    conn: &sqlite::Connection,
    owner: &ProviderLaunchOwnerFence,
    operation: &str,
) -> Result<bool, String> {
    if matches!(
        operation,
        "native-recovered-custody" | "native-runtime-cancellation" | "native-channel-duty-owner"
    ) {
        return Ok(true);
    }
    if !matches!(operation, "settle_cancel" | "certify" | "successor")
        && !operation.starts_with("reconcile/")
    {
        return Ok(false);
    }
    // Any retained native association selects action-time validation; generic
    // producer-supplied proofs without native records keep their own contract.
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM provider_launch_transition_replays WHERE logical_launch_id=?1 AND operation_key IN (?2,?3,?4,?5,?6))",
        params![owner.logical_launch_id.to_string(), format!("{}/native-recovery-receipts", owner.attempt_id),
            format!("{}/native-custody-receipts", owner.attempt_id), format!("{}/native-recovered-custody-receipts", owner.attempt_id),
            format!("{}/native-runtime-cancellation-receipts", owner.attempt_id), format!("{}/native-channel-duty", owner.attempt_id)],
        |row| row.get(0),
    ).map_err(|e| e.to_string())
}

pub(super) fn validate(
    conn: &sqlite::Connection,
    fence: &CompletionAuthorityFence<'_>,
    owner: &ProviderLaunchOwnerFence,
    operation: &str,
    input: &serde_json::Value,
) -> Result<(), String> {
    let generation: String = conn.query_row(
        "SELECT runtime_generation_uuid FROM provider_launch_attempts WHERE attempt_id=?1 AND invocation_uuid=?2",
        params![owner.attempt_id.to_string(), owner.invocation_uuid.to_string()], |row| row.get(0),
    ).map_err(|e| e.to_string())?;
    let published = fence.native_publication(&generation, &owner.invocation_uuid.to_string())?;
    // Validate historical recovery before reuse; never overwrite it with a newer
    // observation or let launch_transition's replay short circuit this join.
    if let Some(recovery) = retained(conn, owner, "native-recovered-custody-receipts")? {
        validate_drain(&published, &recovery["recovery_evidence"]["original_drain"])?;
    }
    match operation {
        "native-recovered-custody" => {
            validate_drain(&published, &input["recovery_evidence"]["original_drain"])
        }
        "native-runtime-cancellation" => validate_supplement(&published, input),
        "native-channel-duty-owner" => validate_channel(&published, owner, input),
        "settle_cancel" | "certify" | "successor" => {
            validate_settlement(conn, owner, &published, input)
        }
        _ if operation.starts_with("reconcile/") => {
            if input[1].is_null() {
                Ok(())
            } else {
                validate_settlement(conn, owner, &published, &input[1])
            }
        }
        _ => Err("native_publication_operation_conflict".into()),
    }
}

fn retained(
    conn: &sqlite::Connection,
    owner: &ProviderLaunchOwnerFence,
    key: &str,
) -> Result<Option<serde_json::Value>, String> {
    let raw: Option<String> = conn.query_row(
        "SELECT result_json FROM provider_launch_transition_replays WHERE logical_launch_id=?1 AND operation_key=?2",
        params![owner.logical_launch_id.to_string(), format!("{}/{key}", owner.attempt_id)], |r| r.get(0),
    ).optional().map_err(|e| e.to_string())?;
    raw.map(|raw| serde_json::from_str(&raw).map_err(|e| e.to_string()))
        .transpose()
}

fn validate_drain(
    published: &NativePublication,
    expected: &serde_json::Value,
) -> Result<(), String> {
    if expected.is_null() || published.original_drain.as_ref() != Some(expected) {
        return Err("native_publication_original_drain_conflict".into());
    }
    Ok(())
}

fn validate_supplement(
    published: &NativePublication,
    input: &serde_json::Value,
) -> Result<(), String> {
    validate_drain(published, &input["original_drain"])?;
    let row = published
        .runtime
        .as_ref()
        .ok_or("native_publication_runtime_absent")?;
    if serde_json::to_value(row).map_err(|e| e.to_string())? != input["original_runtime_row"] {
        return Err("native_publication_runtime_conflict".into());
    }
    if row.spawned_os_pid.is_some()
        && !input["original_drain"]["receipt"]["accepted_cancellation"].is_string()
        && published.producer_quiescent != Some(true)
    {
        return Err("native_original_current_quiescence_absent".into());
    }
    Ok(())
}

fn validate_channel(
    published: &NativePublication,
    owner: &ProviderLaunchOwnerFence,
    input: &serde_json::Value,
) -> Result<(), String> {
    let channel: ProviderLaunchChannelSettlement =
        serde_json::from_value(input.clone()).map_err(|e| e.to_string())?;
    let ProviderLaunchChannelSettlement::ContinuingCustody {
        domain_id,
        original_owner,
        ..
    } = channel
    else {
        return Err("native_publication_channel_kind_conflict".into());
    };
    let drain = published
        .original_drain
        .as_ref()
        .ok_or("native_publication_original_drain_absent")?;
    if drain["domain_id"].as_str() != Some(domain_id.as_str()) || original_owner != *owner {
        return Err("native_publication_channel_owner_conflict".into());
    }
    Ok(())
}

fn validate_settlement(
    conn: &sqlite::Connection,
    owner: &ProviderLaunchOwnerFence,
    published: &NativePublication,
    input: &serde_json::Value,
) -> Result<(), String> {
    let proof: ProviderLaunchCustodyProof =
        serde_json::from_value(input.clone()).map_err(|e| e.to_string())?;
    let row = published
        .runtime
        .as_ref()
        .ok_or("native_publication_runtime_absent")?;
    if row.generation_id.to_string() != proof.runtime_generation_uuid.to_string()
        || row.spawn_invocation_uuid != proof.spawn_invocation_uuid.to_string()
        || row.lifecycle_state != crate::mailbox::RuntimeLifecycleState::Exited
        || row.exited_at.is_none()
        || row.active_delivery_claim_id.is_some()
        || row.active_delivery_claimed_at.is_some()
        || !row.active_delivery_seqs.is_empty()
    {
        return Err("native_publication_runtime_not_settled".into());
    }
    let process_hash = match &row.exact_process_evidence {
        crate::mailbox::ExactProcessEvidence::NotRecorded => None,
        crate::mailbox::ExactProcessEvidence::Recorded(process) => Some(hash(process)?),
    };
    if process_hash != proof.runtime_process_identity_sha256
        || proof.runtime_never_bound != process_hash.is_none()
    {
        return Err("native_publication_process_conflict".into());
    }
    let row_hash = hash(row)?;
    if proof.runtime_settlement_sha256 != row_hash {
        let supplement = retained(conn, owner, "native-runtime-cancellation-receipts")?
            .ok_or("native_publication_supplement_absent")?;
        validate_supplement(published, &supplement)?;
        if hash(&supplement)? != proof.runtime_settlement_sha256
            || supplement["cancellation_terminal_code"].as_str()
                != Some(proof.runtime_terminal_code.as_str())
        {
            return Err("native_publication_supplement_conflict".into());
        }
    } else if serde_json::to_value(row.terminal_reason)
        .map_err(|e| e.to_string())?
        .as_str()
        != Some(proof.runtime_terminal_code.as_str())
    {
        return Err("native_publication_terminal_conflict".into());
    }
    if matches!(
        proof.channel,
        ProviderLaunchChannelSettlement::ContinuingCustody { .. }
    ) {
        validate_channel(
            published,
            owner,
            &serde_json::to_value(&proof.channel).map_err(|e| e.to_string())?,
        )?;
    }
    Ok(())
}

fn hash(value: &impl serde::Serialize) -> Result<String, String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).map_err(|e| e.to_string())?)
    ))
}
