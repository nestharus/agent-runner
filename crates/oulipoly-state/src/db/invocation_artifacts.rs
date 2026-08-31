//! ## Declared roles
//!
//! - accessor
//! - predicate
//! - orchestration
//! - mapper
//! - formatter
//!
//! Role set: { accessor, predicate, orchestration, mapper, formatter }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/invocation_artifacts.rs
//!     role: intrinsic-surface
//!     Domain: invocation-artifact-persistence
//!     Owns:
//!       - StateDb invocation-artifact path construction and JSON payload read/write
//!       - InvocationStart inputs consumed when persisting artifacts
//!       - lc_log_adapter lifecycle emission for artifact operations
//!       - external contract symbols referenced by this concern via its `use`
//!         declarations, intrinsic and subordinate to this persistence domain: InvocationStart, Path, PathBuf, ResultEnvelopeInput, StateDb, lc_log_adapter, result_envelope_payload
//! ```
//!
//! Invocation sidecar artifact pathing, payload mapping, and atomic file writes.

use super::{InvocationStart, StateDb, lc_log_adapter};
use crate::result_envelope::{ResultEnvelopeInput, result_envelope_payload};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvocationOutputArtifactPaths {
    pub stdout: PathBuf,
    pub stderr: PathBuf,
}

#[derive(Debug, Clone, Copy)]
enum RawArtifactKind {
    Stdout,
    Stderr,
    Result,
    EventsJsonl,
}

impl StateDb {
    pub(super) fn invocations_dir(&self) -> Option<PathBuf> {
        if self.is_memory_db() {
            return None;
        }
        Some(self.invocations_dir_path())
    }

    fn is_memory_db(&self) -> bool {
        self.db_path == Path::new(":memory:")
    }

    fn invocations_dir_path(&self) -> PathBuf {
        self.db_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("invocations")
    }

    pub fn invocation_output_artifact_paths(
        &self,
        invocation_uuid: &str,
    ) -> Result<Option<InvocationOutputArtifactPaths>, String> {
        if invocation_uuid.is_empty()
            || invocation_uuid
                .bytes()
                .any(|byte| matches!(byte, b'/' | b'\\' | 0))
        {
            return Err("invalid invocation UUID for output artifact path".to_string());
        }
        let Some(invocations_dir) = self.invocations_dir() else {
            return Ok(None);
        };
        let dir = invocations_dir.join("output");
        Self::ensure_artifact_dir(&dir)?;
        Ok(Some(InvocationOutputArtifactPaths {
            stdout: dir.join(format!("{invocation_uuid}.stdout")),
            stderr: dir.join(format!("{invocation_uuid}.stderr")),
        }))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_invocation_output_pending(
        &self,
        invocation_id: i64,
        invocation_uuid: &str,
        paths: &InvocationOutputArtifactPaths,
        stdout_bytes: u64,
        stdout_sha256: &str,
        stderr_bytes: u64,
        stderr_sha256: &str,
        data_event_count: u64,
    ) -> Result<(), String> {
        let now = Self::current_rfc3339_timestamp();
        self.conn
            .execute(
                "INSERT INTO invocation_output_deliveries (
                    invocation_id, invocation_uuid, provider_outcome_state, delivery_state,
                    stdout_path, stdout_bytes, stdout_sha256,
                    stderr_path, stderr_bytes, stderr_sha256, data_event_count,
                    created_at, updated_at
                 ) VALUES (?1, ?2, 'pending', 'pending', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)
                 ON CONFLICT(invocation_id) DO UPDATE SET
                    invocation_uuid = excluded.invocation_uuid,
                    stdout_path = excluded.stdout_path,
                    stdout_bytes = excluded.stdout_bytes,
                    stdout_sha256 = excluded.stdout_sha256,
                    stderr_path = excluded.stderr_path,
                    stderr_bytes = excluded.stderr_bytes,
                    stderr_sha256 = excluded.stderr_sha256,
                    data_event_count = excluded.data_event_count,
                    updated_at = excluded.updated_at",
                rusqlite::params![
                    invocation_id,
                    invocation_uuid,
                    paths.stdout.to_string_lossy(),
                    i64::try_from(stdout_bytes)
                        .map_err(|_| "stdout byte count exceeds SQLite INTEGER".to_string())?,
                    stdout_sha256,
                    paths.stderr.to_string_lossy(),
                    i64::try_from(stderr_bytes)
                        .map_err(|_| "stderr byte count exceeds SQLite INTEGER".to_string())?,
                    stderr_sha256,
                    i64::try_from(data_event_count)
                        .map_err(|_| "output event count exceeds SQLite INTEGER".to_string())?,
                    now,
                ],
            )
            .map_err(|error| format!("Failed to record pending invocation output: {error}"))?;
        Ok(())
    }

