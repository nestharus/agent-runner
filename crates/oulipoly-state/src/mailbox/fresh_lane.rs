//! Fresh, independent v30 storage. Publication never reads or copies a v29 file.
//! A failed prepublication build leaves only an inert hidden staging directory;
//! a lost publication reply reopens the same immutable identity.
use super::broker_authority::{
    BoundStateSource, activate_with_owner, require_root_owned_ancestors,
};
use super::*;
use crate::StateDb;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

const LANE_DIRECTORY: &str = "v30";
const LANE_PROTOCOL: &str = "fresh-v30-lane-v1";
const FRESH_SCHEMA: &str = include_str!("migrations/0030_fresh_lane.sql");
const FRESH_STATE_SCHEMA: &str = include_str!("migrations/0030_fresh_state_identity.sql");

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FreshV30LaneIdentity {
    pub lane_id: String,
    pub domain_id: String,
    pub source_generation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FreshV30Session {
    pub lane_id: String,
    pub source_generation: String,
    pub session_id: String,
    pub request_id: String,
    pub allocation_id: String,
}

/// A broker-retained connection; no caller supplies a ledger path to an
/// operation. The public path argument is an installation root for bootstrap
/// and private fixtures only, never a wire selector.
pub struct FreshV30Lane {
    sidecar: BrokerSidecar,
    identity: FreshV30LaneIdentity,
}

impl FreshV30Lane {
    /// Reserved namespace for broker-minted session IDs. Legacy unqualified
    /// commands must refuse it even if an old row has the same spelling.
    pub fn is_reserved_session_id(session_id: &str) -> bool {
        let mut parts = session_id.split(':');
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some("v30"), Some(lane), Some(session), None) => [lane, session]
                .into_iter()
                .all(|part| Uuid::parse_str(part).is_ok_and(|id| id.to_string() == part)),
            _ => false,
        }
    }

    pub fn initialize_at(broker_root: &Path) -> Result<FreshV30LaneIdentity, String> {
        require_root()?;
        // The installed broker is a dedicated root process. SQLite may create
        // WAL/SHM while opening, so its process umask must protect those too.
        unsafe { libc::umask(0o077) };
        require_broker_root(broker_root)?;
        let final_root = broker_root.join(LANE_DIRECTORY);
        if final_root.exists() {
            return Ok(Self::open_at(broker_root)?.identity);
        }
        let stage_root = broker_root.join(format!(".v30-fresh-{}", Uuid::new_v4().simple()));
        fs::create_dir(&stage_root).map_err(|e| e.to_string())?;
        fs::set_permissions(&stage_root, fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
        let sidecar_root = stage_root.join("sidecar");
        fs::create_dir(&sidecar_root).map_err(|e| e.to_string())?;
        fs::set_permissions(&sidecar_root, fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
        let state_path = stage_root.join("state.db");
        let mailbox_path = sidecar_root.join("pid-identity.db");
        // Both constructors start from absent files. No old writer or WAL is
        // consulted, and no quiescence proof is relevant to this lane.
        drop(StateDb::open_historical_without_observation(&state_path)?);
        let mailbox = MailboxDb::open_historical_without_observation(&mailbox_path)?;
        let domain_id = mailbox
            .completion_continuation_domain()?
            .ok_or("fresh mailbox domain absent")?;
        drop(mailbox);
        let source_generation = activate_with_owner(&mailbox_path, 0, &stage_root)?;
        let lane_id = Uuid::new_v4().to_string();
        let identity = FreshV30LaneIdentity {
            lane_id,
            domain_id,
            source_generation,
        };
        let state_meta = fs::symlink_metadata(&state_path).map_err(|e| e.to_string())?;
        if !state_meta.is_file()
            || state_meta.is_symlink()
            || state_meta.uid() != 0
            || state_meta.nlink() != 1
        {
            return Err("fresh State file has wrong physical owner".into());
        }
        let sidecar = BrokerSidecar::open_existing(&mailbox_path, &stage_root)?;
        sidecar
            .mailbox()
            .conn
            .execute_batch(FRESH_SCHEMA)
            .map_err(|e| e.to_string())?;
        sidecar
            .mailbox()
            .conn
            .execute(
                "INSERT INTO fresh_lane_identity VALUES(1,?1,?2,?3,?4,?5,?6)",
                params![
                    LANE_PROTOCOL,
                    identity.lane_id,
                    identity.domain_id,
                    identity.source_generation,
                    state_meta.dev() as i64,
                    state_meta.ino() as i64
                ],
            )
            .map_err(|e| e.to_string())?;
        drop(sidecar);
        let state_conn = Connection::open(&state_path).map_err(|e| e.to_string())?;
        state_conn
            .execute_batch(FRESH_STATE_SCHEMA)
            .map_err(|e| e.to_string())?;
        state_conn
            .execute(
                "INSERT INTO fresh_lane_state_identity VALUES(1,?1,?2,?3,?4)",
                params![
                    LANE_PROTOCOL,
                    identity.lane_id,
                    identity.domain_id,
                    identity.source_generation
                ],
            )
            .map_err(|e| e.to_string())?;
        drop(state_conn);
        // The binding names the *published* path while recording the staged
        // inode. It becomes valid only at the atomic no-replace rename.
        let projected = BoundStateSource {
            path: final_root.join("state.db"),
            owner: 0,
            device: state_meta.dev(),
            inode: state_meta.ino(),
        };
        let mut binding = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(sidecar_root.join("state-source.json"))
            .map_err(|e| e.to_string())?;
        serde_json::to_writer(&mut binding, &projected).map_err(|e| e.to_string())?;
        binding.sync_all().map_err(|e| e.to_string())?;
        harden_tree(&stage_root)?;
        File::open(&sidecar_root)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        File::open(&stage_root)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        rename_noreplace(&stage_root, &final_root)?;
        File::open(broker_root)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        Ok(Self::open_at(broker_root)?.identity)
    }

    pub fn open_at(broker_root: &Path) -> Result<Self, String> {
        require_root()?;
        unsafe { libc::umask(0o077) };
        require_broker_root(broker_root)?;
        let lane_root = broker_root.join(LANE_DIRECTORY);
        let sidecar =
            BrokerSidecar::open_existing(&lane_root.join("sidecar/pid-identity.db"), &lane_root)?;
        let state = sidecar.bound_state()?;
        let state_meta =
            fs::symlink_metadata(lane_root.join("state.db")).map_err(|e| e.to_string())?;
        if state_meta.uid() != 0 || state_meta.mode() & 0o077 != 0 || state_meta.nlink() != 1 {
            return Err("fresh State storage is not root-only".into());
        }
        let row = sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT protocol,lane_id,domain_id,source_generation,state_device,state_inode
             FROM fresh_lane_identity WHERE singleton=1",
                [],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, i64>(5)?,
                    ))
                },
            )
            .map_err(|e| format!("fresh lane identity absent: {e}"))?;
        let protected_schema_count: i64 = sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE
             (type='table' AND name IN ('fresh_lane_identity','fresh_lane_session')) OR
             (type='trigger' AND name IN ('fresh_lane_identity_no_update',
              'fresh_lane_identity_no_delete','fresh_lane_session_no_update',
              'fresh_lane_session_no_delete'))",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if protected_schema_count != 6 {
            return Err("fresh lane immutable schema is incomplete".into());
        }
        let request_key_columns: i64 = sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('fresh_lane_session') WHERE name='request_id' AND \"notnull\"=1",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if request_key_columns != 1 {
            return Err("fresh lane session request identity is absent".into());
        }
        let unique_request_indexes: i64 = sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT count(*) FROM pragma_index_list('fresh_lane_session') AS idx
                 WHERE idx.\"unique\"=1
                   AND (SELECT count(*) FROM pragma_index_info(idx.name))=1
                   AND (SELECT name FROM pragma_index_info(idx.name))='request_id'",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if unique_request_indexes != 1 {
            return Err("fresh lane session request identity is not unique".into());
        }
        let identity = FreshV30LaneIdentity {
            lane_id: row.1,
            domain_id: row.2,
            source_generation: row.3,
        };
        for id in [
            &identity.lane_id,
            &identity.domain_id,
            &identity.source_generation,
        ] {
            let parsed = Uuid::parse_str(id).map_err(|_| "invalid fresh lane UUID")?;
            if parsed.to_string() != *id {
                return Err("noncanonical fresh lane UUID".into());
            }
        }
        if row.0 != LANE_PROTOCOL
            || identity.domain_id != sidecar.domain_id()?
            || identity.source_generation != sidecar.source_generation()
            || row.4 != state_meta.dev() as i64
            || row.5 != state_meta.ino() as i64
            || state.path() != lane_root.join("state.db")
        {
            return Err("fresh lane State/mailbox identity mismatch".into());
        }
        let state_conn = Connection::open_with_flags(
            lane_root.join("state.db"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| e.to_string())?;
        let state_identity = state_conn
            .query_row(
                "SELECT protocol,lane_id,domain_id,source_generation
             FROM fresh_lane_state_identity WHERE singleton=1",
                [],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .map_err(|e| format!("fresh State identity absent: {e}"))?;
        let state_trigger_count: i64 = state_conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='trigger' AND name IN
             ('fresh_lane_state_identity_no_update','fresh_lane_state_identity_no_delete')",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if state_identity
            != (
                LANE_PROTOCOL.to_owned(),
                identity.lane_id.clone(),
                identity.domain_id.clone(),
                identity.source_generation.clone(),
            )
            || state_trigger_count != 2
        {
            return Err("fresh State and mailbox lane identities differ".into());
        }
        Ok(Self { sidecar, identity })
    }

    pub fn identity(&self) -> &FreshV30LaneIdentity {
        &self.identity
    }

    /// A request UUID is created before the first send and retained by the
    /// caller until readback. SQLite's unique request key makes concurrent
    /// retries and lost replies return the exact first allocation. The key
    /// never selects a session ID or an old ledger.
    pub fn allocate_session(&mut self, request_id: &str) -> Result<FreshV30Session, String> {
        validate_request_id(request_id)?;
        if let Some(existing) = self.read_session(request_id)? {
            return Ok(existing);
        }
        let allocation_id = Uuid::new_v4().to_string();
        let session = FreshV30Session {
            lane_id: self.identity.lane_id.clone(),
            source_generation: self.identity.source_generation.clone(),
            session_id: format!("v30:{}:{}", self.identity.lane_id, Uuid::new_v4()),
            request_id: request_id.to_owned(),
            allocation_id,
        };
        self.sidecar.mailbox().conn.execute(
            "INSERT INTO fresh_lane_session(session_id,request_id,allocation_id,lane_id,source_generation,allocated_at)
             VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(request_id) DO NOTHING",
            params![session.session_id, session.request_id, session.allocation_id,
                session.lane_id, session.source_generation, Utc::now().to_rfc3339()],
        ).map_err(|e| e.to_string())?;
        self.read_session(request_id)?
            .ok_or_else(|| "fresh session allocation lost its durable row".into())
    }

    /// Exact readback is available without allocating, including after a
    /// restart or while the entry gate is closed.
    pub fn read_session(&self, request_id: &str) -> Result<Option<FreshV30Session>, String> {
        validate_request_id(request_id)?;
        let mut query = self
            .sidecar
            .mailbox()
            .conn
            .prepare(
                "SELECT session_id,request_id,allocation_id,lane_id,source_generation
             FROM fresh_lane_session WHERE request_id=?1",
            )
            .map_err(|e| e.to_string())?;
        let mut rows = query
            .query(params![request_id])
            .map_err(|e| e.to_string())?;
        let session = rows
            .next()
            .map_err(|e| e.to_string())?
            .map(|row| {
                Ok::<_, rusqlite::Error>(FreshV30Session {
                    session_id: row.get(0)?,
                    request_id: row.get(1)?,
                    allocation_id: row.get(2)?,
                    lane_id: row.get(3)?,
                    source_generation: row.get(4)?,
                })
            })
            .transpose()
            .map_err(|e| e.to_string())?;
        if let Some(ref session) = session {
            self.require_session(session)?;
        }
        Ok(session)
    }

    /// Routing prerequisite for a later v30 resume, wake, recipient or ACK
    /// protocol. This alone grants no work, delivery or acknowledgement.
    /// An unqualified `(session,seq)` is not an authority input.
    pub fn require_session(&self, session: &FreshV30Session) -> Result<(), String> {
        if session.lane_id != self.identity.lane_id
            || session.source_generation != self.identity.source_generation
        {
            return Err("wrong fresh lane or generation".into());
        }
        let exists: bool = self
            .sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM fresh_lane_session WHERE session_id=?1
             AND request_id=?2 AND allocation_id=?3 AND lane_id=?4 AND source_generation=?5)",
                params![
                    session.session_id,
                    session.request_id,
                    session.allocation_id,
                    session.lane_id,
                    session.source_generation
                ],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !exists {
            return Err("session was not allocated by this lane".into());
        }
        Ok(())
    }

    /// Exact lane/row membership only; recipient and ACK grants are separate.
    pub fn require_mailbox_row(&self, session: &FreshV30Session, seq: i64) -> Result<(), String> {
        self.require_session(session)?;
        let exists: bool = self
            .sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM mailbox WHERE session_id=?1 AND seq=?2)",
                params![session.session_id, seq],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !exists {
            return Err("mailbox row is not in this lane".into());
        }
        Ok(())
    }
}

