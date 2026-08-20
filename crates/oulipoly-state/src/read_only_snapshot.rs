//! Physical snapshot support for SQLite readers that must not mutate source storage.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const SNAPSHOT_RETRY_INTERVAL: Duration = Duration::from_millis(10);
// One observability scan opens up to three stores serially. Keep transient
// source churn from turning best-effort observation into multi-second delay.
const SNAPSHOT_TIMEOUT: Duration = Duration::from_millis(250);
const SQLITE_ARTIFACT_SUFFIXES: [&str; 3] = ["", "-wal", "-journal"];
const COPY_BUFFER_BYTES: usize = 64 * 1024;
const HELPER_POLL_INTERVAL: Duration = Duration::from_millis(2);
const HELPER_COMPARE_TIMEOUT: Duration = Duration::from_secs(30);
const HELPER_RETRY_ACCEPT_TIMEOUT: Duration = Duration::from_secs(5);
const HELPER_SHUTDOWN_GRACE: Duration = Duration::from_millis(100);
const HELPER_RETRY_MARKER: &str = "retry";
#[cfg(test)]
const STALLED_HELPER_SOURCE_NAME: &str = "stall-snapshot-helper.db";

pub(crate) struct ReadOnlySnapshot {
    _helper: SnapshotHelperGuard,
    path: PathBuf,
}

struct SnapshotHelperGuard {
    child: Option<Child>,
    control: Option<tempfile::TempDir>,
    snapshot: Option<tempfile::TempDir>,
}

impl SnapshotHelperGuard {
    fn control_path(&self) -> io::Result<&Path> {
        self.control
            .as_ref()
            .map(tempfile::TempDir::path)
            .ok_or_else(|| io::Error::other("snapshot helper control is unavailable"))
    }

    fn child_mut(&mut self) -> io::Result<&mut Child> {
        self.child
            .as_mut()
            .ok_or_else(|| io::Error::other("snapshot helper is unavailable"))
    }

    fn request_retry(&mut self, is_cancelled: &dyn Fn() -> bool) -> io::Result<()> {
        ensure_snapshot_not_cancelled(is_cancelled)?;
        let retry_marker = self.control_path()?.join(HELPER_RETRY_MARKER);
        std::fs::write(&retry_marker, [])?;
        let deadline = Instant::now() + HELPER_RETRY_ACCEPT_TIMEOUT;
        while retry_marker.exists() {
            ensure_snapshot_not_cancelled(is_cancelled)?;
            if self.child_mut()?.try_wait()?.is_some() {
                return Err(io::Error::other(
                    "snapshot helper exited before accepting a retry",
                ));
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "snapshot helper did not accept a retry",
                ));
            }
            std::thread::sleep(HELPER_POLL_INTERVAL);
        }
        Ok(())
    }
}

impl Drop for SnapshotHelperGuard {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        drop(child.stdin.take());
        if wait_for_helper_shutdown(&mut child) {
            return;
        }
        let control = self.control.take();
        let snapshot = self.snapshot.take();
        defer_helper_reap(child, control, snapshot);
    }
}

fn wait_for_helper_shutdown(child: &mut Child) -> bool {
    let deadline = Instant::now() + HELPER_SHUTDOWN_GRACE;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(HELPER_POLL_INTERVAL);
            }
            Ok(None) | Err(_) => return false,
        }
    }
}

fn defer_helper_reap(
    mut child: Child,
    control: Option<tempfile::TempDir>,
    snapshot: Option<tempfile::TempDir>,
) {
    // The closed parent pipe leaves cleanup authority with the helper while its
    // potentially stalled exit is kept off the caller's settlement path.
    let _ = std::thread::Builder::new()
        .name("oulipoly-snapshot-helper-reaper".to_string())
        .spawn(move || {
            let _ = child.wait();
            drop(control);
            drop(snapshot);
        });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SqliteArtifactIdentity {
    storage: u64,
    file: u64,
}

#[derive(Debug)]
struct OpenedSqliteArtifact {
    file: File,
    identity: SqliteArtifactIdentity,
}

#[derive(Debug)]
struct OpenedSqliteArtifactSet {
    artifacts: [Option<OpenedSqliteArtifact>; SQLITE_ARTIFACT_SUFFIXES.len()],
}

enum ValidatedSourceArtifact {
    Absent,
    Changed,
    Present {
        file: File,
        identity: SqliteArtifactIdentity,
    },
}

impl ReadOnlySnapshot {
    pub(crate) fn create_with_cancel(
        source: &Path,
        is_cancelled: &dyn Fn() -> bool,
    ) -> io::Result<Self> {
        Self::create_with_retry_timeout(source, SNAPSHOT_TIMEOUT, is_cancelled)
    }

    pub(crate) fn create_with_retry_timeout(
        source: &Path,
        timeout: Duration,
        is_cancelled: &dyn Fn() -> bool,
    ) -> io::Result<Self> {
        Self::create_with_retry_policy(
            source,
            timeout,
            SNAPSHOT_RETRY_INTERVAL,
            is_cancelled,
            || Ok(()),
        )
    }

    #[cfg(test)]
    fn create_with_copy_hook(
        source: &Path,
        after_copy: impl FnMut() -> io::Result<()>,
    ) -> io::Result<Self> {
        Self::create_with_retry_policy(
            source,
            SNAPSHOT_TIMEOUT,
            SNAPSHOT_RETRY_INTERVAL,
            &|| false,
            after_copy,
        )
    }

    fn create_with_retry_policy(
        source: &Path,
        timeout: Duration,
        retry_interval: Duration,
        is_cancelled: &dyn Fn() -> bool,
        mut after_copy: impl FnMut() -> io::Result<()>,
    ) -> io::Result<Self> {
        let source = canonical_sqlite_source(source)?;
        let dir = tempfile::tempdir()?;
        let file_name = source.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "SQLite path has no file name")
        })?;
        let path = dir.path().join(file_name);
        let mut retry_deadline = None;
        let mut helper = start_snapshot_helper(&source, &path, dir)?;
        loop {
            ensure_snapshot_not_cancelled(is_cancelled)?;
            if let Some(deadline) = retry_deadline {
                ensure_retry_active(deadline, is_cancelled)?;
            }
            let control = helper.control_path()?.to_path_buf();
            let outcome =
                wait_for_helper_copy(helper.child_mut()?, &control, is_cancelled, &mut after_copy)?;
            if outcome == HelperCopyOutcome::Stable {
                return Ok(Self {
                    _helper: helper,
                    path,
                });
            }
            let deadline = *retry_deadline.get_or_insert_with(|| Instant::now() + timeout);
            ensure_retry_active(deadline, is_cancelled)?;
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return Err(snapshot_changed_error());
            };
            if remaining.is_zero() {
                return Err(snapshot_changed_error());
            }
            std::thread::sleep(retry_interval.min(remaining));
            helper.request_retry(is_cancelled)?;
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

