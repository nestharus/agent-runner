//! Original-location v2 Bash candidate evidence. Only a retained broker
//! source grant selects these bytes; a pathname alone carries no authority.

const STAMP_BUFFER_BYTES: usize = 64 * 1024;

use oulipoly_state::completion_continuation::{
    MAX_REGISTRATION_BYTES, SourceRegistration, open_source_file, sha256,
};
use oulipoly_state::mailbox::BrokerSourceMaterial;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const IMAGE_LIMIT: usize = 256 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    device: u64,
    inode: u64,
    length: u64,
    sha256: String,
}

fn stamp(file: &mut File, limit: usize, owner_uid: u32) -> Result<FileStamp, String> {
    let before = file.metadata().map_err(|e| e.to_string())?;
    if !before.is_file() || before.uid() != owner_uid || before.len() > limit as u64 {
        return Err("source candidate is not a bounded regular file".into());
    }
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut digest = Sha256::new();
    let mut count = 0u64;
    let mut buffer = [0u8; STAMP_BUFFER_BYTES];
    loop {
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        count = count
            .checked_add(n as u64)
            .ok_or("candidate length overflow")?;
        if count > limit as u64 {
            return Err("source candidate grew beyond bound".into());
        }
        digest.update(&buffer[..n]);
    }
    let after = file.metadata().map_err(|e| e.to_string())?;
    if (before.dev(), before.ino(), before.len()) != (after.dev(), after.ino(), after.len())
        || count != after.len()
    {
        return Err("source candidate changed during read".into());
    }
    Ok(FileStamp {
        device: after.dev(),
        inode: after.ino(),
        length: count,
        sha256: format!("{:x}", digest.finalize()),
    })
}

fn open_pinned(
    directory: &Path,
    relative: &str,
    limit: usize,
    owner_uid: u32,
) -> Result<(File, FileStamp), String> {
    let mut file = open_source_file(directory, relative, limit)?;
    let checked = stamp(&mut file, limit, owner_uid)?;
    Ok((file, checked))
}

/// Held across the one-use transition and rechecked against the original
/// names immediately before worker release. The image FD remains pinned for
/// exec through `/proc/self/fd`; Bash still receives the original registration
/// pathname and original environment values.
pub struct SourcePinnedCandidate {
    pub source: SourceRegistration,
    pub environment: BTreeMap<String, String>,
    pub image: File,
    pub registration_path: PathBuf,
    directory: PathBuf,
    registration: FileStamp,
    environment_stamp: FileStamp,
    image_stamp: FileStamp,
    admitted_registration: Vec<u8>,
    image_relative: String,
    owner_uid: u32,
    directory_device: u64,
    directory_inode: u64,
}

