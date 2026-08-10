use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use oulipoly_runtime::fresh_continuation::{
    ContinuationBlock, ContinuationBlockKind, ContinuationHandoff, HandoffPublisher,
    InvocationDisposition, InvocationOutcome, PublishedHandoff, ResumeAcceptance,
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
        validate_output_dir(&self.planning_root, &output_dir)?;
        let target = output_dir.join(format!("{}.json", handoff.continuation_id));
        let bytes = handoff_bytes(&handoff)?;
        let sha256 = sha256(&bytes);

        if target.exists() {
            return reconcile_existing(&target, &bytes, sha256);
        }

        let temp = output_dir.join(format!(
            ".{}.{}.tmp",
            handoff.continuation_id,
            Uuid::new_v4()
        ));
        write_durable_temp(&temp, &bytes)?;
        let publish_result = match fs::hard_link(&temp, &target) {
            Ok(()) => Ok(PublishedHandoff {
                path: target,
                sha256,
            }),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                reconcile_existing(&target, &bytes, sha256)
            }
            Err(error) => Err(persistence(error)),
        };
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

fn validate_output_dir(planning_root: &Path, output_dir: &Path) -> Result<(), ContinuationBlock> {
    let canonical_root = fs::canonicalize(planning_root).map_err(persistence)?;
    let canonical_output = fs::canonicalize(output_dir).map_err(persistence)?;
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

fn reconcile_existing(
    path: &Path,
    expected: &[u8],
    sha256: String,
) -> Result<PublishedHandoff, ContinuationBlock> {
    if fs::symlink_metadata(path)
        .map_err(persistence)?
        .file_type()
        .is_symlink()
    {
        return Err(conflict("Existing continuation handoff is a symlink"));
    }
    let existing = fs::read(path).map_err(persistence)?;
    if existing != expected {
        return Err(conflict(
            "Existing continuation handoff differs from the requested publication",
        ));
    }
    Ok(PublishedHandoff {
        path: path.to_path_buf(),
        sha256,
    })
}

fn handoff_bytes(handoff: &ContinuationHandoff) -> Result<Vec<u8>, ContinuationBlock> {
    let mut bytes = serde_json::to_vec_pretty(&json!({
        "schema_version": 1,
        "kind": "fresh_continuation_handoff",
        "continuation_id": handoff.continuation_id,
        "resume": outcome_json(&handoff.resume),
        "fresh": handoff.fresh.as_ref().map(outcome_json),
    }))
    .map_err(persistence)?;
    bytes.push(b'\n');
    Ok(bytes)
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