fn start_snapshot_helper(
    source: &Path,
    destination: &Path,
    snapshot: tempfile::TempDir,
) -> io::Result<SnapshotHelperGuard> {
    let mut command = snapshot_helper_command()?;
    let destination_parent = destination.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "snapshot destination has no parent",
        )
    })?;
    let control = tempfile::Builder::new()
        .prefix(".snapshot-helper-")
        .tempdir_in(destination_parent)?;
    command
        .arg(source)
        .arg(destination)
        .arg(control.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Ok(SnapshotHelperGuard {
        child: Some(command.spawn()?),
        control: Some(control),
        snapshot: Some(snapshot),
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HelperCopyOutcome {
    Stable,
    Changed,
}

fn snapshot_helper_command() -> io::Result<Command> {
    let current = std::env::current_exe()?;
    let mut command = Command::new(current);
    command.arg(crate::snapshot_helper::MODE_ARG);
    Ok(command)
}

fn wait_for_helper_copy(
    child: &mut Child,
    control: &Path,
    is_cancelled: &dyn Fn() -> bool,
    after_copy: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<HelperCopyOutcome> {
    let ready = control.join("ready");
    loop {
        terminate_cancelled_helper(is_cancelled)?;
        if control.join("result").exists() {
            return read_helper_result(control);
        }
        if ready.exists() {
            break;
        }
        if child.try_wait()?.is_some() {
            return Err(io::Error::other(
                "snapshot helper exited before assuming cleanup guardianship",
            ));
        }
        std::thread::sleep(HELPER_POLL_INTERVAL);
    }
    after_copy()?;
    std::fs::write(control.join("compare"), [])?;
    loop {
        terminate_cancelled_helper(is_cancelled)?;
        if control.join("result").exists() {
            return read_helper_result(control);
        }
        if child.try_wait()?.is_some() {
            return Err(io::Error::other(
                "snapshot helper exited before assuming cleanup guardianship",
            ));
        }
        std::thread::sleep(HELPER_POLL_INTERVAL);
    }
}

fn terminate_cancelled_helper(is_cancelled: &dyn Fn() -> bool) -> io::Result<()> {
    if !is_cancelled() {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::Interrupted,
        "Read-only SQLite snapshot cancelled",
    ))
}

fn read_helper_result(control: &Path) -> io::Result<HelperCopyOutcome> {
    let result = std::fs::read_to_string(control.join("result")).map_err(|error| {
        io::Error::other(format!("snapshot helper did not publish a result: {error}"))
    })?;
    let mut lines = result.lines();
    match lines.next() {
        Some("stable") => Ok(HelperCopyOutcome::Stable),
        Some("changed") => Ok(HelperCopyOutcome::Changed),
        Some("error") => {
            let kind = helper_error_kind(lines.next().unwrap_or("other"));
            Err(io::Error::new(kind, lines.collect::<Vec<_>>().join("\n")))
        }
        outcome => Err(io::Error::other(format!(
            "snapshot helper exited with an invalid result: {outcome:?}"
        ))),
    }
}

pub(crate) fn run_helper(source: &Path, destination: &Path, control: &Path) -> i32 {
    let parent_connected = match watch_parent_connection() {
        Ok(parent_connected) => parent_connected,
        Err(error) => {
            let result = format!("error\n{}\n{error}", helper_error_kind_code(error.kind()));
            let _ = publish_helper_result(control, &result);
            return 1;
        }
    };
    let is_cancelled = || !parent_connected.load(Ordering::SeqCst);
    #[cfg(test)]
    if source
        .file_name()
        .is_some_and(|name| name == std::ffi::OsStr::new(STALLED_HELPER_SOURCE_NAME))
    {
        let _ = std::fs::write(control.join("ready"), []);
        std::thread::sleep(Duration::from_millis(500));
    }
    let exit_code = 'attempts: loop {
        match execute_helper(source, destination, control, &is_cancelled) {
            Ok(HelperCopyOutcome::Changed) => {
                while parent_connected.load(Ordering::SeqCst) {
                    if control.join(HELPER_RETRY_MARKER).exists() {
                        if prepare_snapshot_retry(control).is_err() {
                            break 'attempts 1;
                        }
                        continue 'attempts;
                    }
                    std::thread::sleep(HELPER_POLL_INTERVAL);
                }
                break 1;
            }
            Ok(HelperCopyOutcome::Stable) => break 0,
            Err(_) if is_cancelled() => break 1,
            Err(error) => {
                let result = format!("error\n{}\n{error}", helper_error_kind_code(error.kind()));
                let _ = publish_helper_result(control, &result);
                break 1;
            }
        }
    };
    while parent_connected.load(Ordering::SeqCst) {
        std::thread::sleep(HELPER_POLL_INTERVAL);
    }
    cleanup_abandoned_helper_files(destination, control);
    exit_code
}

fn watch_parent_connection() -> io::Result<Arc<AtomicBool>> {
    let parent_connected = Arc::new(AtomicBool::new(true));
    let watcher_state = Arc::clone(&parent_connected);
    std::thread::Builder::new()
        .name("oulipoly-snapshot-parent-watch".to_string())
        .spawn(move || {
            let mut stdin = std::io::stdin().lock();
            let mut buffer = [0_u8; 1];
            while matches!(stdin.read(&mut buffer), Ok(1)) {}
            watcher_state.store(false, Ordering::SeqCst);
        })?;
    Ok(parent_connected)
}

fn prepare_snapshot_retry(control: &Path) -> io::Result<()> {
    for marker in ["ready", "compare", "result", ".result-writing"] {
        remove_if_present(&control.join(marker))?;
    }
    remove_if_present(&control.join(HELPER_RETRY_MARKER))
}

fn cleanup_abandoned_helper_files(destination: &Path, control: &Path) {
    for suffix in SQLITE_ARTIFACT_SUFFIXES {
        let _ = std::fs::remove_file(path_with_suffix(destination, suffix));
    }
    for marker in [
        "ready",
        "compare",
        "result",
        ".result-writing",
        HELPER_RETRY_MARKER,
    ] {
        let _ = std::fs::remove_file(control.join(marker));
    }
    let _ = std::fs::remove_dir(control);
    if destination.parent() == control.parent()
        && let Some(parent) = destination.parent()
    {
        let _ = std::fs::remove_dir(parent);
    }
}

fn execute_helper(
    source: &Path,
    destination: &Path,
    control: &Path,
    is_cancelled: &dyn Fn() -> bool,
) -> io::Result<HelperCopyOutcome> {
    let Some(mut copied) = copy_artifact_set_inline(source, destination, is_cancelled)? else {
        publish_helper_result(control, "changed\n")?;
        return Ok(HelperCopyOutcome::Changed);
    };
    std::fs::write(control.join("ready"), [])?;
    let deadline = Instant::now() + HELPER_COMPARE_TIMEOUT;
    while !control.join("compare").exists() {
        ensure_snapshot_not_cancelled(is_cancelled)?;
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "snapshot helper timed out waiting for comparison authority",
            ));
        }
        std::thread::sleep(HELPER_POLL_INTERVAL);
    }
    let outcome = if artifact_sets_match(source, destination, &mut copied, is_cancelled)? {
        HelperCopyOutcome::Stable
    } else {
        HelperCopyOutcome::Changed
    };
    drop(copied);
    publish_helper_result(
        control,
        match outcome {
            HelperCopyOutcome::Stable => "stable\n",
            HelperCopyOutcome::Changed => "changed\n",
        },
    )?;
    Ok(outcome)
}

