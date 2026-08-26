//! Stable cross-platform filesystem identity from an already-open file handle.

use std::fs::File;
use std::io;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OpenFileIdentity {
    pub(crate) storage: u64,
    pub(crate) file: u64,
    pub(crate) links: u64,
}

#[cfg(unix)]
pub(crate) fn open_file_identity(file: &File) -> io::Result<OpenFileIdentity> {
    Ok(unix_metadata_identity(&file.metadata()?))
}

/// Inspect a path without opening another descriptor on Unix, where closing
/// that descriptor could release process-scoped `fcntl` locks on the inode.
#[cfg(unix)]
pub(crate) fn path_file_identity(
    _path: &Path,
    metadata: &std::fs::Metadata,
) -> io::Result<OpenFileIdentity> {
    Ok(unix_metadata_identity(metadata))
}

#[cfg(unix)]
fn unix_metadata_identity(metadata: &std::fs::Metadata) -> OpenFileIdentity {
    use std::os::unix::fs::MetadataExt;

    OpenFileIdentity {
        storage: metadata.dev(),
        file: metadata.ino(),
        links: metadata.nlink(),
    }
}

#[cfg(windows)]
pub(crate) fn path_file_identity(
    path: &Path,
    _metadata: &std::fs::Metadata,
) -> io::Result<OpenFileIdentity> {
    open_file_identity(&File::open(path)?)
}

#[cfg(windows)]
pub(crate) fn open_file_identity(file: &File) -> io::Result<OpenFileIdentity> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    let succeeded =
        unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) };
    if succeeded == 0 {
        return Err(io::Error::last_os_error());
    }
    let information = unsafe { information.assume_init() };
    Ok(OpenFileIdentity {
        storage: u64::from(information.dwVolumeSerialNumber),
        file: (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
        links: u64::from(information.nNumberOfLinks),
    })
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn open_file_identity(_file: &File) -> io::Result<OpenFileIdentity> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "stable filesystem identity is unsupported on this platform",
    ))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn path_file_identity(
    _path: &Path,
    _metadata: &std::fs::Metadata,
) -> io::Result<OpenFileIdentity> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "stable filesystem identity is unsupported on this platform",
    ))
}
