use crate::admission_accounting::RootState;
use crate::identity::{ChildExit, PinnedProcess, observation_unavailable};
use crate::json_artifact;
use crate::root_pid1;
use oulipoly_state::mailbox::BrokerStateCloseCursor;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// State-owned root of the Broker's flight recorder and event store.
pub const BROKER_DIAGNOSTICS: &str = "broker-diagnostics-v1";

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

/// Durable admission stop for the exact owner generation. This is a close
/// intent, not a closed-owner phase or a mailbox acknowledgement.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OwnerCloseIntent {
    pub root: RootRecord,
    pub owner_generation: String,
    pub source_generation: String,
    pub state_cursor: BrokerStateCloseCursor,
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
    /// Process-local admission classification, granted only after the Broker
    /// revalidates State caller settlement and the exact owner-close proof.
    closed_historical: std::collections::HashSet<String>,
    admission_fences: Vec<RootRecord>,
    poisoned: bool,
}

/// `Ok(Err(record))` when the recorded PID1 is observed gone or is not that
/// exact namespace init. A failure to observe it is an error, not an exit.
fn reattach(record: RootRecord) -> io::Result<Result<LiveRoot, RootRecord>> {
    if record.version != 1 {
        return Ok(Err(record));
    }
    let Some(init) = PinnedProcess::open_recorded(
        record.init_host_pid,
        &record.boot_id,
        record.init_starttime_ticks,
        (record.pidns_dev, record.pidns_ino),
    )?
    else {
        return Ok(Err(record));
    };
    match init.is_namespace_init() {
        Ok(true) => Ok(Ok(LiveRoot { record, init })),
        Ok(false) => Ok(Err(record)),
        Err(_) if !init.observe_live()? => Ok(Err(record)),
        Err(error) => Err(observation_unavailable(error)),
    }
}