fn publish_helper_result(control: &Path, result: &str) -> io::Result<()> {
    let staging = control.join(".result-writing");
    std::fs::write(&staging, result)?;
    std::fs::rename(staging, control.join("result"))
}

fn helper_error_kind_code(kind: io::ErrorKind) -> &'static str {
    match kind {
        io::ErrorKind::NotFound => "not-found",
        io::ErrorKind::PermissionDenied => "permission-denied",
        io::ErrorKind::InvalidInput => "invalid-input",
        io::ErrorKind::Interrupted => "interrupted",
        io::ErrorKind::WouldBlock => "would-block",
        io::ErrorKind::TimedOut => "timed-out",
        _ => "other",
    }
}

fn helper_error_kind(code: &str) -> io::ErrorKind {
    match code {
        "not-found" => io::ErrorKind::NotFound,
        "permission-denied" => io::ErrorKind::PermissionDenied,
        "invalid-input" => io::ErrorKind::InvalidInput,
        "interrupted" => io::ErrorKind::Interrupted,
        "would-block" => io::ErrorKind::WouldBlock,
        "timed-out" => io::ErrorKind::TimedOut,
        _ => io::ErrorKind::Other,
    }
}

fn canonical_sqlite_source(source: &Path) -> io::Result<PathBuf> {
    let canonical = std::fs::canonicalize(source)?;
    let metadata = std::fs::metadata(&canonical)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "SQLite source is not a regular file",
        ));
    }
    if crate::filesystem_identity::path_file_identity(&canonical, &metadata)?.links != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "SQLite source has multiple filesystem links",
        ));
    }
    Ok(canonical)
}

fn copy_artifact_set_inline(
    source: &Path,
    destination: &Path,
    is_cancelled: &dyn Fn() -> bool,
) -> io::Result<Option<OpenedSqliteArtifactSet>> {
    let mut artifacts = std::array::from_fn(|_| None);
    for (index, suffix) in SQLITE_ARTIFACT_SUFFIXES.into_iter().enumerate() {
        ensure_snapshot_not_cancelled(is_cancelled)?;
        let source_artifact = path_with_suffix(source, suffix);
        let destination_artifact = path_with_suffix(destination, suffix);
        match open_validated_source_artifact(&source_artifact)? {
            ValidatedSourceArtifact::Absent if suffix.is_empty() => return Ok(None),
            ValidatedSourceArtifact::Absent => remove_if_present(&destination_artifact)?,
            ValidatedSourceArtifact::Changed => return Ok(None),
            ValidatedSourceArtifact::Present { mut file, identity } => {
                copy_artifact(&mut file, &destination_artifact, is_cancelled)?;
                if validated_artifact_identity(&source_artifact)? != Some(identity) {
                    return Ok(None);
                }
                artifacts[index] = Some(OpenedSqliteArtifact { file, identity });
            }
        }
    }
    Ok(Some(OpenedSqliteArtifactSet { artifacts }))
}

