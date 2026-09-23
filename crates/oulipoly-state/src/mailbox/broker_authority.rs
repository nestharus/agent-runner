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
    storage_owner: u32,
    storage_anchor: std::path::PathBuf,
}

/// One exact, generation-bound observation from the broker's retained SQLite
/// connection. It is evidence for reconciliation, never a launch grant.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct BrokerContinuationReadback {
    pub source_generation: String,
    pub root_id: String,
    pub owner: super::CompletionDomainOwner,
    pub attempt: Option<super::ContinuationAttempt>,
    pub phase: Option<String>,
    pub revision: Option<i64>,
    pub claim_present: bool,
    /// Only rows published after v30 activation can enter broker mutations.
    pub broker_owned: bool,
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

    /// The broker must derive `owner` from its pinned guardian/driver registry,
    /// not decode it from the wire. A failed marker write leaves the owner
    /// unmarked and therefore unable to reserve or accept through this lane.
    pub fn publish_exact_owner(
        &mut self,
        owner: &super::CompletionDomainOwner,
        root_id: &str,
    ) -> Result<BrokerContinuationReadback, String> {
        #[cfg(unix)]
        check_storage(&self.mailbox.path, self.storage_owner, &self.storage_anchor)?;
        broker_main_file_must_be_named(&self.mailbox.conn)?;
        if uuid::Uuid::parse_str(&owner.owner_generation)
            .map(|id| id.to_string())
            .ok()
            .as_deref()
            != Some(&owner.owner_generation)
        {
            return Err("broker owner generation is not canonical".into());
        }
        self.mailbox
            .publish_completion_owner_with_kernel_root(owner, Some(root_id))?;
        self.mailbox
            .conn
            .execute(
                "INSERT INTO broker_completion_owner(owner_generation,source_generation,root_id,
                guardian_identity,driver_identity) VALUES(?1,?2,?3,?4,?5)",
                params![
                    owner.owner_generation,
                    self.source_generation,
                    root_id,
                    serde_json::to_string(&owner.guardian_identity).map_err(|e| e.to_string())?,
                    serde_json::to_string(&owner.driver_identity).map_err(|e| e.to_string())?
                ],
            )
            .map_err(|e| e.to_string())?;
        self.read_exact_continuation(
            &self.source_generation,
            root_id,
            &owner.domain_id,
            &owner.supervisor_authority_id,
            &owner.owner_generation,
            None,
        )
    }

    pub fn reserve_exact_attempt(
        &mut self,
        owner: &super::CompletionDomainOwner,
        root_id: &str,
        attempt: &super::ContinuationAttempt,
    ) -> Result<BrokerContinuationReadback, String> {
        let before = self.read_exact_continuation(
            &self.source_generation,
            root_id,
            &owner.domain_id,
            &owner.supervisor_authority_id,
            &owner.owner_generation,
            Some(&attempt.attempt_id),
        )?;
        if !before.broker_owned
            || before.owner != *owner
            || attempt.owner_generation != owner.owner_generation
            || before.attempt.is_some()
        {
            return Err("broker reservation owner/attempt conflict".into());
        }
        self.mailbox.reserve_continuation_attempt(attempt)?;
        let after = self.read_exact_continuation(
            &self.source_generation,
            root_id,
            &owner.domain_id,
            &owner.supervisor_authority_id,
            &owner.owner_generation,
            Some(&attempt.attempt_id),
        )?;
        if after.attempt.as_ref() != Some(attempt)
            || after.phase.as_deref() != Some("reserved")
            || after.revision != Some(1)
            || (attempt.operation == "activation" && !after.claim_present)
        {
            return Err("broker reservation readback conflict".into());
        }
        Ok(after)
    }

    pub fn accept_exact_attempt(
        &mut self,
        owner: &super::CompletionDomainOwner,
        root_id: &str,
        attempt: &super::ContinuationAttempt,
    ) -> Result<
        (
            super::AcceptedNativeGrantSnapshot,
            BrokerContinuationReadback,
        ),
        String,
    > {
        let before = self.read_exact_continuation(
            &self.source_generation,
            root_id,
            &owner.domain_id,
            &owner.supervisor_authority_id,
            &owner.owner_generation,
            Some(&attempt.attempt_id),
        )?;
        if !before.broker_owned
            || before.owner != *owner
            || before.attempt.as_ref() != Some(attempt)
            || before.phase.as_deref() != Some("reserved")
            || before.revision != Some(1)
            || (attempt.operation == "activation" && !before.claim_present)
        {
            return Err("broker acceptance reservation/claim conflict".into());
        }
        let snapshot = self
            .mailbox
            .accept_exact_native_attempt(attempt, owner, root_id)?;
        let after = self.read_exact_continuation(
            &self.source_generation,
            root_id,
            &owner.domain_id,
            &owner.supervisor_authority_id,
            &owner.owner_generation,
            Some(&attempt.attempt_id),
        )?;
        if after.attempt.as_ref() != Some(attempt)
            || after.phase.as_deref() != Some("accepted")
            || after.revision != Some(2)
            || !after.broker_owned
        {
            return Err("broker acceptance readback conflict".into());
        }
        Ok((snapshot, after))
    }

    /// Exact owner, reservation, wake claim and acceptance readback in one
    /// read transaction. All selectors are checked against broker registry
    /// facts before this method is called; no caller-selected pathname exists.
    pub fn read_exact_continuation(
        &self,
        source_generation: &str,
        root_id: &str,
        domain_id: &str,
        supervisor_id: &str,
        owner_generation: &str,
        attempt_id: Option<&str>,
    ) -> Result<BrokerContinuationReadback, String> {
        if source_generation != self.source_generation || root_id.is_empty() {
            return Err("broker source generation/root conflict".into());
        }
        #[cfg(unix)]
        check_storage(&self.mailbox.path, self.storage_owner, &self.storage_anchor)?;
        broker_main_file_must_be_named(&self.mailbox.conn)?;
        let tx = self
            .mailbox
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        let row = tx
            .query_row(
                "SELECT domain_id,supervisor_authority_id,generation,guardian_identity,
                    driver_identity,endpoint FROM completion_continuation_owner
             WHERE generation=?1 AND phase='running' AND kernel_root_id=?2",
                params![owner_generation, root_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or("broker exact running owner absent")?;
        if row.0 != domain_id || row.1 != supervisor_id {
            return Err("broker root/domain/supervisor conflict".into());
        }
        let owner = super::CompletionDomainOwner {
            protocol: crate::completion_continuation::PROTOCOL.into(),
            domain_id: row.0,
            supervisor_authority_id: row.1,
            owner_generation: row.2,
            guardian_identity: serde_json::from_str(&row.3).map_err(|e| e.to_string())?,
            driver_identity: serde_json::from_str(&row.4).map_err(|e| e.to_string())?,
            endpoint: row.5,
        };
        let attempt_row = match attempt_id {
            Some(id) => tx
                .query_row(
                    "SELECT attempt_id,owner_generation,operation,request_sha256,
                        source_registration_id,source_listener_revision,session_id,
                        claim_token,result_path,phase,revision,domain_id
                 FROM completion_continuation_attempt WHERE attempt_id=?1",
                    [id],
                    |row| {
                        Ok((
                            super::ContinuationAttempt {
                                attempt_id: row.get(0)?,
                                owner_generation: row.get(1)?,
                                operation: row.get(2)?,
                                request_sha256: row.get(3)?,
                                source_registration_id: row.get(4)?,
                                source_listener_revision: row.get(5)?,
                                session_id: row.get(6)?,
                                claim_token: row.get(7)?,
                                result_path: row.get(8)?,
                            },
                            row.get::<_, String>(9)?,
                            row.get::<_, i64>(10)?,
                            row.get::<_, String>(11)?,
                        ))
                    },
                )
                .optional()
                .map_err(|e| e.to_string())?,
            None => None,
        };
        if attempt_row.as_ref().is_some_and(|(attempt, _, _, domain)| {
            attempt.owner_generation != owner_generation || domain != domain_id
        }) {
            return Err("broker attempt belongs to another owner".into());
        }
        let claim_present = if let Some((attempt, _, _, _)) = &attempt_row {
            if let (Some(session), Some(token)) = (&attempt.session_id, &attempt.claim_token) {
                tx.query_row("SELECT EXISTS(SELECT 1 FROM session_wake_claim WHERE session_id=?1 AND claim_token=?2)",
                    params![session, token], |row| row.get(0)).map_err(|e| e.to_string())?
            } else {
                false
            }
        } else {
            false
        };
        let broker_owned: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM broker_completion_owner WHERE owner_generation=?1
                AND source_generation=?2 AND root_id=?3 AND guardian_identity=?4
                AND driver_identity=?5)",
                params![
                    owner_generation,
                    self.source_generation,
                    root_id,
                    row.3,
                    row.4
                ],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        let result = BrokerContinuationReadback {
            source_generation: self.source_generation.clone(),
            root_id: root_id.into(),
            owner,
            attempt: attempt_row.as_ref().map(|row| row.0.clone()),
            phase: attempt_row.as_ref().map(|row| row.1.clone()),
            revision: attempt_row.as_ref().map(|row| row.2),
            claim_present,
            broker_owned,
        };
        tx.commit().map_err(|e| e.to_string())?;
        broker_main_file_must_be_named(&self.mailbox.conn)?;
        Ok(result)
    }
}