impl SourcePinnedCandidate {
    pub fn pin(material: &BrokerSourceMaterial, owner_uid: u32) -> Result<Self, String> {
        if material.grant.phase != "reserved"
            || material.grant.revision != 1
            || sha256(&material.registration_bytes) != material.grant.candidate.registration_digest
        {
            return Err("candidate lacks exact reserved grant".into());
        }
        let source: SourceRegistration =
            serde_json::from_slice(&material.registration_bytes).map_err(|e| e.to_string())?;
        source.validate()?;
        if source.registration_id != material.grant.candidate.registration_id
            || source.listener_revision != material.grant.candidate.listener_revision
        {
            return Err("source candidate admission identity changed".into());
        }
        let directory = PathBuf::from(&source.handle_dir);
        let directory_meta = std::fs::metadata(&directory).map_err(|e| e.to_string())?;
        if directory_meta.uid() != owner_uid || !directory_meta.is_dir() {
            return Err("original source directory owner changed".into());
        }
        let (mut registration_file, registration) = open_pinned(
            &directory,
            &source.registration_relative,
            MAX_REGISTRATION_BYTES,
            owner_uid,
        )?;
        registration_file
            .seek(SeekFrom::Start(0))
            .map_err(|e| e.to_string())?;
        let mut actual = Vec::new();
        registration_file
            .read_to_end(&mut actual)
            .map_err(|e| e.to_string())?;
        if actual != material.registration_bytes {
            return Err("original registration bytes differ from State admission".into());
        }
        if stamp(&mut registration_file, MAX_REGISTRATION_BYTES, owner_uid)? != registration {
            return Err("original registration changed during read".into());
        }
        let (mut environment_file, environment_stamp) = open_pinned(
            &directory,
            "delivery-helper-environment.json",
            MAX_REGISTRATION_BYTES,
            owner_uid,
        )?;
        if environment_stamp.sha256 != source.recovery.environment_sha256 {
            return Err("original recovery environment digest changed".into());
        }
        environment_file
            .seek(SeekFrom::Start(0))
            .map_err(|e| e.to_string())?;
        let environment: BTreeMap<String, String> =
            serde_json::from_reader(&mut environment_file).map_err(|e| e.to_string())?;
        if stamp(&mut environment_file, MAX_REGISTRATION_BYTES, owner_uid)? != environment_stamp {
            return Err("original environment changed during read".into());
        }
        let image_relative = Path::new(&source.recovery.path)
            .strip_prefix(&directory)
            .map_err(|e| e.to_string())?
            .to_str()
            .ok_or("non-UTF8 recovery path")?
            .to_owned();
        let (image, image_stamp) =
            open_pinned(&directory, &image_relative, IMAGE_LIMIT, owner_uid)?;
        if image_stamp.sha256 != source.recovery.sha256 {
            return Err("original recovery image digest changed".into());
        }
        let registration_path = directory.join(&source.registration_relative);
        Ok(Self {
            source,
            environment,
            image,
            registration_path,
            directory,
            registration,
            environment_stamp,
            image_stamp,
            admitted_registration: material.registration_bytes.clone(),
            image_relative,
            owner_uid,
            directory_device: directory_meta.dev(),
            directory_inode: directory_meta.ino(),
        })
    }

