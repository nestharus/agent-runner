//! Disk-backed custody for authoritative external-provider launch output.

use oulipoly_provider::generated::{
    LAUNCH_OUTPUT_COMPLETE_MARKER_V1, LAUNCH_OUTPUT_V1, LaunchOutputCompleteMarkerValueV1,
};
use oulipoly_provider::stream::DecodedLaunchEvent;
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct ExecutionOutputSpool {
    inner: Arc<Mutex<ExecutionOutputSpoolState>>,
}

struct ExecutionOutputSpoolState {
    stdout: SpooledStream,
    stderr: SpooledStream,
    data_event_count: u64,
    summary: Option<ExecutionOutputSummary>,
    exit_observed: bool,
}

struct SpooledStream {
    file: File,
    persisted_path: Option<PathBuf>,
    len: u64,
    digest: Sha256,
    last_byte: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionOutputSummary {
    pub stdout_bytes: u64,
    pub stdout_sha256: String,
    pub stderr_bytes: u64,
    pub stderr_sha256: String,
    pub data_event_count: u64,
}

impl ExecutionOutputSpool {
    pub(crate) fn new() -> std::io::Result<Self> {
        Ok(Self {
            inner: Arc::new(Mutex::new(ExecutionOutputSpoolState {
                stdout: SpooledStream::new()?,
                stderr: SpooledStream::new()?,
                data_event_count: 0,
                summary: None,
                exit_observed: false,
            })),
        })
    }

    pub(crate) fn observe(&self, event: &DecodedLaunchEvent) -> Result<(), String> {
        let mut state = self
            .inner
            .lock()
            .map_err(|_| "launch output spool lock was poisoned".to_string())?;
        state.observe(event)
    }

    pub fn write_stdout_to(&self, writer: &mut dyn Write) -> std::io::Result<()> {
        self.with_sealed_state(|state| state.stdout.copy_to(writer))
    }

    pub fn write_stderr_to(&self, writer: &mut dyn Write) -> std::io::Result<()> {
        self.with_sealed_state(|state| state.stderr.copy_to(writer))
    }

    pub fn stdout_bytes(&self) -> std::io::Result<Vec<u8>> {
        self.with_sealed_state(|state| state.stdout.read_all())
    }

    pub fn stderr_bytes(&self) -> std::io::Result<Vec<u8>> {
        self.with_sealed_state(|state| state.stderr.read_all())
    }

    pub fn summary(&self) -> std::io::Result<ExecutionOutputSummary> {
        let state = self
            .inner
            .lock()
            .map_err(|_| std::io::Error::other("launch output spool lock was poisoned"))?;
        state
            .summary
            .clone()
            .ok_or_else(|| std::io::Error::other("launch output spool is not sealed"))
    }

    pub fn persist_for_invocation(
        &self,
        state: &oulipoly_state::StateDb,
        invocation_id: i64,
        invocation_uuid: &str,
    ) -> Result<(), String> {
        let Some(paths) = state.invocation_output_artifact_paths(invocation_uuid)? else {
            return Ok(());
        };
        let summary = {
            let mut spool = self
                .inner
                .lock()
                .map_err(|_| "launch output spool lock was poisoned".to_string())?;
            if spool.summary.is_none() || !spool.exit_observed {
                return Err("launch output spool is not sealed".to_string());
            }
            spool
                .stdout
                .persist_to(&paths.stdout)
                .map_err(|error| format!("persist stdout output artifact: {error}"))?;
            spool
                .stderr
                .persist_to(&paths.stderr)
                .map_err(|error| format!("persist stderr output artifact: {error}"))?;
            spool
                .summary
                .clone()
                .expect("sealed output spool has a summary")
        };
        state.record_invocation_output_pending(
            invocation_id,
            invocation_uuid,
            &paths,
            summary.stdout_bytes,
            &summary.stdout_sha256,
            summary.stderr_bytes,
            &summary.stderr_sha256,
            summary.data_event_count,
        )
    }

    pub fn persist_artifact(
        &self,
        state: &oulipoly_state::StateDb,
        artifact_token: &str,
    ) -> Result<bool, String> {
        let Some(paths) = state.invocation_output_artifact_paths(artifact_token)? else {
            return Ok(false);
        };
        let mut spool = self
            .inner
            .lock()
            .map_err(|_| "launch output spool lock was poisoned".to_string())?;
        if spool.summary.is_none() || !spool.exit_observed {
            return Err("launch output spool is not sealed".to_string());
        }
        spool
            .stdout
            .persist_to(&paths.stdout)
            .map_err(|error| format!("persist stdout output artifact: {error}"))?;
        spool
            .stderr
            .persist_to(&paths.stderr)
            .map_err(|error| format!("persist stderr output artifact: {error}"))?;
        Ok(true)
    }

