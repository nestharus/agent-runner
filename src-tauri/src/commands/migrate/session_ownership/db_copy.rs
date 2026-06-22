//! Declared roles: accessor, filter, mapper, validator

use super::{DryRunError, DryRunOptions};
use oulipoly_state::StateDb;
use rusqlite::{Connection, OpenFlags};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub(crate) struct SnapshotPaths {
    pub(crate) state_copy: PathBuf,
    pub(crate) rollback_copy: PathBuf,
    pub(crate) mailbox_copy: Option<PathBuf>,
    pub(crate) report_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceStat {
    size: u64,
    modified: SystemTime,
}

pub(crate) fn snapshot_inputs(opts: &DryRunOptions) -> Result<SnapshotPaths, DryRunError> {
    prepare_scratch_dir(&opts.scratch_dir)?;
    let mut paths = snapshot_paths(opts);
    snapshot_state_db_readonly(&opts.live_state_db_path, &paths.state_copy)?;
    let mailbox_source = mailbox_source_path(opts);
    paths.mailbox_copy = snapshot_mailbox_db(&mailbox_source, &opts.scratch_dir)?;
    Ok(paths)
}

pub(crate) fn open_copy(path: &Path) -> Result<Connection, DryRunError> {
    Connection::open(path).map_err(Into::into)
}

pub(crate) fn copy_state_to_rollback(paths: &SnapshotPaths) -> Result<(), DryRunError> {
    fs::copy(&paths.state_copy, &paths.rollback_copy)?;
    Ok(())
}

fn prepare_scratch_dir(scratch_dir: &Path) -> Result<(), DryRunError> {
    fs::create_dir_all(scratch_dir).map_err(Into::into)
}

fn snapshot_paths(opts: &DryRunOptions) -> SnapshotPaths {
    let state_copy = opts.scratch_dir.join("state-copy.db");
    let rollback_copy = opts.scratch_dir.join("rollback-copy.db");
    let report_path = opts.scratch_dir.join("dry-run-report.md");
    SnapshotPaths {
        state_copy,
        rollback_copy,
        mailbox_copy: None,
        report_path,
    }
}

fn mailbox_source_path(opts: &DryRunOptions) -> PathBuf {
    opts.mailbox_db_path
        .clone()
        .unwrap_or_else(|| opts.live_state_db_path.with_file_name("pid-identity.db"))
}

pub(crate) fn snapshot_state_db_readonly(src: &Path, dest: &Path) -> Result<(), DryRunError> {
    let before = source_stat(src)?;
    copy_state_snapshot(src, dest)?;
    let after = source_stat(src)?;
    validate_source_unchanged(&before, &after, "live state DB size or mtime changed")
}

fn copy_state_snapshot(src: &Path, dest: &Path) -> Result<(), DryRunError> {
    verify_state_read_only_open(src)?;
    if dest.exists() {
        fs::remove_file(dest)?;
    }
    let conn = Connection::open_with_flags(src, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let dest_text = dest.to_string_lossy().to_string();
    conn.execute("VACUUM main INTO ?1", [&dest_text])?;
    Ok(())
}

fn verify_state_read_only_open(src: &Path) -> Result<(), DryRunError> {
    drop(
        StateDb::open_read_only(src).map_err(|err| {
            DryRunError::new(format!("failed to open state DB read-only: {err:?}"))
        })?,
    );
    Ok(())
}

pub(crate) fn snapshot_mailbox_db(
    src: &Path,
    scratch_dir: &Path,
) -> Result<Option<PathBuf>, DryRunError> {
    let Some(src) = present_mailbox_source(src) else {
        return Ok(None);
    };
    let before = source_stat(src)?;
    let dest = mailbox_destination(src, scratch_dir)?;
    copy_mailbox_snapshot(src, &dest, scratch_dir)?;
    let after = source_stat(src)?;
    validate_source_unchanged(&before, &after, "live mailbox DB size or mtime changed")?;
    Ok(Some(dest))
}

fn present_mailbox_source(src: &Path) -> Option<&Path> {
    src.exists().then_some(src)
}

fn mailbox_destination(src: &Path, scratch_dir: &Path) -> Result<PathBuf, DryRunError> {
    Ok(scratch_dir.join(
        src.file_name()
            .ok_or_else(|| DryRunError::new("mailbox path has no file name"))?,
    ))
}

fn copy_mailbox_snapshot(src: &Path, dest: &Path, scratch_dir: &Path) -> Result<(), DryRunError> {
    fs::copy(src, dest)?;
    for suffix in ["-wal", "-shm"] {
        copy_mailbox_sidecar(src, suffix, scratch_dir)?;
    }
    Ok(())
}

fn copy_mailbox_sidecar(src: &Path, suffix: &str, scratch_dir: &Path) -> Result<(), DryRunError> {
    let sidecar = mailbox_sidecar_path(src, suffix);
    if !sidecar.exists() {
        return Ok(());
    }
    let sidecar_dest = mailbox_sidecar_destination(&sidecar, scratch_dir)?;
    fs::copy(sidecar, sidecar_dest)?;
    Ok(())
}

fn mailbox_sidecar_path(src: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{}", src.display(), suffix))
}

fn mailbox_sidecar_destination(sidecar: &Path, scratch_dir: &Path) -> Result<PathBuf, DryRunError> {
    Ok(scratch_dir.join(
        sidecar
            .file_name()
            .ok_or_else(|| DryRunError::new("mailbox sidecar has no file name"))?,
    ))
}

fn validate_source_unchanged(
    before: &SourceStat,
    after: &SourceStat,
    message: &str,
) -> Result<(), DryRunError> {
    if before != after {
        return Err(DryRunError::new(message));
    }
    Ok(())
}

fn source_stat(path: &Path) -> Result<SourceStat, DryRunError> {
    let metadata = fs::metadata(path)?;
    Ok(SourceStat {
        size: metadata.len(),
        modified: metadata.modified()?,
    })
}