    pub fn verify_at_use(&mut self) -> Result<(), String> {
        let directory = std::fs::symlink_metadata(&self.directory).map_err(|e| e.to_string())?;
        if !directory.is_dir()
            || directory.file_type().is_symlink()
            || directory.uid() != self.owner_uid
            || (directory.dev(), directory.ino()) != (self.directory_device, self.directory_inode)
        {
            return Err("original source directory changed before use".into());
        }
        let (mut registration, stamp_now) = open_pinned(
            &self.directory,
            &self.source.registration_relative,
            MAX_REGISTRATION_BYTES,
            self.owner_uid,
        )?;
        if stamp_now != self.registration {
            return Err("original registration swapped or edited before use".into());
        }
        registration
            .seek(SeekFrom::Start(0))
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        registration
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes != self.admitted_registration {
            return Err("original registration bytes changed before use".into());
        }
        if stamp(&mut registration, MAX_REGISTRATION_BYTES, self.owner_uid)? != self.registration {
            return Err("original registration changed during use".into());
        }
        let (_, environment) = open_pinned(
            &self.directory,
            "delivery-helper-environment.json",
            MAX_REGISTRATION_BYTES,
            self.owner_uid,
        )?;
        if environment != self.environment_stamp {
            return Err("original environment swapped or edited before use".into());
        }
        let (_, image) = open_pinned(
            &self.directory,
            &self.image_relative,
            IMAGE_LIMIT,
            self.owner_uid,
        )?;
        if image != self.image_stamp
            || stamp(&mut self.image, IMAGE_LIMIT, self.owner_uid)? != self.image_stamp
        {
            return Err("original recovery image swapped or edited before use".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_state::completion_continuation::SourceProcessIdentity;
    use oulipoly_state::mailbox::{BrokerSourceCandidate, BrokerSourceEffectGrant};
    use std::fs;

    #[test]
    fn original_candidate_rechecks_names_inodes_and_same_inode_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("handle");
        fs::create_dir(&directory).unwrap();
        let image_path = directory.join("agent-bash");
        fs::write(&image_path, b"exact built image fixture").unwrap();
        let env_path = directory.join("delivery-helper-environment.json");
        fs::write(&env_path, b"{\"PATH\":\"/usr/bin\"}").unwrap();
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../oulipoly-state/tests/fixtures/age360-paired-wire.json"
        ))
        .unwrap();
        let mut source: SourceRegistration =
            serde_json::from_value(fixture["registration"].clone()).unwrap();
        source.spool_root = temp.path().to_str().unwrap().into();
        source.handle = "handle".into();
        source.handle_dir = directory.to_str().unwrap().into();
        source.helper.path = directory.join("runner").to_str().unwrap().into();
        source.recovery.path = image_path.to_str().unwrap().into();
        source.recovery.sha256 = sha256(b"exact built image fixture");
        source.recovery.environment_sha256 = sha256(b"{\"PATH\":\"/usr/bin\"}");
        let registration_path = directory.join(&source.registration_relative);
        let registration_bytes = serde_json::to_vec(&source).unwrap();
        fs::write(&registration_path, &registration_bytes).unwrap();
        let material = BrokerSourceMaterial {
            grant: BrokerSourceEffectGrant {
                grant_id: uuid::Uuid::new_v4().to_string(),
                source_generation: uuid::Uuid::new_v4().to_string(),
                root_id: uuid::Uuid::new_v4().to_string(),
                owner_generation: uuid::Uuid::new_v4().to_string(),
                driver_identity: SourceProcessIdentity {
                    pid: 1,
                    boot_id: "boot".into(),
                    starttime_ticks: 1,
                },
                authority_ordinal: 1,
                candidate: BrokerSourceCandidate {
                    registration_id: source.registration_id.clone(),
                    registration_digest: sha256(&registration_bytes),
                    listener_revision: source.listener_revision,
                    listener: source.listeners[0].clone(),
                },
                phase: "reserved".into(),
                revision: 1,
            },
            registration_bytes: registration_bytes.clone(),
        };
        let owner_uid = unsafe { libc::geteuid() };
        let mut pinned = SourcePinnedCandidate::pin(&material, owner_uid).unwrap();
        pinned.verify_at_use().unwrap();
        fs::write(&registration_path, b"same inode changed").unwrap();
        assert!(pinned.verify_at_use().is_err());
        fs::write(&registration_path, &registration_bytes).unwrap();
        pinned = SourcePinnedCandidate::pin(&material, owner_uid).unwrap();
        fs::rename(&registration_path, directory.join("old-registration")).unwrap();
        fs::write(&registration_path, &registration_bytes).unwrap();
        assert!(pinned.verify_at_use().is_err());
        pinned = SourcePinnedCandidate::pin(&material, owner_uid).unwrap();
        fs::write(&env_path, b"{\"PATH\":\"/other\"}").unwrap();
        assert!(pinned.verify_at_use().is_err());
        fs::write(&env_path, b"{\"PATH\":\"/usr/bin\"}").unwrap();
        pinned = SourcePinnedCandidate::pin(&material, owner_uid).unwrap();
        fs::rename(&env_path, directory.join("old-environment")).unwrap();
        fs::write(&env_path, b"{\"PATH\":\"/usr/bin\"}").unwrap();
        assert!(pinned.verify_at_use().is_err());
        pinned = SourcePinnedCandidate::pin(&material, owner_uid).unwrap();
        fs::write(&image_path, b"mutated original image").unwrap();
        assert!(pinned.verify_at_use().is_err());
        fs::write(&image_path, b"exact built image fixture").unwrap();
        pinned = SourcePinnedCandidate::pin(&material, owner_uid).unwrap();
        fs::rename(&image_path, directory.join("old-image")).unwrap();
        fs::write(&image_path, b"exact built image fixture").unwrap();
        assert!(pinned.verify_at_use().is_err());
    }
}
