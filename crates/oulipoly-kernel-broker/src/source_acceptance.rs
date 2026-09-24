//! Read-only v2 source assessment after physical drain. This returns evidence
//! suitable for a future broker-owned commit, never an accepted State/source
//! transition or a recipient payload. Original files remain same-UID mutable.
use crate::source_physical::{SourceObservation, SourcePhysicalRegistry};
use oulipoly_state::completion_continuation::{
    MAX_REGISTRATION_BYTES, VerifiedCompletion, read_source_file, sha256,
};
use oulipoly_state::mailbox::BrokerSidecar;
use serde_json::Value;
use std::io::Read;
use std::path::Path;

#[derive(Debug, PartialEq, Eq)]
pub struct SourceV2Candidate {
    pub grant_id: String,
    pub registration_id: String,
    pub snapshot_sha256: String,
    pub outcome_sha256: String,
    pub recovery_stdout_sha256: String,
}

/// Causally joins the retained State registration/listener, the original v2
/// outcome and artifact bytes, the exact Bash reply, and the root-only wait,
/// drain and output record. A zero worker exit or matching stdout hash alone
/// cannot produce this assessment. No acceptance marker is written here.
pub fn assess_v2_candidate(
    sidecar: &BrokerSidecar,
    physical: &SourcePhysicalRegistry,
    grant_id: &str,
) -> Result<SourceV2Candidate, String> {
    let record = physical
        .records()
        .iter()
        .find(|record| record.grant.grant_id == grant_id)
        .ok_or("source physical record absent")?;
    let (output, stderr) = match physical.observe(grant_id).map_err(|e| e.to_string())? {
        SourceObservation::Drained {
            worker_wait_status: 0,
            stdout,
            stderr,
            cancel_requested: false,
        } => (stdout, stderr),
        _ => return Err("source recovery has no successful uncancelled physical drain".into()),
    };
    let binding = sidecar.read_consumed_source_candidate(&record.grant)?;
    let source = binding.registration()?;
    let original_registration = read_source_file(
        Path::new(&source.handle_dir),
        &source.registration_relative,
        MAX_REGISTRATION_BYTES,
    )?;
    if original_registration != binding.registration_bytes() {
        return Err("original registration differs from State admission".into());
    }
    let evidence = VerifiedCompletion::from_source_files(&binding)?;
    // The physical stream is unlimited. Only the structured recovery reply has
    // a protocol bound; an oversized or non-JSON stdout remains physical debt.
    const MAX_REPLY_BYTES: usize = MAX_REGISTRATION_BYTES * 2;
    let stdout = physical
        .open_drained_stdout(grant_id)
        .map_err(|e| e.to_string())?;
    let mut reply_bytes = Vec::new();
    stdout
        .take(MAX_REPLY_BYTES as u64 + 1)
        .read_to_end(&mut reply_bytes)
        .map_err(|e| e.to_string())?;
    if reply_bytes.len() > MAX_REPLY_BYTES
        || reply_bytes.len() as u64 != output.byte_len
        || sha256(&reply_bytes) != output.sha256
    {
        return Err("recovery reply is incomplete or exceeds v2 reply bound".into());
    }
    let reply: Value = serde_json::from_slice(&reply_bytes).map_err(|e| e.to_string())?;
    evidence.validate_source_reply(&reply)?;
    if sidecar.read_consumed_source_candidate(&record.grant)? != binding
        || physical.observe(grant_id).map_err(|e| e.to_string())?
            != (SourceObservation::Drained {
                worker_wait_status: 0,
                stdout: output.clone(),
                stderr,
                cancel_requested: false,
            })
    {
        return Err("State or physical source changed during assessment".into());
    }
    Ok(SourceV2Candidate {
        grant_id: grant_id.into(),
        registration_id: source.registration_id,
        snapshot_sha256: evidence.snapshot_sha256,
        outcome_sha256: evidence.outcome_sha256,
        recovery_stdout_sha256: output.sha256,
    })
}
