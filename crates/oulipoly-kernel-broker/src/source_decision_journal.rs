//! Separate, broker-owned exact-source decisions. Issuance and read-only
//! verification do not themselves admit a State source or authorize an effect.
use oulipoly_kernel_broker::entry_registry::ProcessStamp;
use oulipoly_kernel_broker::protocol::{ExactSourceDecisionReadback, ProcessWitness};
use oulipoly_state::mailbox::PreparedProcessStamp;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const TTL_SECONDS: i64 = 120;

fn decision_now() -> io::Result<i64> {
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| error("source decision clock before epoch"))?
            .as_secs(),
    )
    .map_err(|_| error("source decision clock overflow"))?;
    #[cfg(feature = "age319-private-broker-fixture")]
    if std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1").is_some() {
        if let Some(offset) =
            std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_SOURCE_CLOCK_OFFSET_SECONDS_V1")
        {
            let offset = offset
                .to_string_lossy()
                .parse::<i64>()
                .map_err(|_| error("invalid private source decision clock offset"))?;
            return now
                .checked_add(offset)
                .ok_or_else(|| error("private source decision clock overflow"));
        }
    }
    Ok(now)
}
const OBJECTS: [&str; 4] = [
    "CREATE TABLE clock_floor (id INTEGER PRIMARY KEY CHECK(id=1), unix_seconds INTEGER NOT NULL)",
    "CREATE TABLE decisions (request_id TEXT PRIMARY KEY NOT NULL, registration_id TEXT NOT NULL UNIQUE, caller_admission_id TEXT NOT NULL UNIQUE, decision_id TEXT NOT NULL UNIQUE, claims BLOB NOT NULL, issued_unix_seconds INTEGER NOT NULL, expires_unix_seconds INTEGER NOT NULL)",
    "CREATE TRIGGER decisions_no_update BEFORE UPDATE ON decisions BEGIN SELECT RAISE(ABORT, 'immutable source decision'); END",
    "CREATE TRIGGER decisions_no_delete BEFORE DELETE ON decisions BEGIN SELECT RAISE(ABORT, 'immutable source decision'); END",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Claims {
    pub root_id: String,
    pub root_init: PreparedProcessStamp,
    pub source_generation: String,
    pub sidecar_generation: String,
    pub owner_generation: String,
    pub owner_uid: u32,
    pub domain_id: String,
    pub supervisor_id: String,
    pub guardian: ProcessWitness,
    pub driver: ProcessWitness,
    pub peer: ProcessStamp,
    pub owner_session_id: String,
    pub owner_invocation_uuid: String,
    pub capability_digest: String,
    pub handle: String,
    pub caller_admission_id: String,
    pub registration_id: String,
    pub registration_path: PathBuf,
    pub registration_device: u64,
    pub registration_inode: u64,
    pub original_state_device: u64,
    pub original_state_inode: u64,
    pub registration_bytes: Vec<u8>,
    pub registration_sha256: String,
}

pub struct Journal {
    directory: PathBuf,
    path: PathBuf,
    anchor: PathBuf,
    identity: (u64, u64),
    connection: Connection,
}

fn error(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}

fn exact_file(path: &Path, mode: u32, owner: u32, directory: bool) -> io::Result<(u64, u64)> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink()
        || meta.uid() != owner
        || meta.mode() & 0o7777 != mode
        || meta.nlink() != 1 && !directory
        || (directory && !meta.is_dir())
        || (!directory && !meta.is_file())
    {
        return Err(error(
            "exact source journal file identity/permission mismatch",
        ));
    }
    Ok((meta.dev(), meta.ino()))
}

impl Journal {
    /// Exact readback only. This method never begins a writer transaction or
    /// reads the original State database. A State writer may hold its lock.
    pub fn verify_decision(
        &self,
        request_id: &str,
        decision_id: &str,
        claims: &Claims,
    ) -> io::Result<ExactSourceDecisionReadback> {
        self.verify_decision_on(request_id, decision_id, claims, false)
    }

    pub fn verify_committed_retry(
        &self,
        request_id: &str,
        decision_id: &str,
        claims: &Claims,
    ) -> io::Result<ExactSourceDecisionReadback> {
        self.verify_decision_on(request_id, decision_id, claims, true)
    }