    pub fn stdout_ends_with_newline(&self) -> bool {
        self.inner
            .lock()
            .ok()
            .is_some_and(|state| state.summary.is_some() && state.stdout.last_byte == Some(b'\n'))
    }

    fn with_sealed_state<T>(
        &self,
        operation: impl FnOnce(&mut ExecutionOutputSpoolState) -> std::io::Result<T>,
    ) -> std::io::Result<T> {
        let mut state = self
            .inner
            .lock()
            .map_err(|_| std::io::Error::other("launch output spool lock was poisoned"))?;
        if state.summary.is_none() || !state.exit_observed {
            return Err(std::io::Error::other("launch output spool is not sealed"));
        }
        operation(&mut state)
    }
}

impl std::fmt::Debug for ExecutionOutputSpool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let summary = self
            .inner
            .lock()
            .ok()
            .and_then(|state| state.summary.clone());
        formatter
            .debug_struct("ExecutionOutputSpool")
            .field("summary", &summary)
            .finish()
    }
}

impl PartialEq for ExecutionOutputSpool {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl ExecutionOutputSpoolState {
    fn observe(&mut self, event: &DecodedLaunchEvent) -> Result<(), String> {
        if self.exit_observed {
            return Err("launch event observed after final exit".to_string());
        }
        match event {
            DecodedLaunchEvent::Stdout { data, .. } => self.append_stdout(data),
            DecodedLaunchEvent::Stderr { data, .. } => self.append_stderr(data),
            DecodedLaunchEvent::Marker { name, value, .. }
                if name == LAUNCH_OUTPUT_COMPLETE_MARKER_V1 =>
            {
                self.seal(value)
            }
            DecodedLaunchEvent::Exit(_) => self.observe_exit(),
            _ if self.summary.is_some() => {
                Err("launch event observed after output completion marker".to_string())
            }
            _ => Ok(()),
        }
    }

    fn append_stdout(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.ensure_open()?;
        self.stdout
            .append(bytes)
            .map_err(|error| format!("launch output spool write failed for stdout: {error}"))?;
        self.increment_event_count()
    }

    fn append_stderr(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.ensure_open()?;
        self.stderr
            .append(bytes)
            .map_err(|error| format!("launch output spool write failed for stderr: {error}"))?;
        self.increment_event_count()
    }

    fn ensure_open(&self) -> Result<(), String> {
        if self.summary.is_some() {
            return Err("launch output data observed after completion marker".to_string());
        }
        Ok(())
    }

    fn increment_event_count(&mut self) -> Result<(), String> {
        self.data_event_count = self
            .data_event_count
            .checked_add(1)
            .ok_or_else(|| "launch output data event count overflow".to_string())?;
        Ok(())
    }

    fn seal(&mut self, value: &serde_json::Value) -> Result<(), String> {
        self.ensure_open()?;
        let manifest: LaunchOutputCompleteMarkerValueV1 = serde_json::from_value(value.clone())
            .map_err(|error| format!("invalid launch output completion marker: {error}"))?;
        if manifest.protocol != LAUNCH_OUTPUT_V1 {
            return Err("launch output completion protocol mismatch".to_string());
        }
        let summary = self.current_summary();
        if manifest.stdout.bytes != summary.stdout_bytes
            || manifest.stdout.sha256 != summary.stdout_sha256
            || manifest.stderr.bytes != summary.stderr_bytes
            || manifest.stderr.sha256 != summary.stderr_sha256
            || manifest.data_event_count != summary.data_event_count
        {
            return Err("launch output completion marker did not match spooled output".to_string());
        }
        self.stdout
            .seal()
            .map_err(|error| format!("launch output spool seal failed for stdout: {error}"))?;
        self.stderr
            .seal()
            .map_err(|error| format!("launch output spool seal failed for stderr: {error}"))?;
        self.summary = Some(summary);
        Ok(())
    }

    fn observe_exit(&mut self) -> Result<(), String> {
        if self.summary.is_none() {
            return Err("launch output completion marker missing before final exit".to_string());
        }
        self.exit_observed = true;
        Ok(())
    }

    fn current_summary(&self) -> ExecutionOutputSummary {
        ExecutionOutputSummary {
            stdout_bytes: self.stdout.len,
            stdout_sha256: self.stdout.digest_hex(),
            stderr_bytes: self.stderr.len,
            stderr_sha256: self.stderr.digest_hex(),
            data_event_count: self.data_event_count,
        }
    }
}

impl SpooledStream {
    fn new() -> std::io::Result<Self> {
        Ok(Self {
            file: tempfile::tempfile()?,
            persisted_path: None,
            len: 0,
            digest: Sha256::new(),
            last_byte: None,
        })
    }

