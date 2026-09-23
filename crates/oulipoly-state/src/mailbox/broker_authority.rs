//! Root-owned sidecar cutover boundary. This opens the same State database;
//! there is no second acceptance ledger. Migration of the quiesced v29 bytes
//! into this directory is a separate prerequisite to activation.
use super::*;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Component;

/// Retains the broker's live SQLite connection and its broker-minted source
/// generation. Legacy v4 native grants and pre-cutover attempts are not made
/// eligible for native K by opening this connection.
pub struct BrokerSidecar {
    mailbox: MailboxDb,
    source_generation: String,
}

impl BrokerSidecar {
    #[cfg(unix)]
    pub fn open_existing(path: &Path, broker_state_root: &Path) -> Result<Self, String> {
        require_host_root()?;
        open_with_owner(path, 0, broker_state_root)
    }

    #[cfg(not(unix))]
    pub fn open_existing(_path: &Path, _broker_state_root: &Path) -> Result<Self, String> {
        Err("broker-owned sidecar requires Unix root storage".into())
    }

    /// Call only after the v29 source and every old writer have been stopped
    /// and the complete main/WAL state has been copied into root-only storage.
    /// This operation never reads a guardian-selected source pathname.
    #[cfg(unix)]
    pub fn activate_quiesced_copy(path: &Path, broker_state_root: &Path) -> Result<String, String> {
        require_host_root()?;
        activate_with_owner(path, 0, broker_state_root)
    }

    #[cfg(not(unix))]
    pub fn activate_quiesced_copy(
        _path: &Path,
        _broker_state_root: &Path,
    ) -> Result<String, String> {
        Err("broker-owned sidecar requires Unix root storage".into())
    }

    pub fn source_generation(&self) -> &str {
        &self.source_generation
    }

    pub fn mailbox(&self) -> &MailboxDb {
        &self.mailbox
    }

    pub fn mailbox_mut(&mut self) -> &mut MailboxDb {
        &mut self.mailbox
    }
}

#[cfg(unix)]
fn require_host_root() -> Result<(), String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("broker-owned sidecar requires root".into());
    }
    Ok(())
}

#[cfg(unix)]
fn open_with_owner(path: &Path, owner: u32, anchor: &Path) -> Result<BrokerSidecar, String> {
    check_storage(path, owner, anchor)?;
    let authority = MailboxAuthorityFence::acquire(path).map_err(|e| e.to_string())?;
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|error| format!("Failed to open broker sidecar: {error}"))?;
    authority.validate_opened_target()?;
    configure_writable_sidecar_connection(&conn)?;
    let generation = schema::validate_broker_owned(&conn)?;
    let mode: String = conn
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err("broker sidecar requires WAL mode".into());
    }
    check_storage(path, owner, anchor)?;
    Ok(BrokerSidecar {
        mailbox: MailboxDb {
            conn,
            path: path.to_path_buf(),
            access_scope: AccessScope::live("broker_sidecar.live", SqliteDatabaseRole::PidMailbox),
            _read_only_snapshot: None,
            _namespace_authority: Some(authority),
        },
        source_generation: generation,
    })
}

#[cfg(unix)]
fn activate_with_owner(path: &Path, owner: u32, anchor: &Path) -> Result<String, String> {
    check_storage(path, owner, anchor)?;
    let authority = MailboxAuthorityFence::acquire_exclusive(path).map_err(|e| e.to_string())?;
    harden_artifacts(path, owner)?;
    let version: i64 = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|error| error.to_string())?
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if version != schema::CURRENT_VERSION {
        return Err("broker cutover requires an already migrated v29 copy".into());
    }
    let mut mailbox = MailboxDb::open_with_authority(&authority)?;
    schema::validate_exact_v29(&mailbox.conn)?;
    let generation = uuid::Uuid::new_v4().to_string();
    let tx = mailbox
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| format!("Failed to start broker cutover: {error}"))?;
    tx.execute_batch(schema::BROKER_AUTHORITY_SCHEMA)
        .map_err(|error| format!("Failed to create broker authority: {error}"))?;
    tx.execute(
        "INSERT INTO broker_sidecar_authority(singleton,source_generation,activated_at)
         VALUES(1,?1,?2)",
        params![generation, Utc::now().to_rfc3339()],
    )
    .map_err(|error| format!("Failed to mint broker source generation: {error}"))?;
    tx.pragma_update(None, "user_version", schema::BROKER_OWNED_VERSION)
        .map_err(|error| format!("Failed to record broker sidecar version: {error}"))?;
    tx.commit()
        .map_err(|error| format!("Failed to commit broker cutover: {error}"))?;
    harden_artifacts(path, owner)?;
    drop(mailbox);
    drop(authority);
    let opened = open_with_owner(path, owner, anchor)?;
    Ok(opened.source_generation)
}