    fn verify_decision_on(
        &self,
        request_id: &str,
        decision_id: &str,
        claims: &Claims,
        committed_retry: bool,
    ) -> io::Result<ExactSourceDecisionReadback> {
        let canonical = uuid::Uuid::parse_str(request_id)
            .map_err(|_| error("invalid source decision request ID"))?;
        if canonical.to_string() != request_id {
            return Err(error("noncanonical source decision request ID"));
        }
        let canonical =
            uuid::Uuid::parse_str(decision_id).map_err(|_| error("invalid source decision ID"))?;
        if canonical.to_string() != decision_id {
            return Err(error("noncanonical source decision ID"));
        }
        self.verify()?;
        let now = decision_now()?;
        let floor: i64 = self
            .connection
            .query_row(
                "SELECT unix_seconds FROM clock_floor WHERE id=1",
                [],
                |row| row.get(0),
            )
            .map_err(|e| error(e.to_string()))?;
        let stored: Option<(String, Vec<u8>, i64, i64)> = self.connection.query_row(
            "SELECT decision_id,claims,issued_unix_seconds,expires_unix_seconds FROM decisions WHERE request_id=?1",
            [request_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        ).optional().map_err(|e| error(e.to_string()))?;
        let (stored_id, stored_claims, issued, expires) =
            stored.ok_or_else(|| error("exact source decision absent"))?;
        if stored_id != decision_id
            || stored_claims != serde_json::to_vec(claims).map_err(|e| error(e.to_string()))?
            || issued.checked_add(TTL_SECONDS) != Some(expires)
            || now < floor
            || (!committed_retry && now >= expires)
        {
            return Err(error("exact source decision conflict or expired"));
        }
        Ok(readback(request_id, decision_id, claims, issued, expires))
    }

    pub fn open(state: &Path) -> io::Result<Self> {
        let owner = unsafe { libc::geteuid() };
        let directory = state.join("source-decisions");
        let created_dir = match fs::create_dir(&directory) {
            Ok(()) => {
                fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
                File::open(state)?.sync_all()?;
                true
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => false,
            Err(e) => return Err(e),
        };
        exact_file(&directory, 0o700, owner, true)?;
        let path = directory.join("decisions.db");
        let anchor = directory.join("identity.v1");
        if !created_dir && fs::symlink_metadata(&path).is_err() {
            return Err(error("incomplete exact source journal database"));
        }
        let created = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)
        {
            Ok(file) => {
                file.sync_all()?;
                File::open(&directory)?.sync_all()?;
                true
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => false,
            Err(e) => return Err(e),
        };
        let identity = exact_file(&path, 0o600, owner, false)?;
        Self::no_sidecars(&directory)?;
        if created {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&anchor)?;
            file.write_all(
                format!("exact-source-journal-v1 {} {}\n", identity.0, identity.1).as_bytes(),
            )?;
            file.sync_all()?;
            File::open(&directory)?.sync_all()?;
        }
        Self::verify_anchor(&anchor, identity, owner)?;
        if !created {
            Self::rollback_header(&path)?;
        }
        let connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| error(e.to_string()))?;
        connection
            .busy_timeout(std::time::Duration::ZERO)
            .map_err(|e| error(e.to_string()))?;
        let journal = Self {
            directory,
            path,
            anchor,
            identity,
            connection,
        };
        journal
            .connection
            .pragma_update(None, "journal_mode", "DELETE")
            .map_err(|e| error(e.to_string()))?;
        journal
            .connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(|e| error(e.to_string()))?;
        if created {
            for statement in OBJECTS {
                journal
                    .connection
                    .execute_batch(statement)
                    .map_err(|e| error(e.to_string()))?;
            }
            journal
                .connection
                .pragma_update(None, "user_version", 3)
                .map_err(|e| error(e.to_string()))?;
            journal
                .connection
                .pragma_update(None, "application_id", 0x4f535244_i64)
                .map_err(|e| error(e.to_string()))?;
            journal
                .connection
                .execute("INSERT INTO clock_floor VALUES (1, 0)", [])
                .map_err(|e| error(e.to_string()))?;
        }
        journal.verify()?;
        Ok(journal)
    }

