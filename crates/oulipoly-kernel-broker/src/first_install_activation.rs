//! Offline, one-time binding of an empty v30 State source to a schema-2 pair.
//! This record is route evidence only. It grants no launch or legacy entry.
use crate::cutover_gate::EntryGate;
use crate::installed_pair::InstalledPair;
use oulipoly_state::mailbox::EmptyV30BootstrapIdentity;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

const RECORD: &str = "first-install-activation-v1.json";
pub const STAGE_PREFIX: &str = ".first-install-activation-";

#[derive(Clone, Copy)]
pub struct PairPaths<'a> {
    pub manifest: &'a Path,
    pub runner: &'a Path,
    pub broker: &'a Path,
    pub launcher: &'a Path,
    pub bash: &'a Path,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileIdentity {
    pub device: u64,
    pub inode: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirstInstallActivation {
    pub schema: u8,
    pub source: EmptyV30BootstrapIdentity,
    pub pair_generation: String,
    pub manifest: FileIdentity,
    pub runner: FileIdentity,
    pub broker: FileIdentity,
    pub launcher: FileIdentity,
    pub bash: FileIdentity,
}

impl FirstInstallActivation {
    /// Requires final installed images and an offline Broker. A lost reply is
    /// reconciled by exact readback; an interrupted prepublication stage refuses.
    pub fn activate_at(root: &Path, paths: PairPaths<'_>, fixture: bool) -> io::Result<Self> {
        require_root()?;
        let source = EmptyV30BootstrapIdentity::readback_at(root).map_err(io::Error::other)?;
        let record = root.join(RECORD);
        if fs::symlink_metadata(&record).is_ok() {
            return Self::readback_at(root, paths, fixture);
        }
        refuse_stages(root)?;
        // An entry-gate lock means this root has already served or is serving.
        // Fresh activation is available only before its first Broker startup.
        if root.join("entry-gate.lock").exists() || root.join("entry-gate.v1").exists() {
            return Err(io::Error::other("Broker root has already served"));
        }
        refuse_nonbootstrap_root(root)?;
        require_empty_bootstrap_tables(root)?;
        let candidate = Self::capture(&source, paths, fixture)?;
        let gate = EntryGate::open(root)?;
        if gate.is_closed() {
            return Err(io::Error::other("Broker ingress already closed"));
        }
        let stage = root.join(format!("{STAGE_PREFIX}{}", uuid::Uuid::new_v4()));
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&stage)?;
        serde_json::to_writer(&mut output, &candidate)?;
        output.write_all(b"\n")?;
        output.sync_all()?;
        File::open(root)?.sync_all()?;
        // Check again after hashing, before publication. Startup also checks
        // the exact file identities on every incarnation.
        if Self::capture(&source, paths, fixture)? != candidate {
            return Err(io::Error::other("installed pair changed during activation"));
        }
        rename_noreplace(&stage, &record)?;
        File::open(root)?.sync_all()?;
        drop(gate);
        let readback = Self::readback_at(root, paths, fixture)?;
        if readback != candidate {
            return Err(io::Error::other("activation publication changed"));
        }
        Ok(readback)
    }

    pub fn readback_at(root: &Path, paths: PairPaths<'_>, fixture: bool) -> io::Result<Self> {
        require_root()?;
        let source = EmptyV30BootstrapIdentity::readback_at(root).map_err(io::Error::other)?;
        refuse_stages(root)?;
        let path = root.join(RECORD);
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)?;
        let meta = file.metadata()?;
        let named = fs::symlink_metadata(&path)?;
        if !meta.is_file()
            || meta.uid() != 0
            || meta.nlink() != 1
            || meta.mode() & 0o777 != 0o600
            || meta.dev() != named.dev()
            || meta.ino() != named.ino()
            || meta.len() > 4096
        {
            return Err(io::Error::other("activation record is untrusted"));
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let stored: Self = serde_json::from_slice(&bytes)?;
        let observed = Self::capture(&source, paths, fixture)?;
        if stored != observed {
            return Err(io::Error::other(
                "activation identity or installed images changed",
            ));
        }
        Ok(stored)
    }

    fn capture(
        source: &EmptyV30BootstrapIdentity,
        paths: PairPaths<'_>,
        fixture: bool,
    ) -> io::Result<Self> {
        let pair = InstalledPair::load(paths.manifest, !fixture)?;
        if pair.schema != 2 || pair.launcher_sha256.is_none() || pair.bash_sha256.is_none() {
            return Err(io::Error::other(
                "fresh activation requires schema-2 four-image pair",
            ));
        }
        let manifest = fingerprint(paths.manifest, None, !fixture)?;
        let runner = fingerprint(paths.runner, Some(&pair.runner_sha256), !fixture)?;
        let broker = fingerprint(paths.broker, Some(&pair.broker_sha256), !fixture)?;
        let launcher = fingerprint(paths.launcher, pair.launcher_sha256.as_deref(), !fixture)?;
        let bash = fingerprint(paths.bash, pair.bash_sha256.as_deref(), !fixture)?;
        Ok(Self {
            schema: 1,
            source: source.clone(),
            pair_generation: pair.generation,
            manifest,
            runner,
            broker,
            launcher,
            bash,
        })
    }
}

fn require_root() -> io::Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(io::Error::other("first-install activation requires root"));
    }
    Ok(())
}