impl RootRegistry {
    pub fn open(directory: impl AsRef<Path>) -> io::Result<Self> {
        let directory = directory.as_ref().to_path_buf();
        let empty_bootstrap = directory.join("empty-v30-bootstrap-v1.json").exists();
        let mut registry = Self {
            directory,
            live: Vec::new(),
            debt: Vec::new(),
            closed_historical: std::collections::HashSet::new(),
            admission_fences: Vec::new(),
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
            if name == "root-drains" && entry.file_type()?.is_dir() {
                continue;
            }
            if name == "root-pid1" && entry.file_type()?.is_dir() {
                continue;
            }
            if name == "entries" && entry.file_type()?.is_dir() {
                continue;
            }
            // Opened and validated by the serving Broker before this scan.
            if (name == "installed-launches"
                || name == "installed-control-exits"
                || name == "installed-normal-terminals"
                || name == "installed-successor-starts"
                || name == "installed-successor-candidates")
                && empty_bootstrap
                && registry
                    .directory
                    .join("first-install-activation-v1.json")
                    .exists()
                && entry.file_type()?.is_dir()
            {
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
            // The offline empty-first-install bootstrap marker is validated
            // before the serving broker opens this registry.
            if name == "empty-v30-bootstrap-v1.json" {
                let meta = fs::symlink_metadata(entry.path())?;
                if !meta.is_file()
                    || meta.file_type().is_symlink()
                    || meta.uid() != 0
                    || meta.nlink() != 1
                    || meta.mode() & 0o777 != 0o600
                {
                    return Err(io::Error::other("unsafe empty v30 bootstrap marker"));
                }
                continue;
            }
            // serve() validated the exact pair/source/image binding before
            // this scan. It is a fixed first-install record, not a root ID.
            if name == "first-install-activation-v1.json" {
                let meta = fs::symlink_metadata(entry.path())?;
                if !empty_bootstrap
                    || !meta.is_file()
                    || meta.file_type().is_symlink()
                    || meta.uid() != 0
                    || meta.nlink() != 1
                    || meta.mode() & 0o777 != 0o600
                {
                    return Err(io::Error::other("unsafe first-install activation record"));
                }
                continue;
            }
            if empty_bootstrap
                && matches!(
                    name.as_ref(),
                    "state.db"
                        | "state.db-wal"
                        | "state.db-shm"
                        | "state.db-journal"
                        | "state.db.namespace.lock"
                )
            {
                let meta = fs::symlink_metadata(entry.path())?;
                if !meta.is_file()
                    || meta.file_type().is_symlink()
                    || meta.uid() != 0
                    || meta.nlink() != 1
                    || meta.mode() & 0o077 != 0
                {
                    return Err(io::Error::other("unsafe empty v30 State artifact"));
                }
                continue;
            }
            // The serving broker validates the separate exact-source journal
            // before this scan. It is never interpreted as a root record.
            if name == "source-decisions" {
                let meta = fs::symlink_metadata(entry.path())?;
                if !meta.is_dir()
                    || meta.file_type().is_symlink()
                    || meta.uid() != 0
                    || meta.mode() & 0o777 != 0o700
                {
                    return Err(io::Error::other("unsafe source decision journal directory"));
                }
                continue;
            }
            // Phase timing records are diagnostics only, never root records.
            if name == crate::phase_record::DIRECTORY {
                let meta = fs::symlink_metadata(entry.path())?;
                if !meta.is_dir()
                    || meta.file_type().is_symlink()
                    || meta.uid() != 0
                    || meta.mode() & 0o777 != 0o700
                {
                    return Err(io::Error::other("unsafe phase record directory"));
                }
                continue;
            }
            // Installed serve() initializes the recorder before this scan.
            // Its fixed storage also survives subsequent Broker starts; it
            // contains diagnostics, never root records.
            if name == BROKER_DIAGNOSTICS {
                let meta = fs::symlink_metadata(entry.path())?;
                if !meta.file_type().is_dir()
                    || meta.uid() != unsafe { libc::geteuid() }
                    || meta.mode() & 0o777 != 0o700
                {
                    return Err(io::Error::other("unsafe broker diagnostics directory"));
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
            match reattach(record)? {
                Ok(root) => registry.live.push(root),
                Err(record) => registry.debt.push(record),
            }
        }
        registry.check_unique()?;
        let fences = registry.directory.join("root-drains");
        if !fences.exists() {
            fs::DirBuilder::new().mode(0o700).create(&fences)?;
            fs::File::open(&registry.directory)?.sync_all()?;
        }
        let fence_meta = fs::symlink_metadata(&fences)?;
        if !fence_meta.is_dir()
            || fence_meta.file_type().is_symlink()
            || fence_meta.uid() != unsafe { libc::geteuid() }
            || fence_meta.mode() & 0o077 != 0
        {
            return Err(io::Error::other("unsafe root drain fence directory"));
        }
        let mut close_intents = Vec::new();
        for entry in fs::read_dir(&fences)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".close-intent.json") {
                let intent: OwnerCloseIntent =
                    json_artifact::read(&entry.path(), "owner_close_intent_open")?;
                if !entry.file_type()?.is_file()
                    || name != format!("{}.close-intent.json", intent.root.root_id)
                    || intent.owner_generation.is_empty()
                    || intent.source_generation.is_empty()
                {
                    return Err(io::Error::other("owner close intent identity conflict"));
                }
                close_intents.push(intent);
                continue;
            }
            if !entry.file_type()?.is_file() || !name.ends_with(".json") {
                return Err(io::Error::other("unrecognized root drain fence"));
            }
            let record: RootRecord = json_artifact::read(&entry.path(), "root_drain_fence_open")?;
            if name != format!("{}.json", record.root_id)
                || !registry
                    .live
                    .iter()
                    .map(|root| &root.record)
                    .chain(registry.debt.iter())
                    .any(|root| root == &record)
                || registry
                    .admission_fences
                    .iter()
                    .any(|fence| fence.root_id == record.root_id)
            {
                return Err(io::Error::other("root drain fence identity conflict"));
            }
            registry.admission_fences.push(record);
        }
        if close_intents.iter().any(|intent| {
            !registry
                .admission_fences
                .iter()
                .any(|fence| fence == &intent.root)
        }) {
            return Err(io::Error::other(
                "owner close intent without exact root fence",
            ));
        }
        let pid1 = registry.directory.join("root-pid1");
        if !pid1.exists() {
            fs::DirBuilder::new().mode(0o700).create(&pid1)?;
            fs::File::open(&registry.directory)?.sync_all()?;
        }
        let meta = fs::symlink_metadata(&pid1)?;
        if !meta.is_dir()
            || meta.file_type().is_symlink()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o077 != 0
        {
            return Err(io::Error::other("unsafe root PID1 directory"));
        }
        for entry in fs::read_dir(&pid1)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(".json-pending-") {
                return Err(json_artifact::pending_error(
                    &entry.path(),
                    "root_pid1_open",
                ));
            }
            let Some(root_id) = name
                .strip_suffix(".request.json")
                .or_else(|| name.strip_suffix(".terminal.json"))
                .or_else(|| name.strip_suffix(".parent-wait.json"))
            else {
                return Err(io::Error::other("unrecognized root PID1 artifact"));
            };
            let root = registry
                .live
                .iter()
                .map(|r| &r.record)
                .chain(registry.debt.iter())
                .find(|root| root.root_id == root_id)
                .ok_or_else(|| io::Error::other("orphaned root PID1 artifact"))?;
            if !registry.admission_fences.iter().any(|fence| fence == root)
                || !entry.file_type()?.is_file()
            {
                return Err(io::Error::other("unfenced root PID1 artifact"));
            }
            if name.ends_with(".request.json") {
                root_pid1::read_request(&pid1, root)?;
            } else if name.ends_with(".terminal.json") {
                root_pid1::read_terminal(&pid1, root)?
                    .ok_or_else(|| io::Error::other("root PID1 terminal missing"))?;
            } else if !root_pid1::parent_wait_proof(&pid1, root)? {
                return Err(io::Error::other("root PID1 parent wait missing"));
            }
        }
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

    /// A fenced root whose exact PID1 has a durable terminal proof is in the
    /// Broker-owned close progression: its namespace is gone and its close is
    /// accounted per root, so it does not stop concurrent sibling roots. Any
    /// other exited root remains global debt.
    pub fn has_debt(&self) -> bool {
        self.poisoned
            || self.debt.iter().any(|r| self.unaccounted_exit(r))
            || self
                .live
                .iter()
                .any(|r| r.init.verify().is_err() && self.unaccounted_exit(&r.record))
    }

    fn unaccounted_exit(&self, root: &RootRecord) -> bool {
        !self.closed_historical.contains(&root.root_id) && !self.terminal_verified(root)
    }

    /// This changes only in-memory debt classification. Durable root records
    /// and physical proofs remain available for every later E readback.
    pub fn admit_closed_historical(&mut self, root_id: &str) -> io::Result<()> {
        if self.record(root_id).is_none() {
            return Err(io::Error::other("closed historical root absent"));
        }
        self.closed_historical.insert(root_id.to_owned());
        Ok(())
    }

    fn terminal_verified(&self, root: &RootRecord) -> bool {
        self.admission_fences.iter().any(|fence| fence == root)
            && root_pid1::terminal_proof(&self.directory.join("root-pid1"), root)
                .is_ok_and(|proof| proof.is_some())
    }

    /// Read-only physical Q for one terminal root may use its exact PID1
    /// receipt. This is the same accounting as `has_debt`.
    pub fn has_unrelated_debt(&self, _expected: &RootRecord) -> bool {
        self.has_debt()
    }

    pub fn pid1_terminal_proof(&self, expected: &RootRecord) -> io::Result<bool> {
        self.exact_record(expected)?;
        if !self.admission_fences.iter().any(|fence| fence == expected) {
            return Ok(false);
        }
        Ok(root_pid1::terminal_proof(&self.directory.join("root-pid1"), expected)?.is_some())
    }

    pub fn pid1_echild_receipt(&self, expected: &RootRecord) -> io::Result<bool> {
        self.exact_record(expected)?;
        if !self.admission_fences.iter().any(|fence| fence == expected) {
            return Ok(false);
        }
        Ok(root_pid1::read_terminal(&self.pid1_directory(), expected)?.is_some())
    }

    /// Exact live identity is recoverable from the pinned process after a
    /// Broker restart. The separate stable parent publishes its own wait.
    pub fn pid1_exact_live(&self, expected: &RootRecord) -> io::Result<bool> {
        self.exact_record(expected)?;
        Ok(self
            .live
            .iter()
            .any(|root| root.record == *expected && root.init.verify().is_ok()))
    }

    pub fn pid1_directory(&self) -> PathBuf {
        self.directory.join("root-pid1")
    }

    pub fn pid1_parent_wait_proof(&self, expected: &RootRecord) -> io::Result<bool> {
        self.exact_record(expected)?;
        root_pid1::parent_wait_proof(&self.pid1_directory(), expected)
    }

    /// The stable parent of PID1 publishes its exact wait independently of
    /// the serving Broker. This call only reads its durable result.
    pub fn reap_terminal_pid1(&self, expected: &RootRecord) -> io::Result<bool> {
        self.exact_record(expected)?;
        self.pid1_parent_wait_proof(expected)
    }

    pub fn live_roots(&self) -> impl Iterator<Item = &LiveRoot> {
        self.live.iter()
    }

    pub fn record(&self, root_id: &str) -> Option<&RootRecord> {
        self.live
            .iter()
            .map(|root| &root.record)
            .chain(self.debt.iter())
            .find(|record| record.root_id == root_id)
    }

    /// Durable exact-incarnation admission stop. A consumed grant can still
    /// finish its one-use launch; this fence never settles physical or State
    /// obligations. The serving broker serializes this write with the guarded
    /// fresh lane admissions.
    pub fn fence_admission(&mut self, expected: &RootRecord) -> io::Result<()> {
        if self.poisoned {
            return Err(io::Error::other("uncertain root registry"));
        }
        let root = self
            .live
            .iter()
            .find(|root| root.record.root_id == expected.root_id)
            .ok_or_else(|| io::Error::other("exact live root absent"))?;
        if root.record != *expected || root.init.verify().is_err() {
            return Err(io::Error::other("root incarnation changed"));
        }
        if let Some(fence) = self
            .admission_fences
            .iter()
            .find(|fence| fence.root_id == expected.root_id)
        {
            return if fence == expected {
                Ok(())
            } else {
                Err(io::Error::other("root fence incarnation changed"))
            };
        }
        let result = json_artifact::create_new(
            &self.directory.join("root-drains"),
            &format!("{}.json", expected.root_id),
            expected,
        );
        if let Err(error) = result {
            self.poisoned = true;
            return Err(error);
        }
        self.admission_fences.push(expected.clone());
        Ok(())
    }

    /// Admission classification of one entry's root. A live-list root is
    /// `Exited` only when its pidfd reports the exit; a failure to observe it
    /// is unavailable observation and records nothing.
    pub fn admission_state(&self, root_id: &str) -> io::Result<RootState> {
        let exited = || RootState::Exited {
            fenced: self.admission_fenced(root_id),
        };
        match self.live.iter().find(|root| root.record.root_id == root_id) {
            Some(root) => Ok(if root.init.observe_live()? {
                RootState::Live
            } else {
                exited()
            }),
            None if self.debt.iter().any(|root| root.root_id == root_id) => Ok(exited()),
            None => Ok(RootState::Absent),
        }
    }

    pub fn admission_fenced(&self, root_id: &str) -> bool {
        self.poisoned
            || self
                .admission_fences
                .iter()
                .any(|fence| fence.root_id == root_id)
    }

    pub fn read_close_intent(&self, expected: &RootRecord) -> io::Result<Option<OwnerCloseIntent>> {
        self.exact_record(expected)?;
        let path = self
            .directory
            .join("root-drains")
            .join(format!("{}.close-intent.json", expected.root_id));
        if !path.exists() {
            return Ok(None);
        }
        let intent: OwnerCloseIntent = json_artifact::read(&path, "owner_close_intent_read")?;
        if intent.root != *expected || !self.admission_fences.iter().any(|fence| fence == expected)
        {
            return Err(io::Error::other("owner close intent root changed"));
        }
        Ok(Some(intent))
    }

    /// Caller holds the shared fresh admission mutex and has just repeated
    /// close preflight. The old Broker request loop is single threaded.
    pub fn issue_close_intent(
        &mut self,
        intent: &OwnerCloseIntent,
    ) -> io::Result<OwnerCloseIntent> {
        self.exact_record(&intent.root)?;
        if !self
            .admission_fences
            .iter()
            .any(|fence| fence == &intent.root)
        {
            return Err(io::Error::other(
                "owner close intent root drain fence absent",
            ));
        }
        if let Some(existing) = self.read_close_intent(&intent.root)? {
            return if existing == *intent {
                Ok(existing)
            } else {
                Err(io::Error::other(
                    "owner close intent generation or cursor changed",
                ))
            };
        }
        let result = json_artifact::create_new(
            &self.directory.join("root-drains"),
            &format!("{}.close-intent.json", intent.root.root_id),
            intent,
        );
        if let Err(error) = result {
            self.poisoned = true;
            return Err(error);
        }
        self.read_close_intent(&intent.root)?
            .ok_or_else(|| io::Error::other("owner close intent lost after publication"))
    }

    pub fn fenced_root_ids(&self) -> impl Iterator<Item = &str> {
        self.admission_fences
            .iter()
            .map(|fence| fence.root_id.as_str())
    }

    pub fn exact_record(&self, expected: &RootRecord) -> io::Result<()> {
        if self.poisoned {
            return Err(io::Error::other("uncertain root registry"));
        }
        if self
            .live
            .iter()
            .map(|root| &root.record)
            .chain(self.debt.iter())
            .any(|record| record == expected)
        {
            Ok(())
        } else {
            Err(io::Error::other("root incarnation changed or absent"))
        }
    }

    /// Observe the exact pinned PID1. A direct-child wait is available to
    /// older callers; roots launched with a stable parent use its durable
    /// wait after exit. A missing wait never becomes a successful exit.
    pub fn observe_init_exit(&self, expected: &RootRecord) -> io::Result<ChildExit> {
        if self.poisoned || self.debt.iter().any(|root| self.unaccounted_exit(root)) {
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
        match root.init.peek_child_exit() {
            Ok(exit) => Ok(exit),
            Err(error) if error.raw_os_error() == Some(libc::ECHILD) => {
                if root.init.verify().is_ok() {
                    Ok(ChildExit::Running)
                } else if self.pid1_parent_wait_proof(expected)? {
                    Ok(ChildExit::ExitedZero)
                } else {
                    Err(io::Error::other("root PID1 stable parent wait pending"))
                }
            }
            Err(error) => Err(error),
        }
    }

    pub fn insert(&mut self, record: RootRecord) -> io::Result<()> {
        if self.has_debt() {
            return Err(io::Error::other("uncertain root debt"));
        }
        let root =
            reattach(record.clone())?.map_err(|_| io::Error::other("init identity changed"))?;
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

    /// Stop holding the PID1 pin of each root whose terminal state is proven:
    /// the pinned pidfd reports the exact exit, and the exact fence, PID1's
    /// own ECHILD receipt and absence of its incarnation are verified. Each
    /// record stays, pinless exactly as `reattach` leaves a dead PID1 after a
    /// Broker restart, so debt classification, records, fences, receipts and
    /// every obligation keyed by them are unchanged. Nothing is settled.
    pub fn release_terminal_roots(&mut self) -> Vec<RootRecord> {
        if self.poisoned {
            return Vec::new();
        }
        let (released, live): (Vec<_>, Vec<_>) = std::mem::take(&mut self.live)
            .into_iter()
            .partition(|root| {
                matches!(root.init.exited(), Ok(true)) && self.terminal_verified(&root.record)
            });
        self.live = live;
        let records: Vec<_> = released.into_iter().map(|root| root.record).collect();
        self.debt.extend(records.iter().cloned());
        records
    }

    /// No automatic retirement. A failed or vanished root remains recorded; it
    /// blocks new outside launches unless it is closed history or a fenced
    /// root with its exact PID1 terminal proof.
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
    use crate::identity::{ChildExit, OBSERVATION_UNAVAILABLE};
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    #[test]
    fn broker_diagnostics_directory_survives_registry_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let diagnostics = temp.path().join(BROKER_DIAGNOSTICS);
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&diagnostics)
            .unwrap();
        // These are namespace fixtures, not a recorder or serve() check.
        fs::create_dir_all(diagnostics.join("diagnostics/flight-recorder-v1")).unwrap();
        fs::create_dir_all(diagnostics.join("diagnostics/event-store-v1")).unwrap();
        for _ in 0..2 {
            let registry = RootRegistry::open(temp.path()).unwrap();
            assert!(!registry.has_debt());
            assert!(registry.live.is_empty());
        }
    }

    #[test]
    fn broker_diagnostics_does_not_admit_unknown_siblings() {
        let temp = tempfile::tempdir().unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(temp.path().join(BROKER_DIAGNOSTICS))
            .unwrap();
        RootRegistry::open(temp.path()).unwrap();
        fs::create_dir(temp.path().join("broker-diagnostics-v2")).unwrap();
        assert_eq!(
            RootRegistry::open(temp.path()).unwrap_err().to_string(),
            "unrecognized registry entry"
        );
    }

    #[test]
    fn broker_diagnostics_rejects_file_symlink_and_public_directory() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        for kind in ["file", "symlink", "public_directory"] {
            let temp = tempfile::tempdir().unwrap();
            let diagnostics = temp.path().join(BROKER_DIAGNOSTICS);
            match kind {
                "file" => fs::write(&diagnostics, b"").unwrap(),
                "symlink" => symlink(temp.path(), &diagnostics).unwrap(),
                _ => {
                    fs::create_dir(&diagnostics).unwrap();
                    fs::set_permissions(&diagnostics, fs::Permissions::from_mode(0o755)).unwrap();
                }
            }
            assert_eq!(
                RootRegistry::open(temp.path()).unwrap_err().to_string(),
                "unsafe broker diagnostics directory"
            );
        }
    }

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
    fn exact_root_fence_persists_and_other_root_remains_unfenced() {
        let temp = tempfile::tempdir().unwrap();
        let (first_init, first_gate) = gated_child();
        let (second_init, second_gate) = gated_child();
        let first = record(&uuid::Uuid::new_v4().to_string(), &first_init);
        let second = record(&uuid::Uuid::new_v4().to_string(), &second_init);
        for root in [&first, &second] {
            json_artifact::create_new(temp.path(), &format!("{}.json", root.root_id), root)
                .unwrap();
        }
        let mut roots = RootRegistry {
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
            closed_historical: std::collections::HashSet::new(),
            admission_fences: Vec::new(),
            poisoned: false,
        };
        fs::DirBuilder::new()
            .mode(0o700)
            .create(temp.path().join("root-drains"))
            .unwrap();
        let mut stale = first.clone();
        stale.init_starttime_ticks += 1;
        assert!(roots.fence_admission(&stale).is_err());
        assert!(!roots.admission_fenced(&first.root_id));
        roots.fence_admission(&first).unwrap();
        roots.fence_admission(&first).unwrap();
        let intent = OwnerCloseIntent {
            root: first.clone(),
            owner_generation: uuid::Uuid::new_v4().to_string(),
            source_generation: uuid::Uuid::new_v4().to_string(),
            state_cursor: BrokerStateCloseCursor {
                file: oulipoly_state::mailbox::BoundStateFileIdentity {
                    device: 9,
                    inode: 11,
                },
                authority_ordinal: 3,
                admission_id: uuid::Uuid::new_v4().to_string(),
                sidecar_generation: uuid::Uuid::new_v4().to_string(),
                continuity_digest: "a".repeat(64),
            },
        };
        assert_eq!(roots.issue_close_intent(&intent).unwrap(), intent);
        assert_eq!(roots.issue_close_intent(&intent).unwrap(), intent);
        let mut stale_intent = intent.clone();
        stale_intent.owner_generation = uuid::Uuid::new_v4().to_string();
        assert!(roots.issue_close_intent(&stale_intent).is_err());
        assert!(roots.read_close_intent(&second).unwrap().is_none());
        assert!(roots.read_close_intent(&stale).is_err());
        assert!(roots.admission_fenced(&first.root_id));
        assert!(!roots.admission_fenced(&second.root_id));
        assert!(roots.exact_record(&first).is_ok());
        assert!(roots.exact_record(&stale).is_err());
        // These test children are not namespace PID1. Restart therefore
        // classifies them as debt, but must retain the exact durable fence.
        let restarted = RootRegistry::open(temp.path()).unwrap();
        assert!(restarted.has_debt());
        assert!(restarted.admission_fenced(&first.root_id));
        assert_eq!(restarted.read_close_intent(&first).unwrap(), Some(intent));
        assert!(!restarted.admission_fenced(&second.root_id));
        assert!(restarted.exact_record(&first).is_ok());
        let pending = temp.path().join("root-drains/.json-pending-crashed-fence");
        fs::write(&pending, b"{\"incomplete\":").unwrap();
        assert!(RootRegistry::open(temp.path()).is_err());
        drop(first_gate);
        drop(second_gate);
    }

