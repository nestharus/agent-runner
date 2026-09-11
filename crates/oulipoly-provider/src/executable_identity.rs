//! Retained-handle identity and change validation. Paths never supply hash bytes.
//! Roles: accessor, validator.
use sha2::{Digest, Sha256};
use std::fs::File;
use std::sync::Mutex;

#[derive(Debug, Default)]
pub(crate) struct IdentityCache(Mutex<Option<(Vec<u8>, String)>>);

impl IdentityCache {
    pub fn unchanged(&self, file: &File) -> Result<bool, String> {
        let stamp = stamp(file)?;
        Ok(self
            .0
            .lock()
            .map_err(|e| e.to_string())?
            .as_ref()
            .is_some_and(|(previous, _)| *previous == stamp))
    }

    pub fn digest(&self, pinned: &File) -> Result<String, String> {
        let before = stamp(pinned)?;
        let mut cache = self.0.lock().map_err(|e| e.to_string())?;
        if let Some((previous, digest)) = cache.as_ref() {
            if *previous == before {
                return Ok(digest.clone());
            }
        }
        // Linux native pins are O_PATH. This is a descriptor reopen, not a
        // pathname lookup; a renamed/replaced executable cannot donate bytes.
        #[cfg(target_os = "linux")]
        let file = {
            use std::os::fd::AsRawFd;
            File::open(format!("/proc/self/fd/{}", pinned.as_raw_fd()))
                .map_err(|e| e.to_string())?
        };
        #[cfg(not(target_os = "linux"))]
        let file = pinned;
        let length = file.metadata().map_err(|e| e.to_string())?.len();
        if length > 512 * 1024 * 1024 {
            return Err("endpoint_identity_unbounded".into());
        }
        if stamp(&file)? != before {
            return Err("endpoint_identity_changed".into());
        }
        let mut digest = Sha256::new();
        digest.update(&before);
        let mut offset = 0;
        let mut bytes = [0u8; 65536];
        loop {
            let read = read_at(&file, &mut bytes, offset).map_err(|e| e.to_string())?;
            if read == 0 {
                break;
            }
            offset += read as u64;
            if offset > length {
                return Err("endpoint_identity_changed".into());
            }
            digest.update(&bytes[..read]);
        }
        if offset != length || stamp(pinned)? != before || stamp(&file)? != before {
            return Err("endpoint_identity_changed".into());
        }
        let identity = format!("{:x}", digest.finalize());
        *cache = Some((before, identity.clone()));
        Ok(identity)
    }
}

#[cfg(unix)]
fn read_at(file: &File, bytes: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, bytes, offset)
}
#[cfg(windows)]
fn read_at(file: &File, bytes: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, bytes, offset)
}
#[cfg(not(any(unix, windows)))]
fn read_at(_: &File, _: &mut [u8], _: u64) -> std::io::Result<usize> {
    Err(std::io::Error::other("endpoint_identity_unsupported"))
}

#[cfg(unix)]
pub(crate) fn stamp(file: &File) -> Result<Vec<u8>, String> {
    use std::os::unix::fs::MetadataExt;
    let m = file.metadata().map_err(|e| e.to_string())?;
    if !m.is_file() {
        return Err("endpoint_identity_not_file".into());
    }
    serde_json::to_vec(&(
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    ))
    .map_err(|e| e.to_string())
}

#[cfg(windows)]
pub(crate) fn stamp(file: &File) -> Result<Vec<u8>, String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_BASIC_INFO, FILE_ID_INFO, FileBasicInfo, FileIdInfo,
        GetFileInformationByHandle, GetFileInformationByHandleEx,
    };
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let mut basic: FILE_BASIC_INFO = unsafe { std::mem::zeroed() };
    let mut id: FILE_ID_INFO = unsafe { std::mem::zeroed() };
    let handle = file.as_raw_handle();
    // Volume/file index distinguish identical-byte replacement; change time
    // catches metadata revisions. Resolver denies write/delete sharing for the
    // entire retained handle lifetime, including while serving a cached hash.
    if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0
        || unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileBasicInfo,
                (&mut basic as *mut FILE_BASIC_INFO).cast(),
                std::mem::size_of_val(&basic) as u32,
            )
        } == 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            (&mut id as *mut FILE_ID_INFO).cast(),
            std::mem::size_of_val(&id) as u32,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if info.dwFileAttributes & 0x10 != 0 {
        return Err("endpoint_identity_not_file".into());
    }
    serde_json::to_vec(&(
        "windows-handle-v1",
        id.VolumeSerialNumber,
        id.FileId.Identifier,
        info.nFileSizeHigh,
        info.nFileSizeLow,
        basic.CreationTime,
        basic.LastWriteTime,
        basic.ChangeTime,
    ))
    .map_err(|e| e.to_string())
}
#[cfg(not(any(unix, windows)))]
pub(crate) fn stamp(_: &File) -> Result<Vec<u8>, String> {
    Err("endpoint_identity_unsupported".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom, Write};

    #[test]
    fn cached_identity_revalidates_retained_revision_and_replacement() {
        let root = std::env::temp_dir().join(format!("receipt-identity-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("provider");
        std::fs::write(&path, b"version-one").unwrap();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let cache = IdentityCache::default();
        let initial = cache.digest(&file).unwrap();
        assert!(cache.unchanged(&file).unwrap());
        assert_eq!(initial, cache.digest(&file).unwrap());
        // A hit is actually cached, not an equal fresh hash: poison only the
        // cached result, then require that hit. Never used by production code.
        cache.0.lock().unwrap().as_mut().unwrap().1 = "cache-hit".into();
        assert_eq!(cache.digest(&file).unwrap(), "cache-hit");
        file.seek(SeekFrom::Start(0)).unwrap();
        file.write_all(b"different-version-and-length").unwrap();
        file.flush().unwrap();
        assert!(!cache.unchanged(&file).unwrap());
        assert_ne!(cache.digest(&file).unwrap(), "cache-hit");
        std::fs::write(root.join("replacement"), b"different-version-and-length").unwrap();
        let other = File::open(root.join("replacement")).unwrap();
        assert_ne!(
            cache.digest(&file).unwrap(),
            IdentityCache::default().digest(&other).unwrap()
        );
        drop(file);
        drop(other);
        std::fs::remove_dir_all(root).unwrap();
    }
    use std::fs::OpenOptions;

    #[cfg(windows)]
    #[test]
    fn windows_resolver_pin_denies_write_and_delete_for_cached_lifetime() {
        use crate::resolver::{ProviderArtifactRef, ProviderResolveOptions, ProviderResolver};
        let root =
            std::env::temp_dir().join(format!("receipt-windows-pin-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("provider.exe");
        std::fs::write(&path, b"offline-fixture").unwrap();
        let resolved = ProviderResolver::new(ProviderResolveOptions::default())
            .resolve(&ProviderArtifactRef::Path { path: path.clone() }, None)
            .unwrap();
        let file = resolved.pinned_executable();
        let cache = IdentityCache::default();
        let identity = cache.digest(&file).unwrap();
        assert!(OpenOptions::new().write(true).open(&path).is_err());
        assert!(std::fs::remove_file(&path).is_err());
        assert_eq!(identity, cache.digest(&file).unwrap());
        drop(file);
        drop(resolved);
        std::fs::remove_dir_all(root).unwrap();
    }
}
