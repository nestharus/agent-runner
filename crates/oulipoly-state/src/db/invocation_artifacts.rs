use super::{InvocationStart, StateDb, lc_log_adapter};
use crate::result_envelope::{ResultEnvelopeInput, result_envelope_payload};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy)]
enum RawArtifactKind {
    Stdout,
    Stderr,
    Result,
    EventsJsonl,
}

impl StateDb {
    pub(super) fn invocations_dir(&self) -> Option<PathBuf> {
        if self.db_path == Path::new(":memory:") {
            return None;
        }
        let parent = self
            .db_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Some(parent.join("invocations"))
    }

    pub(super) fn write_invocation_artifact(
        &self,
        start: &InvocationStart,
        started_at: &str,
    ) -> Result<(), String> {
        let Some(dir) = self.invocations_dir() else {
            return Ok(());
        };
        Self::ensure_artifact_dir(&dir)?;
        let bytes = Self::invocation_artifact_bytes(start, started_at)?;
        let (tmp_path, final_path) =
            Self::artifact_paths(&dir, &start.invocation_uuid, "invocation");
        Self::write_artifact_atomically(&tmp_path, &final_path, &bytes)
    }

    fn ensure_artifact_dir(dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("create_dir_all({}): {e}", dir.display()))
    }

    fn invocation_artifact_bytes(
        start: &InvocationStart,
        started_at: &str,
    ) -> Result<Vec<u8>, String> {
        let payload = Self::invocation_artifact_payload(start, started_at);
        serde_json::to_vec(&payload).map_err(|e| format!("serialize invocation artifact: {e}"))
    }

    fn invocation_artifact_payload(start: &InvocationStart, started_at: &str) -> serde_json::Value {
        serde_json::json!({
            "id": start.invocation_uuid,
            "status": "running",
            "pid": std::process::id(),
            "started_at": started_at,
            "model_name": start.model_name,
            "provider_name": start.provider_name,
        })
    }

    fn artifact_paths(dir: &Path, uuid: &str, extension: &str) -> (PathBuf, PathBuf) {
        (
            dir.join(format!("{uuid}.{extension}.tmp")),
            dir.join(format!("{uuid}.{extension}")),
        )
    }

    fn write_artifact_atomically(
        tmp_path: &Path,
        final_path: &Path,
        bytes: &[u8],
    ) -> Result<(), String> {
        std::fs::write(tmp_path, bytes)
            .map_err(|e| format!("write({}): {e}", tmp_path.display()))?;
        std::fs::rename(tmp_path, final_path).map_err(|e| {
            format!(
                "rename({} -> {}): {e}",
                tmp_path.display(),
                final_path.display()
            )
        })
    }

    pub(super) fn write_result_artifact(
        &self,
        input: ResultEnvelopeInput<'_>,
    ) -> Result<(), String> {
        let Some(dir) = self.invocations_dir() else {
            return Ok(());
        };
        Self::ensure_artifact_dir(&dir)?;
        let bytes = Self::result_artifact_bytes(input)?;
        let (tmp_path, final_path) = Self::artifact_paths(&dir, input.id, "result");
        Self::write_artifact_atomically(&tmp_path, &final_path, &bytes)
    }

    fn result_artifact_bytes(input: ResultEnvelopeInput<'_>) -> Result<Vec<u8>, String> {
        let payload = Self::result_artifact_payload(input);
        serde_json::to_vec(&payload).map_err(|e| format!("serialize result artifact: {e}"))
    }

    fn result_artifact_payload(input: ResultEnvelopeInput<'_>) -> serde_json::Value {
        result_envelope_payload(input)
    }

    pub(super) fn raw_paths_for(
        &self,
        invocation_uuid: &str,
    ) -> Option<lc_log_adapter::RawArtifactPaths> {
        let state_dir = Self::state_dir_for(&self.db_path)?;
        Some(Self::raw_paths_map_for(state_dir, invocation_uuid))
    }

    fn is_memory_db_path(path: &Path) -> bool {
        path == Path::new(":memory:")
    }

    fn state_dir_for(db_path: &Path) -> Option<&Path> {
        (!Self::is_memory_db_path(db_path))
            .then(|| db_path.parent())
            .flatten()
    }

    fn raw_paths_map_for(state_dir: &Path, uuid: &str) -> lc_log_adapter::RawArtifactPaths {
        let raw_io_dir = state_dir.join("invocations").join("raw-io");
        lc_log_adapter::RawArtifactPaths {
            stdout_path: raw_io_dir.join(Self::format_raw_artifact_filename(
                uuid,
                RawArtifactKind::Stdout,
            )),
            stderr_path: raw_io_dir.join(Self::format_raw_artifact_filename(
                uuid,
                RawArtifactKind::Stderr,
            )),
            result_path: raw_io_dir.join(Self::format_raw_artifact_filename(
                uuid,
                RawArtifactKind::Result,
            )),
            events_jsonl_path: raw_io_dir.join(Self::format_raw_artifact_filename(
                uuid,
                RawArtifactKind::EventsJsonl,
            )),
        }
    }

    fn format_raw_artifact_filename(uuid: &str, kind: RawArtifactKind) -> String {
        let suffix = match kind {
            RawArtifactKind::Stdout => "stdout",
            RawArtifactKind::Stderr => "stderr",
            RawArtifactKind::Result => "result",
            RawArtifactKind::EventsJsonl => "events.jsonl",
        };
        format!("{uuid}.{suffix}")
    }
}