fn validate_request_id(request_id: &str) -> Result<(), String> {
    let parsed = Uuid::parse_str(request_id).map_err(|_| "invalid fresh request UUID")?;
    if parsed.is_nil() || parsed.to_string() != request_id {
        return Err("noncanonical or nil fresh request UUID".into());
    }
    Ok(())
}

fn require_root() -> Result<(), String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("fresh v30 lane requires root".into());
    }
    Ok(())
}

fn require_broker_root(path: &Path) -> Result<(), String> {
    #[cfg(feature = "age319-private-broker-fixture")]
    if fs::read_to_string("/proc/self/uid_map")
        .ok()
        .is_some_and(|map| map.split_ascii_whitespace().nth(2) == Some("1"))
    {
        let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if path.is_absolute()
            && meta.is_dir()
            && !meta.is_symlink()
            && meta.uid() == 0
            && meta.mode() & 0o077 == 0
        {
            return Ok(());
        }
        return Err("private fresh broker root is not owner-only".into());
    }
    require_root_owned_ancestors(path)
}

fn harden_tree(root: &Path) -> Result<(), String> {
    for item in fs::read_dir(root).map_err(|e| e.to_string())? {
        let path = item.map_err(|e| e.to_string())?.path();
        let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if meta.file_type().is_symlink() || meta.uid() != 0 || (meta.is_file() && meta.nlink() != 1)
        {
            return Err("fresh lane staging artifact is untrusted".into());
        }
        if meta.is_dir() {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
            harden_tree(&path)?;
        } else if meta.is_file() {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                .map_err(|e| e.to_string())?;
        } else {
            return Err("fresh lane staging artifact type is unsupported".into());
        }
    }
    Ok(())
}

fn rename_noreplace(from: &Path, to: &Path) -> Result<(), String> {
    let from = CString::new(from.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let to = CString::new(to.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
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
        return Err(format!(
            "fresh v30 publication refused: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}
