//! Cross-process marker verification refresh lock.
//!
//! ## Declared roles
//! orchestration, accessor, formatter, mapper
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/quota/marker_verification/lock.rs
//!     role: adapter
//!     Translates:
//!       - process data-home environment contract (`OULIPOLY_DATA_HOME`, `XDG_DATA_HOME`, `dirs::data_dir`)
//!       - filesystem lock contract (`std::fs`, `std::fs::File`, `std::fs::OpenOptions`, `fs4::FileExt::lock`, `fs4::FileExt::unlock`)
//!       - marker verification lock naming contract (`sanitize_lock_name`, `.lock`, `oulipoly-agent-runner/usage-refresh-locks`)
//! ```

use crate::quota::lock_paths;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

/// Per-provider blocking file lock — `flock(2)` over a single sentinel
/// per provider under the usage-refresh-locks directory. Lifetime owns the
/// open fd; drop releases the lock.
pub(crate) struct RefreshFileLock {
    file: File,
}

impl RefreshFileLock {
    pub(crate) fn acquire_blocking(provider_name: &str) -> Result<Self, String> {
        let path = provider_lock_path(provider_name);
        ensure_lock_directory(&path)?;
        let file = open_lock_file(&path)?;
        lock_file(&file, &path)?;
        Ok(Self { file })
    }
}

impl Drop for RefreshFileLock {
    fn drop(&mut self) {
        let _ = <File as fs4::FileExt>::unlock(&self.file);
    }
}

fn ensure_lock_directory(path: &Path) -> Result<(), String> {
    let Some(dir) = path.parent() else {
        return Err(lock_path_parent_error(path));
    };
    fs::create_dir_all(dir).map_err(|err| create_lock_dir_error(dir, &err))
}

fn open_lock_file(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|err| open_lock_file_error(path, &err))
}

fn lock_file(file: &File, path: &Path) -> Result<(), String> {
    <File as fs4::FileExt>::lock(file).map_err(|err| flock_error(path, &err))
}

fn lock_path_parent_error(path: &Path) -> String {
    format!(
        "failed to resolve usage-refresh-locks dir for {}",
        path.display()
    )
}

fn create_lock_dir_error(dir: &Path, err: &std::io::Error) -> String {
    format!(
        "failed to create usage-refresh-locks dir {}: {err}",
        dir.display()
    )
}

fn open_lock_file_error(path: &Path, err: &std::io::Error) -> String {
    format!("failed to open lock file {}: {err}", path.display())
}

fn flock_error(path: &Path, err: &std::io::Error) -> String {
    format!("failed to flock {}: {err}", path.display())
}

fn provider_lock_path(provider_name: &str) -> PathBuf {
    usage_lock_dir().join(provider_lock_file_name(provider_name))
}

fn provider_lock_file_name(provider_name: &str) -> String {
    format!("{}.lock", sanitize_lock_name(provider_name))
}

pub(super) fn usage_lock_dir() -> PathBuf {
    lock_paths::app_data_dir().join("usage-refresh-locks")
}

pub(super) fn sanitize_lock_name(name: &str) -> String {
    lock_paths::sanitize_lock_name(name)
}