#[cfg(unix)]
fn harden_artifacts(path: &Path, owner: u32) -> Result<(), String> {
    for artifact in [
        path.to_path_buf(),
        path_with_storage_suffix(path, "-wal"),
        path_with_storage_suffix(path, "-shm"),
        path_with_storage_suffix(path, "-journal"),
        mailbox_authority_path(path),
    ] {
        match fs::symlink_metadata(&artifact) {
            Ok(meta)
                if meta.is_file()
                    && !meta.file_type().is_symlink()
                    && meta.uid() == owner
                    && meta.nlink() == 1 =>
            {
                fs::set_permissions(&artifact, fs::Permissions::from_mode(0o600))
                    .map_err(|error| error.to_string())?;
            }
            Err(error) if error.kind() == ErrorKind::NotFound && artifact != path => {}
            _ => {
                return Err(format!(
                    "broker sidecar artifact changed: {}",
                    artifact.display()
                ));
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn check_storage(path: &Path, owner: u32, anchor: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path.file_name() != Some(std::ffi::OsStr::new("pid-identity.db"))
        || path.parent().and_then(Path::file_name) != Some(std::ffi::OsStr::new("sidecar"))
        || !path.starts_with(anchor)
        || path
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
    {
        return Err("broker sidecar path is not the fixed storage name".into());
    }
    let mut directory = path.parent().ok_or("broker sidecar parent missing")?;
    loop {
        let meta = fs::symlink_metadata(directory).map_err(|error| error.to_string())?;
        if !meta.is_dir()
            || meta.file_type().is_symlink()
            || meta.uid() != owner
            || meta.mode() & 0o022 != 0
        {
            return Err("broker sidecar has an untrusted path ancestor".into());
        }
        if directory == path.parent().unwrap() && meta.mode() & 0o077 != 0 {
            return Err("broker sidecar directory is not root-only".into());
        }
        if directory == anchor {
            break;
        }
        directory = directory.parent().ok_or("broker sidecar anchor missing")?;
    }
    for artifact in [
        path.to_path_buf(),
        path_with_storage_suffix(path, "-wal"),
        path_with_storage_suffix(path, "-shm"),
        path_with_storage_suffix(path, "-journal"),
        mailbox_authority_path(path),
    ] {
        match fs::symlink_metadata(&artifact) {
            Ok(meta) => {
                if !meta.is_file()
                    || meta.file_type().is_symlink()
                    || meta.uid() != owner
                    || meta.nlink() != 1
                    || meta.mode() & 0o077 != 0
                {
                    return Err(format!(
                        "broker sidecar artifact is not root-only: {}",
                        artifact.display()
                    ));
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound && artifact != path => {}
            Err(error) => return Err(format!("broker sidecar artifact unavailable: {error}")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[cfg(unix)]
    #[test]
    fn quiesced_v29_copy_activates_once_and_direct_writers_refuse_v30() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("sidecar");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.join("pid-identity.db");
        drop(MailboxDb::open(&path).unwrap());
        for artifact in [path.clone(), mailbox_authority_path(&path)] {
            fs::set_permissions(artifact, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let uid = unsafe { libc::geteuid() };
        assert!(open_with_owner(&path, uid, root.path()).is_err());
        assert!(check_storage(&path, uid.saturating_add(1), root.path()).is_err());
        let generation = activate_with_owner(&path, uid, root.path()).unwrap();
        assert_eq!(
            open_with_owner(&path, uid, root.path())
                .unwrap()
                .source_generation(),
            generation
        );
        assert!(MailboxDb::open(&path).is_err());
        assert!(MailboxDb::open_existing_native_authority(&path).is_err());
        assert!(crate::pid_identity::PidIdentityDb::open(&path).is_err());
        assert!(activate_with_owner(&path, uid, root.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn broker_storage_rejects_exposed_directory_and_symlinked_main_or_wal() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("sidecar");
        fs::create_dir(&directory).unwrap();
        let path = directory.join("pid-identity.db");
        fs::write(&path, b"not sqlite").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let uid = unsafe { libc::geteuid() };
        assert!(check_storage(&path, uid, root.path()).is_err());
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(check_storage(&path, uid, root.path()).is_ok());

        let other = root.path().join("other");
        fs::write(&other, b"copy").unwrap();
        fs::remove_file(&path).unwrap();
        symlink(&other, &path).unwrap();
        assert!(check_storage(&path, uid, root.path()).is_err());
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"not sqlite").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&other, path_with_storage_suffix(&path, "-wal")).unwrap();
        assert!(check_storage(&path, uid, root.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn committed_v29_wal_content_survives_quiesced_copy_and_broker_restart() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.pid-identity.db");
        drop(MailboxDb::open(&source).unwrap());
        let source_connection = Connection::open(&source).unwrap();
        source_connection
            .execute_batch(
                "PRAGMA wal_autocheckpoint=0;
                 CREATE TABLE cutover_probe(value TEXT NOT NULL);
                 INSERT INTO cutover_probe VALUES('committed-in-wal');",
            )
            .unwrap();
        let directory = root.path().join("sidecar");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let target = directory.join("pid-identity.db");
        source_connection
            .execute("VACUUM INTO ?1", [target.to_str().unwrap()])
            .unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        drop(source_connection);

        let uid = unsafe { libc::geteuid() };
        let generation = activate_with_owner(&target, uid, root.path()).unwrap();
        let first = open_with_owner(&target, uid, root.path()).unwrap();
        let value: String = first
            .mailbox
            .conn
            .query_row("SELECT value FROM cutover_probe", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "committed-in-wal");
        drop(first);
        assert_eq!(
            open_with_owner(&target, uid, root.path())
                .unwrap()
                .source_generation(),
            generation
        );
    }
}