fn artifact_sets_match(
    source: &Path,
    destination: &Path,
    copied: &mut OpenedSqliteArtifactSet,
    is_cancelled: &dyn Fn() -> bool,
) -> io::Result<bool> {
    artifact_sets_match_with_hook(source, destination, copied, is_cancelled, |_| Ok(()))
}

fn artifact_sets_match_with_hook(
    source: &Path,
    destination: &Path,
    copied: &mut OpenedSqliteArtifactSet,
    is_cancelled: &dyn Fn() -> bool,
    mut after_artifact: impl FnMut(usize) -> io::Result<()>,
) -> io::Result<bool> {
    for (index, suffix) in SQLITE_ARTIFACT_SUFFIXES.into_iter().enumerate() {
        ensure_snapshot_not_cancelled(is_cancelled)?;
        let source_artifact = path_with_suffix(source, suffix);
        let destination_artifact = path_with_suffix(destination, suffix);
        match copied.artifacts[index].as_mut() {
            None if validated_artifact_identity(&source_artifact)?.is_none() => {}
            Some(opened)
                if validated_artifact_identity(&source_artifact)? == Some(opened.identity) =>
            {
                if !files_match(&mut opened.file, &destination_artifact, is_cancelled)?
                    || validated_artifact_identity(&source_artifact)? != Some(opened.identity)
                {
                    return Ok(false);
                }
            }
            _ => return Ok(false),
        }
        after_artifact(index)?;
    }
    artifact_identity_set_matches(source, copied, is_cancelled)
}

fn artifact_identity_set_matches(
    source: &Path,
    copied: &OpenedSqliteArtifactSet,
    is_cancelled: &dyn Fn() -> bool,
) -> io::Result<bool> {
    for (index, suffix) in SQLITE_ARTIFACT_SUFFIXES.into_iter().enumerate() {
        ensure_snapshot_not_cancelled(is_cancelled)?;
        let source_artifact = path_with_suffix(source, suffix);
        let expected = copied.artifacts[index]
            .as_ref()
            .map(|artifact| artifact.identity);
        if validated_artifact_identity(&source_artifact)? != expected {
            return Ok(false);
        }
    }
    Ok(true)
}

fn copy_artifact(
    reader: &mut File,
    destination: &Path,
    is_cancelled: &dyn Fn() -> bool,
) -> io::Result<()> {
    reader.seek(SeekFrom::Start(0))?;
    let mut writer = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(destination)?;
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        ensure_snapshot_not_cancelled(is_cancelled)?;
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(());
        }
        writer.write_all(&buffer[..read])?;
    }
}

fn files_match(left: &mut File, right: &Path, is_cancelled: &dyn Fn() -> bool) -> io::Result<bool> {
    left.seek(SeekFrom::Start(0))?;
    let mut right = match File::open(right) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };
    let mut left_buffer = [0_u8; COPY_BUFFER_BYTES];
    let mut right_buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        ensure_snapshot_not_cancelled(is_cancelled)?;
        let left_read = left.read(&mut left_buffer)?;
        let right_read = right.read(&mut right_buffer)?;
        if left_read != right_read || left_buffer[..left_read] != right_buffer[..right_read] {
            return Ok(false);
        }
        if left_read == 0 {
            return Ok(true);
        }
    }
}

fn open_validated_source_artifact(path: &Path) -> io::Result<ValidatedSourceArtifact> {
    open_validated_source_artifact_with_hook(path, || Ok(()))
}

fn open_validated_source_artifact_with_hook(
    path: &Path,
    after_open: impl FnOnce() -> io::Result<()>,
) -> io::Result<ValidatedSourceArtifact> {
    let Some(identity) = validated_artifact_identity(path)? else {
        return Ok(ValidatedSourceArtifact::Absent);
    };
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(ValidatedSourceArtifact::Changed);
        }
        Err(err) => return Err(err),
    };
    after_open()?;
    let Some(opened_identity) = validated_artifact_metadata(path, &file, &file.metadata()?)? else {
        return Ok(ValidatedSourceArtifact::Changed);
    };
    if opened_identity != identity || validated_artifact_identity(path)? != Some(identity) {
        return Ok(ValidatedSourceArtifact::Changed);
    }
    Ok(ValidatedSourceArtifact::Present { file, identity })
}

fn validated_artifact_identity(path: &Path) -> io::Result<Option<SqliteArtifactIdentity>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    let identity = match crate::filesystem_identity::path_file_identity(path, &metadata) {
        Ok(identity) => identity,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    if identity.links == 0 {
        return Ok(None);
    }
    validated_artifact_identity_parts(path, &metadata, identity).map(Some)
}

fn validated_artifact_metadata(
    path: &Path,
    file: &File,
    metadata: &std::fs::Metadata,
) -> io::Result<Option<SqliteArtifactIdentity>> {
    let identity = crate::filesystem_identity::open_file_identity(file)?;
    if identity.links == 0 {
        return Ok(None);
    }
    validated_artifact_identity_parts(path, metadata, identity).map(Some)
}

