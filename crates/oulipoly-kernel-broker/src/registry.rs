use crate::identity::{ChildExit, PinnedProcess, boot_id};
use crate::json_artifact;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
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
            // Fresh storage is a distinct authority. Only its exact published
            // directory and the initializer's exact abandoned staging names
            // coexist with old root records. Never descend into either here.
            if name == "v30" || is_fresh_staging_name(&name) {
                let meta = fs::symlink_metadata(entry.path())?;
                if !meta.file_type().is_dir() || meta.uid() != 0 || meta.mode() & 0o777 != 0o700 {
                    return Err(io::Error::other("unsafe fresh lane directory"));
                }
                continue;
            }
            // The work registry is opened separately by the broker before it
            // serves requests. Never silently skip an arbitrary directory.
            if name == "works" && entry.file_type()?.is_dir() {
                continue;
            }
            if name == "entries" && entry.file_type()?.is_dir() {
                continue;
            }
            if name == "released-handoffs" {
                let meta = fs::symlink_metadata(entry.path())?;
                if !meta.is_dir()
                    || meta.file_type().is_symlink()
                    || meta.uid() != 0
                    || meta.mode() & 0o777 != 0o700
                {
                    return Err(io::Error::other("unsafe released handoff directory"));
                }
                continue;
            }
            if name == "grants" && entry.file_type()?.is_dir() {
                continue;
            }
            if name == "terminals" && entry.file_type()?.is_dir() {
                continue;
            }
            #[cfg(feature = "age319-private-broker-fixture")]
            if name == "private-launches" && entry.file_type()?.is_dir() {
                continue;
            }
            // SourcePhysicalRegistry opens and validates this fixed broker
            // directory before any post-owner readback.
            if name == "source-physical" && entry.file_type()?.is_dir() {
                continue;
            }
            // serve() has already opened and validated this fixed root-only
            // storage before it opens the root registry.
            if name == "sidecar" && entry.file_type()?.is_dir() {
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
            if name.starts_with(".json-pending-") {
                return Err(json_artifact::pending_error(
                    &entry.path(),
                    "root_registry_open",
                ));
            }
            if !name.ends_with(".json") || !entry.file_type()?.is_file() {
                return Err(io::Error::other("unrecognized registry entry"));
            }
            let record: RootRecord = json_artifact::read(&entry.path(), "root_registry_open")?;
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

    /// Observe only the exact PID1 child pinned by this broker incarnation.
    /// Zero exit is a physical process observation, not proof that ECHILD was
    /// reached, that accepted work or effects settled, or that an owner closed.
    /// A restart cannot reconstruct the parent/child wait status from a PID.
    pub fn observe_init_exit(&self, expected: &RootRecord) -> io::Result<ChildExit> {
        if self.poisoned || !self.debt.is_empty() {
            return Err(io::Error::other("uncertain root registry"));
        }
        let root = self
            .live
            .iter()
            .find(|root| root.record.root_id == expected.root_id)
            .ok_or_else(|| io::Error::other("exact live root absent"))?;
        if root.record != *expected {
            return Err(io::Error::other("root PID1 incarnation changed"));
        }
        root.init.peek_child_exit()
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
        let persisted = json_artifact::create_new(
            &self.directory,
            &format!("{}.json", record.root_id),
            &record,
        );
        if let Err(error) = persisted {
            // A complete final name or an unresolved private temporary may
            // exist. Reopen exact custody before admitting more roots.
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

fn is_fresh_staging_name(name: &str) -> bool {
    let Some(suffix) = name.strip_prefix(".v30-fresh-") else {
        return false;
    };
    suffix.len() == 32
        && uuid::Uuid::parse_str(suffix)
            .is_ok_and(|id| id.get_version_num() == 4 && id.simple().to_string() == suffix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::ChildExit;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    fn gated_child() -> (PinnedProcess, UnixStream) {
        let (parent, mut child_gate) = UnixStream::pair().unwrap();
        let pid = unsafe { libc::fork() };
        if pid == 0 {
            drop(parent);
            let mut release = [0u8; 1];
            let code = if child_gate.read_exact(&mut release).is_ok() {
                0
            } else {
                70
            };
            unsafe { libc::_exit(code) }
        }
        assert!(pid > 0);
        drop(child_gate);
        (PinnedProcess::open(pid).unwrap(), parent)
    }

    fn record(id: &str, init: &PinnedProcess) -> RootRecord {
        RootRecord {
            version: 1,
            boot_id: init.boot_id.clone(),
            root_id: id.into(),
            owner_uid: unsafe { libc::getuid() },
            init_host_pid: init.host_pid,
            init_starttime_ticks: init.starttime_ticks,
            pidns_dev: init.pidns_dev,
            pidns_ino: init.pidns_ino,
        }
    }

    fn await_exit(registry: &RootRegistry, root: &RootRecord) -> ChildExit {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let status = registry.observe_init_exit(root).unwrap();
            if status != ChildExit::Running {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "child exit observation timed out"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn exact_init_exit_readback_distinguishes_other_root_crash_and_restart() {
        let temp = tempfile::tempdir().unwrap();
        let (first_init, mut first_gate) = gated_child();
        let (second_init, second_gate) = gated_child();
        let first = record(&uuid::Uuid::new_v4().to_string(), &first_init);
        let second = record(&uuid::Uuid::new_v4().to_string(), &second_init);
        let registry = RootRegistry {
            directory: temp.path().into(),
            live: vec![
                LiveRoot {
                    record: first.clone(),
                    init: first_init,
                },
                LiveRoot {
                    record: second.clone(),
                    init: second_init,
                },
            ],
            debt: Vec::new(),
            poisoned: false,
        };
        assert_eq!(
            registry.observe_init_exit(&first).unwrap(),
            ChildExit::Running
        );
        assert_eq!(
            registry.observe_init_exit(&second).unwrap(),
            ChildExit::Running
        );
        let mut wrong_incarnation = first.clone();
        wrong_incarnation.init_starttime_ticks += 1;
        assert!(registry.observe_init_exit(&wrong_incarnation).is_err());
        let mut unknown_root = first.clone();
        unknown_root.root_id = uuid::Uuid::new_v4().to_string();
        assert!(registry.observe_init_exit(&unknown_root).is_err());

        first_gate.write_all(b"go").unwrap();
        assert_eq!(await_exit(&registry, &first), ChildExit::ExitedZero);
        assert_eq!(
            registry.observe_init_exit(&second).unwrap(),
            ChildExit::Running
        );
        assert_eq!(
            unsafe { libc::kill(second.init_host_pid, libc::SIGKILL) },
            0
        );
        assert_eq!(await_exit(&registry, &second), ChildExit::ExitedAbnormally);
        drop(second_gate);

        // WNOWAIT preserved both statuses until this broker reaps them.
        for pid in [first.init_host_pid, second.init_host_pid] {
            let mut status = 0;
            assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
        }
        assert!(registry.observe_init_exit(&first).is_err());
        fs::write(
            temp.path().join(format!("{}.json", first.root_id)),
            serde_json::to_vec(&first).unwrap(),
        )
        .unwrap();
        let restarted = RootRegistry::open(temp.path()).unwrap();
        assert_eq!(restarted.debt_records().len(), 1);
        assert!(restarted.observe_init_exit(&first).is_err());
    }
}