#[cfg(target_os = "linux")]
fn broker_main_file_must_be_named(conn: &Connection) -> Result<(), String> {
    let mut moved: libc::c_int = -1;
    let status = unsafe {
        rusqlite::ffi::sqlite3_file_control(
            conn.handle(),
            c"main".as_ptr(),
            rusqlite::ffi::SQLITE_FCNTL_HAS_MOVED,
            (&mut moved as *mut libc::c_int).cast(),
        )
    };
    if status != rusqlite::ffi::SQLITE_OK || moved != 0 {
        return Err("broker SQLite main file moved".into());
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn broker_main_file_must_be_named(_conn: &Connection) -> Result<(), String> {
    Err("broker readback requires Linux SQLite file verification".into())
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
        storage_owner: owner,
        storage_anchor: anchor.to_path_buf(),
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
    tx.execute_batch(schema::BROKER_OWNER_SCHEMA)
        .map_err(|error| format!("Failed to create broker owner provenance: {error}"))?;
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
    use crate::completion_continuation::{PROTOCOL, SourceProcessIdentity};
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

    #[cfg(unix)]
    #[test]
    fn exact_owner_attempt_claim_and_acceptance_use_retained_broker_connection() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.pid-identity.db");
        let mut db = MailboxDb::open(&source).unwrap();
        let domain = db.completion_continuation_domain().unwrap().unwrap();
        let live = crate::pid_identity::read_current_process_identity().unwrap();
        let identity = SourceProcessIdentity {
            pid: live.os_pid,
            boot_id: live.os_boot_id,
            starttime_ticks: live.os_pid_starttime_ticks,
        };
        let root_id = uuid::Uuid::new_v4().to_string();
        let owner = super::super::CompletionDomainOwner {
            protocol: PROTOCOL.into(),
            domain_id: domain.clone(),
            supervisor_authority_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: uuid::Uuid::new_v4().to_string(),
            guardian_identity: identity.clone(),
            driver_identity: identity,
            endpoint: "/fixture/owner".into(),
        };
        db.publish_completion_owner_with_kernel_root(&owner, Some(&root_id))
            .unwrap();
        db.conn.execute("INSERT INTO session_wake_claim(session_id,claim_token,claimed_at,reason,auto_wake_count) VALUES('session','claim','2026-09-23T00:00:00Z','fixture',1)", []).unwrap();
        let attempt = super::super::ContinuationAttempt {
            attempt_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: owner.owner_generation.clone(),
            operation: "activation".into(),
            request_sha256: "a".repeat(64),
            source_registration_id: None,
            source_listener_revision: None,
            session_id: Some("session".into()),
            claim_token: Some("claim".into()),
            result_path: "/fixture/result".into(),
        };
        db.reserve_continuation_attempt(&attempt).unwrap();
        drop(db);
        let directory = root.path().join("sidecar");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let target = directory.join("pid-identity.db");
        Connection::open(&source)
            .unwrap()
            .execute("VACUUM INTO ?1", [target.to_str().unwrap()])
            .unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        let uid = unsafe { libc::geteuid() };
        let generation = activate_with_owner(&target, uid, root.path()).unwrap();
        let mut broker = open_with_owner(&target, uid, root.path()).unwrap();
        let read = |broker: &BrokerSidecar,
                    generation: &str,
                    root: &str,
                    owner_id: &str,
                    attempt_id: Option<&str>| {
            broker.read_exact_continuation(
                generation,
                root,
                &domain,
                &owner.supervisor_authority_id,
                owner_id,
                attempt_id,
            )
        };
        let reserved = read(
            &broker,
            &generation,
            &root_id,
            &owner.owner_generation,
            Some(&attempt.attempt_id),
        )
        .unwrap();
        assert_eq!(reserved.owner, owner);
        assert_eq!(reserved.attempt, Some(attempt.clone()));
        assert_eq!(reserved.phase.as_deref(), Some("reserved"));
        assert_eq!(reserved.revision, Some(1));
        assert!(reserved.claim_present);
        assert!(
            !reserved.broker_owned,
            "copied v29 owner is only observable"
        );
        assert!(
            read(
                &broker,
                "wrong-generation",
                &root_id,
                &owner.owner_generation,
                Some(&attempt.attempt_id)
            )
            .is_err()
        );
        assert!(
            read(
                &broker,
                &generation,
                &uuid::Uuid::new_v4().to_string(),
                &owner.owner_generation,
                Some(&attempt.attempt_id)
            )
            .is_err()
        );
        assert!(
            read(
                &broker,
                &generation,
                &root_id,
                &uuid::Uuid::new_v4().to_string(),
                Some(&attempt.attempt_id)
            )
            .is_err()
        );
        assert!(
            read(
                &broker,
                &generation,
                &root_id,
                &owner.owner_generation,
                Some(&uuid::Uuid::new_v4().to_string())
            )
            .unwrap()
            .attempt
            .is_none()
        );
        MailboxDb::open(&source)
            .unwrap()
            .accept_continuation_attempt(&attempt)
            .unwrap();
        assert_eq!(
            read(
                &broker,
                &generation,
                &root_id,
                &owner.owner_generation,
                Some(&attempt.attempt_id)
            )
            .unwrap()
            .phase
            .as_deref(),
            Some("reserved"),
            "a stale writer's retired source cannot alter the broker endpoint"
        );
        broker
            .mailbox_mut()
            .accept_exact_native_attempt(&attempt, &owner, &root_id)
            .unwrap();
        let accepted = read(
            &broker,
            &generation,
            &root_id,
            &owner.owner_generation,
            Some(&attempt.attempt_id),
        )
        .unwrap();
        assert_eq!(accepted.phase.as_deref(), Some("accepted"));
        assert_eq!(accepted.revision, Some(2));
        assert!(accepted.claim_present);
        assert!(!accepted.broker_owned);
        drop(broker);
        let mut broker = open_with_owner(&target, uid, root.path()).unwrap();
        assert_eq!(
            read(
                &broker,
                &generation,
                &root_id,
                &owner.owner_generation,
                Some(&attempt.attempt_id)
            )
            .unwrap(),
            accepted
        );
        let mut new_owner = owner.clone();
        new_owner.owner_generation = uuid::Uuid::new_v4().to_string();
        let published = broker.publish_exact_owner(&new_owner, &root_id).unwrap();
        assert!(published.broker_owned);
        let new_attempt = super::super::ContinuationAttempt {
            attempt_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: new_owner.owner_generation.clone(),
            operation: "transport".into(),
            request_sha256: "b".repeat(64),
            source_registration_id: None,
            source_listener_revision: None,
            session_id: None,
            claim_token: None,
            result_path: "/fixture/transport-result".into(),
        };
        let new_reserved = broker
            .reserve_exact_attempt(&new_owner, &root_id, &new_attempt)
            .unwrap();
        assert!(new_reserved.broker_owned);
        assert_eq!(new_reserved.phase.as_deref(), Some("reserved"));
        assert!(
            broker
                .reserve_exact_attempt(&new_owner, &root_id, &new_attempt)
                .is_err(),
            "reservation replay must not create a second row"
        );
        let (_, new_accepted) = broker
            .accept_exact_attempt(&new_owner, &root_id, &new_attempt)
            .unwrap();
        assert_eq!(new_accepted.phase.as_deref(), Some("accepted"));
        assert!(
            broker
                .accept_exact_attempt(&new_owner, &root_id, &new_attempt)
                .is_err(),
            "acceptance replay must not advance twice"
        );
        drop(broker);
        assert_eq!(
            read(
                &open_with_owner(&target, uid, root.path()).unwrap(),
                &generation,
                &root_id,
                &new_owner.owner_generation,
                Some(&new_attempt.attempt_id)
            )
            .unwrap(),
            new_accepted
        );

        let copied = directory.join("copied.db");
        fs::copy(&target, &copied).unwrap();
        fs::set_permissions(&copied, fs::Permissions::from_mode(0o600)).unwrap();
        let old = directory.join("old.db");
        let pinned = open_with_owner(&target, uid, root.path()).unwrap();
        fs::rename(&target, &old).unwrap();
        fs::rename(&copied, &target).unwrap();
        assert!(
            read(
                &pinned,
                &generation,
                &root_id,
                &owner.owner_generation,
                Some(&attempt.attempt_id)
            )
            .is_err()
        );
        drop(pinned);
        let wal = path_with_storage_suffix(&target, "-wal");
        let _ = fs::remove_file(&wal);
        std::os::unix::fs::symlink(&old, &wal).unwrap();
        assert!(open_with_owner(&target, uid, root.path()).is_err());
    }
}
