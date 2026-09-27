//! Immutable old-authority receipt for a released child. An interrupted write
//! poisons new handoff minting while leaving the old service able to settle
//! v29 debt. No file in this directory is a caller-supplied selector.
use oulipoly_state::mailbox::FreshReleasedHandoff;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

pub struct ReleasedHandoffRegistry {
    directory: PathBuf,
    rows: Vec<FreshReleasedHandoff>,
    uncertain: bool,
}

impl ReleasedHandoffRegistry {
    pub fn open(directory: &Path) -> io::Result<Self> {
        let meta = fs::symlink_metadata(directory)?;
        if !meta.is_dir()
            || meta.file_type().is_symlink()
            || meta.uid() != 0
            || meta.mode() & 0o777 != 0o700
        {
            return Err(io::Error::other("unsafe released handoff directory"));
        }
        let mut result = Self {
            directory: directory.into(),
            rows: Vec::new(),
            uncertain: false,
        };
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let file = entry.path();
            let meta = fs::symlink_metadata(&file)?;
            if !name.ends_with(".json")
                || !meta.is_file()
                || meta.file_type().is_symlink()
                || meta.uid() != 0
                || meta.mode() & 0o777 != 0o600
                || meta.nlink() != 1
            {
                result.uncertain = true;
                continue;
            }
            let Ok(bytes) = fs::read(&file) else {
                result.uncertain = true;
                continue;
            };
            let Ok(row) = serde_json::from_slice::<FreshReleasedHandoff>(&bytes) else {
                result.uncertain = true;
                continue;
            };
            if name != format!("{}.json", row.old_release.prepared.root_id)
                || result.rows.iter().any(|previous| {
                    previous.old_release.prepared.joined_child
                        == row.old_release.prepared.joined_child
                        || previous.d_key == row.d_key
                        || previous.invocation_uuid == row.invocation_uuid
                })
            {
                result.uncertain = true;
                continue;
            }
            result.rows.push(row);
        }
        Ok(result)
    }

    pub fn existing(&self, root_id: &str) -> Option<&FreshReleasedHandoff> {
        self.rows
            .iter()
            .find(|row| row.old_release.prepared.root_id == root_id)
    }

    pub fn is_uncertain(&self) -> bool {
        self.uncertain
    }

    pub fn persist(&mut self, row: FreshReleasedHandoff) -> io::Result<FreshReleasedHandoff> {
        if self.uncertain || self.existing(&row.old_release.prepared.root_id).is_some() {
            return Err(io::Error::other(
                "released handoff already spent or uncertain",
            ));
        }
        let path = self
            .directory
            .join(format!("{}.json", row.old_release.prepared.root_id));
        // Once create_new succeeds, any later error retains unknown debt. An
        // exact retry can only recover after reopening and validating the row.
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) => {
                self.uncertain = true;
                return Err(error);
            }
        };
        let result = (|| {
            #[cfg(feature = "age319-private-broker-fixture")]
            if std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_FAIL_HANDOFF_SYNC_V1").is_some()
                && unsafe { libc::geteuid() } == 0
                && fs::read_to_string("/proc/self/uid_map")
                    .ok()
                    .is_some_and(|map| map.split_ascii_whitespace().nth(2) == Some("1"))
            {
                file.write_all(b"{")?;
                return Err(io::Error::other("private interrupted handoff before fsync"));
            }
            serde_json::to_writer(&mut file, &row)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            File::open(&self.directory)?.sync_all()?;
            let read: FreshReleasedHandoff = serde_json::from_slice(&fs::read(&path)?)?;
            if read != row {
                return Err(io::Error::other("released handoff readback changed"));
            }
            Ok(read)
        })();
        match result {
            Ok(read) => {
                self.rows.push(read.clone());
                Ok(read)
            }
            Err(error) => {
                self.uncertain = true;
                Err(error)
            }
        }
    }
}