    pub fn mark_invocation_output_delivered(&self, invocation_id: i64) -> Result<(), String> {
        let now = Self::current_rfc3339_timestamp();
        let changed = self
            .conn
            .execute(
                "UPDATE invocation_output_deliveries
                 SET delivery_state = 'delivered', delivered_at = ?2, updated_at = ?2,
                     delivery_failure_stage = NULL, delivery_failure_kind = NULL,
                     delivery_failure_bytes = NULL
                 WHERE invocation_id = ?1 AND provider_outcome_state = 'settled'",
                rusqlite::params![invocation_id, now],
            )
            .map_err(|error| format!("Failed to mark invocation output delivered: {error}"))?;
        if changed != 1 {
            return Err(
                "invocation output is missing or provider outcome is unsettled".to_string(),
            );
        }
        Ok(())
    }

    pub fn mark_invocation_output_delivery_failed(
        &self,
        invocation_id: i64,
        stage: &str,
        kind: &str,
        delivered_bytes: Option<u64>,
    ) -> Result<(), String> {
        let now = Self::current_rfc3339_timestamp();
        let delivered_bytes = delivered_bytes
            .map(i64::try_from)
            .transpose()
            .map_err(|_| "delivered byte count exceeds SQLite INTEGER".to_string())?;
        let changed = self
            .conn
            .execute(
                "UPDATE invocation_output_deliveries
                 SET delivery_state = 'failed', delivery_failure_stage = ?2,
                     delivery_failure_kind = ?3, delivery_failure_bytes = ?4,
                     updated_at = ?5
                 WHERE invocation_id = ?1 AND provider_outcome_state = 'settled'",
                rusqlite::params![invocation_id, stage, kind, delivered_bytes, now],
            )
            .map_err(|error| {
                format!("Failed to mark invocation output delivery failed: {error}")
            })?;
        if changed != 1 {
            return Err(
                "invocation output is missing or provider outcome is unsettled".to_string(),
            );
        }
        Ok(())
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
        std::fs::create_dir_all(dir).map_err(|e| Self::format_create_artifact_dir_error(dir, e))
    }

    fn invocation_artifact_bytes(
        start: &InvocationStart,
        started_at: &str,
    ) -> Result<Vec<u8>, String> {
        let payload = Self::invocation_artifact_payload(start, started_at);
        serde_json::to_vec(&payload).map_err(Self::format_invocation_artifact_serialize_error)
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
            .map_err(|e| Self::format_artifact_write_error(tmp_path, e))?;
        std::fs::rename(tmp_path, final_path)
            .map_err(|e| Self::format_artifact_rename_error(tmp_path, final_path, e))
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
        serde_json::to_vec(&payload).map_err(Self::format_result_artifact_serialize_error)
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
        format!("{uuid}.{}", Self::raw_artifact_suffix(kind))
    }

    fn raw_artifact_suffix(kind: RawArtifactKind) -> &'static str {
        match kind {
            RawArtifactKind::Stdout => "stdout",
            RawArtifactKind::Stderr => "stderr",
            RawArtifactKind::Result => "result",
            RawArtifactKind::EventsJsonl => "events.jsonl",
        }
    }

    fn format_create_artifact_dir_error(dir: &Path, e: std::io::Error) -> String {
        format!("create_dir_all({}): {e}", dir.display())
    }

    fn format_invocation_artifact_serialize_error(e: serde_json::Error) -> String {
        format!("serialize invocation artifact: {e}")
    }

    fn format_artifact_write_error(tmp_path: &Path, e: std::io::Error) -> String {
        format!("write({}): {e}", tmp_path.display())
    }

    fn format_artifact_rename_error(
        tmp_path: &Path,
        final_path: &Path,
        e: std::io::Error,
    ) -> String {
        format!(
            "rename({} -> {}): {e}",
            tmp_path.display(),
            final_path.display()
        )
    }

    fn format_result_artifact_serialize_error(e: serde_json::Error) -> String {
        format!("serialize result artifact: {e}")
    }
}
