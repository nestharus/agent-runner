//! ## Declared roles
//!
//! `orchestration`, `validator`, `formatter`, `mapper`, `predicate`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/run/continuation_handoff.rs
//!     role: adapter
//!     Translates:
//!       - runtime-handoff-publisher-port-contract
//!       - immutable-filesystem-handoff-schema-contract
//! ```

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use oulipoly_runtime::fresh_continuation::{
    ArtifactIdentity, ContinuationBlock, ContinuationBlockKind, ContinuationEvidence,
    ContinuationHandoff, FreshContinuationRequest, HandoffPublisher, InvocationDisposition,
    InvocationOutcome, PublishedHandoff, ResumeAcceptance,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct FilesystemHandoffPublisher {
    planning_root: PathBuf,
}

#[cfg_attr(not(test), allow(dead_code))]
impl FilesystemHandoffPublisher {
    pub(crate) fn new(planning_root: PathBuf) -> Self {
        Self { planning_root }
    }
}

impl HandoffPublisher for FilesystemHandoffPublisher {
    fn publish(
        &mut self,
        handoff: ContinuationHandoff,
    ) -> Result<PublishedHandoff, ContinuationBlock> {
        validate_continuation_id(&handoff.continuation_id)?;
        let output_dir = self.planning_root.join("continuations");
        fs::create_dir_all(&output_dir).map_err(persistence)?;
        let canonical_root = canonical_path(&self.planning_root)?;
        let canonical_output = canonical_path(&output_dir)?;
        validate_output_dir(&canonical_root, &canonical_output)?;
        let target_name = handoff_name(&handoff.continuation_id);
        let target = publication_path(&output_dir, &target_name);
        let bytes = handoff_bytes(&handoff)?;
        let sha256 = sha256(&bytes);

        if target.exists() {
            return reconcile_existing(&target, &bytes, sha256);
        }

        let temp_name = temporary_handoff_name(&handoff.continuation_id, Uuid::new_v4());
        let temp = publication_path(&output_dir, &temp_name);
        write_durable_temp(&temp, &bytes)?;
        let publish_result = link_publication(&temp, &target, &bytes, sha256);
        let _ = fs::remove_file(temp);
        publish_result
    }
}

fn validate_continuation_id(continuation_id: &str) -> Result<(), ContinuationBlock> {
    if continuation_id.is_empty()
        || !continuation_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ContinuationBlock {
            kind: ContinuationBlockKind::InvalidEvidence,
            message: "Invalid continuation ID for handoff publication".to_string(),
        });
    }
    Ok(())
}

fn canonical_path(path: &Path) -> Result<PathBuf, ContinuationBlock> {
    fs::canonicalize(path).map_err(persistence)
}

fn validate_output_dir(
    canonical_root: &Path,
    canonical_output: &Path,
) -> Result<(), ContinuationBlock> {
    if !canonical_output.starts_with(canonical_root) {
        return Err(ContinuationBlock {
            kind: ContinuationBlockKind::InvalidEvidence,
            message: "Continuation handoff directory escapes the planning root".to_string(),
        });
    }
    Ok(())
}

fn write_durable_temp(path: &Path, bytes: &[u8]) -> Result<(), ContinuationBlock> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(persistence)?;
    file.write_all(bytes).map_err(persistence)?;
    file.sync_all().map_err(persistence)
}

fn link_publication(
    temp: &Path,
    target: &Path,
    expected: &[u8],
    sha256: String,
) -> Result<PublishedHandoff, ContinuationBlock> {
    match fs::hard_link(temp, target) {
        Ok(()) => Ok(published_handoff(target, sha256)),
        Err(error) => reconcile_link_error(error, target, expected, sha256),
    }
}

fn reconcile_link_error(
    error: std::io::Error,
    target: &Path,
    expected: &[u8],
    sha256: String,
) -> Result<PublishedHandoff, ContinuationBlock> {
    if error.kind() == std::io::ErrorKind::AlreadyExists {
        return reconcile_existing(target, expected, sha256);
    }
    Err(persistence(error))
}

fn reconcile_existing(
    path: &Path,
    expected: &[u8],
    sha256: String,
) -> Result<PublishedHandoff, ContinuationBlock> {
    let file_type = existing_file_type(path)?;
    validate_existing_file_type(file_type)?;
    let existing = existing_handoff_bytes(path)?;
    validate_existing_bytes(&existing, expected)?;
    Ok(published_handoff(path, sha256))
}

