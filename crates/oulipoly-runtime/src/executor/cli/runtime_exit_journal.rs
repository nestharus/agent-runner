//! Original native executor operations in its existing producer-custody journal.
//! An intent is not an outcome. Results remain separate, including rejections;
//! missing results are pending operations and must still satisfy State legality.
use super::spawn_identity::GenerationOperationOutcome;
use super::spawn_identity::{GenerationOperationError, SpawnIdentityContext};
use oulipoly_provider::custody::durable;
use oulipoly_state::mailbox::{
    DrainFinishResult, GenerationMutation, GenerationRejection, RuntimeGenerationRow,
    RuntimeLifecycleState, RuntimeTerminalReason,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ExitOperation {
    FinalizeDrain,
    FinishDrain,
    NonOrderly,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ExitIntent {
    pub generation: String,
    pub invocation: String,
    pub operation: ExitOperation,
    pub reason: String,
    pub exit_code: Option<i32>,
    pub drain_request: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ExitObservation {
    pub intent: ExitIntent,
    pub before: Option<serde_json::Value>,
    pub result: Option<ExitResult>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ExitDisposition {
    HelperReturned,
    Applied,
    AlreadyApplied,
    Finished,
    AlreadyExited,
    Rejected,
    Failed,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ExitResult {
    pub disposition: ExitDisposition,
    pub rejected: bool,
    // Explicitly classified at the actual State response. Historical generic
    // invariant rejections remain unknown; never parse Debug strings as proof.
    #[serde(default)]
    pub custody_refused: bool,
    pub returned: String,
}

pub(super) struct PendingExit(Option<PathBuf>);
impl PendingExit {
    pub(super) fn begin(
        context: &SpawnIdentityContext,
        operation: ExitOperation,
        reason: RuntimeTerminalReason,
        exit_code: Option<i32>,
        drain_request: Option<String>,
    ) -> Result<Self, GenerationOperationError> {
        let Some(root) = &context.native_exit_journal else {
            return Ok(Self(None));
        };
        let path = root.join(uuid::Uuid::new_v4().to_string());
        let intent = ExitIntent {
            generation: context.generation_id.to_string(),
            invocation: context.invocation_uuid.clone(),
            operation,
            reason: serde_json::to_value(reason)
                .map_err(|_| GenerationOperationError::Unknown)?
                .as_str()
                .ok_or(GenerationOperationError::Unknown)?
                .to_string(),
            exit_code,
            drain_request,
        };
        // Before any potentially lossy SQL/helper step. Never report successful
        // finalization when this write returns an error.
        #[cfg(feature = "age360-fault-fixtures")]
        if intent.operation == ExitOperation::FinishDrain {
            oulipoly_state::completion_continuation::age360_fault_barrier(
                "native-finish-before-intent",
            );
        }
        retain_prerequisite(
            &path.join("intent.json"),
            &intent,
            "native-exit-intent-failed",
        );
        Ok(Self(Some(path)))
    }
    pub(super) fn observe_before(
        &self,
        context: &SpawnIdentityContext,
        db: &oulipoly_state::mailbox::MailboxDb,
    ) -> Result<(), GenerationOperationError> {
        if self.0.is_none() {
            return Ok(());
        }
        // The original caller still owns this pending transition. A returning
        // storage error does not hand that ownership to abnormal cleanup.
        let row = loop {
            match db
                .runtime_lifecycle_reader()
                .runtime_generation(&context.generation_id)
            {
                Ok(Some(row)) => break row,
                Ok(None) => return Err(GenerationOperationError::MissingGeneration),
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
            }
        };
        self.before(&row)
    }
    pub(super) fn before(
        &self,
        row: &RuntimeGenerationRow,
    ) -> Result<(), GenerationOperationError> {
        if let Some(path) = &self.0 {
            #[cfg(feature = "age360-fault-fixtures")]
            oulipoly_state::completion_continuation::age360_fault_barrier(
                "native-exit-before-predecessor-write",
            );
            retain_prerequisite(
                &path.join("before.json"),
                row,
                "native-exit-predecessor-failed",
            );
        }
        Ok(())
    }
    pub(super) fn finish<T: std::fmt::Debug>(
        self,
        result: Result<T, GenerationOperationError>,
    ) -> Result<T, GenerationOperationError> {
        let disposition = match &result {
            Ok(_) => ExitDisposition::HelperReturned,
            Err(GenerationOperationError::Rejected(_)) => ExitDisposition::Rejected,
            Err(_) => ExitDisposition::Failed,
        };
        self.retain_result(&result, disposition, false)?;
        result
    }
    pub(super) fn finish_drain(
        self,
        result: Result<DrainFinishResult, GenerationOperationError>,
    ) -> Result<(), GenerationOperationError> {
        let disposition = match &result {
            Ok(DrainFinishResult::Finished(_)) => ExitDisposition::Finished,
            Ok(DrainFinishResult::AlreadyExited(_)) => ExitDisposition::AlreadyExited,
            Ok(DrainFinishResult::NotDraining(_) | DrainFinishResult::Rejected(_)) => {
                ExitDisposition::Rejected
            }
            Err(_) => ExitDisposition::Failed,
        };
        let custody_refused = matches!(
            &result,
            Ok(DrainFinishResult::Rejected(
                GenerationRejection::CustodyNotQuiescent
            ))
        );
        self.retain_result(&result, disposition, custody_refused)?;
        match result? {
            DrainFinishResult::Finished(_) | DrainFinishResult::AlreadyExited(_) => Ok(()),
            DrainFinishResult::NotDraining(actual) => Err(GenerationOperationError::Rejected(
                GenerationRejection::IllegalPredecessor {
                    expected: RuntimeLifecycleState::Draining,
                    actual,
                },
            )),
            DrainFinishResult::Rejected(rejection) => {
                Err(GenerationOperationError::Rejected(rejection))
            }
        }
    }
    pub(super) fn finish_non_orderly(
        self,
        result: Result<GenerationMutation<RuntimeGenerationRow>, GenerationOperationError>,
    ) -> Result<GenerationOperationOutcome, GenerationOperationError> {
        let disposition = match &result {
            Ok(GenerationMutation::Applied(_)) => ExitDisposition::Applied,
            Ok(GenerationMutation::AlreadyApplied(_)) => ExitDisposition::AlreadyApplied,
            Ok(GenerationMutation::Rejected(_)) => ExitDisposition::Rejected,
            Err(_) => ExitDisposition::Failed,
        };
        let custody_refused = matches!(
            &result,
            Ok(GenerationMutation::Rejected(
                GenerationRejection::CustodyNotQuiescent
            ))
        );
        self.retain_result(&result, disposition, custody_refused)?;
        match result? {
            GenerationMutation::Applied(_) => Ok(GenerationOperationOutcome::Applied),
            GenerationMutation::AlreadyApplied(_) => Ok(GenerationOperationOutcome::AlreadyApplied),
            GenerationMutation::Rejected(reason) => Err(GenerationOperationError::Rejected(reason)),
        }
    }
    fn retain_result(
        &self,
        result: &impl std::fmt::Debug,
        disposition: ExitDisposition,
        custody_refused: bool,
    ) -> Result<(), GenerationOperationError> {
        if let Some(path) = &self.0 {
            let observed = ExitResult {
                rejected: disposition == ExitDisposition::Rejected,
                custody_refused,
                disposition,
                returned: format!("{result:?}"),
            };
            #[cfg(feature = "age360-fault-fixtures")]
            oulipoly_state::completion_continuation::age360_fault_barrier(
                "native-exit-result-retention",
            );
            durable::write_json(&path.join("result.json"), &observed).map_err(|_| {
                #[cfg(feature = "age360-fault-fixtures")]
                oulipoly_state::completion_continuation::age360_fault_barrier(
                    "native-exit-result-retention-failed",
                );
                GenerationOperationError::StorageFailure
            })?;
        }
        Ok(())
    }
}

/// Keep the exact original operation and observation in its living caller until
/// its existing journal can retain them. No retry creates a new observation or
/// reconstructs a lost classification. Killing this owner before retention can
/// still lose those facts; this is not a reboot/all-owner-loss guarantee.
fn retain_prerequisite(path: &Path, value: &impl Serialize, _failure_boundary: &str) {
    let mut write = PrerequisiteWrite::default();
    while write.retain(path, value).is_err() {
        #[cfg(feature = "age360-fault-fixtures")]
        oulipoly_state::completion_continuation::age360_fault_barrier(_failure_boundary);
        std::thread::sleep(std::time::Duration::from_millis(100));
        #[cfg(feature = "age360-fault-fixtures")]
        oulipoly_state::completion_continuation::age360_fault_barrier(
            "native-exit-prerequisite-retry",
        );
    }
}

// One privately created scratch inode belongs to this living write, not to
// every retry. Never scan/delete historical or other writers' files.
#[derive(Default)]
struct PrerequisiteWrite {
    scratch: Option<tempfile::NamedTempFile>,
    published: bool,
}
impl PrerequisiteWrite {
    fn retain(&mut self, path: &Path, value: &impl Serialize) -> Result<(), String> {
        if !self.published {
            self.publish(path, value)?;
        }
        // A returning directory sync error is not absence of publication.
        // Keep syncing it, without creating/replacing another scratch file.
        let parent = path.parent().ok_or("journal parent absent")?;
        std::fs::File::open(parent)
            .and_then(|file| file.sync_all())
            .map_err(|e| e.to_string())
    }

    fn publish(&mut self, path: &Path, value: &impl Serialize) -> Result<(), String> {
        if self.scratch.is_none() {
            self.scratch = Some(prepare_prerequisite(path, value)?);
        }
        match self
            .scratch
            .take()
            .expect("prepared private write")
            .persist(path)
        {
            Ok(_) => self.published = true,
            Err(error) => {
                let message = error.error.to_string();
                self.scratch = Some(error.file);
                return Err(message);
            }
        }
        Ok(())
    }
}

fn prepare_prerequisite(
    path: &Path,
    value: &impl Serialize,
) -> Result<tempfile::NamedTempFile, String> {
    use std::io::Write;
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    let parent = path.parent().ok_or("journal parent absent")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut file = tempfile::Builder::new()
        .prefix(&format!(
            "{}.tmp-",
            path.file_stem().unwrap_or_default().to_string_lossy()
        ))
        .tempfile_in(parent)
        .map_err(|e| e.to_string())?;
    // Failed preparation drops only this create-new private scratch. Once
    // prepared, returning rename failures retain/reuse the same file and bytes.
    file.write_all(&bytes)
        .and_then(|()| file.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    Ok(file)
}

pub(crate) fn read(
    root: &Path,
    generation: &str,
    invocation: &str,
) -> Result<Vec<ExitObservation>, String> {
    if !root.try_exists().map_err(|e| e.to_string())? {
        return Ok(vec![]);
    }
    let mut paths = std::fs::read_dir(root)
        .map_err(|e| e.to_string())?
        .map(|e| e.map(|e| e.path()).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort(); // stable transport only; order never selects an outcome
    let mut observations = Vec::new();
    for path in paths {
        let intent_path = path.join("intent.json");
        // A failed intent write can leave only a temporary file. No operation
        // followed that write, and its unretained facts cannot be reconstructed.
        if !intent_path.try_exists().map_err(|e| e.to_string())? {
            continue;
        }
        let intent: ExitIntent = durable::read_json(&intent_path)?;
        if intent.generation != generation || intent.invocation != invocation {
            return Err("native_exit_journal_identity_conflict".into());
        }
        observations.push(ExitObservation {
            intent,
            before: optional(&path.join("before.json"))?,
            result: optional(&path.join("result.json"))?,
        });
    }
    Ok(observations)
}
fn optional<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    if path.try_exists().map_err(|e| e.to_string())? {
        durable::read_json(path).map(Some)
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn returned_retention_failure_is_not_success_and_does_not_erase_intent() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("operation");
        let intent = ExitIntent {
            generation: "g".into(),
            invocation: "i".into(),
            operation: ExitOperation::NonOrderly,
            reason: "abnormal_termination".into(),
            exit_code: None,
            drain_request: None,
        };
        durable::write_json(&path.join("intent.json"), &intent).unwrap();
        std::fs::create_dir(path.join("result.json")).unwrap();
        assert_eq!(
            PendingExit(Some(path.clone())).finish(Ok(())),
            Err(GenerationOperationError::StorageFailure)
        );
        assert!(read(root.path(), "g", "i").is_err()); // unreadable result isn't absence
        std::fs::remove_dir(path.join("result.json")).unwrap();
        let retained = read(root.path(), "g", "i").unwrap();
        assert_eq!(retained.len(), 1);
        assert!(retained[0].result.is_none());
        assert!(read(root.path(), "other-generation", "i").is_err());
    }
}
