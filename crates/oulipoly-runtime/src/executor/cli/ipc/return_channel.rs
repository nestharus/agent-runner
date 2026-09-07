//! Strict opened-file return-channel custody. Roles: orchestration, validator.
use super::return_channel_cleanup::cleanup_return_channel;
use super::return_channel_parent::parse_return_channel_parent_invocation;
use super::return_channel_path::{return_channel_dir, return_channel_path};
use crate::executor::ReturnedArtifactRef;
use oulipoly_provider::custody::ActorSettlementReceipt;
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub const MAX_RETURN_CHANNEL_BYTES: u64 = 1024 * 1024;
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum ReturnChannelSettlement {
    NotCreated,
    EmptyRemoved,
    ArtifactsCommitted(Vec<ReturnedArtifactRef>),
    Quarantined {
        path: PathBuf,
        sha256: Option<String>,
        artifacts: Vec<ReturnedArtifactRef>,
    },
    CleanupFailed {
        path: PathBuf,
        artifacts: Vec<ReturnedArtifactRef>,
    },
}
impl ReturnChannelSettlement {
    pub fn transferable(&self) -> bool {
        matches!(self, Self::NotCreated | Self::EmptyRemoved)
    }
    pub fn artifacts(&self) -> &[ReturnedArtifactRef] {
        match self {
            Self::ArtifactsCommitted(a)
            | Self::Quarantined { artifacts: a, .. }
            | Self::CleanupFailed { artifacts: a, .. } => a,
            _ => &[],
        }
    }
}
#[derive(Clone, Copy)]
enum ChannelUse {
    AllocatedSeal,
    StandaloneConsumption,
}