fn existing_file_type(path: &Path) -> Result<fs::FileType, ContinuationBlock> {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type())
        .map_err(persistence)
}

fn existing_handoff_bytes(path: &Path) -> Result<Vec<u8>, ContinuationBlock> {
    fs::read(path).map_err(persistence)
}

fn validate_existing_file_type(file_type: fs::FileType) -> Result<(), ContinuationBlock> {
    if file_type.is_symlink() {
        return Err(conflict("Existing continuation handoff is a symlink"));
    }
    Ok(())
}

fn validate_existing_bytes(existing: &[u8], expected: &[u8]) -> Result<(), ContinuationBlock> {
    if existing != expected {
        return Err(conflict(
            "Existing continuation handoff differs from the requested publication",
        ));
    }
    Ok(())
}

fn handoff_name(continuation_id: &str) -> String {
    format!("{continuation_id}.json")
}

fn temporary_handoff_name(continuation_id: &str, nonce: Uuid) -> String {
    format!(".{continuation_id}.{nonce}.tmp")
}

fn publication_path(output_dir: &Path, name: &str) -> PathBuf {
    output_dir.join(name)
}

fn published_handoff(path: &Path, sha256: String) -> PublishedHandoff {
    PublishedHandoff {
        path: path.to_path_buf(),
        sha256,
    }
}

fn handoff_bytes(handoff: &ContinuationHandoff) -> Result<Vec<u8>, ContinuationBlock> {
    let mut bytes = serde_json::to_vec_pretty(&json!({
        "schema_version": 1,
        "kind": "fresh_continuation_handoff",
        "continuation_id": handoff.continuation_id,
        "fresh_prompt": handoff.fresh_prompt,
        "request": request_json(&handoff.request),
        "resume": outcome_json(&handoff.resume),
        "fresh": handoff.fresh.as_ref().map(outcome_json),
    }))
    .map_err(persistence)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn request_json(request: &FreshContinuationRequest) -> Value {
    json!({
        "schema_version": 1,
        "kind": "fresh_continuation_request",
        "question_id": request.question_id,
        "origin_invocation_id": request.origin_invocation_id,
        "origin_session_id": request.origin_session_id,
        "planning_root": request.planning_root,
        "worktree": request.worktree,
        "last_successful_boundary": request.last_successful_boundary,
        "active_blocked_boundary": request.active_blocked_boundary,
        "target_model": request.target_model,
        "evidence": evidence_json(&request.evidence),
    })
}

fn evidence_json(evidence: &ContinuationEvidence) -> Value {
    json!({
        "question": artifact_json(&evidence.question),
        "answer": artifact_json(&evidence.answer),
        "session_graph": artifact_json(&evidence.session_graph),
        "origin_trace": artifact_json(&evidence.origin_trace),
        "ticket_snapshot": artifact_json(&evidence.ticket_snapshot),
    })
}

fn artifact_json(artifact: &ArtifactIdentity) -> Value {
    json!({
        "path": artifact.path,
        "sha256": artifact.sha256,
    })
}

fn outcome_json(outcome: &InvocationOutcome) -> Value {
    json!({
        "invocation_id": outcome.invocation_id,
        "session_id": outcome.session_id,
        "physical_exit_code": outcome.physical_exit_code,
        "acceptance": acceptance_name(&outcome.acceptance),
        "disposition": disposition_json(&outcome.disposition),
    })
}

fn acceptance_name(acceptance: &ResumeAcceptance) -> &'static str {
    match acceptance {
        ResumeAcceptance::Accepted => "accepted",
        ResumeAcceptance::Rejected => "rejected",
        ResumeAcceptance::Unconfirmed => "unconfirmed",
        ResumeAcceptance::NotApplicable => "not_applicable",
    }
}

fn disposition_json(disposition: &InvocationDisposition) -> Value {
    match disposition {
        InvocationDisposition::Succeeded => json!({"status": "succeeded"}),
        InvocationDisposition::Failed {
            error_category,
            terminal_reason,
        } => json!({
            "status": "failed",
            "error_category": error_category,
            "terminal_reason": terminal_reason,
        }),
    }
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn conflict(message: &str) -> ContinuationBlock {
    ContinuationBlock {
        kind: ContinuationBlockKind::Conflict,
        message: message.to_string(),
    }
}

fn persistence(error: impl std::fmt::Display) -> ContinuationBlock {
    ContinuationBlock {
        kind: ContinuationBlockKind::Persistence,
        message: error.to_string(),
    }
}