fn fingerprint(
    path: &Path,
    expected: Option<&str>,
    require_ancestry: bool,
) -> io::Result<FileIdentity> {
    if require_ancestry {
        trusted_ancestry(path)?;
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let before = file.metadata()?;
    if !before.is_file()
        || before.uid() != 0
        || before.nlink() != 1
        || before.mode() & 0o022 != 0
        || (expected.is_some() && before.mode() & 0o100 == 0)
        || before.len() == 0
    {
        return Err(io::Error::other(format!(
            "installed pair file is untrusted: {} uid={} mode={:o} links={}",
            path.display(),
            before.uid(),
            before.mode() & 0o7777,
            before.nlink()
        )));
    }
    let mut hash = Sha256::new();
    io::copy(&mut file, &mut hash)?;
    let sha256 = format!("{:x}", hash.finalize());
    let after = file.metadata()?;
    let named = fs::symlink_metadata(path)?;
    if before.dev() != after.dev()
        || before.ino() != after.ino()
        || before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || before.dev() != named.dev()
        || before.ino() != named.ino()
        || expected.is_some_and(|digest| digest != sha256)
    {
        return Err(io::Error::other(
            "installed pair file changed or digest mismatched",
        ));
    }
    Ok(FileIdentity {
        device: before.dev(),
        inode: before.ino(),
        sha256,
    })
}

fn trusted_ancestry(path: &Path) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(io::Error::other("installed path is relative"));
    }
    let mut part = path;
    loop {
        let meta = fs::symlink_metadata(part)?;
        if meta.uid() != 0 || meta.mode() & 0o022 != 0 || meta.file_type().is_symlink() {
            return Err(io::Error::other("installed path ancestry is untrusted"));
        }
        if part == Path::new("/") {
            return Ok(());
        }
        part = part
            .parent()
            .ok_or_else(|| io::Error::other("invalid installed path"))?;
    }
}

fn refuse_stages(root: &Path) -> io::Result<()> {
    for entry in fs::read_dir(root)? {
        if entry?
            .file_name()
            .to_string_lossy()
            .starts_with(STAGE_PREFIX)
        {
            return Err(io::Error::other(
                "incomplete first-install activation stage",
            ));
        }
    }
    Ok(())
}

fn refuse_nonbootstrap_root(root: &Path) -> io::Result<()> {
    for entry in fs::read_dir(root)? {
        let name = entry?.file_name();
        if ![
            "state.db",
            "sidecar",
            "v30",
            "empty-v30-bootstrap-v1.json",
            "entry-gate.lock",
            "state.db-wal",
            "state.db-shm",
            "state.db.namespace.lock",
        ]
        .iter()
        .any(|allowed| name == *allowed)
        {
            return Err(io::Error::other(format!(
                "Broker root has nonbootstrap entry {}",
                name.to_string_lossy()
            )));
        }
        if name == "state.db-wal" || name == "state.db-shm" || name == "state.db.namespace.lock" {
            let meta = fs::symlink_metadata(root.join(&name))?;
            if !meta.is_file() || meta.uid() != 0 || meta.nlink() != 1 || meta.mode() & 0o077 != 0 {
                return Err(io::Error::other("untrusted bootstrap SQLite sidecar"));
            }
        }
    }
    Ok(())
}

fn require_empty_bootstrap_tables(root: &Path) -> io::Result<()> {
    for (relative, seeded) in [
        ("state.db", &["completed_turn_recovery_epoch"][..]),
        (
            "sidecar/pid-identity.db",
            &[
                "mailbox_sidecar_identity",
                "completion_continuation_domain",
                "completion_supervisor_authority",
                "completion_continuation_attempt_search_generation",
                "broker_sidecar_authority",
            ][..],
        ),
        (
            "v30/state.db",
            &["completed_turn_recovery_epoch", "fresh_lane_state_identity"][..],
        ),
        (
            "v30/sidecar/pid-identity.db",
            &[
                "mailbox_sidecar_identity",
                "completion_continuation_domain",
                "completion_supervisor_authority",
                "completion_continuation_attempt_search_generation",
                "broker_sidecar_authority",
                "fresh_lane_identity",
            ][..],
        ),
    ] {
        let conn =
            Connection::open_with_flags(root.join(relative), OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(io::Error::other)?;
        let mut query = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            )
            .map_err(io::Error::other)?;
        let names = query
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(io::Error::other)?;
        let mut seen = Vec::new();
        for name in names {
            let name = name.map_err(io::Error::other)?;
            let escaped = name.replace('"', "\"\"");
            let count: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM \"{escaped}\""), [], |row| {
                    row.get(0)
                })
                .map_err(io::Error::other)?;
            let expected = if seeded.contains(&name.as_str()) {
                1
            } else {
                0
            };
            if count != expected {
                return Err(io::Error::other("Broker State is not an empty bootstrap"));
            }
            seen.push(name);
        }
        if seeded
            .iter()
            .any(|name| !seen.iter().any(|found| found.as_str() == *name))
        {
            return Err(io::Error::other("empty bootstrap seed table absent"));
        }
    }
    let provider = root.join("v30/fresh-provider");
    if fs::read_dir(provider)?.next().is_some() {
        return Err(io::Error::other("fresh provider ledger is not empty"));
    }
    Ok(())
}

fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    let from = CString::new(from.as_os_str().as_bytes()).map_err(io::Error::other)?;
    let to = CString::new(to.as_os_str().as_bytes()).map_err(io::Error::other)?;
    if unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
