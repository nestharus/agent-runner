//! Bounded no-follow source reads. Same UID remains the trust principal.
use std::path::{Component, Path};

#[cfg(unix)]
pub fn open_source_file(
    directory: &Path,
    relative: &str,
    limit: usize,
) -> Result<std::fs::File, String> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::OpenOptionsExt;
    if std::fs::canonicalize(directory).map_err(|e| e.to_string())? != directory {
        return Err("registered source directory is not canonical".into());
    }
    let mut dir = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(directory)
        .map_err(|e| e.to_string())?;
    let components: Vec<_> = Path::new(relative).components().collect();
    if components.is_empty() {
        return Err("empty source relative path".into());
    }
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err("source path escapes registered directory".into());
        };
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::CString::new(name.as_bytes()).map_err(|e| e.to_string())?;
        let last = index + 1 == components.len();
        let flags = libc::O_RDONLY
            | libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | if last {
                libc::O_NONBLOCK
            } else {
                libc::O_DIRECTORY
            };
        let fd = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let file = unsafe { std::fs::File::from_raw_fd(fd) };
        if !last {
            dir = file;
            continue;
        }
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file() || metadata.len() > limit as u64 {
            return Err("source is not a bounded regular file".into());
        }
        return Ok(file);
    }
    Err("source file not reached".into())
}

#[cfg(not(unix))]
pub fn open_source_file(
    _directory: &Path,
    _relative: &str,
    _limit: usize,
) -> Result<std::fs::File, String> {
    Err("completion-continuation-v2 source reads require the supported Unix native lane".into())
}

pub fn read_source_file(directory: &Path, relative: &str, limit: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let file = open_source_file(directory, relative, limit)?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("source grew beyond bound".into());
    }
    Ok(bytes)
}
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn source_reads_and_executable_opens_reject_parent_symlinks_and_nonregular_files() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("image"), b"bytes").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
        assert!(open_source_file(root.path(), "escape/image", 100).is_err());
        assert!(read_source_file(root.path(), "../image", 100).is_err());
        assert!(read_source_file(root.path(), "escape", 100).is_err());
        std::fs::write(root.path().join("large"), b"12345").unwrap();
        assert!(read_source_file(root.path(), "large", 4).is_err());
        assert_eq!(read_source_file(root.path(), "large", 5).unwrap(), b"12345");
    }
}