    fn append(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        let next_len = self
            .len
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| std::io::Error::other("launch output length overflow"))?;
        self.file.write_all(bytes)?;
        self.digest.update(bytes);
        self.last_byte = bytes.last().copied().or(self.last_byte);
        self.len = next_len;
        Ok(())
    }

    fn seal(&mut self) -> std::io::Result<()> {
        self.file.flush()?;
        self.file.sync_data()
    }

    fn copy_to(&mut self, writer: &mut dyn Write) -> std::io::Result<()> {
        self.file.seek(SeekFrom::Start(0))?;
        let result = std::io::copy(&mut (&mut self.file).take(self.len), writer).map(|_| ());
        let restore = self.file.seek(SeekFrom::End(0)).map(|_| ());
        result.and(restore)
    }

    fn persist_to(&mut self, final_path: &Path) -> std::io::Result<()> {
        if self.persisted_path.as_deref() == Some(final_path) {
            return Ok(());
        }
        let tmp_path = final_path.with_extension(format!(
            "{}.tmp",
            final_path
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or("output")
        ));
        let _ = std::fs::remove_file(&tmp_path);
        self.file.seek(SeekFrom::Start(0))?;
        let mut destination = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&tmp_path)?;
        let copy_result = std::io::copy(&mut (&mut self.file).take(self.len), &mut destination);
        if let Err(error) = copy_result {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(error);
        }
        destination.flush()?;
        destination.sync_all()?;
        drop(destination);
        publish_output_file(&tmp_path, final_path)?;
        self.file = OpenOptions::new().read(true).write(true).open(final_path)?;
        self.file.seek(SeekFrom::End(0))?;
        self.persisted_path = Some(final_path.to_path_buf());
        Ok(())
    }

    fn read_all(&mut self) -> std::io::Result<Vec<u8>> {
        let capacity = usize::try_from(self.len)
            .map_err(|_| std::io::Error::other("launch output is too large to materialize"))?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(capacity).map_err(|_| {
            std::io::Error::other("failed to allocate complete launch output buffer")
        })?;
        self.file.seek(SeekFrom::Start(0))?;
        let result = (&mut self.file)
            .take(self.len)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let restore = self.file.seek(SeekFrom::End(0)).map(|_| ());
        result.and_then(|bytes| restore.map(|_| bytes))
    }

    fn digest_hex(&self) -> String {
        self.digest
            .clone()
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

#[cfg(not(windows))]
fn publish_output_file(tmp_path: &Path, final_path: &Path) -> std::io::Result<()> {
    std::fs::rename(tmp_path, final_path)?;
    if let Some(parent) = final_path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(windows)]
fn publish_output_file(tmp_path: &Path, final_path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};

    let source = tmp_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = final_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_provider::generated::LaunchOutputChannelSummaryV1;
    use oulipoly_provider::generated::{ProcessStatus, TerminalSignal, TerminalSignalKind};
    use oulipoly_provider::stream::LaunchExit;

    #[test]
    fn verifies_manifest_and_seals_complete_binary_streams() {
        let spool = ExecutionOutputSpool::new().expect("spool");
        spool
            .observe(&DecodedLaunchEvent::Stdout {
                seq: 1,
                data: vec![0, 1, 255],
            })
            .expect("stdout");
        spool
            .observe(&DecodedLaunchEvent::Stderr {
                seq: 2,
                data: b"err".to_vec(),
            })
            .expect("stderr");
        let expected = ExecutionOutputSummary {
            stdout_bytes: 3,
            stdout_sha256: digest_hex(&[0, 1, 255]),
            stderr_bytes: 3,
            stderr_sha256: digest_hex(b"err"),
            data_event_count: 2,
        };
        spool
            .observe(&completion_event(3, &expected))
            .expect("completion");
        spool.observe(&exit_event(4)).expect("exit");

        assert_eq!(spool.summary().expect("summary"), expected);
        assert_eq!(spool.stdout_bytes().expect("read stdout"), [0, 1, 255]);
        assert_eq!(spool.stderr_bytes().expect("read stderr"), b"err");
    }

    #[test]
    fn rejects_exit_without_completion_marker() {
        let spool = ExecutionOutputSpool::new().expect("spool");
        let error = spool.observe(&exit_event(1)).expect_err("missing marker");
        assert!(error.contains("completion marker missing"));
    }

    #[test]
    fn rejects_manifest_that_does_not_match_spooled_bytes() {
        let spool = ExecutionOutputSpool::new().expect("spool");
        spool
            .observe(&DecodedLaunchEvent::Stdout {
                seq: 1,
                data: b"actual".to_vec(),
            })
            .expect("stdout");
        let wrong = ExecutionOutputSummary {
            stdout_bytes: 5,
            stdout_sha256: digest_hex(b"wrong"),
            stderr_bytes: 0,
            stderr_sha256: digest_hex(b""),
            data_event_count: 1,
        };
        let error = spool
            .observe(&completion_event(2, &wrong))
            .expect_err("mismatch");
        assert!(error.contains("did not match"));
    }

    #[test]
    fn persists_invocation_owned_output_before_outcome_and_delivery_settlement() {
        let dir = tempfile::tempdir().expect("temp dir");
        let state = oulipoly_state::StateDb::open(&dir.path().join("state.db")).expect("state");
        let invocation_uuid = "30900000-0000-4000-8000-000000000001";
        let invocation_id = state
            .start_invocation(&oulipoly_state::InvocationStart {
                invocation_uuid: invocation_uuid.to_string(),
                model_name: "model".to_string(),
                provider_name: "provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .expect("start invocation");
        let spool = ExecutionOutputSpool::new().expect("spool");
        spool
            .observe(&DecodedLaunchEvent::Stdout {
                seq: 1,
                data: vec![0, 1, 255],
            })
            .expect("stdout");
        let summary = ExecutionOutputSummary {
            stdout_bytes: 3,
            stdout_sha256: digest_hex(&[0, 1, 255]),
            stderr_bytes: 0,
            stderr_sha256: digest_hex(b""),
            data_event_count: 1,
        };
        spool
            .observe(&completion_event(2, &summary))
            .expect("completion");
        spool.observe(&exit_event(3)).expect("exit");

        spool
            .persist_for_invocation(&state, invocation_id, invocation_uuid)
            .expect("persist output");
        let paths = state
            .invocation_output_artifact_paths(invocation_uuid)
            .expect("paths")
            .expect("disk-backed paths");
        assert_eq!(
            std::fs::read(&paths.stdout).expect("stdout artifact"),
            [0, 1, 255]
        );
        assert_eq!(
            output_states(&state, invocation_id),
            ("pending".to_string(), "pending".to_string())
        );

        state
            .finalize_invocation(invocation_id, true, 0, None, None)
            .expect("settle provider outcome");
        assert_eq!(
            output_states(&state, invocation_id),
            ("settled".to_string(), "pending".to_string())
        );
        state
            .mark_invocation_output_delivery_failed(
                invocation_id,
                "stdout_flush",
                "broken_pipe",
                Some(1),
            )
            .expect("record caller delivery failure");
        assert_eq!(
            output_states(&state, invocation_id),
            ("settled".to_string(), "failed".to_string())
        );
        assert_eq!(
            std::fs::read(&paths.stdout).expect("artifact survives delivery failure"),
            [0, 1, 255]
        );
        state
            .mark_invocation_output_delivered(invocation_id)
            .expect("settle delivery");
        assert_eq!(
            output_states(&state, invocation_id),
            ("settled".to_string(), "delivered".to_string())
        );
    }

    fn output_states(state: &oulipoly_state::StateDb, invocation_id: i64) -> (String, String) {
        state
            .connection()
            .query_row(
                "SELECT provider_outcome_state, delivery_state
                 FROM invocation_output_deliveries WHERE invocation_id = ?1",
                rusqlite::params![invocation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("output states")
    }

    fn completion_event(seq: u64, summary: &ExecutionOutputSummary) -> DecodedLaunchEvent {
        let value = LaunchOutputCompleteMarkerValueV1 {
            protocol: LAUNCH_OUTPUT_V1.to_string(),
            stdout: LaunchOutputChannelSummaryV1 {
                bytes: summary.stdout_bytes,
                sha256: summary.stdout_sha256.clone(),
            },
            stderr: LaunchOutputChannelSummaryV1 {
                bytes: summary.stderr_bytes,
                sha256: summary.stderr_sha256.clone(),
            },
            data_event_count: summary.data_event_count,
        };
        DecodedLaunchEvent::Marker {
            seq,
            name: LAUNCH_OUTPUT_COMPLETE_MARKER_V1.to_string(),
            value: serde_json::to_value(value).expect("completion value"),
        }
    }

    fn exit_event(seq: u64) -> DecodedLaunchEvent {
        DecodedLaunchEvent::Exit(LaunchExit {
            seq,
            status: ProcessStatus::Exited { code: 0 },
            terminal_signal: TerminalSignal {
                kind: TerminalSignalKind::CleanExit,
                evidence: None,
                observed_at_unix_ms: 1,
            },
            session: None,
        })
    }

    fn digest_hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}