pub struct ReturnChannel {
    path: PathBuf,
    dir: PathBuf,
    file: Option<File>,
    directory: Option<File>,
    producer: Uuid,
    emergency_cleanup: bool,
}
impl ReturnChannel {
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// The root is a caller-owned private directory, not an environment-derived
    /// authority. Final directory and filename are deterministic and create-new.
    pub fn for_attempt(
        root: &Path,
        parent: Uuid,
        logical: Uuid,
        attempt: Uuid,
        producer: Uuid,
    ) -> Result<Self, String> {
        let dir = root
            .join(parent.to_string())
            .join(logical.to_string())
            .join(attempt.to_string());
        private_parents(root, &dir)?;
        Self::create(dir, producer)
    }
    fn create(dir: PathBuf, producer: Uuid) -> Result<Self, String> {
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&dir).map_err(|e| e.to_string())?;
        let directory = open_directory(&dir).map_err(|e| e.to_string())?;
        let path = return_channel_path(&dir);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(&path).map_err(|e| e.to_string())?;
        Ok(Self {
            path,
            dir,
            file: Some(file),
            directory: Some(directory),
            producer,
            emergency_cleanup: true,
        })
    }
    /// Only this explicit operation can produce a settlement. Drop cannot.
    /// Accepted refs must be durably retained on the producing invocation by
    /// `commit`, which records promotion before effects.
    pub fn seal(
        mut self,
        actors: &[ActorSettlementReceipt],
        commit: impl FnOnce(&[ReturnedArtifactRef]) -> Result<(), String>,
    ) -> ReturnChannelSettlement {
        self.emergency_cleanup = false;
        if actors.is_empty() || !actors.iter().all(ActorSettlementReceipt::effect_incapable) {
            return self.quarantine(None, vec![]);
        }
        self.seal_settled(commit)
    }
    fn seal_settled(
        &mut self,
        commit: impl FnOnce(&[ReturnedArtifactRef]) -> Result<(), String>,
    ) -> ReturnChannelSettlement {
        self.read_settled(commit, ChannelUse::AllocatedSeal)
    }
    fn read_settled(
        &mut self,
        commit: impl FnOnce(&[ReturnedArtifactRef]) -> Result<(), String>,
        use_kind: ChannelUse,
    ) -> ReturnChannelSettlement {
        self.emergency_cleanup = false;
        if !self.identical() {
            return self.quarantine(None, vec![]);
        }
        let metadata = match self.file.as_ref().unwrap().metadata() {
            Ok(m) => m,
            Err(_) => return self.cleanup_failed(vec![]),
        };
        if metadata.len() > MAX_RETURN_CHANNEL_BYTES {
            return self.quarantine(None, vec![]);
        }
        let mut bytes = Vec::new();
        if self
            .file
            .as_mut()
            .unwrap()
            .seek(SeekFrom::Start(0))
            .is_err()
            || self
                .file
                .as_mut()
                .unwrap()
                .take(MAX_RETURN_CHANNEL_BYTES + 1)
                .read_to_end(&mut bytes)
                .is_err()
        {
            return self.cleanup_failed(vec![]);
        }
        let digest = Some(format!("{:x}", Sha256::digest(&bytes)));
        if bytes.len() as u64 != metadata.len()
            || bytes.len() as u64 > MAX_RETURN_CHANNEL_BYTES
            || !self.identical()
            || self.file.as_ref().unwrap().metadata().ok().map(|m| m.len()) != Some(metadata.len())
        {
            return self.quarantine(digest, vec![]);
        }
        let (artifacts, valid) = strict_records(
            &bytes,
            self.producer,
            matches!(use_kind, ChannelUse::StandaloneConsumption),
        );
        // Retain valid prefix records even when another record is malformed.
        if !artifacts.is_empty() && commit(&artifacts).is_err() {
            return self.cleanup_failed(artifacts);
        }
        if !valid {
            return self.quarantine(digest, artifacts);
        }
        if !self.remove_consumed_channel(use_kind) {
            return self.cleanup_failed(artifacts);
        }
        if artifacts.is_empty() {
            ReturnChannelSettlement::EmptyRemoved
        } else {
            ReturnChannelSettlement::ArtifactsCommitted(artifacts)
        }
    }
    fn remove_consumed_channel(&mut self, use_kind: ChannelUse) -> bool {
        #[cfg(windows)]
        if matches!(use_kind, ChannelUse::StandaloneConsumption) {
            return self.remove_standalone_windows();
        }
        #[cfg(not(windows))]
        let _ = use_kind;
        if !self.identical()
            || std::fs::remove_file(&self.path).is_err()
            || std::fs::remove_dir(&self.dir).is_err()
            || !absent(&self.path)
            || !absent(&self.dir)
            || !unlinked(self.file.as_ref().unwrap())
            || !unlinked(self.directory.as_ref().unwrap())
        {
            return false;
        }
        self.close_checked()
    }
    #[cfg(windows)]
    fn remove_standalone_windows(&mut self) -> bool {
        // Windows deletion can stay pending until the last open handle closes.
        // This checks ordinary consumption only, not link-count/ACL transfer proof.
        if !self.identical() {
            return false;
        }
        let removed_file = std::fs::remove_file(&self.path).is_ok();
        let closed_file = self.file.take().is_some_and(close_file_checked);
        let removed_dir = std::fs::remove_dir(&self.dir).is_ok();
        let closed_dir = self.directory.take().is_some_and(close_file_checked);
        removed_file
            && closed_file
            && removed_dir
            && closed_dir
            && absent(&self.path)
            && absent(&self.dir)
    }
    fn identical(&self) -> bool {
        let (Some(file), Some(directory)) = (&self.file, &self.directory) else {
            return false;
        };
        opened_matches_path(file, &self.path, false)
            && opened_matches_path(directory, &self.dir, true)
    }
    fn close_checked(&mut self) -> bool {
        let file_ok = self.file.take().is_some_and(close_file_checked);
        let dir_ok = self.directory.take().is_some_and(close_file_checked);
        file_ok && dir_ok
    }
    fn quarantine(
        &self,
        sha256: Option<String>,
        artifacts: Vec<ReturnedArtifactRef>,
    ) -> ReturnChannelSettlement {
        ReturnChannelSettlement::Quarantined {
            path: self.path.clone(),
            sha256,
            artifacts,
        }
    }
    fn cleanup_failed(&self, artifacts: Vec<ReturnedArtifactRef>) -> ReturnChannelSettlement {
        ReturnChannelSettlement::CleanupFailed {
            path: self.path.clone(),
            artifacts,
        }
    }
}
impl Drop for ReturnChannel {
    fn drop(&mut self) {
        if self.emergency_cleanup && self.identical() {
            cleanup_return_channel(&self.path, &self.dir);
        }
    }
}
fn open_directory(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS);
    }
    options.open(path)
}
#[cfg(unix)]
fn close_file_checked(file: File) -> bool {
    use std::os::fd::IntoRawFd;
    unsafe { libc::close(file.into_raw_fd()) == 0 }
}
#[cfg(windows)]
fn close_file_checked(file: File) -> bool {
    use std::os::windows::io::IntoRawHandle;
    unsafe { windows_sys::Win32::Foundation::CloseHandle(file.into_raw_handle()) != 0 }
}
#[cfg(not(any(unix, windows)))]
fn close_file_checked(file: File) -> bool {
    drop(file);
    false
}
#[cfg(unix)]
fn opened_matches_path(file: &File, path: &Path, directory: bool) -> bool {
    match (file.metadata(), std::fs::symlink_metadata(path)) {
        (Ok(a), Ok(b)) => {
            use std::os::unix::fs::MetadataExt;
            let mode = if directory { 0o700 } else { 0o600 };
            (if directory { b.is_dir() } else { b.is_file() })
                && same_file(&a, &b)
                && a.uid() == unsafe { libc::geteuid() }
                && b.uid() == a.uid()
                && a.mode() & 0o7777 == mode
                && b.mode() & 0o7777 == mode
        }
        _ => false,
    }
}
#[cfg(windows)]
fn opened_matches_path(file: &File, path: &Path, directory: bool) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !(if directory {
        meta.is_dir()
    } else {
        meta.is_file()
    }) {
        return false;
    }
    let current = if directory {
        open_directory(path)
    } else {
        File::open(path)
    };
    match (
        windows_file_identity(file),
        current.and_then(|file| windows_file_identity(&file)),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}