fn validated_artifact_identity_parts(
    path: &Path,
    metadata: &std::fs::Metadata,
    identity: crate::filesystem_identity::OpenFileIdentity,
) -> io::Result<SqliteArtifactIdentity> {
    if !metadata.file_type().is_file() {
        return Err(unsupported_artifact_error(path, "is not a regular file"));
    }
    if identity.links != 1 {
        return Err(unsupported_artifact_error(
            path,
            "has multiple filesystem links",
        ));
    }
    Ok(SqliteArtifactIdentity {
        storage: identity.storage,
        file: identity.file,
    })
}

fn unsupported_artifact_error(path: &Path, reason: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("SQLite source artifact {} {reason}", path.display()),
    )
}

fn ensure_snapshot_not_cancelled(is_cancelled: &dyn Fn() -> bool) -> io::Result<()> {
    if is_cancelled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Read-only SQLite snapshot cancelled",
        ));
    }
    Ok(())
}

fn ensure_retry_active(deadline: Instant, is_cancelled: &dyn Fn() -> bool) -> io::Result<()> {
    ensure_snapshot_not_cancelled(is_cancelled)?;
    (Instant::now() < deadline)
        .then_some(())
        .ok_or_else(snapshot_changed_error)
}

fn snapshot_changed_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::WouldBlock,
        "SQLite source continued changing while retrying a read-only snapshot",
    )
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

