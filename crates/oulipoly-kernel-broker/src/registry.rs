use crate::identity::{PinnedProcess, boot_id};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RootRecord {
    pub version: u32,
    pub boot_id: String,
    pub root_id: String,
    pub owner_uid: u32,
    pub init_host_pid: i32,
    pub init_starttime_ticks: u64,
    pub pidns_dev: u64,
    pub pidns_ino: u64,
}

#[derive(Debug)]
pub struct LiveRoot {
    pub record: RootRecord,
    pub init: PinnedProcess,
}

#[derive(Debug)]
pub struct RootRegistry {
    directory: PathBuf,
    live: Vec<LiveRoot>,
    debt: Vec<RootRecord>,
    poisoned: bool,
}

fn reattach(record: RootRecord) -> Result<LiveRoot, RootRecord> {
    if record.version != 1 || boot_id().ok().as_deref() != Some(record.boot_id.as_str()) {
        return Err(record);
    }
    let Ok(init) = PinnedProcess::open(record.init_host_pid) else {
        return Err(record);
    };
    if init.boot_id != record.boot_id
        || init.starttime_ticks != record.init_starttime_ticks
        || init.pidns_dev != record.pidns_dev
        || init.pidns_ino != record.pidns_ino
        || !matches!(init.is_namespace_init(), Ok(true))
    {
        return Err(record);
    }
    Ok(LiveRoot { record, init })
}

impl RootRegistry {
    pub fn open(directory: impl AsRef<Path>) -> io::Result<Self> {
        let directory = directory.as_ref().to_path_buf();
        let mut registry = Self {
            directory,
            live: Vec::new(),
            debt: Vec::new(),
            poisoned: false,
        };
        for entry in fs::read_dir(&registry.directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // The work registry is opened separately by the broker before it
            // serves requests. Never silently skip an arbitrary directory.
            if name == "works" && entry.file_type()?.is_dir() {
                continue;
            }
            if name == "entries" && entry.file_type()?.is_dir() {
                continue;
            }
            if name == "grants" && entry.file_type()?.is_dir() {
                continue;
            }
            if name == "terminals" && entry.file_type()?.is_dir() {
                continue;
            }
            // serve() has already opened and validated this fixed root-only
            // storage before it opens the root registry.
            if name == "sidecar" && entry.file_type()?.is_dir() {
                continue;
            }
            // An interrupted prepublication snapshot is inert. Keep the
            // broker available for an explicit gate abort while the fixed
            // sidecar name is absent; never treat this stage as State authority.
            if let Some(id) = name.strip_prefix("sidecar-stage-")
                && uuid::Uuid::parse_str(id).is_ok_and(|parsed| parsed.to_string() == id)
                && entry.file_type()?.is_dir()
            {
                let metadata = fs::symlink_metadata(entry.path())?;
                if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
                    return Err(io::Error::other("unsafe inert sidecar stage"));
                }
                continue;
            }
            // EntryGate::open validated these exact files and holds the
            // singleton lock before this registry scan. Unknown files still
            // stop recovery rather than being mistaken for root records.
            if matches!(name.as_ref(), "entry-gate.lock" | "entry-gate.v1")
                && entry.file_type()?.is_file()
            {
                continue;
            }
            if !name.ends_with(".json") || !entry.file_type()?.is_file() {
                return Err(io::Error::other("unrecognized registry entry"));
            }
            let record: RootRecord = serde_json::from_slice(&fs::read(entry.path())?)?;
            if format!("{}.json", record.root_id) != name
                || uuid::Uuid::parse_str(&record.root_id).is_err()
            {
                return Err(io::Error::other("registry filename/ID mismatch"));
            }
            match reattach(record) {
                Ok(root) => registry.live.push(root),
                Err(record) => registry.debt.push(record),
            }
        }
        registry.check_unique()?;
        Ok(registry)
    }

    fn check_unique(&self) -> io::Result<()> {
        let mut ids = std::collections::HashSet::new();
        let mut namespaces = std::collections::HashSet::new();
        for root in &self.live {
            if !ids.insert(&root.record.root_id)
                || !namespaces.insert((root.record.pidns_dev, root.record.pidns_ino))
            {
                return Err(io::Error::other("duplicate root ID or live PID namespace"));
            }
        }
        for root in &self.debt {
            if !ids.insert(&root.root_id) {
                return Err(io::Error::other("duplicate root ID"));
            }
        }
        Ok(())
    }

    pub fn has_debt(&self) -> bool {
        self.poisoned || !self.debt.is_empty() || self.live.iter().any(|r| r.init.verify().is_err())
    }

    pub fn live_roots(&self) -> impl Iterator<Item = &LiveRoot> {
        self.live.iter()
    }

    pub fn insert(&mut self, record: RootRecord) -> io::Result<()> {
        if self.has_debt() {
            return Err(io::Error::other("uncertain root debt"));
        }
        let root =
            reattach(record.clone()).map_err(|_| io::Error::other("init identity changed"))?;
        if self.live.iter().any(|r| {
            r.record.root_id == record.root_id
                || (r.record.pidns_dev, r.record.pidns_ino) == (record.pidns_dev, record.pidns_ino)
        }) {
            return Err(io::Error::other("duplicate live root"));
        }
        let path = self.directory.join(format!("{}.json", record.root_id));
        let persisted = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)?;
            serde_json::to_writer(&mut file, &record)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            File::open(&self.directory)?.sync_all()
        })();
        if let Err(error) = persisted {
            // A partial or merely unsynced record may be on disk. Do not
            // continue from the old in-memory view after this ambiguity.
            self.poisoned = true;
            return Err(error);
        }
        self.live.push(root);
        Ok(())
    }

    /// No automatic retirement. A failed or vanished root remains recorded and
    /// blocks new outside launches until a future settlement protocol handles it.
    pub fn debt_records(&self) -> &[RootRecord] {
        &self.debt
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cutover_gate::EntryGate;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn prepublication_stage_is_inert_and_gate_can_explicitly_rollback_after_restart() {
        let directory = tempfile::tempdir().unwrap();
        let mut gate = EntryGate::open(directory.path()).unwrap();
        gate.close().unwrap();
        drop(gate);
        let stage = directory
            .path()
            .join(format!("sidecar-stage-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&stage).unwrap();
        fs::set_permissions(&stage, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(RootRegistry::open(directory.path()).is_ok());
        let mut restarted = EntryGate::open(directory.path()).unwrap();
        assert!(restarted.is_closed());
        restarted.abort_before_publication().unwrap();
        assert!(!restarted.is_closed());
        drop(restarted);
        fs::set_permissions(&stage, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(RootRegistry::open(directory.path()).is_err());
    }

    #[test]
    fn malformed_stage_name_does_not_hide_unknown_registry_entry() {
        let directory = tempfile::tempdir().unwrap();
        let stage = directory.path().join("sidecar-stage-not-a-uuid");
        fs::create_dir(&stage).unwrap();
        assert!(RootRegistry::open(directory.path()).is_err());
    }
}