    fn no_sidecars(directory: &Path) -> io::Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_name() != "decisions.db" && entry.file_name() != "identity.v1" {
                return Err(error("incomplete exact source journal artifact"));
            }
        }
        Ok(())
    }

    fn rollback_header(path: &Path) -> io::Result<()> {
        let mut header = [0u8; 100];
        File::open(path)?.read_exact(&mut header)?;
        if &header[..16] != b"SQLite format 3\0" || header[18] != 1 || header[19] != 1 {
            return Err(error("exact source journal header or WAL mode mismatch"));
        }
        Ok(())
    }

    fn verify_anchor(anchor: &Path, identity: (u64, u64), owner: u32) -> io::Result<()> {
        exact_file(anchor, 0o600, owner, false)?;
        let meta = fs::metadata(anchor)?;
        if meta.len() > 128
            || fs::read(anchor)?
                != format!("exact-source-journal-v1 {} {}\n", identity.0, identity.1).as_bytes()
        {
            return Err(error(
                "exact source journal persistent inode anchor mismatch",
            ));
        }
        Ok(())
    }

    fn verify(&self) -> io::Result<()> {
        let owner = unsafe { libc::geteuid() };
        exact_file(&self.directory, 0o700, owner, true)?;
        if exact_file(&self.path, 0o600, owner, false)? != self.identity {
            return Err(error("exact source journal inode changed"));
        }
        Self::no_sidecars(&self.directory)?;
        Self::verify_anchor(&self.anchor, self.identity, owner)?;
        Self::rollback_header(&self.path)?;
        let mode: String = self
            .connection
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .map_err(|e| error(e.to_string()))?;
        let synchronous: i64 = self
            .connection
            .query_row("PRAGMA synchronous", [], |r| r.get(0))
            .map_err(|e| error(e.to_string()))?;
        if mode != "delete" || synchronous != 2 {
            return Err(error("exact source journal durability mode mismatch"));
        }
        let version: i64 = self
            .connection
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(|e| error(e.to_string()))?;
        // Earlier rows lack either original State identity or the distinct
        // retained mailbox generation. Neither can be inferred from a copy.
        if version != 3 {
            return Err(error("exact source journal schema version mismatch"));
        }
        let application_id: i64 = self
            .connection
            .query_row("PRAGMA application_id", [], |r| r.get(0))
            .map_err(|e| error(e.to_string()))?;
        if application_id != 0x4f535244 {
            return Err(error("exact source journal application mismatch"));
        }
        let actual: Vec<String> = self
            .connection
            .prepare("SELECT sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY name")
            .map_err(|e| error(e.to_string()))?
            .query_map([], |r| r.get(0))
            .map_err(|e| error(e.to_string()))?
            .collect::<Result<_, _>>()
            .map_err(|e| error(e.to_string()))?;
        if actual.len() != 4
            || !actual
                .iter()
                .all(|sql| OBJECTS.iter().any(|item| item == sql))
        {
            return Err(error("exact source journal schema mismatch"));
        }
        let integrity: String = self
            .connection
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .map_err(|e| error(e.to_string()))?;
        if integrity != "ok" {
            return Err(error("exact source journal integrity failure"));
        }
        Ok(())
    }

    pub fn issue(
        &mut self,
        request_id: &str,
        claims: &Claims,
    ) -> io::Result<ExactSourceDecisionReadback> {
        let now = decision_now()?;
        self.issue_at(request_id, claims, now)
    }

    fn issue_at(
        &mut self,
        request_id: &str,
        claims: &Claims,
        now: i64,
    ) -> io::Result<ExactSourceDecisionReadback> {
        let canonical = uuid::Uuid::parse_str(request_id)
            .map_err(|_| error("invalid source decision request ID"))?;
        if canonical.to_string() != request_id {
            return Err(error("noncanonical source decision request ID"));
        }
        self.verify()?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| error(e.to_string()))?;
        let floor: i64 = transaction
            .query_row("SELECT unix_seconds FROM clock_floor WHERE id=1", [], |r| {
                r.get(0)
            })
            .map_err(|e| error(e.to_string()))?;
        if now < floor {
            return Err(error("exact source journal clock rollback"));
        }
        let existing: Option<(String, Vec<u8>, i64, i64)> = transaction.query_row(
            "SELECT decision_id, claims, issued_unix_seconds, expires_unix_seconds FROM decisions WHERE request_id=?1",
            [request_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .optional().map_err(|e| error(e.to_string()))?;
        let claims_bytes = serde_json::to_vec(claims).map_err(|e| error(e.to_string()))?;
        let (decision_id, issued, expires) =
            if let Some((decision_id, prior, issued, expires)) = existing {
                if prior != claims_bytes
                    || now >= expires
                    || issued.checked_add(TTL_SECONDS) != Some(expires)
                {
                    return Err(error("exact source decision replay conflict or expired"));
                }
                (decision_id, issued, expires)
            } else {
                let existing_registration: Option<String> = transaction
                    .query_row(
                        "SELECT request_id FROM decisions WHERE registration_id=?1",
                        [&claims.registration_id],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(|e| error(e.to_string()))?;
                if existing_registration.is_some() {
                    return Err(error("duplicate exact source registration"));
                }
                let existing_admission: Option<String> = transaction
                    .query_row(
                        "SELECT request_id FROM decisions WHERE caller_admission_id=?1",
                        [&claims.caller_admission_id],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(|e| error(e.to_string()))?;
                if existing_admission.is_some() {
                    return Err(error("duplicate exact source admission"));
                }
                let decision_id = uuid::Uuid::new_v4().to_string();
                let expires = now
                    .checked_add(TTL_SECONDS)
                    .ok_or_else(|| error("source decision expiry overflow"))?;
                transaction
                    .execute(
                        "INSERT INTO decisions VALUES (?1,?2,?3,?4,?5,?6,?7)",
                        params![
                            request_id,
                            claims.registration_id,
                            claims.caller_admission_id,
                            decision_id,
                            claims_bytes,
                            now,
                            expires
                        ],
                    )
                    .map_err(|e| error(e.to_string()))?;
                (decision_id, now, expires)
            };
        transaction
            .execute("UPDATE clock_floor SET unix_seconds=?1 WHERE id=1", [now])
            .map_err(|e| error(e.to_string()))?;
        transaction.commit().map_err(|e| error(e.to_string()))?;
        self.verify()?;
        Ok(readback(request_id, &decision_id, claims, issued, expires))
    }
}

fn readback(
    request_id: &str,
    decision_id: &str,
    claims: &Claims,
    issued: i64,
    expires: i64,
) -> ExactSourceDecisionReadback {
    ExactSourceDecisionReadback {
        request_id: request_id.into(),
        decision_id: decision_id.into(),
        issued_unix_seconds: issued,
        expires_unix_seconds: expires,
        root_id: claims.root_id.clone(),
        root_init: claims.root_init.clone(),
        source_generation: claims.source_generation.clone(),
        sidecar_generation: claims.sidecar_generation.clone(),
        owner_generation: claims.owner_generation.clone(),
        owner_uid: claims.owner_uid,
        domain_id: claims.domain_id.clone(),
        supervisor_id: claims.supervisor_id.clone(),
        guardian: claims.guardian.clone(),
        driver: claims.driver.clone(),
        issuer: claims.peer.clone(),
        owner_session_id: claims.owner_session_id.clone(),
        owner_invocation_uuid: claims.owner_invocation_uuid.clone(),
        capability_digest: claims.capability_digest.clone(),
        handle: claims.handle.clone(),
        caller_admission_id: claims.caller_admission_id.clone(),
        registration_id: claims.registration_id.clone(),
        registration_path: claims.registration_path.clone(),
        registration_device: claims.registration_device,
        registration_inode: claims.registration_inode,
        original_state_device: claims.original_state_device,
        original_state_inode: claims.original_state_inode,
        registration_len: claims.registration_bytes.len() as u64,
        registration_sha256: claims.registration_sha256.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims() -> Claims {
        let stamp = PreparedProcessStamp {
            host_pid: 31,
            boot_id: "boot".into(),
            starttime_ticks: 41,
            pidns_dev: 51,
            pidns_ino: 61,
        };
        Claims {
            root_id: "root".into(),
            root_init: stamp,
            source_generation: "source".into(),
            sidecar_generation: "sidecar".into(),
            owner_generation: "owner".into(),
            owner_uid: 0,
            domain_id: "domain".into(),
            supervisor_id: "supervisor".into(),
            guardian: ProcessWitness {
                host_pid: 32,
                boot_id: "boot".into(),
                starttime_ticks: 42,
            },
            driver: ProcessWitness {
                host_pid: 33,
                boot_id: "boot".into(),
                starttime_ticks: 43,
            },
            peer: ProcessStamp {
                host_pid: 34,
                boot_id: "boot".into(),
                starttime_ticks: 44,
                pidns_dev: 51,
                pidns_ino: 62,
            },
            owner_session_id: "session".into(),
            owner_invocation_uuid: "invocation".into(),
            capability_digest: "digest".into(),
            handle: "handle".into(),
            caller_admission_id: "admission".into(),
            registration_id: "registration".into(),
            registration_path: "/tmp/registration".into(),
            registration_device: 1,
            registration_inode: 2,
            original_state_device: 3,
            original_state_inode: 4,
            registration_bytes: b"exact bytes".to_vec(),
            registration_sha256: "sha256".into(),
        }
    }

    #[test]
    fn decision_is_immutable_across_reply_loss_restart_expiry_and_clock_rollback() {
        let temp = tempfile::tempdir().unwrap();
        let request = uuid::Uuid::new_v4().to_string();
        let mut journal = Journal::open(temp.path()).unwrap();
        let first = journal.issue_at(&request, &claims(), 1_000).unwrap();
        assert_eq!(journal.issue_at(&request, &claims(), 1_001).unwrap(), first);
        assert!(
            journal
                .issue_at(&uuid::Uuid::new_v4().to_string(), &claims(), 1_002)
                .unwrap_err()
                .to_string()
                .contains("duplicate exact source registration")
        );
        let mut copied_handle = claims();
        copied_handle.registration_id = "another-registration".into();
        assert!(
            journal
                .issue_at(&uuid::Uuid::new_v4().to_string(), &copied_handle, 1_002)
                .unwrap_err()
                .to_string()
                .contains("duplicate exact source admission")
        );
        let mut changed = claims();
        changed.registration_bytes.push(0);
        assert!(
            journal
                .issue_at(&request, &changed, 1_003)
                .unwrap_err()
                .to_string()
                .contains("replay conflict")
        );
        drop(journal);
        let mut reopened = Journal::open(temp.path()).unwrap();
        assert_eq!(
            reopened.issue_at(&request, &claims(), 1_004).unwrap(),
            first
        );
        assert!(
            reopened
                .issue_at(&request, &claims(), 999)
                .unwrap_err()
                .to_string()
                .contains("clock rollback")
        );
        assert!(
            reopened
                .issue_at(&request, &claims(), first.expires_unix_seconds)
                .unwrap_err()
                .to_string()
                .contains("expired")
        );
        let count: i64 = reopened
            .connection
            .query_row("SELECT count(*) FROM decisions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        assert!(
            reopened
                .connection
                .execute("DELETE FROM decisions", [])
                .is_err()
        );
        assert!(
            reopened
                .connection
                .execute("UPDATE decisions SET registration_id='copy'", [])
                .is_err()
        );
    }

    #[test]
    fn read_only_verification_requires_exact_unexpired_journal_claims() {
        let temp = tempfile::tempdir().unwrap();
        let request = uuid::Uuid::new_v4().to_string();
        let mut journal = Journal::open(temp.path()).unwrap();
        let issued = journal.issue(&request, &claims()).unwrap();
        let before: i64 = journal
            .connection
            .query_row(
                "SELECT unix_seconds FROM clock_floor WHERE id=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            journal
                .verify_decision(&request, &issued.decision_id, &claims())
                .unwrap(),
            issued
        );
        let mut wrong = claims();
        wrong.capability_digest = "other".into();
        assert!(
            journal
                .verify_decision(&request, &issued.decision_id, &wrong)
                .is_err()
        );
        wrong = claims();
        wrong.original_state_inode += 1;
        assert!(
            journal
                .verify_decision(&request, &issued.decision_id, &wrong)
                .is_err()
        );
        wrong = claims();
        wrong.registration_bytes.push(0);
        assert!(
            journal
                .verify_decision(&request, &issued.decision_id, &wrong)
                .is_err()
        );
        assert!(
            journal
                .verify_decision(&request, &uuid::Uuid::new_v4().to_string(), &claims())
                .is_err()
        );
        assert!(
            journal
                .verify_decision(
                    &uuid::Uuid::new_v4().to_string(),
                    &issued.decision_id,
                    &claims()
                )
                .is_err()
        );
        let after: i64 = journal
            .connection
            .query_row(
                "SELECT unix_seconds FROM clock_floor WHERE id=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(before, after);
        assert_eq!(
            journal
                .connection
                .query_row::<i64, _, _>("SELECT count(*) FROM decisions", [], |row| row.get(0))
                .unwrap(),
            1
        );
        drop(journal);
        let reopened = Journal::open(temp.path()).unwrap();
        assert_eq!(
            reopened
                .verify_decision(&request, &issued.decision_id, &claims())
                .unwrap(),
            issued
        );

        let expired = tempfile::tempdir().unwrap();
        let mut journal = Journal::open(expired.path()).unwrap();
        let past = i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        )
        .unwrap()
            - TTL_SECONDS
            - 2;
        let old = journal.issue_at(&request, &claims(), past).unwrap();
        assert!(
            journal
                .verify_decision(&request, &old.decision_id, &claims())
                .unwrap_err()
                .to_string()
                .contains("expired")
        );
    }

    #[test]
    fn malformed_or_stale_identity_rows_cannot_verify() {
        let temp = tempfile::tempdir().unwrap();
        let request = uuid::Uuid::new_v4().to_string();
        let mut journal = Journal::open(temp.path()).unwrap();
        let issued = journal.issue(&request, &claims()).unwrap();
        let replace_claims = |journal: &Journal, bytes: Vec<u8>| {
            journal
                .connection
                .execute_batch("DROP TRIGGER decisions_no_update")
                .unwrap();
            journal
                .connection
                .execute(
                    "UPDATE decisions SET claims=?1 WHERE request_id=?2",
                    params![bytes, request],
                )
                .unwrap();
            journal.connection.execute_batch(OBJECTS[2]).unwrap();
        };
        let mut stale = claims();
        stale.original_state_inode += 1;
        replace_claims(&journal, serde_json::to_vec(&stale).unwrap());
        assert!(
            journal
                .verify_decision(&request, &issued.decision_id, &claims())
                .is_err()
        );
        drop(journal);
        let journal = Journal::open(temp.path()).unwrap();
        replace_claims(
            &journal,
            br#"{"missing":"original-state-identity"}"#.to_vec(),
        );
        assert!(
            journal
                .verify_decision(&request, &issued.decision_id, &claims())
                .is_err()
        );
    }

    #[test]
    fn journal_refuses_incomplete_schema_inode_mode_and_wal_artifacts() {
        for fault in [
            "schema",
            "schema_object",
            "inode",
            "anchor",
            "mode",
            "wal",
            "wal_header",
            "incomplete",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let journal = Journal::open(temp.path()).unwrap();
            let path = temp.path().join("source-decisions/decisions.db");
            match fault {
                "schema" => {
                    drop(journal);
                    Connection::open(&path)
                        .unwrap()
                        .pragma_update(None, "user_version", 1)
                        .unwrap();
                    assert!(Journal::open(temp.path()).is_err());
                }
                "schema_object" => {
                    drop(journal);
                    Connection::open(&path)
                        .unwrap()
                        .execute_batch("DROP TRIGGER decisions_no_delete")
                        .unwrap();
                    assert!(Journal::open(temp.path()).is_err());
                }
                "inode" => {
                    let displaced = temp.path().join("displaced-decisions.db");
                    fs::rename(&path, &displaced).unwrap();
                    fs::copy(&displaced, &path).unwrap();
                    assert!(
                        journal
                            .verify()
                            .unwrap_err()
                            .to_string()
                            .contains("inode changed")
                    );
                    drop(journal);
                    assert!(Journal::open(temp.path()).is_err());
                }
                "anchor" => {
                    fs::remove_file(temp.path().join("source-decisions/identity.v1")).unwrap();
                    assert!(journal.verify().is_err());
                    drop(journal);
                    assert!(Journal::open(temp.path()).is_err());
                }
                "mode" => {
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
                    assert!(journal.verify().is_err());
                }
                "wal" => {
                    fs::write(path.with_extension("db-wal"), b"orphan").unwrap();
                    assert!(journal.verify().is_err());
                }
                "wal_header" => {
                    drop(journal);
                    Connection::open(&path)
                        .unwrap()
                        .execute_batch("PRAGMA journal_mode=WAL")
                        .unwrap();
                    assert!(Journal::open(temp.path()).is_err());
                }
                "incomplete" => {
                    fs::write(temp.path().join("source-decisions/partial"), b"partial").unwrap();
                    assert!(journal.verify().is_err());
                }
                _ => unreachable!(),
            }
        }
    }
}