fn path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut path = path.as_os_str().to_owned();
    path.push(suffix);
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_retries_after_four_consecutive_source_changes() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        std::fs::write(&source, "initial").unwrap();
        let changes = std::cell::Cell::new(0);

        let snapshot = ReadOnlySnapshot::create_with_copy_hook(&source, || {
            let change = changes.get();
            if change < 4 {
                std::fs::write(&source, format!("change-{change}"))?;
                changes.set(change + 1);
            }
            Ok(())
        })
        .unwrap();

        assert_eq!(changes.get(), 4);
        assert_eq!(std::fs::read(snapshot.path()).unwrap(), b"change-3");
    }

    #[test]
    fn snapshot_wal_churn_exhaustion_is_bounded_and_preserves_source_artifacts() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        let wal = path_with_suffix(&source, "-wal");
        std::fs::write(&source, "initial").unwrap();
        std::fs::write(&wal, "initial-wal").unwrap();
        let changes = std::cell::Cell::new(0);
        let expected_wal = std::cell::RefCell::new(b"initial-wal".to_vec());
        let started = Instant::now();

        let result = ReadOnlySnapshot::create_with_retry_policy(
            &source,
            Duration::from_millis(35),
            Duration::from_millis(1),
            &|| false,
            || {
                let bytes = format!("change-{}", changes.get()).into_bytes();
                std::fs::write(&wal, &bytes)?;
                *expected_wal.borrow_mut() = bytes;
                changes.set(changes.get() + 1);
                Ok(())
            },
        );

        let error = match result {
            Ok(_) => panic!("continuous source changes must exhaust the retry budget"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(changes.get() > 1);
        assert_eq!(std::fs::read(&source).unwrap(), b"initial");
        assert_eq!(std::fs::read(&wal).unwrap(), *expected_wal.borrow());
        assert!(!path_with_suffix(&source, "-journal").exists());
    }

    #[test]
    fn snapshot_cancellation_interrupts_retry_without_mutating_source() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        std::fs::write(&source, "initial").unwrap();
        let cancelled = std::cell::Cell::new(false);

        let result = ReadOnlySnapshot::create_with_retry_policy(
            &source,
            Duration::from_secs(1),
            Duration::from_millis(10),
            &|| cancelled.get(),
            || {
                cancelled.set(true);
                Ok(())
            },
        );

        let error = match result {
            Ok(_) => panic!("cancelled snapshot must not be published"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(std::fs::read(&source).unwrap(), b"initial");
    }

    #[test]
    fn cancellation_during_multi_chunk_copy_stops_before_the_source_is_consumed() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        let destination = directory.path().join("snapshot.db");
        let source_bytes = vec![b'x'; COPY_BUFFER_BYTES * 4];
        std::fs::write(&source, &source_bytes).unwrap();
        let mut reader = File::open(&source).unwrap();
        let checks = std::cell::Cell::new(0);

        let result = copy_artifact(&mut reader, &destination, &|| {
            let check = checks.get();
            checks.set(check + 1);
            check >= 2
        });

        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
        let copied_bytes = std::fs::metadata(&destination).unwrap().len();
        assert_eq!(copied_bytes, (COPY_BUFFER_BYTES * 2) as u64);
        assert!(copied_bytes < source_bytes.len() as u64);
        assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
    }

    #[test]
    fn stable_first_snapshot_attempt_completes_without_spending_retry_budget() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        std::fs::write(&source, vec![b'x'; COPY_BUFFER_BYTES * 2]).unwrap();

        let snapshot = ReadOnlySnapshot::create_with_retry_policy(
            &source,
            Duration::ZERO,
            Duration::ZERO,
            &|| false,
            || Ok(()),
        )
        .expect("a stable first attempt must not be rejected by the retry deadline");

        assert_eq!(
            std::fs::read(snapshot.path()).unwrap(),
            std::fs::read(source).unwrap()
        );
    }

    #[test]
    fn retry_does_not_start_after_the_deadline_expires() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        std::fs::write(&source, "initial").unwrap();
        let attempts = std::cell::Cell::new(0);

        let result = ReadOnlySnapshot::create_with_retry_policy(
            &source,
            Duration::from_millis(30),
            Duration::from_secs(1),
            &|| false,
            || {
                attempts.set(attempts.get() + 1);
                std::fs::write(&source, format!("change-{}", attempts.get()))
            },
        );

        let error = match result {
            Ok(_) => panic!("a retry must not start after deadline exhaustion"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(attempts.get(), 1);
    }

    #[test]
    fn stable_retry_that_started_before_the_deadline_may_finish_after_it() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        std::fs::write(&source, "initial").unwrap();
        let attempts = std::cell::Cell::new(0);

        let snapshot = ReadOnlySnapshot::create_with_retry_policy(
            &source,
            Duration::from_millis(100),
            Duration::from_millis(1),
            &|| false,
            || {
                let attempt = attempts.get();
                attempts.set(attempt + 1);
                if attempt == 0 {
                    std::fs::write(&source, "stable retry")?;
                } else {
                    std::thread::sleep(Duration::from_millis(150));
                }
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(attempts.get(), 2);
        assert_eq!(std::fs::read(snapshot.path()).unwrap(), b"stable retry");
    }

    #[test]
    fn changed_retry_that_finishes_after_the_deadline_does_not_start_a_third_pass() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        std::fs::write(&source, "initial").unwrap();
        let attempts = std::cell::Cell::new(0);

        let result = ReadOnlySnapshot::create_with_retry_policy(
            &source,
            Duration::from_millis(100),
            Duration::from_millis(1),
            &|| false,
            || {
                let attempt = attempts.get();
                attempts.set(attempt + 1);
                if attempt > 0 {
                    std::thread::sleep(Duration::from_millis(150));
                }
                std::fs::write(&source, format!("change-{}", attempt + 1))
            },
        );

        let error = match result {
            Ok(_) => panic!("a changed late retry must not publish or start another pass"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(attempts.get(), 2);
    }

    #[test]
    fn same_bytes_replacement_after_copy_forces_a_fresh_attempt() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        let replaced = directory.path().join("state-replaced.db");
        std::fs::write(&source, "stable bytes").unwrap();
        let attempts = std::cell::Cell::new(0);

        let snapshot = ReadOnlySnapshot::create_with_copy_hook(&source, || {
            let attempt = attempts.get();
            attempts.set(attempt + 1);
            if attempt == 0 {
                std::fs::rename(&source, &replaced)?;
                std::fs::write(&source, "stable bytes")?;
            }
            Ok(())
        })
        .unwrap();

        assert_eq!(attempts.get(), 2);
        assert_eq!(std::fs::read(snapshot.path()).unwrap(), b"stable bytes");
    }

    #[test]
    fn copied_artifact_set_retains_the_open_source_after_its_path_is_removed() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        let destination = directory.path().join("snapshot.db");
        std::fs::write(&source, "retained source").unwrap();
        let mut copied = copy_artifact_set_inline(&source, &destination, &|| false)
            .unwrap()
            .unwrap();

        std::fs::remove_file(&source).unwrap();
        let retained = copied.artifacts[0].as_mut().unwrap();
        retained.file.seek(SeekFrom::Start(0)).unwrap();
        let mut retained_bytes = Vec::new();
        retained.file.read_to_end(&mut retained_bytes).unwrap();

        assert_eq!(retained_bytes, b"retained source");
        assert!(!artifact_sets_match(&source, &destination, &mut copied, &|| false).unwrap());
    }

    #[test]
    fn same_bytes_wal_replacement_after_copy_forces_a_fresh_attempt() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        let wal = path_with_suffix(&source, "-wal");
        let replaced = directory.path().join("state-replaced.db-wal");
        std::fs::write(&source, "stable main").unwrap();
        std::fs::write(&wal, "stable wal").unwrap();
        let attempts = std::cell::Cell::new(0);

        let snapshot = ReadOnlySnapshot::create_with_copy_hook(&source, || {
            let attempt = attempts.get();
            attempts.set(attempt + 1);
            if attempt == 0 {
                std::fs::rename(&wal, &replaced)?;
                std::fs::write(&wal, "stable wal")?;
            }
            Ok(())
        })
        .unwrap();

        assert_eq!(attempts.get(), 2);
        assert_eq!(
            std::fs::read(path_with_suffix(snapshot.path(), "-wal")).unwrap(),
            b"stable wal"
        );
    }

    #[test]
    fn unlinked_wal_after_open_is_a_retryable_source_change() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        let wal = path_with_suffix(&source, "-wal");
        std::fs::write(&source, "stable main").unwrap();
        std::fs::write(&wal, "transient wal").unwrap();

        let opened =
            open_validated_source_artifact_with_hook(&wal, || std::fs::remove_file(&wal)).unwrap();

        assert!(matches!(opened, ValidatedSourceArtifact::Changed));
        let snapshot = ReadOnlySnapshot::create_with_cancel(&source, &|| false).unwrap();
        assert_eq!(std::fs::read(snapshot.path()).unwrap(), b"stable main");
        assert!(!path_with_suffix(snapshot.path(), "-wal").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn snapshot_helper_preserves_record_locks_without_retaining_replaced_files() {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::MetadataExt;

        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        let state = crate::StateDb::open(&source).unwrap();
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&source)
            .unwrap();
        let mut lock = libc::flock {
            l_type: libc::F_WRLCK as libc::c_short,
            l_whence: libc::SEEK_SET as libc::c_short,
            l_start: 0,
            l_len: 1,
            l_pid: 0,
        };
        assert_eq!(
            unsafe { libc::fcntl(lock_file.as_raw_fd(), libc::F_SETLK, &lock) },
            0
        );
        let inode = lock_file.metadata().unwrap().ino();
        assert!(process_holds_record_lock(inode));

        let descriptor_count = process_descriptors_under(directory.path());
        for generation in 0..32 {
            drop(ReadOnlySnapshot::create_with_cancel(&source, &|| false).unwrap());
            assert!(process_holds_record_lock(inode));
            assert_eq!(
                process_descriptors_under(directory.path()),
                descriptor_count
            );
            for suffix in SQLITE_ARTIFACT_SUFFIXES {
                let artifact = path_with_suffix(&source, suffix);
                let retired = directory
                    .path()
                    .join(format!("retired-{generation}{suffix}"));
                if artifact.exists() {
                    std::fs::rename(&artifact, &retired).unwrap();
                }
                std::fs::write(&artifact, format!("generation {generation}{suffix}")).unwrap();
                if retired.exists() {
                    std::fs::remove_file(retired).unwrap();
                }
            }
        }
        lock.l_type = libc::F_UNLCK as libc::c_short;
        assert_eq!(
            unsafe { libc::fcntl(lock_file.as_raw_fd(), libc::F_SETLK, &lock) },
            0
        );
        drop(state);
    }

    #[test]
    fn snapshot_helper_exits_and_cleans_up_when_parent_connection_closes() {
        let source_directory = tempfile::tempdir().unwrap();
        let source = source_directory.path().join("state.db");
        std::fs::write(&source, vec![7_u8; COPY_BUFFER_BYTES * 4]).unwrap();
        let snapshot_directory = tempfile::tempdir().unwrap().keep();
        let destination = snapshot_directory.join("state.db");
        let control = snapshot_directory.join("control");
        std::fs::create_dir(&control).unwrap();

        let mut child = snapshot_helper_command().unwrap();
        child
            .arg(&source)
            .arg(&destination)
            .arg(&control)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = child.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !control.join("ready").exists() {
            assert!(
                child.try_wait().unwrap().is_none(),
                "snapshot helper exited before publishing readiness"
            );
            assert!(
                Instant::now() < deadline,
                "snapshot helper did not become ready"
            );
            std::thread::sleep(HELPER_POLL_INTERVAL);
        }

        drop(child.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "disconnected snapshot helper did not exit"
            );
            std::thread::sleep(HELPER_POLL_INTERVAL);
        };

        assert!(!status.success());
        assert!(source.exists());
        assert!(!snapshot_directory.exists());
    }

    #[test]
    fn stalled_snapshot_helper_is_reaped_without_blocking_owner_shutdown() {
        let source_directory = tempfile::tempdir().unwrap();
        let source = source_directory.path().join(STALLED_HELPER_SOURCE_NAME);
        std::fs::write(&source, vec![7_u8; COPY_BUFFER_BYTES * 4]).unwrap();
        let snapshot = tempfile::tempdir().unwrap();
        let snapshot_directory = snapshot.path().to_path_buf();
        let destination = snapshot_directory.join("state.db");
        let mut helper = start_snapshot_helper(&source, &destination, snapshot).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !helper.control_path().unwrap().join("ready").exists() {
            assert!(helper.child_mut().unwrap().try_wait().unwrap().is_none());
            assert!(
                Instant::now() < deadline,
                "snapshot helper did not enter the stalled fixture"
            );
            std::thread::sleep(HELPER_POLL_INTERVAL);
        }

        let cancellation_checks = std::sync::atomic::AtomicUsize::new(0);
        let error = helper
            .request_retry(&|| cancellation_checks.fetch_add(1, Ordering::SeqCst) > 0)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(
            helper
                .control_path()
                .unwrap()
                .join(HELPER_RETRY_MARKER)
                .exists(),
            "retry cancellation happened before the helper request was published"
        );
        let started = Instant::now();
        drop(helper);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "stalled snapshot helper blocked owner shutdown"
        );

        let deadline = Instant::now() + Duration::from_secs(5);
        while snapshot_directory.exists() && Instant::now() < deadline {
            std::thread::sleep(HELPER_POLL_INTERVAL);
        }
        assert!(
            !snapshot_directory.exists(),
            "deferred helper reaper left snapshot storage behind"
        );
        assert!(source.exists());
    }

    #[test]
    fn snapshot_owner_process_exit_does_not_leave_snapshot_storage() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        let snapshot_temp = directory.path().join("snapshot-temp");
        let snapshot_path = directory.path().join("snapshot-path");
        std::fs::write(&source, vec![7_u8; COPY_BUFFER_BYTES * 4]).unwrap();
        std::fs::create_dir(&snapshot_temp).unwrap();

        let status = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("read_only_snapshot::tests::abrupt_snapshot_owner_fixture")
            .arg("--nocapture")
            .env("OULIPOLY_TEST_ABRUPT_SNAPSHOT_SOURCE", &source)
            .env("OULIPOLY_TEST_ABRUPT_SNAPSHOT_PATH", &snapshot_path)
            .env("TMPDIR", &snapshot_temp)
            .status()
            .unwrap();
        assert!(status.success(), "snapshot owner fixture failed: {status}");

        let leaked_path = PathBuf::from(std::fs::read_to_string(&snapshot_path).unwrap());
        let deadline = Instant::now() + Duration::from_secs(5);
        while leaked_path.exists() && Instant::now() < deadline {
            std::thread::sleep(HELPER_POLL_INTERVAL);
        }
        assert!(
            !leaked_path.exists(),
            "snapshot storage survived its owner process: {}",
            leaked_path.display()
        );
    }

    #[test]
    fn snapshot_owner_exit_between_changed_attempts_does_not_leave_snapshot_storage() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        let snapshot_temp = directory.path().join("snapshot-temp");
        let snapshot_path = directory.path().join("snapshot-path");
        std::fs::write(&source, vec![7_u8; COPY_BUFFER_BYTES * 4]).unwrap();
        std::fs::create_dir(&snapshot_temp).unwrap();

        let mut child = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("read_only_snapshot::tests::changed_snapshot_owner_fixture")
            .arg("--nocapture")
            .env("OULIPOLY_TEST_CHANGED_SNAPSHOT_SOURCE", &source)
            .env("OULIPOLY_TEST_CHANGED_SNAPSHOT_PATH", &snapshot_path)
            .env("OULIPOLY_TEST_CHANGED_SNAPSHOT_TEMP", &snapshot_temp)
            .env("TMPDIR", &snapshot_temp)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !snapshot_path.exists() {
            assert!(child.try_wait().unwrap().is_none());
            assert!(
                Instant::now() < deadline,
                "changed snapshot owner did not reach the retry boundary"
            );
            std::thread::sleep(HELPER_POLL_INTERVAL);
        }
        let leaked_path = PathBuf::from(std::fs::read_to_string(&snapshot_path).unwrap());
        child.kill().unwrap();
        child.wait().unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        while leaked_path.exists() && Instant::now() < deadline {
            std::thread::sleep(HELPER_POLL_INTERVAL);
        }
        assert!(
            !leaked_path.exists(),
            "changed snapshot storage survived its owner process: {}",
            leaked_path.display()
        );
    }

    #[test]
    fn abrupt_snapshot_owner_fixture() {
        let Some(source) = std::env::var_os("OULIPOLY_TEST_ABRUPT_SNAPSHOT_SOURCE") else {
            return;
        };
        let snapshot_path = std::env::var_os("OULIPOLY_TEST_ABRUPT_SNAPSHOT_PATH").unwrap();
        let snapshot = ReadOnlySnapshot::create_with_cancel(Path::new(&source), &|| false).unwrap();
        std::fs::write(
            snapshot_path,
            snapshot
                .path()
                .parent()
                .unwrap()
                .to_string_lossy()
                .as_bytes(),
        )
        .unwrap();

        std::process::exit(0);
    }

    #[test]
    fn changed_snapshot_owner_fixture() {
        let Some(source) = std::env::var_os("OULIPOLY_TEST_CHANGED_SNAPSHOT_SOURCE") else {
            return;
        };
        let snapshot_path = std::env::var_os("OULIPOLY_TEST_CHANGED_SNAPSHOT_PATH").unwrap();
        let snapshot_temp = std::env::var_os("OULIPOLY_TEST_CHANGED_SNAPSHOT_TEMP").unwrap();
        let source = Path::new(&source);
        let _ = ReadOnlySnapshot::create_with_retry_policy(
            source,
            Duration::from_secs(30),
            Duration::from_secs(30),
            &|| false,
            || {
                std::fs::write(source, vec![8_u8; COPY_BUFFER_BYTES * 4])?;
                let snapshot_directory = std::fs::read_dir(&snapshot_temp)?
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .find(|path| {
                        path.file_name()
                            .is_some_and(|name| name.to_string_lossy().starts_with(".tmp"))
                    })
                    .ok_or_else(|| io::Error::other("snapshot directory was not created"))?;
                std::fs::write(
                    &snapshot_path,
                    snapshot_directory.to_string_lossy().as_bytes(),
                )
            },
        );
    }

    #[cfg(target_os = "linux")]
    fn process_holds_record_lock(inode: u64) -> bool {
        let pid = std::process::id().to_string();
        let inode_suffix = format!(":{inode}");
        std::fs::read_to_string("/proc/locks")
            .unwrap()
            .lines()
            .any(|line| {
                let fields = line.split_whitespace().collect::<Vec<_>>();
                fields.get(4) == Some(&pid.as_str())
                    && fields
                        .get(5)
                        .is_some_and(|identity| identity.ends_with(&inode_suffix))
            })
    }

    #[cfg(target_os = "linux")]
    fn process_descriptors_under(directory: &Path) -> usize {
        std::fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| std::fs::read_link(entry.path()).ok())
            .filter(|target| target.starts_with(directory))
            .count()
    }

    #[test]
    fn main_replacement_after_its_check_is_rejected_by_final_artifact_set_join() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        let wal = path_with_suffix(&source, "-wal");
        let destination = directory.path().join("snapshot.db");
        let replaced = directory.path().join("state-replaced.db");
        std::fs::write(&source, "stable main").unwrap();
        std::fs::write(&wal, "stable wal").unwrap();
        let mut copied = copy_artifact_set_inline(&source, &destination, &|| false)
            .unwrap()
            .unwrap();

        let matches =
            artifact_sets_match_with_hook(&source, &destination, &mut copied, &|| false, |index| {
                if index == 0 {
                    std::fs::rename(&source, &replaced)?;
                    std::fs::write(&source, "replacement main")?;
                }
                Ok(())
            })
            .unwrap();

        assert!(!matches);
    }

    #[cfg(unix)]
    #[test]
    fn aliased_wal_and_journal_artifacts_fail_closed() {
        use std::os::unix::fs::symlink;

        for suffix in ["-wal", "-journal"] {
            let directory = tempfile::tempdir().unwrap();
            let source = directory.path().join("state.db");
            let sidecar = path_with_suffix(&source, suffix);
            let target = directory.path().join(format!("aliased{suffix}"));
            std::fs::write(&source, "stable main").unwrap();
            std::fs::write(&target, "sidecar bytes").unwrap();
            symlink(&target, &sidecar).unwrap();

            let error = match ReadOnlySnapshot::create_with_cancel(&source, &|| false) {
                Ok(_) => panic!("an aliased {suffix} artifact must fail closed"),
                Err(error) => error,
            };

            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        }
    }
}