#[cfg(windows)]
fn windows_file_identity(file: &File) -> std::io::Result<(u32, u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut value = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), value.as_mut_ptr()) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let value = unsafe { value.assume_init() };
    Ok((
        value.dwVolumeSerialNumber,
        (u64::from(value.nFileIndexHigh) << 32) | u64::from(value.nFileIndexLow),
    ))
}
#[cfg(not(any(unix, windows)))]
fn opened_matches_path(_: &File, _: &Path, _: bool) -> bool {
    false
}
#[cfg(unix)]
fn same_file(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino() && a.nlink() == 1 && b.nlink() == 1
        || a.is_dir() && b.is_dir() && a.dev() == b.dev() && a.ino() == b.ino()
}
#[cfg(unix)]
fn unlinked(file: &File) -> bool {
    use std::os::unix::fs::MetadataExt;
    file.metadata().is_ok_and(|m| m.nlink() == 0)
}
#[cfg(not(unix))]
fn unlinked(_: &File) -> bool {
    false
}
fn absent(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
}
fn private_parents(root: &Path, final_dir: &Path) -> Result<(), String> {
    let parent = final_dir.parent().ok_or("missing channel parent")?;
    let mut current = root.to_path_buf();
    for component in parent
        .strip_prefix(root)
        .map_err(|e| e.to_string())?
        .components()
    {
        current.push(component);
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&current) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e.to_string()),
        }
        if !std::fs::symlink_metadata(&current)
            .map_err(|e| e.to_string())?
            .is_dir()
        {
            return Err("substituted channel directory".into());
        }
    }
    Ok(())
}
fn strict_records(
    bytes: &[u8],
    producer: Uuid,
    retain_foreign_for_validation: bool,
) -> (Vec<ReturnedArtifactRef>, bool) {
    let Ok(body) = std::str::from_utf8(bytes) else {
        return (vec![], false);
    };
    let mut valid = body.trim().is_empty() || body.ends_with('\n');
    let mut refs = vec![];
    for line in body.lines().filter(|line| !line.trim().is_empty()) {
        let parsed = serde_json::from_str::<ReturnedArtifactRef>(line);
        match parsed {
            Ok(reference) if strict_shape(line, &reference) => {
                let matches_producer = reference.producer_invocation_uuid == producer;
                valid &= matches_producer;
                if matches_producer || retain_foreign_for_validation {
                    refs.push(reference);
                }
            }
            _ => valid = false,
        }
    }
    (refs, valid)
}
fn strict_shape(line: &str, reference: &ReturnedArtifactRef) -> bool {
    // Round-trip rejects unknown fields at every nesting level as well as
    // duplicate/noncanonical fields that serde would otherwise discard.
    let Ok(input) = serde_json::from_str::<serde_json::Value>(line) else {
        return false;
    };
    let output = if input.get("schema_version").is_some() {
        versioned_receipt_shape(line)
    } else {
        serde_json::to_value(reference).ok()
    };
    output.as_ref() == Some(&input)
        && reference.sha256.len() == 64
        && reference.sha256.bytes().all(|b| b.is_ascii_hexdigit())
}
// Messenger writes ReturnedArtifact (schema v1), whereas in-process callers
// can supply the projected ReturnedArtifactRef. Validate the producer's own
// wire type before projection; schema_version is not an arbitrary extra field.
// Typed decoding rejects duplicate fields; the round-trip still rejects all
// unknown fields, including nested ones. Unsupported versions stay quarantined.
fn versioned_receipt_shape(line: &str) -> Option<serde_json::Value> {
    let receipt: oulipoly_agent_messenger::ReturnedArtifact = serde_json::from_str(line).ok()?;
    if receipt.schema_version != 1 {
        return None;
    }
    serde_json::to_value(receipt).ok()
}
pub(crate) fn prepare_return_channel(
    parent: Option<&str>,
) -> Result<Option<ReturnChannel>, String> {
    let Some(parent) = parent else {
        return Ok(None);
    };
    let invocation = parse_return_channel_parent_invocation(parent)?;
    let dir = return_channel_dir(&invocation);
    std::fs::create_dir_all(dir.parent().ok_or("missing channel root")?)
        .map_err(|e| e.to_string())?;
    ReturnChannel::create(
        dir,
        Uuid::parse_str(&invocation.id).map_err(|e| e.to_string())?,
    )
    .map(Some)
}
// Standalone callers consume artifacts, not a transfer certificate. Their
// explicit Child lifecycle precedes this read; no lifecycle authority is minted.
pub(crate) fn read_and_cleanup_return_channel(
    channel: Option<ReturnChannel>,
) -> Result<Vec<ReturnedArtifactRef>, String> {
    let Some(mut channel) = channel else {
        return Ok(vec![]);
    };
    // Standalone finalization owns the existing producer-fence validation and
    // returned_artifacts error category. Carry structurally valid foreign refs
    // to that rejecting sink, but quarantine their channel: never bless or
    // delete them as accepted custody. Allocated seals never commit such refs.
    let settlement = channel.read_settled(|_| Ok(()), ChannelUse::StandaloneConsumption);
    match settlement {
        ReturnChannelSettlement::NotCreated | ReturnChannelSettlement::EmptyRemoved => Ok(vec![]),
        ReturnChannelSettlement::ArtifactsCommitted(refs) => Ok(refs),
        ReturnChannelSettlement::Quarantined {
            path, artifacts, ..
        }
        | ReturnChannelSettlement::CleanupFailed { path, artifacts } => {
            let error = format!(
                "return_channel_custody_uncertain;retained={}",
                path.display()
            );
            if artifacts.is_empty() {
                return Err(error);
            }
            // The unlinked caller must retain valid producing-invocation refs
            // even if cleanup failed. Nonempty effects are never empty custody;
            // this standalone boundary returns no transfer certificate.
            tracing::warn!(
                error,
                "Return channel retained artifacts with uncertain cleanup"
            );
            Ok(artifacts)
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    fn channel(root: &Path) -> ReturnChannel {
        ReturnChannel::for_attempt(
            root,
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::nil(),
        )
        .unwrap()
    }
    #[test]
    fn strict_blank_eof_removes_exact_file_and_directory() {
        for bytes in [b"".as_slice(), b" \n\t\r\n"] {
            let dir = tempfile::tempdir().unwrap();
            let mut channel = channel(dir.path());
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                channel
                    .file
                    .as_ref()
                    .unwrap()
                    .metadata()
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                channel
                    .directory
                    .as_ref()
                    .unwrap()
                    .metadata()
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            std::fs::write(channel.path(), bytes).unwrap();
            let path = channel.path.clone();
            let folder = channel.dir.clone();
            assert_eq!(
                channel.seal_settled(|_| panic!("empty must not commit artifacts")),
                ReturnChannelSettlement::EmptyRemoved
            );
            assert!(absent(&path) && absent(&folder));
        }
    }
    #[test]
    fn broadened_file_or_directory_permissions_cannot_certify_empty_custody() {
        use std::os::unix::fs::PermissionsExt;
        for directory in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut c = channel(dir.path());
            let target = if directory { &c.dir } else { &c.path };
            let mode = if directory { 0o755 } else { 0o644 };
            std::fs::set_permissions(target, std::fs::Permissions::from_mode(mode)).unwrap();
            assert!(matches!(
                c.seal_settled(|_| Ok(())),
                ReturnChannelSettlement::Quarantined { .. }
            ));
            assert!(c.path().exists());
        }
    }
    #[test]
    fn malformed_truncated_extra_and_unbounded_content_never_looks_empty() {
        for bytes in [
            b"{\n".to_vec(),
            b"{}".to_vec(),
            b"{}\n{}\n".to_vec(),
            vec![b' '; MAX_RETURN_CHANNEL_BYTES as usize + 1],
            vec![0xff],
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut channel = channel(dir.path());
            std::fs::write(channel.path(), bytes).unwrap();
            let path = channel.path.clone();
            assert!(matches!(
                channel.seal_settled(|_| Ok(())),
                ReturnChannelSettlement::Quarantined { .. }
            ));
            drop(channel);
            assert!(path.exists(), "quarantine must survive Drop");
        }
    }
    #[test]
    fn substitution_read_failure_directory_cleanup_failure_and_unsettled_actors_block() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = channel(dir.path());
        std::fs::remove_file(c.path()).unwrap();
        std::fs::write(c.path(), b"").unwrap();
        assert!(matches!(
            c.seal_settled(|_| Ok(())),
            ReturnChannelSettlement::Quarantined { .. }
        ));
        let mut c = channel(dir.path());
        c.file = Some(OpenOptions::new().write(true).open(c.path()).unwrap());
        assert!(matches!(
            c.seal_settled(|_| Ok(())),
            ReturnChannelSettlement::CleanupFailed { .. }
        ));
        let mut c = channel(dir.path());
        std::fs::write(c.dir.join("extra"), b"").unwrap();
        assert!(matches!(
            c.seal_settled(|_| Ok(())),
            ReturnChannelSettlement::CleanupFailed { .. }
        ));
        let c = channel(dir.path());
        let path = c.path.clone();
        assert!(matches!(
            c.seal(&[], |_| Ok(())),
            ReturnChannelSettlement::Quarantined { .. }
        ));
        assert!(path.exists());
        let c = channel(dir.path());
        let path = c.path.clone();
        drop(c);
        assert!(absent(&path)); // cleanup, no returned certificate
    }
    fn artifact(producer: Uuid) -> ReturnedArtifactRef {
        serde_json::from_value(serde_json::json!({
        "version_id":format!("store://return/{producer}/fixture/1"),"name":"fixture",
        "store_address":{"workflow_run_id":format!("return:{producer}"),"artifact_name":"fixture","version":1},
        "sha256":"a".repeat(64),"content_len":1,"format_hint":null,"verdict_line":null,
        "source":{"kind":"inline_bytes"},"producer_invocation_uuid":producer,"returned_at":"2026-09-07T00:00:00Z"
    })).unwrap()
    }
    #[test]
    fn foreign_producer_is_quarantined_and_only_standalone_passes_it_to_the_rejecting_sink() {
        let dir = tempfile::tempdir().unwrap();
        let mut allocated = channel(dir.path());
        let reference = artifact(Uuid::new_v4());
        let bytes = format!("{}\n", serde_json::to_string(&reference).unwrap());
        std::fs::write(allocated.path(), &bytes).unwrap();
        let result = allocated.seal_settled(|_| panic!("foreign ref cannot be committed"));
        assert!(matches!(
            result,
            ReturnChannelSettlement::Quarantined { .. }
        ));
        assert!(result.artifacts().is_empty());
        assert!(!result.transferable());
        assert!(allocated.path().exists());

        let standalone = channel(dir.path());
        let path = standalone.path().to_owned();
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(
            read_and_cleanup_return_channel(Some(standalone)).unwrap(),
            vec![reference]
        );
        assert!(
            path.exists(),
            "rejected-provenance evidence must remain quarantined"
        );
    }
    #[test]
    fn valid_artifacts_commit_to_producer_and_partial_records_are_retained_not_transferred() {
        for suffix in ["", "{\n"] {
            let dir = tempfile::tempdir().unwrap();
            let mut c = channel(dir.path());
            let reference = artifact(c.producer);
            std::fs::write(
                c.path(),
                format!("{}\n{suffix}", serde_json::to_string(&reference).unwrap()),
            )
            .unwrap();
            let mut committed = false;
            let result = c.seal_settled(|refs| {
                assert_eq!(refs, &[reference]);
                committed = true;
                Ok(())
            });
            assert!(committed);
            assert!(!result.transferable());
            assert_eq!(result.artifacts().len(), 1);
            assert_eq!(
                matches!(result, ReturnChannelSettlement::ArtifactsCommitted(_)),
                suffix.is_empty()
            );
        }
        let dir = tempfile::tempdir().unwrap();
        let mut c = channel(dir.path());
        let reference = artifact(c.producer);
        std::fs::write(
            c.path(),
            format!("{}\n", serde_json::to_string(&reference).unwrap()),
        )
        .unwrap();
        assert!(matches!(
            c.seal_settled(|_| Err("injected commit failure".into())),
            ReturnChannelSettlement::CleanupFailed { .. }
        ));
        let mut c = channel(dir.path());
        let mut extra = serde_json::to_value(artifact(c.producer)).unwrap();
        extra["extra"] = true.into();
        std::fs::write(c.path(), format!("{extra}\n")).unwrap();
        assert!(matches!(
            c.seal_settled(|_| panic!("extra is not accepted")),
            ReturnChannelSettlement::Quarantined { .. }
        ));
    }
    fn messenger_receipt(
        reference: &ReturnedArtifactRef,
    ) -> oulipoly_agent_messenger::ReturnedArtifact {
        let mut value = serde_json::to_value(reference).unwrap();
        value["schema_version"] = 1.into();
        serde_json::from_value(value).unwrap()
    }
    #[test]
    fn messenger_wire_receipts_commit_and_retain_prefix_without_weakening_custody() {
        for suffix in ["", "{\n"] {
            let dir = tempfile::tempdir().unwrap();
            let mut c = channel(dir.path());
            let reference = artifact(c.producer);
            oulipoly_agent_messenger::append_return_channel(
                c.path(),
                &messenger_receipt(&reference),
            )
            .unwrap();
            use std::io::Write;
            OpenOptions::new()
                .append(true)
                .open(c.path())
                .unwrap()
                .write_all(suffix.as_bytes())
                .unwrap();
            let mut committed = false;
            let result = c.seal_settled(|refs| {
                assert_eq!(refs, &[reference]);
                committed = true;
                Ok(())
            });
            assert!(
                committed,
                "messenger's actual schema-v1 wire must be retained"
            );
            assert_eq!(result.artifacts().len(), 1);
            assert!(!result.transferable());
            assert_eq!(
                matches!(result, ReturnChannelSettlement::ArtifactsCommitted(_)),
                suffix.is_empty()
            );
        }
    }
    #[test]
    fn versioned_receipt_rejects_unknown_versions_fields_and_duplicate_keys() {
        let reference = artifact(Uuid::nil());
        let wire = serde_json::to_value(messenger_receipt(&reference)).unwrap();
        let mut unknown_version = wire.clone();
        unknown_version["schema_version"] = 2.into();
        let mut null_version = wire.clone();
        null_version["schema_version"] = serde_json::Value::Null;
        let mut extra = wire.clone();
        extra["unexpected"] = true.into();
        let mut nested_extra = wire.clone();
        nested_extra["store_address"]["unexpected"] = true.into();
        let duplicate_version = wire.to_string().replacen(
            "\"schema_version\":1",
            "\"schema_version\":1,\"schema_version\":1",
            1,
        );
        let duplicate_name = wire.to_string().replacen(
            "\"name\":\"fixture\"",
            "\"name\":\"fixture\",\"name\":\"fixture\"",
            1,
        );
        for bytes in [
            unknown_version.to_string(),
            null_version.to_string(),
            extra.to_string(),
            nested_extra.to_string(),
            duplicate_version,
            duplicate_name,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut c = channel(dir.path());
            std::fs::write(c.path(), format!("{bytes}\n")).unwrap();
            let result = c.seal_settled(|_| panic!("invalid wire must not commit"));
            assert!(
                matches!(result, ReturnChannelSettlement::Quarantined { .. }),
                "{bytes}"
            );
            assert!(result.artifacts().is_empty());
            assert!(!result.transferable());
            assert!(c.path().exists());
        }
    }
    #[test]
    fn messenger_wire_standalone_retains_refs_and_allocated_foreign_stays_quarantined() {
        let dir = tempfile::tempdir().unwrap();
        let standalone = channel(dir.path());
        let reference = artifact(standalone.producer);
        oulipoly_agent_messenger::append_return_channel(
            standalone.path(),
            &messenger_receipt(&reference),
        )
        .unwrap();
        assert_eq!(
            read_and_cleanup_return_channel(Some(standalone)).unwrap(),
            vec![reference]
        );

        let mut allocated = channel(dir.path());
        let foreign = artifact(Uuid::new_v4());
        oulipoly_agent_messenger::append_return_channel(
            allocated.path(),
            &messenger_receipt(&foreign),
        )
        .unwrap();
        let result = allocated.seal_settled(|_| panic!("foreign wire cannot commit"));
        assert!(matches!(
            result,
            ReturnChannelSettlement::Quarantined { .. }
        ));
        assert!(result.artifacts().is_empty());
        assert!(!result.transferable());
        assert!(allocated.path().exists());
    }
    #[test]
    fn deterministic_attempt_identity_is_exclusive() {
        let root = tempfile::tempdir().unwrap();
        let parent = Uuid::new_v4();
        let logical = Uuid::new_v4();
        let attempt = Uuid::new_v4();
        let c =
            ReturnChannel::for_attempt(root.path(), parent, logical, attempt, Uuid::nil()).unwrap();
        assert_eq!(
            c.path(),
            root.path()
                .join(parent.to_string())
                .join(logical.to_string())
                .join(attempt.to_string())
                .join("returns.jsonl")
        );
        assert!(
            ReturnChannel::for_attempt(root.path(), parent, logical, attempt, Uuid::nil()).is_err()
        );
        let second =
            ReturnChannel::for_attempt(root.path(), parent, logical, Uuid::new_v4(), Uuid::nil())
                .unwrap();
        assert_ne!(c.path(), second.path());
    }
}
