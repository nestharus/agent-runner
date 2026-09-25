//! Durable, one-use host entry reservation. A reservation is not permission to
//! launch a Runner: the guardian must bind it before any future child gate can
//! be opened. Records remain debt until exact State terminal/publication settlement.
use crate::identity::{PinnedProcess, boot_id};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProcessStamp {
    pub host_pid: i32,
    pub boot_id: String,
    pub starttime_ticks: u64,
    pub pidns_dev: u64,
    pub pidns_ino: u64,
}

impl From<&PinnedProcess> for ProcessStamp {
    fn from(process: &PinnedProcess) -> Self {
        Self {
            host_pid: process.host_pid,
            boot_id: process.boot_id.clone(),
            starttime_ticks: process.starttime_ticks,
            pidns_dev: process.pidns_dev,
            pidns_ino: process.pidns_ino,
        }
    }
}

impl ProcessStamp {
    fn matches(&self, process: &PinnedProcess) -> io::Result<bool> {
        process.verify()?;
        Ok(self == &Self::from(process))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EntryRecord {
    pub version: u32,
    pub root_id: String,
    pub owner_uid: u32,
    pub entry: ProcessStamp,
    pub prepared_guardian: Option<ProcessStamp>,
    pub domain_id: Option<String>,
    // Older unbound reservations remain readable; an older bound record with
    // no supervisor binding fails validation instead of acquiring authority.
    #[serde(default)]
    pub supervisor_authority_id: Option<String>,
    pub guardian: Option<ProcessStamp>,
    /// Set durably before attempting a root fork. An uncertain response or
    /// broker restart can never issue a second child for this entry.
    #[serde(default)]
    pub join_consumed: bool,
    /// Bound after the broker pins the direct child of root PID1 and before
    /// its pre-exec gate opens. Older spent joins lack this witness and cannot
    /// use host owner verification after restart.
    #[serde(default)]
    pub joined_child: Option<ProcessStamp>,
    /// Bound once after held J and before v30 prepared State publication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepared_driver: Option<ProcessStamp>,
    /// The exact State terminal/publication identity that released this
    /// one-use entry. An unknown caller presentation remains in State.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_settlement: Option<EntryTerminalSettlement>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EntryTerminalSettlement {
    pub d_key: String,
    pub handoff_id: String,
    pub invocation_uuid: String,
    pub session_id: String,
    pub actor: ProcessStamp,
    pub parent_grant_id: String,
    pub publication_sha256: String,
}

pub struct EntryRegistry {
    directory: PathBuf,
    records: Vec<EntryRecord>,
    poisoned: bool,
}

impl EntryRegistry {
    pub fn open(directory: impl AsRef<Path>) -> io::Result<Self> {
        let directory = directory.as_ref().to_path_buf();
        let mut records = Vec::new();
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !entry.file_type()?.is_file() || !name.ends_with(".json") {
                return Err(io::Error::other("unrecognized entry grant record"));
            }
            let record: EntryRecord = serde_json::from_slice(&fs::read(entry.path())?)?;
            if record.version != 1
                || format!("{}.json", record.root_id) != name
                || uuid::Uuid::parse_str(&record.root_id).is_err()
                || record.domain_id.is_some() != record.guardian.is_some()
                || record.supervisor_authority_id.is_some() != record.guardian.is_some()
                || record.guardian.is_some() && record.prepared_guardian != record.guardian
                || record.join_consumed && record.guardian.is_none()
                || record.joined_child.is_some() && !record.join_consumed
                || record.prepared_driver.is_some() && record.joined_child.is_none()
                || record.terminal_settlement.is_some() && record.joined_child.is_none()
                || record
                    .terminal_settlement
                    .as_ref()
                    .is_some_and(|settlement| {
                        settlement.handoff_id.is_empty()
                            || settlement.d_key.is_empty()
                            || settlement.invocation_uuid.is_empty()
                            || settlement.session_id.is_empty()
                            || settlement.parent_grant_id.is_empty()
                            || settlement.publication_sha256.len() != 64
                            || record.joined_child.as_ref() != Some(&settlement.actor)
                    })
                || record
                    .domain_id
                    .as_ref()
                    .is_some_and(|id| uuid::Uuid::parse_str(id).is_err())
                || record
                    .supervisor_authority_id
                    .as_ref()
                    .is_some_and(|id| uuid::Uuid::parse_str(id).is_err())
                || records
                    .iter()
                    .any(|r: &EntryRecord| r.root_id == record.root_id)
            {
                return Err(io::Error::other("invalid entry grant record"));
            }
            records.push(record);
        }
        Ok(Self {
            directory,
            records,
            poisoned: false,
        })
    }

    pub fn reserve(&mut self, uid: u32, entry: &PinnedProcess) -> io::Result<String> {
        if self.has_debt()
            || self.has_unsettled_join()
            || self
                .records
                .iter()
                .any(|r| r.entry.matches(entry).unwrap_or(false))
        {
            return Err(io::Error::other(
                "entry reservation uncertain or already exists",
            ));
        }
        entry.verify()?;
        let root_id = uuid::Uuid::new_v4().to_string();
        let record = EntryRecord {
            version: 1,
            root_id: root_id.clone(),
            owner_uid: uid,
            entry: entry.into(),
            prepared_guardian: None,
            domain_id: None,
            supervisor_authority_id: None,
            guardian: None,
            join_consumed: false,
            joined_child: None,
            prepared_driver: None,
            terminal_settlement: None,
        };
        let path = self.directory.join(format!("{root_id}.json"));
        let result = (|| {
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
        if let Err(error) = result {
            self.poisoned = true;
            return Err(error);
        }
        self.records.push(record);
        Ok(root_id)
    }

    pub fn bind_guardian(
        &mut self,
        root_id: &str,
        domain_id: &str,
        supervisor_authority_id: &str,
        uid: u32,
        guardian: &PinnedProcess,
    ) -> io::Result<()> {
        if self.has_debt()
            || uuid::Uuid::parse_str(domain_id).is_err()
            || uuid::Uuid::parse_str(supervisor_authority_id).is_err()
        {
            return Err(io::Error::other("invalid or uncertain entry grant"));
        }
        let index = self
            .records
            .iter()
            .position(|r| r.root_id == root_id)
            .ok_or_else(|| io::Error::other("unknown entry grant"))?;
        let current = &self.records[index];
        if current.domain_id.is_some() || current.guardian.is_some() || current.owner_uid != uid {
            return Err(io::Error::other("entry grant already bound or wrong UID"));
        }
        let entry = PinnedProcess::open(current.entry.host_pid)?;
        if !current.entry.matches(&entry)?
            || !guardian.in_namespace(entry.namespace())?
            || !guardian.direct_child_of(&entry)?
            || current.prepared_guardian.as_ref() != Some(&ProcessStamp::from(guardian))
        {
            return Err(io::Error::other(
                "guardian is not the exact host entry child",
            ));
        }
        let mut bound = current.clone();
        bound.domain_id = Some(domain_id.to_owned());
        bound.supervisor_authority_id = Some(supervisor_authority_id.to_owned());
        bound.guardian = Some(guardian.into());
        self.replace(index, bound, &entry, guardian)
    }

    /// Readback is available only to the exact still-live reserving entry.
    /// A root UUID by itself never confers authority.
    pub fn bound_entry(
        &self,
        root_id: &str,
        uid: u32,
        entry: &PinnedProcess,
    ) -> io::Result<&EntryRecord> {
        if self.has_debt() {
            return Err(io::Error::other("uncertain entry grant debt"));
        }
        let record = self
            .record(root_id)
            .ok_or_else(|| io::Error::other("unknown entry"))?;
        if record.owner_uid != uid || !record.entry.matches(entry)? {
            return Err(io::Error::other("entry identity mismatch"));
        }
        let guardian = record
            .guardian
            .as_ref()
            .ok_or_else(|| io::Error::other("unbound entry"))?;
        let live = PinnedProcess::open(guardian.host_pid)?;
        if !guardian.matches(&live)?
            || record.domain_id.is_none()
            || record.supervisor_authority_id.is_none()
        {
            return Err(io::Error::other("guardian grant identity mismatch"));
        }
        Ok(record)
    }

    pub fn prepare_guardian(
        &mut self,
        root_id: &str,
        uid: u32,
        entry: &PinnedProcess,
        guardian: &PinnedProcess,
    ) -> io::Result<()> {
        if self.has_debt() {
            return Err(io::Error::other("uncertain entry grant debt"));
        }
        let index = self
            .records
            .iter()
            .position(|r| r.root_id == root_id)
            .ok_or_else(|| io::Error::other("unknown entry grant"))?;
        let current = &self.records[index];
        if current.owner_uid != uid
            || !current.entry.matches(entry)?
            || current.prepared_guardian.is_some()
            || current.guardian.is_some()
            || !guardian.in_namespace(entry.namespace())?
            || !guardian.direct_child_of(entry)?
        {
            return Err(io::Error::other("guardian prepare identity mismatch"));
        }
        let mut prepared = current.clone();
        prepared.prepared_guardian = Some(guardian.into());
        self.replace(index, prepared, entry, guardian)
    }

    pub fn consume_join(
        &mut self,
        root_id: &str,
        domain_id: &str,
        supervisor_id: &str,
        guardian_pid: i32,
        uid: u32,
        entry: &PinnedProcess,
    ) -> io::Result<()> {
        let current = self.bound_entry(root_id, uid, entry)?;
        if current.join_consumed
            || current.domain_id.as_deref() != Some(domain_id)
            || current.supervisor_authority_id.as_deref() != Some(supervisor_id)
            || current.guardian.as_ref().map(|p| p.host_pid) != Some(guardian_pid)
        {
            return Err(io::Error::other(
                "join grant already consumed or mismatched",
            ));
        }
        let index = self
            .records
            .iter()
            .position(|r| r.root_id == root_id)
            .unwrap();
        let guardian = PinnedProcess::open(guardian_pid)?;
        let mut consumed = current.clone();
        consumed.join_consumed = true;
        self.replace(index, consumed, entry, &guardian)
    }

    pub fn bind_joined_child(
        &mut self,
        root_id: &str,
        uid: u32,
        entry: &PinnedProcess,
        child: &PinnedProcess,
    ) -> io::Result<()> {
        let current = self.bound_entry(root_id, uid, entry)?;
        if !current.join_consumed || current.joined_child.is_some() {
            return Err(io::Error::other(
                "root join child already bound or not consumed",
            ));
        }
        child.verify()?;
        let index = self
            .records
            .iter()
            .position(|record| record.root_id == root_id)
            .ok_or_else(|| io::Error::other("unknown joined root"))?;
        let guardian = PinnedProcess::open(current.guardian.as_ref().unwrap().host_pid)?;
        let mut bound = current.clone();
        bound.joined_child = Some(child.into());
        self.replace(index, bound, entry, &guardian)
    }

    pub fn bind_prepared_driver(
        &mut self,
        root_id: &str,
        uid: u32,
        guardian: &PinnedProcess,
        driver: &PinnedProcess,
    ) -> io::Result<ProcessStamp> {
        if self.has_debt() {
            return Err(io::Error::other("prepared driver registry uncertain"));
        }
        let index = self
            .records
            .iter()
            .position(|record| record.root_id == root_id)
            .ok_or_else(|| io::Error::other("prepared driver entry absent"))?;
        let current = &self.records[index];
        if current.owner_uid != uid
            || current.joined_child.is_none()
            || current.prepared_driver.is_some()
            || current.guardian.as_ref() != Some(&ProcessStamp::from(guardian))
            || !driver.direct_child_of(guardian)?
            || !driver.in_namespace(guardian.namespace())?
        {
            return Err(io::Error::other(
                "prepared driver is not exact guardian child",
            ));
        }
        let entry = PinnedProcess::open(current.entry.host_pid)?;
        if !current.entry.matches(&entry)? {
            return Err(io::Error::other("prepared entry incarnation changed"));
        }
        driver.verify()?;
        let stamp = ProcessStamp::from(driver);
        let mut bound = current.clone();
        bound.prepared_driver = Some(stamp.clone());
        self.replace(index, bound, &entry, guardian)?;
        driver.verify()?;
        Ok(stamp)
    }

    pub fn settle_join(
        &mut self,
        root_id: &str,
        settlement: EntryTerminalSettlement,
    ) -> io::Result<()> {
        let index = self
            .records
            .iter()
            .position(|record| record.root_id == root_id)
            .ok_or_else(|| io::Error::other("terminal entry absent"))?;
        let current = &self.records[index];
        if !current.join_consumed
            || current.joined_child.as_ref() != Some(&settlement.actor)
            || current
                .terminal_settlement
                .as_ref()
                .is_some_and(|prior| prior != &settlement)
        {
            return Err(io::Error::other("terminal entry identity changed"));
        }
        if current.terminal_settlement.is_some() {
            return Ok(());
        }
        let entry = PinnedProcess::open(current.entry.host_pid)?;
        let guardian = PinnedProcess::open(
            current
                .guardian
                .as_ref()
                .ok_or_else(|| io::Error::other("terminal guardian absent"))?
                .host_pid,
        )?;
        if !current.entry.matches(&entry)?
            || !current.guardian.as_ref().unwrap().matches(&guardian)?
        {
            return Err(io::Error::other("terminal entry process changed"));
        }
        let mut settled = current.clone();
        settled.terminal_settlement = Some(settlement);
        self.replace(index, settled, &entry, &guardian)
    }

    fn replace(
        &mut self,
        index: usize,
        bound: EntryRecord,
        entry: &PinnedProcess,
        guardian: &PinnedProcess,
    ) -> io::Result<()> {
        let tmp = self
            .directory
            .join(format!(".{}.{}.tmp", bound.root_id, uuid::Uuid::new_v4()));
        let path = self.directory.join(format!("{}.json", bound.root_id));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?;
            serde_json::to_writer(&mut file, &bound)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            entry.verify()?;
            guardian.verify()?;
            fs::rename(&tmp, path)?;
            File::open(&self.directory)?.sync_all()
        })();
        if let Err(error) = result {
            self.poisoned = true;
            return Err(error);
        }
        self.records[index] = bound;
        Ok(())
    }

    pub fn record(&self, root_id: &str) -> Option<&EntryRecord> {
        self.records.iter().find(|r| r.root_id == root_id)
    }

    /// A joined child's historical entry may exit while its root PID1 and
    /// guardian remain live. Read-only source attestation still refuses an
    /// uncertain registry write without treating that normal exit as debt.
    pub fn has_uncertain_write(&self) -> bool {
        self.poisoned
    }

    pub fn has_debt(&self) -> bool {
        self.poisoned
            || self
                .records
                .iter()
                .filter(|r| r.terminal_settlement.is_none())
                .any(|r| {
                    boot_id().ok().as_deref() != Some(r.entry.boot_id.as_str())
                        || PinnedProcess::open(r.entry.host_pid)
                            .and_then(|p| r.entry.matches(&p))
                            .ok()
                            != Some(true)
                        || r.prepared_guardian.as_ref().is_some_and(|stamp| {
                            PinnedProcess::open(stamp.host_pid)
                                .and_then(|p| stamp.matches(&p))
                                .ok()
                                != Some(true)
                        })
                        || r.prepared_driver.as_ref().is_some_and(|stamp| {
                            PinnedProcess::open(stamp.host_pid)
                                .and_then(|p| stamp.matches(&p))
                                .ok()
                                != Some(true)
                        })
                })
    }

    pub fn has_unsettled_join(&self) -> bool {
        self.records
            .iter()
            .any(|record| record.join_consumed && record.terminal_settlement.is_none())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    #[test]
    fn exact_direct_child_binds_once_and_survives_registry_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let mut registry = EntryRegistry::open(temp.path()).unwrap();
        let entry = PinnedProcess::open(std::process::id() as i32).unwrap();
        let uid = unsafe { libc::getuid() };
        let root = registry.reserve(uid, &entry).unwrap();
        let domain = uuid::Uuid::new_v4().to_string();
        let supervisor = uuid::Uuid::new_v4().to_string();
        assert!(
            registry
                .bind_guardian(&root, &domain, &supervisor, uid, &entry)
                .is_err()
        );
        assert!(
            registry
                .bind_guardian(
                    &uuid::Uuid::new_v4().to_string(),
                    &domain,
                    &supervisor,
                    uid,
                    &entry
                )
                .is_err()
        );
        let (mut parent_gate, mut child_gate) = UnixStream::pair().unwrap();
        let child = unsafe { libc::fork() };
        assert!(child >= 0);
        if child == 0 {
            drop(parent_gate);
            let status = (|| {
                let guardian = PinnedProcess::open(std::process::id() as i32)?;
                let mut restarted = EntryRegistry::open(temp.path())?;
                assert!(
                    restarted
                        .bind_guardian(&root, &domain, &supervisor, uid, &guardian)
                        .is_err()
                );
                // Finish the negative reopen before the parent atomically
                // replaces the record. Reopening during its temporary-file
                // publication can make this child exit before preparation.
                child_gate.write_all(&[0])?;
                let mut release = [0u8; 1];
                child_gate.read_exact(&mut release)?;
                assert_eq!(release, [1]);
                restarted = EntryRegistry::open(temp.path())?;
                restarted.bind_guardian(&root, &domain, &supervisor, uid, &guardian)?;
                child_gate.write_all(&[2])?;
                child_gate.read_exact(&mut release)?;
                assert_eq!(release, [3]);
                Ok::<(), io::Error>(())
            })();
            unsafe { libc::_exit(if status.is_ok() { 0 } else { 1 }) }
        }
        drop(child_gate);
        let mut ready = [0u8; 1];
        parent_gate.read_exact(&mut ready).unwrap();
        assert_eq!(ready, [0]);
        let guardian = PinnedProcess::open(child).unwrap();
        registry
            .prepare_guardian(&root, uid, &entry, &guardian)
            .unwrap();
        assert!(
            registry
                .prepare_guardian(&root, uid, &entry, &guardian)
                .is_err()
        );
        let sibling = unsafe { libc::fork() };
        assert!(sibling >= 0);
        if sibling == 0 {
            let sibling = PinnedProcess::open(std::process::id() as i32).unwrap();
            let mut reopened = EntryRegistry::open(temp.path()).unwrap();
            let denied = reopened
                .bind_guardian(&root, &domain, &supervisor, uid, &sibling)
                .is_err();
            unsafe { libc::_exit(if denied { 0 } else { 1 }) }
        }
        let mut sibling_status = 0;
        assert_eq!(
            unsafe { libc::waitpid(sibling, &mut sibling_status, 0) },
            sibling
        );
        assert!(libc::WIFEXITED(sibling_status) && libc::WEXITSTATUS(sibling_status) == 0);
        parent_gate.write_all(&[1]).unwrap();
        let mut bound = [0u8; 1];
        parent_gate.read_exact(&mut bound).unwrap();
        assert_eq!(bound, [2]);
        let reopened = EntryRegistry::open(temp.path()).unwrap();
        let bound = reopened.bound_entry(&root, uid, &entry).unwrap();
        assert!(
            !fs::read_to_string(temp.path().join(format!("{root}.json")))
                .unwrap()
                .contains("prepared_driver")
        );
        assert_eq!(bound.guardian.as_ref().unwrap().host_pid, child);
        assert_eq!(
            bound.supervisor_authority_id.as_deref(),
            Some(supervisor.as_str())
        );
        parent_gate.write_all(&[3]).unwrap();
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(child, &mut status, 0) }, child);
        assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
        let mut restarted = EntryRegistry::open(temp.path()).unwrap();
        let record = restarted.record(&root).unwrap();
        assert_eq!(record.domain_id.as_deref(), Some(domain.as_str()));
        assert_eq!(
            record.supervisor_authority_id.as_deref(),
            Some(supervisor.as_str())
        );
        assert_eq!(record.guardian.as_ref().unwrap().host_pid, child);
        assert!(restarted.bound_entry(&root, uid, &entry).is_err());
        assert!(
            restarted
                .bind_guardian(&root, &domain, &supervisor, uid, &entry)
                .is_err()
        );
        assert!(restarted.reserve(uid, &entry).is_err());
    }
}