    fn open_descriptors() -> usize {
        fs::read_dir("/proc/self/fd").unwrap().count()
    }

    fn reap(pid: i32) {
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
    }

    /// Run `observe` with no descriptor available to this process. Only a
    /// re-executed disposable test process calls this.
    fn without_descriptors<T>(observe: impl FnOnce() -> T) -> T {
        // Fill the descriptor table below a small soft limit, as an
        // exhausted Broker would have it; existing descriptors keep working.
        let mut saved = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        assert_eq!(
            unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut saved) },
            0
        );
        let small = libc::rlimit {
            rlim_cur: 64,
            rlim_max: saved.rlim_max,
        };
        assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &small) }, 0);
        let mut filled = Vec::new();
        loop {
            let fd = unsafe { libc::dup(2) };
            if fd < 0 {
                assert_eq!(
                    io::Error::last_os_error().raw_os_error(),
                    Some(libc::EMFILE)
                );
                break;
            }
            filled.push(fd);
        }
        let probe = fs::File::open("/proc/self/stat").map(drop);
        let result = observe();
        for fd in filled {
            unsafe { libc::close(fd) };
        }
        assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &saved) }, 0);
        assert_eq!(probe.unwrap_err().raw_os_error(), Some(libc::EMFILE));
        result
    }

    /// EMFILE while observing a live root is unavailable observation: neither
    /// an exit nor debt, and nothing changes. A pinned root's exit is still
    /// observed through its pidfd. The fault runs in a re-executed process.
    #[test]
    fn failed_observation_is_unavailable_not_exit_and_records_nothing() {
        const NAME: &str =
            "registry::tests::failed_observation_is_unavailable_not_exit_and_records_nothing";
        if std::env::var_os("AGE380_OBSERVATION_TEST_CHILD").is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--nocapture", "--test-threads=1"])
                .env("AGE380_OBSERVATION_TEST_CHILD", "1")
                .status()
                .unwrap();
            assert!(status.success(), "disposable observation control failed");
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let (live_init, live_gate) = gated_child();
        let (exited_init, mut exited_gate) = gated_child();
        let live = record(&uuid::Uuid::new_v4().to_string(), &live_init);
        let exited = record(&uuid::Uuid::new_v4().to_string(), &exited_init);
        exited_gate.write_all(&[1]).unwrap();
        reap(exited_init.host_pid);
        let roots = RootRegistry {
            directory: temp.path().into(),
            live: [(&live, live_init), (&exited, exited_init)]
                .into_iter()
                .map(|(record, init)| LiveRoot {
                    record: record.clone(),
                    init,
                })
                .collect(),
            debt: Vec::new(),
            closed_historical: Default::default(),
            admission_fences: Vec::new(),
            poisoned: false,
        };
        let classify = |roots: &RootRegistry| {
            [&live, &exited].map(|root| {
                roots
                    .admission_state(&root.root_id)
                    .map_err(|error| error.to_string())
            })
        };
        let before = classify(&roots);
        assert_eq!(
            before,
            [Ok(RootState::Live), Ok(RootState::Exited { fenced: false })]
        );
        assert!(roots.has_debt(), "the observed unfenced exit is debt");

        let (faulted, reattached) =
            without_descriptors(|| (classify(&roots), reattach(live.clone()).map(|_| ())));
        let [unobserved, observed_exit] = faulted;
        let unobserved = unobserved.unwrap_err();
        assert!(
            unobserved.starts_with(OBSERVATION_UNAVAILABLE),
            "{unobserved}"
        );
        assert!(unobserved.contains("os error 24"), "{unobserved}");
        assert_eq!(observed_exit, Ok(RootState::Exited { fenced: false }));
        // A restart that cannot observe a recorded root does not file it as
        // a dead PID1.
        let reattached = reattached.unwrap_err().to_string();
        assert!(
            reattached.starts_with(OBSERVATION_UNAVAILABLE),
            "{reattached}"
        );

        // Nothing was recorded: the same classification, lists and files.
        assert_eq!(classify(&roots), before);
        assert_eq!((roots.live.len(), roots.debt.len()), (2, 0));
        assert!(fs::read_dir(temp.path()).unwrap().next().is_none());
        drop(live_gate);
    }

    /// PID1 receipts are root-only files, so this runs as mapped root in a
    /// disposable user, PID and mount namespace with a private /tmp.
    #[test]
    fn terminal_release_drops_pin_but_keeps_record_and_debt_classification() {
        if std::env::var_os("AGE380_RELEASE_TEST_CHILD").is_none() {
            let private_tmp = tempfile::tempdir().unwrap();
            let status = std::process::Command::new("unshare")
                .args(["-Urpfm", "--mount-proc", "/bin/sh", "-c",
                    "mount --bind \"$1\" /tmp && exec \"$2\" --exact registry::tests::terminal_release_drops_pin_but_keeps_record_and_debt_classification --nocapture",
                    "release-control"])
                .arg(private_tmp.path())
                .arg(std::env::current_exe().unwrap())
                .env_remove("TMPDIR")
                .env("AGE380_RELEASE_TEST_CHILD", "1")
                .status()
                .unwrap();
            assert!(status.success(), "disposable release control failed");
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        for name in ["root-drains", "root-pid1"] {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(temp.path().join(name))
                .unwrap();
        }
        let (done_init, mut done_gate) = gated_child();
        let (crashed_init, crashed_gate) = gated_child();
        let (live_init, live_gate) = gated_child();
        let done = record(&uuid::Uuid::new_v4().to_string(), &done_init);
        let crashed = record(&uuid::Uuid::new_v4().to_string(), &crashed_init);
        let live = record(&uuid::Uuid::new_v4().to_string(), &live_init);
        let mut roots = RootRegistry {
            directory: temp.path().into(),
            live: [
                (&done, done_init),
                (&crashed, crashed_init),
                (&live, live_init),
            ]
            .into_iter()
            .map(|(record, init)| LiveRoot {
                record: record.clone(),
                init,
            })
            .collect(),
            debt: Vec::new(),
            closed_historical: std::collections::HashSet::new(),
            admission_fences: Vec::new(),
            poisoned: false,
        };
        roots.fence_admission(&done).unwrap();
        roots.fence_admission(&crashed).unwrap();
        root_pid1::publish_request(&roots.pid1_directory(), &done).unwrap();
        root_pid1::publish_terminal(&roots.pid1_directory(), &done).unwrap();

        // A receipt while the exact PID1 still lives is not terminal.
        assert!(roots.release_terminal_roots().is_empty());
        assert!(!roots.has_debt());

        done_gate.write_all(b"go").unwrap();
        reap(done.init_host_pid);
        assert_eq!(
            unsafe { libc::kill(crashed.init_host_pid, libc::SIGKILL) },
            0
        );
        reap(crashed.init_host_pid);
        assert!(roots.pid1_terminal_proof(&done).unwrap());

        let classify = |roots: &RootRegistry| {
            (
                roots.has_debt(),
                [&done, &crashed, &live].map(|root| {
                    (
                        roots.exact_record(root).is_ok(),
                        roots.admission_fenced(&root.root_id),
                        roots.unaccounted_exit(root),
                        roots.pid1_exact_live(root).unwrap(),
                    )
                }),
            )
        };
        let before = (classify(&roots), open_descriptors());
        assert!(before.0.0, "the crashed root without a receipt is debt");

        // Only the proven root releases its pidfd and namespace handle. The
        // crashed root has no terminal proof and keeps its pin.
        assert_eq!(roots.release_terminal_roots(), vec![done.clone()]);
        assert_eq!(open_descriptors(), before.1 - 2);
        assert_eq!(classify(&roots), before.0);
        assert_eq!(roots.debt_records(), &[done.clone()]);
        assert_eq!(
            roots
                .live_roots()
                .map(|root| &root.record)
                .collect::<Vec<_>>(),
            vec![&crashed, &live]
        );
        assert!(roots.pid1_terminal_proof(&done).unwrap());
        assert!(roots.pid1_echild_receipt(&done).unwrap());
        assert!(roots.release_terminal_roots().is_empty());

        // Closed history still accounts the released record afterward.
        roots.admit_closed_historical(&done.root_id).unwrap();
        assert!(!roots.unaccounted_exit(&done));
        drop(crashed_gate);
        drop(live_gate);
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
            closed_historical: std::collections::HashSet::new(),
            admission_fences: Vec::new(),
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
