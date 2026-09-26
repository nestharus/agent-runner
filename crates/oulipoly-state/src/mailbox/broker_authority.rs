//! Root-owned copy-cutover boundary. Its offline publication carries a
//! quiesced v29 sidecar and retained payloads; fresh independent v30 storage
//! is initialized separately by `fresh_lane` and never uses this copy path.
use super::*;
use crate::StateDb;
use crate::db::ExactSourceProjection;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Component;

/// Retains the broker's live SQLite connection and its broker-minted source
/// generation. Legacy v4 native grants and pre-cutover attempts are not made
/// eligible for native K by opening this connection.
pub struct BrokerSidecar {
    mailbox: MailboxDb,
    source_generation: String,
    storage_owner: u32,
    storage_anchor: std::path::PathBuf,
    state_source: Option<BoundStateSource>,
}

/// Identity of the original State file selected by the broker-owned source
/// binding. A later State transaction must compare these values to its own
/// opened database before consuming a source decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoundStateFileIdentity {
    pub device: u64,
    pub inode: u64,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct BoundStateSource {
    pub(super) path: std::path::PathBuf,
    pub(super) owner: u32,
    pub(super) device: u64,
    pub(super) inode: u64,
}

#[cfg(unix)]
pub(super) fn verify_bound_state_source(source: &BoundStateSource) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(&source.path).map_err(|e| e.to_string())?;
    if !source.path.is_absolute()
        || source.path.file_name() != Some(std::ffi::OsStr::new("state.db"))
        || source
            .path
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
        || !meta.is_file()
        || meta.file_type().is_symlink()
        || meta.nlink() != 1
        || (meta.uid(), meta.dev(), meta.ino()) != (source.owner, source.device, source.inode)
    {
        return Err("broker StateDb source identity changed".into());
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_bound_state_source(_source: &BoundStateSource) -> Result<(), String> {
    Err("broker StateDb source requires Unix identity".into())
}

#[cfg(unix)]
fn read_state_source_binding(
    sidecar: &Path,
    storage_owner: u32,
) -> Result<Option<BoundStateSource>, String> {
    let parent = sidecar.parent().ok_or("broker sidecar parent absent")?;
    for entry in std::fs::read_dir(parent).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(".state-source-pending-")
        {
            return Err("unresolved broker State source publication".into());
        }
    }
    let path = sidecar
        .parent()
        .ok_or("broker sidecar parent absent")?
        .join("state-source.json");
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if !meta.is_file()
        || meta.file_type().is_symlink()
        || meta.uid() != storage_owner
        || meta.nlink() != 1
        || meta.mode() & 0o777 != 0o600
    {
        return Err("broker StateDb binding is not root-only".into());
    }
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let source: BoundStateSource = serde_json::from_slice(&bytes).map_err(|error| {
        use sha2::{Digest, Sha256};
        eprintln!(
            "oulipoly JSON artifact: stage=state_source_binding source={} bytes={} sha256={:x} cause={error}",
            path.display(), bytes.len(), Sha256::digest(&bytes)
        );
        "broker State source JSON read failed".to_owned()
    })?;
    verify_bound_state_source(&source)?;
    Ok(Some(source))
}

#[cfg(unix)]
fn write_state_source_binding(
    directory: &Path,
    state_path: &Path,
    source_owner: u32,
    storage_owner: u32,
) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(state_path).map_err(|e| e.to_string())?;
    let source = BoundStateSource {
        path: state_path.into(),
        owner: source_owner,
        device: meta.dev(),
        inode: meta.ino(),
    };
    verify_bound_state_source(&source)?;
    let binding_path = directory.join("state-source.json");
    let temporary = directory.join(format!(".state-source-pending-{}", uuid::Uuid::new_v4()));
    let result = (|| -> Result<(), String> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        serde_json::to_writer(&mut file, &source).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        let file_meta = file.metadata().map_err(|e| e.to_string())?;
        if file_meta.uid() != storage_owner {
            return Err("broker StateDb binding storage owner changed".into());
        }
        std::fs::hard_link(&temporary, &binding_path).map_err(|e| e.to_string())?;
        std::fs::File::open(directory)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| e.to_string())
    })();
    let _ = std::fs::remove_file(&temporary);
    result?;
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct BrokerRepairReadback {
    pub source_generation: String,
    pub root_id: String,
    pub owner_generation: String,
    pub authority_ordinal: i64,
    pub has_more: bool,
    pub pending_registration_ids: Vec<String>,
}

/// Bounded source selection from the retained v30 sidecar. This is a preview,
/// not an executable grant: the registered snapshot and recovery image have
/// not yet been moved into broker custody. In particular, a caller must not
/// use this record to open the registration's original handle directory.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct BrokerSourceSelection {
    pub source_generation: String,
    pub root_id: String,
    pub owner_generation: String,
    pub authority_ordinal: i64,
    pub candidate: Option<BrokerSourceCandidate>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct BrokerSourceCandidate {
    pub registration_id: String,
    pub registration_digest: String,
    pub listener_revision: u64,
    pub listener: crate::completion_continuation::ListenerIdentity,
}

/// Durable one-use source debt. `reserved` has no effect authority until a
/// physically held recovery child consumes it; `unknown` cannot mint another
/// grant. No production transition to `consumed` exists before that custody
/// path is wired.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct BrokerSourceEffectGrant {
    pub grant_id: String,
    pub source_generation: String,
    pub root_id: String,
    pub owner_generation: String,
    pub driver_identity: crate::completion_continuation::SourceProcessIdentity,
    pub authority_ordinal: i64,
    pub candidate: BrokerSourceCandidate,
    pub phase: String,
    pub revision: i64,
}

/// A pending recipient selected on the retained broker connection. This is
/// metadata for planning only: the row can change, and neither the session ID
/// nor the payload digest is a recipient authentication or work grant.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct BrokerRecipientSelection {
    pub source_generation: String,
    pub root_id: String,
    pub owner_generation: String,
    pub authority_ordinal: i64,
    pub candidate: Option<BrokerRecipientCandidate>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct BrokerRecipientCandidate {
    pub session_id: String,
    pub seq: i64,
    pub kind: String,
    pub handle: String,
    pub payload_sha256: String,
    pub payload_byte_len: i64,
}

/// Exact reserved row and its State-admitted original registration bytes.
/// Only the retained broker sidecar can construct this material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrokerSourceMaterial {
    pub grant: BrokerSourceEffectGrant,
    pub registration_bytes: Vec<u8>,
}

/// Exact root-only evidence file seal. The broker verifies the physical and
/// original bytes before constructing this; State pins it to the consumed
/// grant and source at the SQLite fence.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BrokerSourceEvidenceSeal {
    pub manifest_sha256: String,
    pub manifest_device: u64,
    pub manifest_inode: u64,
    pub manifest_byte_len: u64,
    pub snapshot_sha256: String,
    pub outcome_sha256: String,
    pub recovery_stdout_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrokerSourceEvidenceReadback {
    pub grant_id: String,
    pub source_generation: String,
    pub registration_id: String,
    pub seal: Option<BrokerSourceEvidenceSeal>,
    pub phase: String,
    pub revision: i64,
}

/// A positive-only inventory of the retained broker's source-effect ledger.
/// Zero rows do not certify that State, the fresh lane, or copied v29 WAL has
/// no obligations. In particular, `consumed_without_evidence` preserves the
/// gap between spending W and recording its physical result.
#[derive(Debug, Clone, Default, serde::Serialize, PartialEq, Eq)]
pub struct BrokerSourceEffectObligations {
    pub reserved: usize,
    pub consumed_without_evidence: usize,
    pub unknown: usize,
    pub captured: usize,
    pub accepted: usize,
}

impl BrokerSourceEffectObligations {
    pub fn unsettled(&self) -> usize {
        self.reserved + self.consumed_without_evidence + self.unknown + self.captured
    }
}

fn source_effect_obligations_on(
    conn: &Connection,
    source_generation: &str,
    root_id: &str,
    owner_generation: &str,
) -> Result<BrokerSourceEffectObligations, String> {
    let orphan_grant: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM broker_source_effect_grant g
         LEFT JOIN broker_prepared_owner p ON p.root_id=g.root_id
         WHERE p.owner_generation IS NULL OR p.owner_generation!=g.owner_generation
            OR p.source_generation!=g.source_generation)",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if orphan_grant {
        return Err("orphan broker source grant row".into());
    }
    // A foreign/orphan evidence row has no trustworthy root attribution. It
    // invalidates this readback rather than disappearing in the join.
    let orphan: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM broker_source_evidence e
         LEFT JOIN broker_source_effect_grant g ON g.grant_id=e.grant_id
         WHERE g.grant_id IS NULL OR e.source_generation!=g.source_generation
            OR e.registration_id!=g.registration_id)",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if orphan {
        return Err("orphan broker source evidence row".into());
    }
    let mut statement = conn
        .prepare(
            "SELECT g.source_generation,g.owner_generation,g.phase,e.phase,
                    g.registration_id,e.source_generation,e.registration_id
         FROM broker_source_effect_grant g
         LEFT JOIN broker_source_evidence e ON e.grant_id=g.grant_id
         WHERE g.root_id=?1 ORDER BY g.grant_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([root_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut result = BrokerSourceEffectObligations::default();
    for row in rows {
        let (
            generation,
            owner,
            phase,
            evidence,
            registration,
            evidence_generation,
            evidence_registration,
        ) = row.map_err(|e| e.to_string())?;
        if generation != source_generation || owner != owner_generation {
            return Err("source effect grant has wrong root owner or generation".into());
        }
        if evidence.is_some()
            && (evidence_generation.as_deref() != Some(generation.as_str())
                || evidence_registration.as_deref() != Some(registration.as_str()))
        {
            return Err("source effect evidence attribution changed".into());
        }
        match (phase.as_str(), evidence.as_deref()) {
            ("reserved", None) => result.reserved += 1,
            ("consumed", None) => result.consumed_without_evidence += 1,
            ("unknown", None) | ("consumed", Some("unknown")) => result.unknown += 1,
            ("consumed", Some("captured")) => result.captured += 1,
            ("consumed", Some("accepted")) => result.accepted += 1,
            _ => return Err("source effect grant/evidence phase conflict".into()),
        }
    }
    Ok(result)
}

fn consume_source_grant_row(
    conn: &Connection,
    material: &BrokerSourceMaterial,
) -> Result<(), String> {
    let changed = conn
        .execute(
            "UPDATE broker_source_effect_grant SET phase='consumed',revision=2
         WHERE grant_id=?1 AND source_generation=?2 AND root_id=?3
           AND owner_generation=?4 AND driver_identity=?5 AND authority_ordinal=?6
           AND registration_id=?7 AND registration_digest=?8 AND registration_bytes=?9
           AND listener_revision=?10 AND listener_json=?11
           AND phase='reserved' AND revision=1",
            params![
                material.grant.grant_id,
                material.grant.source_generation,
                material.grant.root_id,
                material.grant.owner_generation,
                serde_json::to_string(&material.grant.driver_identity).map_err(|e| e.to_string())?,
                material.grant.authority_ordinal,
                material.grant.candidate.registration_id,
                material.grant.candidate.registration_digest,
                material.registration_bytes,
                i64::try_from(material.grant.candidate.listener_revision)
                    .map_err(|_| "listener revision overflow")?,
                serde_json::to_string(&material.grant.candidate.listener)
                    .map_err(|e| e.to_string())?,
            ],
        )
        .map_err(|e| e.to_string())?;
    if changed != 1 {
        return Err("source grant already consumed or changed".into());
    }
    Ok(())
}

#[cfg(test)]
mod source_grant_cas_tests {
    use super::*;

    #[test]
    fn exact_source_row_consumes_once_and_refuses_wrong_sibling_or_stale_material() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE broker_source_effect_grant (
            grant_id TEXT PRIMARY KEY, source_generation TEXT, root_id TEXT,
            owner_generation TEXT, driver_identity TEXT, authority_ordinal INTEGER,
            registration_id TEXT, registration_digest TEXT, registration_bytes BLOB,
            listener_revision INTEGER, listener_json TEXT, phase TEXT, revision INTEGER);",
        )
        .unwrap();
        let bytes = b"exact registration".to_vec();
        let material = BrokerSourceMaterial {
            grant: BrokerSourceEffectGrant {
                grant_id: uuid::Uuid::new_v4().to_string(),
                source_generation: uuid::Uuid::new_v4().to_string(),
                root_id: uuid::Uuid::new_v4().to_string(),
                owner_generation: uuid::Uuid::new_v4().to_string(),
                driver_identity: crate::completion_continuation::SourceProcessIdentity {
                    pid: 10,
                    boot_id: uuid::Uuid::new_v4().to_string(),
                    starttime_ticks: 20,
                },
                authority_ordinal: 7,
                candidate: BrokerSourceCandidate {
                    registration_id: uuid::Uuid::new_v4().to_string(),
                    registration_digest: crate::completion_continuation::sha256(&bytes),
                    listener_revision: 2,
                    listener: crate::completion_continuation::ListenerIdentity {
                        listener_id: uuid::Uuid::new_v4().to_string(),
                        session_id: "exact".into(),
                        owner_invocation_uuid: uuid::Uuid::new_v4().to_string(),
                    },
                },
                phase: "reserved".into(),
                revision: 1,
            },
            registration_bytes: bytes,
        };
        conn.execute("INSERT INTO broker_source_effect_grant VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'reserved',1)", params![
            material.grant.grant_id,
            material.grant.source_generation,
            material.grant.root_id,
            material.grant.owner_generation,
            serde_json::to_string(&material.grant.driver_identity).unwrap(),
            material.grant.authority_ordinal,
            material.grant.candidate.registration_id,
            material.grant.candidate.registration_digest,
            material.registration_bytes,
            material.grant.candidate.listener_revision as i64,
            serde_json::to_string(&material.grant.candidate.listener).unwrap(),
        ]).unwrap();
        let mut wrong = material.clone();
        wrong.grant.grant_id = uuid::Uuid::new_v4().to_string();
        assert!(consume_source_grant_row(&conn, &wrong).is_err());
        wrong = material.clone();
        wrong.grant.root_id = uuid::Uuid::new_v4().to_string();
        assert!(consume_source_grant_row(&conn, &wrong).is_err());
        wrong = material.clone();
        wrong.grant.candidate.registration_id = uuid::Uuid::new_v4().to_string();
        assert!(consume_source_grant_row(&conn, &wrong).is_err());
        wrong = material.clone();
        wrong.grant.candidate.listener.session_id = "sibling".into();
        assert!(consume_source_grant_row(&conn, &wrong).is_err());
        wrong = material.clone();
        wrong.grant.authority_ordinal -= 1;
        assert!(consume_source_grant_row(&conn, &wrong).is_err());
        consume_source_grant_row(&conn, &material).unwrap();
        assert!(consume_source_grant_row(&conn, &material).is_err());
        let row: (String, i64) = conn
            .query_row(
                "SELECT phase,revision FROM broker_source_effect_grant",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(row, ("consumed".into(), 2));
    }
}

/// Installer-owned proof that the old service images and every sidecar writer
/// were stopped, joined and fenced. No production constructor exists yet:
/// the installed service/launcher census and new-entry fence must be added
/// before a caller can request even an offline snapshot.
pub struct QuiescedCutoverProof {
    _private: (),
}

#[cfg(feature = "age319-private-broker-fixture")]
impl QuiescedCutoverProof {
    /// Only private namespace fixtures may stand in for the unavailable
    /// installed writer census. This feature is absent from normal builds.
    pub fn private_fixture() -> Self {
        Self { _private: () }
    }
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

/// Metadata read from the retained v30 connection. This does not grant
/// delivery or acknowledgement: a wire caller still needs an independently
/// proved recipient/session relationship and exact payload verification.
/// The full row can exceed the current broker response frame; this type is
/// internal readback, not a wire response.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct BrokerMailboxReadback {
    pub source_generation: String,
    pub row: super::MailboxRow,
}

/// A process incarnation observed by the host broker. Namespace identity is
/// retained in addition to PID/starttime so a reused PID is not authority.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PreparedProcessStamp {
    pub host_pid: i32,
    pub boot_id: String,
    pub starttime_ticks: u64,
    pub pidns_dev: u64,
    pub pidns_ino: u64,
}

impl PreparedProcessStamp {
    fn valid(&self) -> bool {
        self.host_pid > 0
            && uuid::Uuid::parse_str(&self.boot_id).is_ok()
            && self.starttime_ticks > 0
            && self.pidns_dev > 0
            && self.pidns_ino > 0
    }
}

/// Inert v30 owner evidence. `endpoint` is only the proposed guardian socket;
/// no completion session or running owner exists at this phase. It has no row
/// in the v18 running-owner table and grants no election, reservation,
/// acceptance, source, or child execution.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PreparedBrokerOwner {
    pub source_generation: String,
    pub root_id: String,
    pub owner_uid: u32,
    pub domain_id: String,
    pub supervisor_authority_id: String,
    pub owner_generation: String,
    pub endpoint: String,
    pub entry: PreparedProcessStamp,
    pub guardian: PreparedProcessStamp,
    pub driver: PreparedProcessStamp,
    pub root_init: PreparedProcessStamp,
    pub joined_child: PreparedProcessStamp,
}

impl PreparedBrokerOwner {
    fn valid(&self) -> bool {
        let canonical = |value: &str| {
            uuid::Uuid::parse_str(value)
                .map(|id| id.to_string() == value)
                .unwrap_or(false)
        };
        [
            &self.source_generation,
            &self.root_id,
            &self.domain_id,
            &self.supervisor_authority_id,
            &self.owner_generation,
        ]
        .into_iter()
        .all(|id| canonical(id))
            && !self.endpoint.is_empty()
            && self.endpoint.len() <= 1024
            && !self.endpoint.contains('\0')
            && [
                &self.entry,
                &self.guardian,
                &self.driver,
                &self.root_init,
                &self.joined_child,
            ]
            .into_iter()
            .all(PreparedProcessStamp::valid)
            && self.entry.boot_id == self.guardian.boot_id
            && self.entry.boot_id == self.driver.boot_id
            && self.entry.boot_id == self.root_init.boot_id
            && self.entry.boot_id == self.joined_child.boot_id
            && self.entry.pidns_dev == self.guardian.pidns_dev
            && self.entry.pidns_ino == self.guardian.pidns_ino
            && self.entry.pidns_dev == self.driver.pidns_dev
            && self.entry.pidns_ino == self.driver.pidns_ino
            && self.root_init.pidns_dev == self.joined_child.pidns_dev
            && self.root_init.pidns_ino == self.joined_child.pidns_ino
            && (self.entry.pidns_dev, self.entry.pidns_ino)
                != (self.root_init.pidns_dev, self.root_init.pidns_ino)
            && self.entry.host_pid != self.guardian.host_pid
            && self.guardian.host_pid != self.driver.host_pid
            && self.root_init.host_pid != self.joined_child.host_pid
    }
}

/// Durable transaction evidence only. A consumer still needs the broker to
/// verify every live actor after the child crosses the physical gate.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct BrokerReleaseEvidence {
    pub prepared: PreparedBrokerOwner,
    pub release_id: String,
    pub owner: super::CompletionDomainOwner,
}

/// Exact native grant readback from the retained v30 connection. This is
/// acceptance and one-to-one binding evidence, not worker execution or Q.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct BrokerNativeGrantReadback {
    pub source_generation: String,
    pub binding: super::NativeGrantBinding,
}

impl BrokerSidecar {
    pub fn retained_mailbox_generation(&self) -> Result<String, String> {
        self.check_mailbox_read(&self.source_generation)?;
        self.mailbox.sidecar_generation()
    }

    fn verify_exact_source_owner(
        &self,
        exact: &ExactSourceProjection,
        root_id: &str,
        owner: &CompletionDomainOwner,
    ) -> Result<(), String> {
        if exact.source_generation != self.source_generation
            || exact.root_id != root_id
            || exact.owner_generation != owner.owner_generation
            || exact.supervisor_id != owner.supervisor_authority_id
            || exact.binding.registration()?.domain_id != owner.domain_id
        {
            return Err("exact source root/generation/owner conflict".into());
        }
        let release = self.read_exact_release(
            &exact.source_generation,
            &exact.root_id,
            &exact.owner_generation,
        )?;
        if release.owner != *owner {
            return Err("exact source prepared/released owner changed".into());
        }
        let claims: serde_json::Value =
            serde_json::from_slice(&exact.readback_json).map_err(|e| e.to_string())?;
        let witness = |stamp: &PreparedProcessStamp| {
            serde_json::json!({
                "host_pid": stamp.host_pid, "boot_id": stamp.boot_id,
                "starttime_ticks": stamp.starttime_ticks,
            })
        };
        if claims["root_init"]
            != serde_json::to_value(&release.prepared.root_init).map_err(|e| e.to_string())?
            || claims["sidecar_generation"] != self.mailbox.sidecar_generation()?
            || exact.continuity.sidecar_generation != self.mailbox.sidecar_generation()?
            || claims["guardian"] != witness(&release.prepared.guardian)
            || claims["driver"] != witness(&release.prepared.driver)
            || claims["domain_id"] != owner.domain_id
            || claims["owner_invocation_uuid"]
                != exact.binding.registration()?.owner_invocation_uuid
            || claims["owner_session_id"] != exact.binding.registration()?.owner_session_id
        {
            return Err("exact source decision/prepared incarnation conflict".into());
        }
        for stamp in [
            &release.prepared.root_init,
            &release.prepared.guardian,
            &release.prepared.driver,
        ] {
            let expected = crate::pid_identity::ProcessIdentity {
                os_pid: i64::from(stamp.host_pid),
                os_boot_id: stamp.boot_id.clone(),
                os_pid_starttime_ticks: i64::try_from(stamp.starttime_ticks)
                    .map_err(|e| e.to_string())?,
            };
            if crate::pid_identity::read_live_process_identity(expected.os_pid)? != Some(expected) {
                return Err("exact source prepared/released owner is not live".into());
            }
        }
        Ok(())
    }

    fn exact_source_receipt_matches(&self, exact: &ExactSourceProjection) -> Result<bool, String> {
        let row: Option<(
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            Vec<u8>,
            Vec<u8>,
            Vec<u8>,
            i64,
        )> = self
            .mailbox
            .conn
            .query_row(
                "SELECT admission_id,request_id,decision_id,root_id,source_generation,
                 owner_generation,supervisor_id,issuer_stamp_json,registration_sha256,
                 registration_bytes,binding_bytes,broker_readback_json,authority_ordinal
                 FROM broker_exact_source_projection WHERE registration_id=?1",
                [&exact.registration_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                        r.get(9)?,
                        r.get(10)?,
                        r.get(11)?,
                        r.get(12)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some(row) = row else {
            return Ok(false);
        };
        if row.0 != exact.admission_id
            || row.1 != exact.request_id
            || row.2 != exact.decision_id
            || row.3 != exact.root_id
            || row.4 != exact.source_generation
            || row.5 != exact.owner_generation
            || row.6 != exact.supervisor_id
            || row.7 != exact.issuer_stamp_json
            || row.8 != exact.registration_sha256
            || row.9 != exact.binding.registration_bytes()
            || row.10 != exact.binding.encoded()?
            || row.11 != exact.readback_json
            || row.12 != exact.continuity.authority_ordinal
        {
            return Err("exact source retained receipt/State mismatch".into());
        }
        if !self
            .mailbox
            .exact_source_materialization_matches(&exact.binding)?
        {
            return Err("exact source retained registration projection mismatch".into());
        }
        Ok(true)
    }

    fn project_next_exact_source(
        &mut self,
        state: &StateDb,
        root_id: &str,
        owner: &CompletionDomainOwner,
        expected_ordinal: i64,
    ) -> Result<bool, String> {
        let head = self.mailbox.completion_continuity_head()?;
        let current = head.as_ref().map_or(0, |h| h.authority_ordinal);
        if current != expected_ordinal {
            // A committed sidecar with a lost response is reconciled by its
            // exact State decision, receipt, and materialization readback.
            if expected_ordinal >= 0 && current == expected_ordinal + 1 {
                if let Some(exact) = state.exact_source_projection_at(current)? {
                    self.verify_exact_source_owner(&exact, root_id, owner)?;
                    if self.exact_source_receipt_matches(&exact)? {
                        return Ok(true);
                    }
                }
            }
            return Err("broker exact projection cursor changed".into());
        }
        state.completion_repair_has_suffix(head.as_ref())?;
        let Some(exact) = state.exact_source_projection_at(current + 1)? else {
            return Ok(false);
        };
        self.verify_exact_source_owner(&exact, root_id, owner)?;
        if exact.continuity.previous_continuity_digest
            != head
                .as_ref()
                .map_or(super::COMPLETION_CONTINUITY_GENESIS_DIGEST, |h| {
                    h.continuity_digest.as_str()
                })
        {
            return Err("exact source continuity predecessor changed".into());
        }
        let source = exact.binding.registration()?;
        let paths = source.paths();
        let listener = exact.binding.admission_listener()?;
        let input = CompletionEventRegistrationInput {
            event_id: &source.handle,
            delivery_mode: &source.delivery_mode,
            owner_session_id: Some(&listener.session_id),
            owner_invocation_uuid: Some(&listener.owner_invocation_uuid),
            state_dir: &source.handle_dir,
            meta_path: &paths[0],
            log_path: &paths[1],
            rc_path: &paths[2],
        };
        exact
            .binding
            .validate_input(exact.binding.caller_admission_id(), &input)?;
        let fence = self.mailbox.begin_completion_authority_fence()?;
        if fence.sidecar_generation()? != exact.continuity.sidecar_generation
            || fence.completion_continuity_head()?.as_ref() != head.as_ref()
        {
            return Err("exact source retained generation/cursor changed".into());
        }
        fence.preflight_continuation_binding(&exact.binding, true)?;
        fence.require_continuation_binding(&source.handle, true)?;
        fence.preflight_completion_event_registration(&input)?;
        fence.register_exact_source_projection(input, &exact)?;
        if !self.exact_source_receipt_matches(&exact)? {
            return Err("exact source projection commit readback absent".into());
        }
        Ok(true)
    }

    /// Read the source-effect grants on this retained connection for one
    /// broker-persisted root/PID1 incarnation. The caller must first check the
    /// exact RootRecord and admission fence. This is only a positive blocker;
    /// the old State WAL and fresh v30 stores have separate writers.
    pub fn read_root_source_effect_obligations(
        &self,
        root_id: &str,
        root_init: &PreparedProcessStamp,
    ) -> Result<BrokerSourceEffectObligations, String> {
        self.check_mailbox_read(&self.source_generation)?;
        let owner: Option<String> = self
            .mailbox
            .conn
            .query_row(
                "SELECT owner_generation FROM broker_prepared_owner WHERE root_id=?1",
                [root_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let owner = owner.ok_or("broker prepared owner absent for root source readback")?;
        let prepared = self.read_exact_prepared_owner(&self.source_generation, root_id, &owner)?;
        if &prepared.root_init != root_init {
            return Err("broker source root PID1 incarnation changed".into());
        }
        let obligations = source_effect_obligations_on(
            &self.mailbox.conn,
            &self.source_generation,
            root_id,
            &owner,
        )?;
        self.check_mailbox_read(&self.source_generation)?;
        Ok(obligations)
    }

    pub(super) fn bound_state(&self) -> Result<StateDb, String> {
        let source = self
            .state_source
            .as_ref()
            .ok_or("broker StateDb source binding absent")?;
        verify_bound_state_source(source)?;
        let state = StateDb::open_broker_repair_read_only(&source.path)?;
        verify_bound_state_source(source)?;
        Ok(state)
    }

    /// Uses the broker's persisted source authority, never a caller path.
    /// This only checks file metadata and does not open or read State, so the
    /// decision verifier can call it while a State writer holds its lock.
    pub fn bound_state_file_identity(&self) -> Result<BoundStateFileIdentity, String> {
        let source = self
            .state_source
            .as_ref()
            .ok_or("broker StateDb source binding absent")?;
        verify_bound_state_source(source)?;
        Ok(BoundStateFileIdentity {
            device: source.device,
            inode: source.inode,
        })
    }

    /// Read of the original, inode-bound State invocation before a broker
    /// source decision. This never registers a source or advances a lifecycle.
    pub fn verify_bound_invocation(
        &self,
        invocation_uuid: &str,
        session_id: &str,
        capability: &crate::CompletionRegistrationAuthority,
    ) -> Result<BoundStateFileIdentity, String> {
        let state = self.bound_state()?;
        let row: Option<(Option<String>, Option<String>, Option<String>)> = state
            .connection()
            .query_row(
                "SELECT completion_registration_capability_digest,provider_session_id,session_id
                 FROM invocations WHERE invocation_uuid=?1",
                [invocation_uuid],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let source = self
            .state_source
            .as_ref()
            .ok_or("broker StateDb source binding absent")?;
        verify_bound_state_source(source)?;
        let (Some(expected), provider_session, fallback_session) =
            row.ok_or("original State invocation absent")?
        else {
            return Err("original State invocation capability absent".into());
        };
        let mut digest = Sha256::new();
        digest.update(b"oulipoly-completion-registration-authority-v1");
        digest.update(capability.process_environment_value().as_bytes());
        let observed = format!("{:x}", digest.finalize());
        if expected.len() != observed.len()
            || expected
                .bytes()
                .zip(observed.bytes())
                .fold(0_u8, |diff, (a, b)| diff | (a ^ b))
                != 0
            || provider_session.or(fallback_session).as_deref() != Some(session_id)
        {
            return Err("original State invocation/session/capability mismatch".into());
        }
        self.bound_state_file_identity()
    }

    /// Read only the current cursor and one bounded unaccepted page. The
    /// server's pinned driver check must precede this call.
    pub fn read_bounded_repair(
        &self,
        source_generation: &str,
        root_id: &str,
        owner: &CompletionDomainOwner,
    ) -> Result<BrokerRepairReadback, String> {
        self.check_mailbox_read(source_generation)?;
        let state = self.bound_state()?;
        let head = self.mailbox.completion_continuity_head()?;
        let ordinal = head.as_ref().map_or(0, |head| head.authority_ordinal);
        let pending = self
            .mailbox
            .unaccepted_completion_continuations(&owner.supervisor_authority_id, 16)?;
        let pending_registration_ids = pending
            .into_iter()
            .map(|binding| binding.registration().map(|source| source.registration_id))
            .collect::<Result<Vec<_>, _>>()?;
        let has_more = state.completion_repair_has_suffix(head.as_ref())?;
        self.check_mailbox_read(source_generation)?;
        Ok(BrokerRepairReadback {
            source_generation: self.source_generation.clone(),
            root_id: root_id.into(),
            owner_generation: owner.owner_generation.clone(),
            authority_ordinal: ordinal,
            has_more,
            pending_registration_ids,
        })
    }

    /// Select the first still-registered source under the current supervisor
    /// scope, only after all admitted State rows have reached the sidecar.
    /// The server must establish the live root, guardian and exact driver
    /// before calling this. No caller registration ID or path is accepted.
    pub fn read_bounded_source_selection(
        &self,
        source_generation: &str,
        root_id: &str,
        owner: &CompletionDomainOwner,
    ) -> Result<BrokerSourceSelection, String> {
        self.check_mailbox_read(source_generation)?;
        let state = self.bound_state()?;
        let head = self.mailbox.completion_continuity_head()?;
        if state.completion_repair_has_suffix(head.as_ref())? {
            return Err("broker source selection requires complete State projection".into());
        }
        let candidate = self
            .mailbox
            .unaccepted_completion_continuations(&owner.supervisor_authority_id, 1)?
            .into_iter()
            .next()
            .map(|binding| -> Result<BrokerSourceCandidate, String> {
                let source = binding.registration()?;
                if source.domain_id != owner.domain_id {
                    return Err("broker selected source domain conflict".into());
                }
                if state.admitted_completion_continuation(&binding)?.as_ref() != Some(&binding) {
                    return Err("broker selected sidecar-only source lacks State admission".into());
                }
                if let Some(exact) = state.exact_source_projection_for_registration(&source.registration_id)? {
                    self.verify_exact_source_owner(&exact, root_id, owner)?;
                    if !self.exact_source_receipt_matches(&exact)?
                        || exact.binding.encoded()? != binding.encoded()?
                    {
                        return Err("exact source decision projection unavailable for source selection".into());
                    }
                } else {
                    let orphan: bool = self.mailbox.conn.query_row(
                        "SELECT EXISTS(SELECT 1 FROM broker_exact_source_projection WHERE registration_id=?1)",
                        [&source.registration_id], |r| r.get(0)
                    ).map_err(|e| e.to_string())?;
                    if orphan || state.exact_source_projection_unavailable(&source.registration_id)? {
                        return Err("orphan or unavailable exact source projection".into());
                    }
                }
                Ok(BrokerSourceCandidate {
                    registration_id: source.registration_id,
                    registration_digest: binding.registration_digest().into(),
                    listener_revision: source.listener_revision,
                    listener: binding.admission_listener()?,
                })
            })
            .transpose()?;
        if state.completion_repair_has_suffix(head.as_ref())? {
            return Err("broker State source changed during selection".into());
        }
        self.check_mailbox_read(source_generation)?;
        Ok(BrokerSourceSelection {
            source_generation: self.source_generation.clone(),
            root_id: root_id.into(),
            owner_generation: owner.owner_generation.clone(),
            authority_ordinal: head.map_or(0, |head| head.authority_ordinal),
            candidate,
        })
    }

    /// Read one pending recipient from at most 32 sessions. The broker must
    /// first prove the exact live root, owner and driver. This does not claim
    /// the session, read payload bytes onto the wire, or authorize delivery.
    pub fn read_bounded_recipient_selection(
        &mut self,
        source_generation: &str,
        root_id: &str,
        owner: &CompletionDomainOwner,
    ) -> Result<BrokerRecipientSelection, String> {
        self.check_mailbox_read(source_generation)?;
        let state = self.bound_state()?;
        let head = self.mailbox.completion_continuity_head()?;
        if state.completion_repair_has_suffix(head.as_ref())? {
            return Err("broker recipient selection requires complete State projection".into());
        }
        let sessions = self
            .mailbox
            .wake_sessions()
            .pending_delivery_session_ids(32)?;
        let mut candidate = None;
        for session_id in sessions {
            if self.mailbox.notifications_paused(&session_id)? {
                continue;
            }
            let Some(row) = self
                .mailbox
                .list_pending_for_delivery_after(&session_id, None, 0, 1)?
                .into_iter()
                .next()
            else {
                continue;
            };
            // Verification uses the retained root-owned payload repository.
            // The response carries only its exact digest and length.
            self.mailbox.payloads().verify_mailbox_row_payload(&row)?;
            let (Some(payload_sha256), Some(payload_byte_len)) =
                (row.payload_sha256, row.payload_byte_len)
            else {
                return Err("broker pending recipient has no retained payload identity".into());
            };
            if payload_byte_len < 0 || row.session_id != session_id {
                return Err("broker pending recipient identity changed".into());
            }
            candidate = Some(BrokerRecipientCandidate {
                session_id,
                seq: row.seq,
                kind: row.kind,
                handle: row.handle,
                payload_sha256,
                payload_byte_len,
            });
            break;
        }
        if state.completion_repair_has_suffix(head.as_ref())? {
            return Err("broker State source changed during recipient selection".into());
        }
        self.check_mailbox_read(source_generation)?;
        Ok(BrokerRecipientSelection {
            source_generation: self.source_generation.clone(),
            root_id: root_id.into(),
            owner_generation: owner.owner_generation.clone(),
            authority_ordinal: head.map_or(0, |head| head.authority_ordinal),
            candidate,
        })
    }

    /// Reserve only the broker's first still-pending State-projected source.
    /// The caller supplies no registration, listener, path or grant ID.
    pub fn reserve_source_effect_grant(
        &mut self,
        source_generation: &str,
        root_id: &str,
        owner: &CompletionDomainOwner,
    ) -> Result<BrokerSourceEffectGrant, String> {
        let running = self.read_exact_continuation(
            source_generation,
            root_id,
            &owner.domain_id,
            &owner.supervisor_authority_id,
            &owner.owner_generation,
            None,
        )?;
        if !running.broker_owned || running.owner != *owner {
            return Err("broker source grant requires exact running owner".into());
        }
        let selection = self.read_bounded_source_selection(source_generation, root_id, owner)?;
        let candidate = selection
            .candidate
            .ok_or("broker source grant has no pending source")?;
        let binding = self
            .mailbox
            .unaccepted_completion_continuations(&owner.supervisor_authority_id, 1)?
            .into_iter()
            .next()
            .ok_or("broker source binding vanished")?;
        let registration = binding.registration()?;
        if registration.registration_id != candidate.registration_id
            || binding.registration_digest() != candidate.registration_digest
            || registration.listener_revision != candidate.listener_revision
            || binding.admission_listener()? != candidate.listener
        {
            return Err("broker source binding changed before grant".into());
        }
        if self
            .read_source_effect_grant(source_generation, root_id, owner)?
            .is_some()
        {
            return Err("broker source grant already exists".into());
        }
        let grant_id = uuid::Uuid::new_v4().to_string();
        self.mailbox
            .conn
            .execute(
                "INSERT INTO broker_source_effect_grant (
             grant_id,source_generation,root_id,owner_generation,driver_identity,
             authority_ordinal,registration_id,registration_digest,registration_bytes,
             listener_revision,listener_json,phase,revision)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'reserved',1)",
                params![
                    grant_id,
                    source_generation,
                    root_id,
                    owner.owner_generation,
                    serde_json::to_string(&owner.driver_identity).map_err(|e| e.to_string())?,
                    selection.authority_ordinal,
                    candidate.registration_id,
                    candidate.registration_digest,
                    binding.registration_bytes(),
                    i64::try_from(candidate.listener_revision)
                        .map_err(|_| "broker source listener revision exceeds SQLite range")?,
                    serde_json::to_string(&candidate.listener).map_err(|e| e.to_string())?
                ],
            )
            .map_err(|e| format!("broker source grant reservation failed: {e}"))?;
        let exact = self
            .read_source_effect_grant(source_generation, root_id, owner)?
            .ok_or("broker source grant disappeared after reservation")?;
        if exact.grant_id != grant_id
            || exact.phase != "reserved"
            || exact.revision != 1
            || exact.candidate != candidate
            || exact.authority_ordinal != selection.authority_ordinal
        {
            return Err("broker source grant reservation/readback conflict".into());
        }
        Ok(exact)
    }

    /// Exact readback after a lost reservation reply. Selection is repeated
    /// from retained State/sidecar; a caller-selected source cannot find a
    /// grant for another registration.
    pub fn read_source_effect_grant(
        &self,
        source_generation: &str,
        root_id: &str,
        owner: &CompletionDomainOwner,
    ) -> Result<Option<BrokerSourceEffectGrant>, String> {
        let running = self.read_exact_continuation(
            source_generation,
            root_id,
            &owner.domain_id,
            &owner.supervisor_authority_id,
            &owner.owner_generation,
            None,
        )?;
        if !running.broker_owned || running.owner != *owner {
            return Err("broker source grant read requires exact running owner".into());
        }
        let selection = self.read_bounded_source_selection(source_generation, root_id, owner)?;
        let Some(candidate) = selection.candidate else {
            return Ok(None);
        };
        let row = self
            .mailbox
            .conn
            .query_row(
                "SELECT grant_id,source_generation,root_id,owner_generation,driver_identity,
             authority_ordinal,registration_digest,registration_bytes,
             listener_revision,listener_json,phase,revision
             FROM broker_source_effect_grant WHERE registration_id=?1",
                [&candidate.registration_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, Vec<u8>>(7)?,
                        row.get::<_, i64>(8)?,
                        row.get::<_, String>(9)?,
                        row.get::<_, String>(10)?,
                        row.get::<_, i64>(11)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((
            grant_id,
            generation,
            root,
            owner_id,
            driver_json,
            ordinal,
            digest,
            registration_bytes,
            listener_revision,
            listener_json,
            phase,
            revision,
        )) = row
        else {
            return Ok(None);
        };
        let listener_revision = u64::try_from(listener_revision)
            .map_err(|_| "broker source grant listener revision is negative")?;
        let driver_identity = serde_json::from_str(&driver_json).map_err(|e| e.to_string())?;
        let listener: crate::completion_continuation::ListenerIdentity =
            serde_json::from_str(&listener_json).map_err(|e| e.to_string())?;
        if generation != source_generation
            || root != root_id
            || owner_id != owner.owner_generation
            || driver_identity != owner.driver_identity
            || uuid::Uuid::parse_str(&grant_id)
                .map(|id| id.to_string() != grant_id)
                .unwrap_or(true)
            || digest != candidate.registration_digest
            || registration_bytes.len() > crate::completion_continuation::MAX_REGISTRATION_BYTES
            || crate::completion_continuation::sha256(&registration_bytes) != digest
            || listener_revision != candidate.listener_revision
            || listener != candidate.listener
            || ordinal > selection.authority_ordinal
            || !matches!(
                (phase.as_str(), revision),
                ("reserved", 1) | ("consumed" | "unknown", 2..)
            )
        {
            return Err("broker source grant exact readback conflict".into());
        }
        Ok(Some(BrokerSourceEffectGrant {
            grant_id,
            source_generation: generation,
            root_id: root,
            owner_generation: owner_id,
            driver_identity,
            authority_ordinal: ordinal,
            candidate,
            phase,
            revision,
        }))
    }

    /// Read the exact original registration stored with the reserved grant.
    /// The caller must check the original pathname and recovery image before
    /// consuming this one-use authority.
    pub fn read_reserved_source_material(
        &self,
        source_generation: &str,
        root_id: &str,
        owner: &CompletionDomainOwner,
    ) -> Result<BrokerSourceMaterial, String> {
        let grant = self
            .read_source_effect_grant(source_generation, root_id, owner)?
            .ok_or("reserved source grant absent")?;
        if grant.phase != "reserved" || grant.revision != 1 {
            return Err("source grant is not reserved".into());
        }
        let bytes: Vec<u8> = self
            .mailbox
            .conn
            .query_row(
                "SELECT registration_bytes FROM broker_source_effect_grant
             WHERE grant_id=?1 AND registration_id=?2 AND phase='reserved' AND revision=1",
                params![grant.grant_id, grant.candidate.registration_id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        let registration: crate::completion_continuation::SourceRegistration =
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        registration.validate()?;
        if bytes.len() > crate::completion_continuation::MAX_REGISTRATION_BYTES
            || crate::completion_continuation::sha256(&bytes) != grant.candidate.registration_digest
            || registration.registration_id != grant.candidate.registration_id
            || registration.listener_revision != grant.candidate.listener_revision
        {
            return Err("reserved source registration changed".into());
        }
        Ok(BrokerSourceMaterial {
            grant,
            registration_bytes: bytes,
        })
    }

    /// Post-owner readback for a physical grant. This is candidate evidence,
    /// not an acceptance transition: the original v2 files are mutable and a
    /// separate broker-owned snapshot/commit fence is still required.
    pub fn read_consumed_source_candidate(
        &self,
        physical_grant: &BrokerSourceEffectGrant,
    ) -> Result<crate::completion_continuation::AdmittedSourceBinding, String> {
        self.check_mailbox_read(&physical_grant.source_generation)?;
        if physical_grant.phase != "consumed" || physical_grant.revision != 2 {
            return Err("physical source is not a consumed grant".into());
        }
        let row: (
            String,
            String,
            String,
            String,
            i64,
            String,
            Vec<u8>,
            i64,
            String,
            String,
            i64,
        ) = self
            .mailbox
            .conn
            .query_row(
                "SELECT source_generation,root_id,owner_generation,driver_identity,
                    authority_ordinal,registration_digest,registration_bytes,
                    listener_revision,listener_json,phase,revision
             FROM broker_source_effect_grant WHERE grant_id=?1 AND registration_id=?2",
                params![
                    physical_grant.grant_id,
                    physical_grant.candidate.registration_id
                ],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                    ))
                },
            )
            .map_err(|e| e.to_string())?;
        if row.0 != physical_grant.source_generation
            || row.1 != physical_grant.root_id
            || row.2 != physical_grant.owner_generation
            || row.3
                != serde_json::to_string(&physical_grant.driver_identity)
                    .map_err(|e| e.to_string())?
            || row.4 != physical_grant.authority_ordinal
            || row.5 != physical_grant.candidate.registration_digest
            || row.7
                != i64::try_from(physical_grant.candidate.listener_revision)
                    .map_err(|e| e.to_string())?
            || serde_json::from_str::<crate::completion_continuation::ListenerIdentity>(&row.8)
                .map_err(|e| e.to_string())?
                != physical_grant.candidate.listener
            || row.9 != "consumed"
            || row.10 != 2
            || crate::completion_continuation::sha256(&row.6) != row.5
        {
            return Err("consumed source grant changed after physical launch".into());
        }
        let scope: (String, String, String) = self.mailbox.conn.query_row(
            "SELECT domain_id,supervisor_authority_id,driver_identity
             FROM broker_prepared_owner WHERE owner_generation=?1 AND source_generation=?2 AND root_id=?3",
            params![physical_grant.owner_generation, physical_grant.source_generation, physical_grant.root_id],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        ).map_err(|e| e.to_string())?;
        let retained: (Vec<u8>, String, String) = self.mailbox.conn.query_row(
            "SELECT binding,phase,supervisor_authority_id FROM completion_continuation_source
             WHERE registration_id=?1 AND domain_id=(SELECT domain_id FROM completion_continuation_domain)",
            [&physical_grant.candidate.registration_id],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        ).map_err(|e| e.to_string())?;
        let binding = crate::completion_continuation::AdmittedSourceBinding::decode(&retained.0)?;
        let prepared_driver: PreparedProcessStamp =
            serde_json::from_str(&scope.2).map_err(|e| e.to_string())?;
        if retained.1 != "registered"
            || retained.2 != scope.1
            || binding.registration()?.domain_id != scope.0
            || i64::from(prepared_driver.host_pid) != physical_grant.driver_identity.pid
            || prepared_driver.boot_id != physical_grant.driver_identity.boot_id
            || i64::try_from(prepared_driver.starttime_ticks).map_err(|e| e.to_string())?
                != physical_grant.driver_identity.starttime_ticks
            || binding.registration_bytes() != row.6
            || binding.registration_digest() != row.5
            || binding.admission_listener()? != physical_grant.candidate.listener
            || binding.registration()?.listener_revision
                != physical_grant.candidate.listener_revision
        {
            return Err("consumed source projection changed".into());
        }
        let state = self.bound_state()?;
        if state.admitted_completion_continuation(&binding)?.as_ref() != Some(&binding) {
            return Err("consumed source lost exact State admission".into());
        }
        self.check_mailbox_read(&physical_grant.source_generation)?;
        Ok(binding)
    }

    pub fn read_source_evidence(
        &self,
        grant: &BrokerSourceEffectGrant,
    ) -> Result<Option<BrokerSourceEvidenceReadback>, String> {
        self.check_mailbox_read(&grant.source_generation)?;
        let row: Option<(String, String, String, Option<String>, String, i64)> = self
            .mailbox
            .conn
            .query_row(
                "SELECT grant_id,source_generation,registration_id,seal_json,phase,revision
                 FROM broker_source_evidence WHERE grant_id=?1",
                [&grant.grant_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;
        row.map(
            |(grant_id, generation, registration_id, json, phase, revision)| {
                if grant_id != grant.grant_id
                    || generation != grant.source_generation
                    || registration_id != grant.candidate.registration_id
                    || !matches!(
                        (phase.as_str(), revision),
                        ("unknown", 1) | ("captured", 1) | ("accepted", 2)
                    )
                    || (phase == "unknown") != json.is_none()
                {
                    return Err("broker source evidence row changed".into());
                }
                let seal = json
                    .map(|value| serde_json::from_str(&value).map_err(|e| e.to_string()))
                    .transpose()?;
                Ok(BrokerSourceEvidenceReadback {
                    grant_id,
                    source_generation: generation,
                    registration_id,
                    seal,
                    phase,
                    revision,
                })
            },
        )
        .transpose()
    }

    /// A failed capture or lost reply must leave one durable unknown row. The
    /// consumed one-use grant remains debt even if this second write fails.
    pub fn retain_unknown_source_evidence(
        &mut self,
        grant: &BrokerSourceEffectGrant,
    ) -> Result<BrokerSourceEvidenceReadback, String> {
        self.check_mailbox_read(&grant.source_generation)?;
        if grant.phase != "consumed" || grant.revision != 2 {
            return Err("unknown evidence requires consumed source grant".into());
        }
        self.mailbox
            .conn
            .execute(
                "INSERT OR IGNORE INTO broker_source_evidence
             (grant_id,source_generation,registration_id,seal_json,phase,revision)
             SELECT grant_id,source_generation,registration_id,NULL,'unknown',1
             FROM broker_source_effect_grant
             WHERE grant_id=?1 AND source_generation=?2 AND registration_id=?3
               AND phase='consumed' AND revision=2",
                params![
                    grant.grant_id,
                    grant.source_generation,
                    grant.candidate.registration_id
                ],
            )
            .map_err(|e| e.to_string())?;
        self.read_source_evidence(grant)?
            .ok_or("unknown source evidence readback absent".into())
    }

    /// Pin an already fsynced root-only snapshot. A duplicate capture cannot
    /// replace its seal, and unknown debt cannot be upgraded by retry.
    pub fn record_source_evidence_snapshot(
        &mut self,
        grant: &BrokerSourceEffectGrant,
        seal: &BrokerSourceEvidenceSeal,
    ) -> Result<BrokerSourceEvidenceReadback, String> {
        let binding = self.read_consumed_source_candidate(grant)?;
        if seal.manifest_inode == 0
            || seal.manifest_byte_len == 0
            || seal.manifest_byte_len > 32 * 1024 * 1024
            || [
                &seal.manifest_sha256,
                &seal.snapshot_sha256,
                &seal.outcome_sha256,
                &seal.recovery_stdout_sha256,
            ]
            .into_iter()
            .any(|value| !crate::completion_continuation::is_sha256(value))
            || binding.registration()?.registration_id != grant.candidate.registration_id
        {
            return Err("invalid broker source evidence seal".into());
        }
        let json = serde_json::to_string(seal).map_err(|e| e.to_string())?;
        let changed = self
            .mailbox
            .conn
            .execute(
                "INSERT INTO broker_source_evidence
             (grant_id,source_generation,registration_id,seal_json,phase,revision)
             SELECT grant_id,source_generation,registration_id,?4,'captured',1
             FROM broker_source_effect_grant
             WHERE grant_id=?1 AND source_generation=?2 AND registration_id=?3
               AND phase='consumed' AND revision=2",
                params![
                    grant.grant_id,
                    grant.source_generation,
                    grant.candidate.registration_id,
                    json
                ],
            )
            .map_err(|e| e.to_string())?;
        if changed != 1 {
            return Err("consumed source evidence capture CAS failed".into());
        }
        let readback = self
            .read_source_evidence(grant)?
            .ok_or("captured source evidence readback absent")?;
        if readback.phase != "captured"
            || readback.revision != 1
            || readback.seal.as_ref() != Some(seal)
        {
            return Err("captured source evidence readback conflict".into());
        }
        Ok(readback)
    }

    /// Exact positive fence, intentionally closed until the fresh v30 lane
    /// writes an independently broker-authenticated admission provenance row.
    /// Old v29 re-admission, physical zero exit, and a matching hash cannot
    /// populate that row. This transition does not release or notify.
    pub fn commit_source_evidence_acceptance(
        &mut self,
        grant: &BrokerSourceEffectGrant,
        seal: &BrokerSourceEvidenceSeal,
    ) -> Result<BrokerSourceEvidenceReadback, String> {
        let binding = self.read_consumed_source_candidate(grant)?;
        let before = self
            .read_source_evidence(grant)?
            .ok_or("captured source evidence absent")?;
        if before.phase != "captured" || before.revision != 1 || before.seal.as_ref() != Some(seal)
        {
            return Err("source evidence commit seal changed".into());
        }
        let changed = self
            .mailbox
            .conn
            .execute(
                "UPDATE broker_source_evidence SET phase='accepted',revision=2
             WHERE grant_id=?1 AND source_generation=?2 AND registration_id=?3
               AND seal_json=?4 AND phase='captured' AND revision=1
               AND EXISTS (
                 SELECT 1 FROM broker_source_effect_grant g
                 WHERE g.grant_id=?1 AND g.source_generation=?2
                   AND g.registration_id=?3 AND g.phase='consumed' AND g.revision=2)
               AND EXISTS (
                 SELECT 1 FROM broker_fresh_source_admission a
                 WHERE a.registration_id=?3 AND a.source_generation=?2
                   AND a.registration_digest=?5 AND a.state_admission_id=?6)",
                params![
                    grant.grant_id,
                    grant.source_generation,
                    grant.candidate.registration_id,
                    serde_json::to_string(seal).map_err(|e| e.to_string())?,
                    binding.registration_digest(),
                    binding.caller_admission_id(),
                ],
            )
            .map_err(|e| e.to_string())?;
        if changed != 1 {
            return Err(
                "fresh v30 source admission authority absent or source commit changed".into(),
            );
        }
        let after = self
            .read_source_evidence(grant)?
            .ok_or("accepted source evidence readback absent")?;
        if after.phase != "accepted" || after.revision != 2 || after.seal.as_ref() != Some(seal) {
            return Err("accepted source evidence readback conflict".into());
        }
        Ok(after)
    }

    /// One conditional irreversible transition. A lost reply cannot consume
    /// again; the caller must retain the consumed row as unknown debt until
    /// an exact held-child physical record is fsynced.
    pub fn consume_reserved_source_effect_grant(
        &mut self,
        material: &BrokerSourceMaterial,
        owner: &CompletionDomainOwner,
    ) -> Result<BrokerSourceEffectGrant, String> {
        let exact = self.read_reserved_source_material(
            &material.grant.source_generation,
            &material.grant.root_id,
            owner,
        )?;
        if exact != *material {
            return Err("source material changed before consume".into());
        }
        let synchronous: i64 = self
            .mailbox
            .conn
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        let journal: String = self
            .mailbox
            .conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        if synchronous != 2 || !journal.eq_ignore_ascii_case("wal") {
            return Err("source consume requires durable FULL WAL sidecar".into());
        }
        consume_source_grant_row(&self.mailbox.conn, material)?;
        let consumed = self
            .read_source_effect_grant(
                &material.grant.source_generation,
                &material.grant.root_id,
                owner,
            )?
            .ok_or("consumed source grant disappeared")?;
        if consumed.phase != "consumed"
            || consumed.revision != 2
            || consumed.grant_id != material.grant.grant_id
        {
            return Err("consumed source grant readback conflict".into());
        }
        Ok(consumed)
    }

    /// On broker restart the old in-memory gate and child custody cannot be
    /// reconstructed. Retain the one-use debt; never convert it to a fresh
    /// reservation or a false never-started outcome.
    pub fn orphan_reserved_source_grants(&mut self) -> Result<usize, String> {
        self.check_mailbox_read(&self.source_generation)?;
        self.mailbox
            .conn
            .execute(
                "UPDATE broker_source_effect_grant SET phase='unknown',revision=revision+1
             WHERE phase='reserved'",
                [],
            )
            .map_err(|e| e.to_string())
    }

    /// StateDb supplies every admitted binding; the root-retained sidecar is
    /// the only projection writer. A page failure leaves its durable cursor
    /// at the last committed row for exact retry after broker/driver loss.
    pub fn repair_bounded_suffix(
        &mut self,
        source_generation: &str,
        root_id: &str,
        owner: &CompletionDomainOwner,
        expected_ordinal: i64,
    ) -> Result<BrokerRepairReadback, String> {
        self.check_mailbox_read(source_generation)?;
        let mut state = self.bound_state()?;
        if self.project_next_exact_source(&state, root_id, owner, expected_ordinal)? {
            let readback = self.read_bounded_repair(source_generation, root_id, owner)?;
            return Ok(readback);
        }
        let (authority_ordinal, has_more, pending_registration_ids) = state
            .repair_pending_domain_completion_continuations_on(
                &mut self.mailbox,
                &owner.domain_id,
                &owner.supervisor_authority_id,
                expected_ordinal,
                64,
                16,
            )?;
        verify_bound_state_source(
            self.state_source
                .as_ref()
                .ok_or("broker StateDb source binding absent")?,
        )?;
        self.check_mailbox_read(source_generation)?;
        Ok(BrokerRepairReadback {
            source_generation: self.source_generation.clone(),
            root_id: root_id.into(),
            owner_generation: owner.owner_generation.clone(),
            authority_ordinal,
            has_more,
            pending_registration_ids,
        })
    }

    /// Root-side precursor for new completion mail. The caller must already
    /// have broker-proven source and recipient facts; this does not create a
    /// wire ingress grant. Payload bytes are copied into the repository
    /// addressed from this retained root-owned sidecar, never a caller path.
    /// No delivery or ACK state changes here.
    pub fn enqueue_notification_bytes<'a>(
        &mut self,
        source_generation: &str,
        mut input: AgentBashCompleteEnqueue<'a>,
        bytes: &'a [u8],
    ) -> Result<EnqueueResult, String> {
        self.check_mailbox_read(source_generation)?;
        if bytes.len() > 32 * 1024 * 1024 {
            return Err("broker notification payload exceeds ingress bound".into());
        }
        let payload =
            std::str::from_utf8(bytes).map_err(|_| "broker notification payload is not UTF-8")?;
        let value: serde_json::Value =
            serde_json::from_str(payload).map_err(|_| "broker notification payload is not JSON")?;
        if !value.is_object()
            || value
                .get("handle")
                .and_then(|v| v.as_str())
                .is_some_and(|handle| handle != input.handle)
            || value.get("completion_protocol").is_some_and(|protocol| {
                protocol.as_str() != Some(crate::completion_continuation::PROTOCOL)
            })
            || value
                .get("protocol")
                .is_some_and(|protocol| protocol.as_str().is_none_or(str::is_empty))
            || (value.get("completion_protocol").is_none()
                && value
                    .get("protocol")
                    .and_then(|v| v.as_str())
                    .is_none_or(str::is_empty))
        {
            return Err("broker notification payload protocol or handle differs".into());
        }
        input.payload_json = payload;
        let result = self.mailbox.enqueue_agent_bash_complete(&input)?;
        let row = match &result {
            EnqueueResult::Inserted(row) | EnqueueResult::AlreadyEnqueued(row) => row,
            EnqueueResult::Conflict { existing } => existing,
        };
        self.mailbox.payloads().verify_mailbox_row_payload(row)?;
        self.check_mailbox_read(source_generation)?;
        Ok(result)
    }

    /// Exact PK lookup for a session. The broker must supply the session from
    /// its own authority, not accept it as mutation authority from a caller.
    /// This only reads row metadata; byte consumption still verifies the
    /// immutable broker-owned file against the exact row.
    pub fn read_exact_mailbox_row(
        &self,
        source_generation: &str,
        session_id: &str,
        seq: i64,
    ) -> Result<Option<BrokerMailboxReadback>, String> {
        self.check_mailbox_read(source_generation)?;
        if session_id.is_empty() || seq <= 0 {
            return Err("broker mailbox exact key is invalid".into());
        }
        let row = self
            .mailbox
            .conn
            .query_row(
                &format!(
                    "SELECT {MAILBOX_ROW_COLUMNS} FROM mailbox WHERE seq=?1 AND session_id=?2"
                ),
                params![seq, session_id],
                map_mailbox_row,
            )
            .optional()
            .map_err(|error| format!("broker mailbox exact read failed: {error}"))?;
        self.check_mailbox_read(source_generation)?;
        Ok(row.map(|row| BrokerMailboxReadback {
            source_generation: self.source_generation.clone(),
            row,
        }))
    }

    /// Cursor-bound pending read. The existing delivery query uses its live
    /// indexes and excludes terminal rows; it never scans historical mailbox
    /// rows or holds a write transaction while the caller consumes a page.
    pub fn read_pending_mailbox_page(
        &self,
        source_generation: &str,
        session_id: &str,
        after_seq: i64,
        limit: usize,
    ) -> Result<Vec<BrokerMailboxReadback>, String> {
        self.check_mailbox_read(source_generation)?;
        if session_id.is_empty() || after_seq < 0 || !(1..=64).contains(&limit) {
            return Err("broker mailbox page key or limit is invalid".into());
        }
        let rows = self
            .mailbox
            .list_pending_for_delivery_after(session_id, None, after_seq, limit)?;
        self.check_mailbox_read(source_generation)?;
        Ok(rows
            .into_iter()
            .map(|row| BrokerMailboxReadback {
                source_generation: self.source_generation.clone(),
                row,
            })
            .collect())
    }

    fn check_mailbox_read(&self, source_generation: &str) -> Result<(), String> {
        if source_generation != self.source_generation {
            return Err("broker mailbox source generation changed".into());
        }
        #[cfg(unix)]
        check_storage(&self.mailbox.path, self.storage_owner, &self.storage_anchor)?;
        broker_main_file_must_be_named(&self.mailbox.conn)
    }

    /// Persist only broker-observed preparation on the retained root-owned
    /// connection. A duplicate or copied v29 running generation is refused.
    /// The caller still must prove live pinned actors before invoking this;
    /// State never treats this record as a running owner.
    pub fn prepare_exact_owner(
        &mut self,
        prepared: &PreparedBrokerOwner,
    ) -> Result<PreparedBrokerOwner, String> {
        if !prepared.valid() || prepared.source_generation != self.source_generation {
            return Err("invalid broker prepared owner binding".into());
        }
        #[cfg(unix)]
        check_storage(&self.mailbox.path, self.storage_owner, &self.storage_anchor)?;
        broker_main_file_must_be_named(&self.mailbox.conn)?;
        let tx = self
            .mailbox
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let old: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM completion_continuation_owner WHERE generation=?1)",
                [&prepared.owner_generation],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if old {
            return Err("prepared generation already belongs to a running owner".into());
        }
        let encode =
            |stamp: &PreparedProcessStamp| serde_json::to_string(stamp).map_err(|e| e.to_string());
        tx.execute(
            "INSERT INTO broker_prepared_owner(owner_generation,source_generation,root_id,
             owner_uid,domain_id,supervisor_authority_id,endpoint,entry_identity,guardian_identity,
             driver_identity,root_init_identity,joined_child_identity)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                prepared.owner_generation,
                prepared.source_generation,
                prepared.root_id,
                i64::from(prepared.owner_uid),
                prepared.domain_id,
                prepared.supervisor_authority_id,
                prepared.endpoint,
                encode(&prepared.entry)?,
                encode(&prepared.guardian)?,
                encode(&prepared.driver)?,
                encode(&prepared.root_init)?,
                encode(&prepared.joined_child)?,
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        self.read_exact_prepared_owner(
            &prepared.source_generation,
            &prepared.root_id,
            &prepared.owner_generation,
        )
    }

    /// Exact readback also serves lost-reply reconciliation. It does not
    /// report current liveness or authorize release after broker restart.
    pub fn read_exact_prepared_owner(
        &self,
        source_generation: &str,
        root_id: &str,
        owner_generation: &str,
    ) -> Result<PreparedBrokerOwner, String> {
        if source_generation != self.source_generation {
            return Err("broker prepared source generation changed".into());
        }
        #[cfg(unix)]
        check_storage(&self.mailbox.path, self.storage_owner, &self.storage_anchor)?;
        broker_main_file_must_be_named(&self.mailbox.conn)?;
        let row = self
            .mailbox
            .conn
            .query_row(
                "SELECT owner_uid,domain_id,supervisor_authority_id,endpoint,entry_identity,
                 guardian_identity,driver_identity,root_init_identity,joined_child_identity
                 FROM broker_prepared_owner WHERE source_generation=?1 AND root_id=?2
                 AND owner_generation=?3",
                params![source_generation, root_id, owner_generation],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, String>(8)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or("broker prepared owner absent")?;
        let decode = |value: &str| serde_json::from_str(value).map_err(|e| e.to_string());
        let prepared = PreparedBrokerOwner {
            source_generation: source_generation.into(),
            root_id: root_id.into(),
            owner_uid: u32::try_from(row.0).map_err(|_| "broker prepared UID invalid")?,
            domain_id: row.1,
            supervisor_authority_id: row.2,
            owner_generation: owner_generation.into(),
            endpoint: row.3,
            entry: decode(&row.4)?,
            guardian: decode(&row.5)?,
            driver: decode(&row.6)?,
            root_init: decode(&row.7)?,
            joined_child: decode(&row.8)?,
        };
        if !prepared.valid() {
            return Err("broker prepared owner row invalid".into());
        }
        broker_main_file_must_be_named(&self.mailbox.conn)?;
        Ok(prepared)
    }

    /// Commit only after the serving broker has written its retained gate.
    /// A lost reply is reconciled with `read_exact_release`, never a retry.
    pub fn commit_exact_prepared_release(
        &mut self,
        expected: &PreparedBrokerOwner,
    ) -> Result<BrokerReleaseEvidence, String> {
        if !expected.valid() || expected.source_generation != self.source_generation {
            return Err("invalid exact release binding".into());
        }
        let prepared = self.read_exact_prepared_owner(
            &expected.source_generation,
            &expected.root_id,
            &expected.owner_generation,
        )?;
        if &prepared != expected {
            return Err("prepared release actor or endpoint changed".into());
        }
        let identity = |stamp: &PreparedProcessStamp| -> Result<_, String> {
            Ok(crate::completion_continuation::SourceProcessIdentity {
                pid: i64::from(stamp.host_pid),
                boot_id: stamp.boot_id.clone(),
                starttime_ticks: i64::try_from(stamp.starttime_ticks)
                    .map_err(|_| "release process starttime overflow")?,
            })
        };
        let owner = super::CompletionDomainOwner {
            protocol: crate::completion_continuation::PROTOCOL.into(),
            domain_id: prepared.domain_id.clone(),
            supervisor_authority_id: prepared.supervisor_authority_id.clone(),
            owner_generation: prepared.owner_generation.clone(),
            guardian_identity: identity(&prepared.guardian)?,
            driver_identity: identity(&prepared.driver)?,
            endpoint: prepared.endpoint.clone(),
        };
        #[cfg(unix)]
        check_storage(&self.mailbox.path, self.storage_owner, &self.storage_anchor)?;
        broker_main_file_must_be_named(&self.mailbox.conn)?;
        let release_id = uuid::Uuid::new_v4().to_string();
        let tx = self
            .mailbox
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let guardian =
            serde_json::to_string(&owner.guardian_identity).map_err(|e| e.to_string())?;
        let driver = serde_json::to_string(&owner.driver_identity).map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO broker_owner_release(owner_generation,source_generation,root_id,
             release_id,guardian_identity,driver_identity,committed_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                owner.owner_generation,
                prepared.source_generation,
                prepared.root_id,
                release_id,
                guardian,
                driver,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO broker_completion_owner(owner_generation,source_generation,root_id,
             guardian_identity,driver_identity) VALUES(?1,?2,?3,?4,?5)",
            params![
                owner.owner_generation,
                prepared.source_generation,
                prepared.root_id,
                guardian,
                driver,
            ],
        )
        .map_err(|e| e.to_string())?;
        MailboxDb::publish_completion_owner_on(&tx, &owner, Some(&prepared.root_id))?;
        tx.commit().map_err(|e| e.to_string())?;
        self.read_exact_release(
            &prepared.source_generation,
            &prepared.root_id,
            &prepared.owner_generation,
        )
    }

    /// Exact committed readback for a lost release reply. This checks durable
    /// State only and deliberately makes no live-process authorization claim.
    pub fn read_exact_release(
        &self,
        source_generation: &str,
        root_id: &str,
        owner_generation: &str,
    ) -> Result<BrokerReleaseEvidence, String> {
        let prepared =
            self.read_exact_prepared_owner(source_generation, root_id, owner_generation)?;
        let release_id: String = self
            .mailbox
            .conn
            .query_row(
                "SELECT release_id FROM broker_owner_release WHERE owner_generation=?1
                 AND source_generation=?2 AND root_id=?3",
                params![owner_generation, source_generation, root_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or("exact broker release absent")?;
        let readback = self.read_exact_continuation(
            source_generation,
            root_id,
            &prepared.domain_id,
            &prepared.supervisor_authority_id,
            owner_generation,
            None,
        )?;
        if !readback.broker_owned
            || readback.owner.endpoint != prepared.endpoint
            || readback.owner.guardian_identity.pid != i64::from(prepared.guardian.host_pid)
            || readback.owner.guardian_identity.boot_id != prepared.guardian.boot_id
            || readback.owner.guardian_identity.starttime_ticks
                != i64::try_from(prepared.guardian.starttime_ticks).map_err(|e| e.to_string())?
            || readback.owner.driver_identity.pid != i64::from(prepared.driver.host_pid)
            || readback.owner.driver_identity.boot_id != prepared.driver.boot_id
            || readback.owner.driver_identity.starttime_ticks
                != i64::try_from(prepared.driver.starttime_ticks).map_err(|e| e.to_string())?
        {
            return Err("broker release owner provenance mismatch".into());
        }
        Ok(BrokerReleaseEvidence {
            prepared,
            release_id,
            owner: readback.owner,
        })
    }
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
    pub fn activate_quiesced_copy(
        path: &Path,
        broker_state_root: &Path,
        _proof: &QuiescedCutoverProof,
    ) -> Result<String, String> {
        require_host_root()?;
        require_root_owned_ancestors(broker_state_root)?;
        activate_with_owner(path, 0, broker_state_root)
    }

    /// The private user-namespace broker fixture stages beneath a temporary
    /// parent. It still exercises exact v29-to-v30 activation and storage
    /// checks from the fixture anchor down, without treating /tmp as an
    /// installed root-owned ancestor.
    #[cfg(all(unix, feature = "age319-private-broker-fixture"))]
    pub fn activate_private_fixture_copy(
        path: &Path,
        broker_state_root: &Path,
        _proof: &QuiescedCutoverProof,
    ) -> Result<String, String> {
        require_host_root()?;
        activate_with_owner(path, 0, broker_state_root)
    }

    /// Private namespace fixture for a quiesced copy containing retained
    /// payload files. Production still requires the installed writer census.
    #[cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]
    pub fn stage_private_fixture_copy(
        source: &Path,
        broker_state_root: &Path,
        _proof: &QuiescedCutoverProof,
    ) -> Result<std::path::PathBuf, String> {
        require_host_root()?;
        stage_with_owner(source, 0, broker_state_root, 0)
    }

    #[cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]
    pub fn publish_private_fixture_copy(
        source: &Path,
        stage: &Path,
        broker_state_root: &Path,
        _proof: &QuiescedCutoverProof,
    ) -> Result<String, String> {
        require_host_root()?;
        publish_with_owner(source, 0, stage, broker_state_root, 0)
    }

    #[cfg(all(unix, feature = "age319-private-broker-fixture"))]
    pub fn bind_private_fixture_state_source(
        sidecar_path: &Path,
        state_path: &Path,
    ) -> Result<(), String> {
        require_host_root()?;
        write_state_source_binding(
            sidecar_path
                .parent()
                .ok_or("broker sidecar parent absent")?,
            state_path,
            0,
            0,
        )
    }

    /// Prepare an inert, root-only SQLite snapshot and retained payload copy
    /// of an exact v29 source.
    /// The returned directory is never consulted by the broker. Installation
    /// must separately stop and join all old writers, fence new entries,
    /// verify source handles, and then publish/activate under that proof.
    /// Staging alone grants no authority and never changes the source.
    #[cfg(unix)]
    pub fn stage_offline_v29_snapshot(
        source: &Path,
        source_owner: u32,
        broker_state_root: &Path,
        _proof: &QuiescedCutoverProof,
    ) -> Result<std::path::PathBuf, String> {
        require_host_root()?;
        require_root_owned_ancestors(broker_state_root)?;
        stage_with_owner(source, source_owner, broker_state_root, 0)
    }

    /// Publish only a previously validated database and payload snapshot under
    /// an installed quiescence proof. The directory rename never replaces an
    /// existing sidecar. A crash after rename leaves v29 at the fixed name
    /// and broker startup refuses until activation resumes explicitly.
    #[cfg(target_os = "linux")]
    pub fn publish_and_activate_quiesced_copy(
        source: &Path,
        source_owner: u32,
        stage: &Path,
        broker_state_root: &Path,
        _proof: &QuiescedCutoverProof,
    ) -> Result<String, String> {
        require_host_root()?;
        require_root_owned_ancestors(broker_state_root)?;
        publish_with_owner(source, source_owner, stage, broker_state_root, 0)
    }

    #[cfg(not(unix))]
    pub fn activate_quiesced_copy(
        _path: &Path,
        _broker_state_root: &Path,
        _proof: &QuiescedCutoverProof,
    ) -> Result<String, String> {
        Err("broker-owned sidecar requires Unix root storage".into())
    }

    pub fn source_generation(&self) -> &str {
        &self.source_generation
    }

    /// Domain identity is read from the retained broker connection for v30
    /// entry. The retired user-side copy never supplies this binding.
    pub fn domain_id(&self) -> Result<String, String> {
        #[cfg(unix)]
        check_storage(&self.mailbox.path, self.storage_owner, &self.storage_anchor)?;
        broker_main_file_must_be_named(&self.mailbox.conn)?;
        self.mailbox
            .completion_continuation_domain()?
            .ok_or_else(|| "broker completion domain absent".into())
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
        if attempt.operation == "source_recovery" {
            return Err(
                "broker source recovery requires exact registration/listener file custody and a one-use effect grant".into(),
            );
        }
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
        self.mailbox
            .reserve_continuation_attempt_for_broker(attempt, &owner.driver_identity)?;
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

    /// Withdraw one exact unaccepted proposal after a failed launch response.
    /// A lost reply is reconciled by reading this same PK; accepted work is
    /// never revoked and the driver must not issue a replacement meanwhile.
    pub fn revoke_exact_unaccepted_attempt(
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
            || before.attempt.as_ref() != Some(attempt)
        {
            return Err("broker withdrawal owner/attempt conflict".into());
        }
        if before.phase.as_deref() != Some("reserved") || before.revision != Some(1) {
            return Err("broker withdrawal requires unaccepted reservation".into());
        }
        if !self
            .mailbox
            .try_revoke_unaccepted_continuation_attempt_for_broker(
                attempt,
                &owner.driver_identity,
            )?
        {
            return Err("broker withdrawal lost unaccepted reservation".into());
        }
        let after = self.read_exact_continuation(
            &self.source_generation,
            root_id,
            &owner.domain_id,
            &owner.supervisor_authority_id,
            &owner.owner_generation,
            Some(&attempt.attempt_id),
        )?;
        if after.attempt.as_ref() != Some(attempt)
            || after.phase.as_deref() != Some("never_started")
            || after.revision != Some(2)
            || after.claim_present
        {
            return Err("broker withdrawal readback conflict".into());
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
        if attempt.operation == "source_recovery" {
            return Err(
                "broker source recovery acceptance requires physical effect custody".into(),
            );
        }
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

    /// Bind a broker grant only to this connection's accepted, broker-owned
    /// attempt. The accepted snapshot is checked against the live row by the
    /// existing State CAS; a copied v29 row or a caller-selected MailboxDb can
    /// never enter this method. Exact readback also resolves a lost CAS reply.
    pub fn bind_exact_native_grant_v30(
        &mut self,
        source_generation: &str,
        root_id: &str,
        owner: &super::CompletionDomainOwner,
        accepted: &super::AcceptedNativeGrantSnapshot,
        grant_id: &str,
        request_sha256: &str,
    ) -> Result<BrokerNativeGrantReadback, String> {
        let existing = self.read_exact_native_grant_v30(
            source_generation,
            root_id,
            owner,
            accepted,
            grant_id,
            request_sha256,
        )?;
        if let Some(existing) = existing {
            return Ok(existing);
        }
        self.mailbox
            .bind_exact_native_grant_for_broker(accepted, grant_id, request_sha256)?;
        self.read_exact_native_grant_v30(
            source_generation,
            root_id,
            owner,
            accepted,
            grant_id,
            request_sha256,
        )?
        .ok_or("broker native binding absent after commit".into())
    }

    /// Exact PK readback for N/K uncertainty. A historical copied v29 native
    /// binding remains in the database, but its owner lacks broker provenance.
    pub fn read_exact_native_grant_v30(
        &self,
        source_generation: &str,
        root_id: &str,
        owner: &super::CompletionDomainOwner,
        accepted: &super::AcceptedNativeGrantSnapshot,
        grant_id: &str,
        request_sha256: &str,
    ) -> Result<Option<BrokerNativeGrantReadback>, String> {
        self.check_mailbox_read(source_generation)?;
        let exact = self.read_exact_continuation(
            source_generation,
            root_id,
            &owner.domain_id,
            &owner.supervisor_authority_id,
            &owner.owner_generation,
            Some(&accepted.attempt.attempt_id),
        )?;
        if !exact.broker_owned
            || exact.owner != *owner
            || exact.attempt.as_ref() != Some(&accepted.attempt)
            || exact.phase.as_deref() != Some("accepted")
            || exact.revision != Some(2)
            || !exact.claim_present
            || accepted.phase != "accepted"
            || accepted.revision != 2
            || accepted.integrated
            || accepted.custodian_identity.is_some()
            || accepted.adopter_identity.is_some()
            || accepted.domain_id != owner.domain_id
            || accepted.kernel_root_id != root_id
            || accepted.supervisor_authority_id != owner.supervisor_authority_id
            || accepted.owner_generation != owner.owner_generation
            || accepted.guardian_identity != owner.guardian_identity
            || accepted.attempt.operation != "activation"
        {
            return Err("broker native K requires exact v30 accepted owner/attempt".into());
        }
        let (integrated, custodian, adopter): (i64, Option<String>, Option<String>) = self
            .mailbox
            .conn
            .query_row(
                "SELECT integrated,custodian_identity,adopter_identity
                 FROM completion_continuation_attempt WHERE attempt_id=?1",
                [&accepted.attempt.attempt_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|error| error.to_string())?;
        if integrated != 0 || custodian.is_some() || adopter.is_some() {
            return Err("broker native K attempt already has physical custody".into());
        }
        let binding = self
            .mailbox
            .native_grant_binding(&accepted.attempt.attempt_id)?;
        let Some(binding) = binding else {
            return Ok(None);
        };
        let accepted_sha256 = crate::completion_continuation::sha256(
            &serde_json::to_vec(accepted).map_err(|error| error.to_string())?,
        );
        if binding.attempt_id != accepted.attempt.attempt_id
            || binding.grant_id != grant_id
            || binding.protocol != "native-continuation-v1"
            || binding.accepted_revision != 2
            || binding.domain_id != owner.domain_id
            || binding.kernel_root_id != root_id
            || binding.supervisor_authority_id != owner.supervisor_authority_id
            || binding.owner_generation != owner.owner_generation
            || binding.guardian_identity != owner.guardian_identity
            || binding.accepted_snapshot_sha256 != accepted_sha256
            || binding.custodian_request_sha256 != request_sha256
        {
            return Err("broker native K exact binding conflict".into());
        }
        self.check_mailbox_read(source_generation)?;
        Ok(Some(BrokerNativeGrantReadback {
            source_generation: source_generation.into(),
            binding,
        }))
    }

    /// Attach only through this retained broker connection after the server
    /// has authenticated the exact v30 N/t grant and observed a held worker.
    /// An uncertain insert is resolved by exact readback, never by re-forking.
    pub fn attach_exact_native_worker_v30(
        &mut self,
        source_generation: &str,
        attempt_id: &str,
        grant_id: &str,
        root_id: &str,
        evidence: &super::BrokerNativeAttachEvidence,
    ) -> Result<super::NativeWorkerAttach, String> {
        self.check_mailbox_read(source_generation)?;
        let binding = self
            .mailbox
            .native_grant_binding(attempt_id)?
            .ok_or("broker native attach binding absent")?;
        if binding.grant_id != grant_id
            || binding.kernel_root_id != root_id
            || binding.attempt_id != attempt_id
            || evidence.attempt_id != attempt_id
            || evidence.grant_id != grant_id
            || evidence.kernel_root_id != root_id
            || self.mailbox.native_worker_attach(attempt_id)?.is_some()
        {
            return Err("broker native attach exact attempt conflict".into());
        }
        let attached = self
            .mailbox
            .attach_broker_native_worker(&binding, evidence)?;
        self.check_mailbox_read(source_generation)?;
        if self.mailbox.native_worker_attach(attempt_id)?.as_ref() != Some(&attached) {
            return Err("broker native attach readback conflict".into());
        }
        Ok(attached)
    }

    /// Readback is diagnostic while K or Q is uncertain; presence is never
    /// itself a gate-release or physical-drain certificate.
    pub fn read_native_worker_attach_v30(
        &self,
        source_generation: &str,
        attempt_id: &str,
        grant_id: &str,
    ) -> Result<Option<super::NativeWorkerAttach>, String> {
        self.check_mailbox_read(source_generation)?;
        let attached = self.mailbox.native_worker_attach(attempt_id)?;
        if attached
            .as_ref()
            .is_some_and(|row| row.grant_id != grant_id)
        {
            return Err("broker native attach grant changed".into());
        }
        self.check_mailbox_read(source_generation)?;
        Ok(attached)
    }

    /// Commit Q only from broker-observed nested PID1 terminal and parent
    /// wait receipts for the same previously attached work. Exact readback
    /// resolves a lost response without manufacturing a second settlement.
    pub fn settle_exact_native_q_v30(
        &mut self,
        source_generation: &str,
        evidence: &super::BrokerNativeKernelQEvidence,
    ) -> Result<super::NativeKernelQSettlement, String> {
        self.check_mailbox_read(source_generation)?;
        let attached = self
            .mailbox
            .native_worker_attach(&evidence.attempt_id)?
            .ok_or("broker native Q attach absent")?;
        if attached.grant_id != evidence.grant_id
            || attached.evidence.work_incarnation_id != evidence.work_incarnation_id
            || attached.evidence.kernel_root_id != evidence.kernel_root_id
            || attached.evidence.pid1_identity != evidence.pid1_identity
            || attached.evidence.worker_identity != evidence.worker_identity
        {
            return Err("broker native Q attached work changed".into());
        }
        if let Some(existing) = self.mailbox.native_kernel_q(&evidence.attempt_id)? {
            if existing.grant_id != evidence.grant_id
                || existing.evidence.terminal_receipt_sha256 != evidence.terminal_receipt_sha256
                || existing.evidence.pid1_wait_status != evidence.pid1_wait_status
                || existing.evidence.work_incarnation_id != evidence.work_incarnation_id
            {
                return Err("broker native Q replay conflict".into());
            }
            return Ok(existing);
        }
        let settled = self
            .mailbox
            .settle_broker_native_kernel_q(&attached, evidence)?;
        self.check_mailbox_read(source_generation)?;
        if self.mailbox.native_kernel_q(&evidence.attempt_id)?.as_ref() != Some(&settled) {
            return Err("broker native Q readback conflict".into());
        }
        Ok(settled)
    }

    pub fn native_q_settled_v30(
        &self,
        source_generation: &str,
        attempt_id: &str,
        grant_id: &str,
    ) -> Result<bool, String> {
        self.check_mailbox_read(source_generation)?;
        let row = self.mailbox.native_kernel_q(attempt_id)?;
        if row.as_ref().is_some_and(|row| row.grant_id != grant_id) {
            return Err("broker native Q grant conflict".into());
        }
        self.check_mailbox_read(source_generation)?;
        Ok(row.is_some())
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
                AND driver_identity=?5 AND (
                  NOT EXISTS(SELECT 1 FROM broker_prepared_owner WHERE owner_generation=?1)
                  OR EXISTS(SELECT 1 FROM broker_owner_release WHERE owner_generation=?1
                    AND source_generation=?2 AND root_id=?3 AND guardian_identity=?4
                    AND driver_identity=?5)))",
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

#[cfg(unix)]
pub(super) fn require_root_owned_ancestors(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("cutover storage root must be absolute".into());
    }
    let mut current = path;
    loop {
        let meta = fs::symlink_metadata(current).map_err(|error| error.to_string())?;
        if !meta.is_dir()
            || meta.file_type().is_symlink()
            || meta.uid() != 0
            || meta.mode() & 0o022 != 0
        {
            return Err("cutover storage has an untrusted ancestor".into());
        }
        if current == Path::new("/") {
            break;
        }
        current = current.parent().ok_or("cutover storage parent missing")?;
    }
    Ok(())
}

#[cfg(unix)]
#[derive(Clone, Debug, PartialEq, Eq)]
struct SourceArtifact {
    path: std::path::PathBuf,
    identity: Option<(u64, u64, u64, i64, i64)>,
}

#[cfg(unix)]
fn source_artifacts_unchanged(before: &[SourceArtifact], after: &[SourceArtifact]) -> bool {
    before
        .iter()
        .zip(after)
        .enumerate()
        .all(|(index, (old, new))| {
            old == new
            // SQLite may create an empty WAL and SHM when reading a clean
            // WAL-mode database. A nonempty new WAL is always a refusal.
            || (index == 1 && old.identity.is_none() && new.identity.is_some_and(|id| id.2 == 0))
            || (index == 2 && old.identity.is_none() && new.identity.is_some())
        })
}

#[cfg(test)]
thread_local! {
    static AFTER_SNAPSHOT_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = Default::default();
}

#[cfg(test)]
fn after_snapshot_hook() {
    AFTER_SNAPSHOT_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(unix)]
fn source_artifacts(path: &Path, owner: u32) -> Result<Vec<SourceArtifact>, String> {
    if !path.is_absolute()
        || path.file_name() != Some(std::ffi::OsStr::new("pid-identity.db"))
        || path
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
    {
        return Err("cutover requires an exact absolute v29 source name".into());
    }
    let mut result = Vec::new();
    for (index, artifact) in [
        path.to_path_buf(),
        path_with_storage_suffix(path, "-wal"),
        path_with_storage_suffix(path, "-shm"),
        path_with_storage_suffix(path, "-journal"),
    ]
    .into_iter()
    .enumerate()
    {
        let identity = match fs::symlink_metadata(&artifact) {
            Ok(meta) => {
                if !meta.is_file()
                    || meta.file_type().is_symlink()
                    || meta.uid() != owner
                    || meta.nlink() != 1
                {
                    return Err(format!(
                        "untrusted cutover source artifact: {}",
                        artifact.display()
                    ));
                }
                // SQLite may update the shared-memory lock bytes while a
                // reader opens; its inode must remain fixed, while the WAL
                // and main bytes must also retain their size and timestamp.
                if index == 2 {
                    Some((meta.dev(), meta.ino(), 0, 0, 0))
                } else {
                    Some((
                        meta.dev(),
                        meta.ino(),
                        meta.len(),
                        meta.mtime(),
                        meta.mtime_nsec(),
                    ))
                }
            }
            Err(error) if index != 0 && error.kind() == ErrorKind::NotFound => None,
            Err(error) => return Err(format!("cutover source artifact unavailable: {error}")),
        };
        result.push(SourceArtifact {
            path: artifact,
            identity,
        });
    }
    Ok(result)
}

#[cfg(unix)]
fn complete_v29_fingerprint(conn: &Connection) -> Result<[u8; 32], String> {
    schema::validate_exact_v29(conn)?;
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if integrity != "ok" {
        return Err(format!("cutover SQLite integrity failure: {integrity}"));
    }
    let foreign_key_failure: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if foreign_key_failure {
        return Err("cutover source has foreign-key violations".into());
    }
    super::broker_payload_custody::projected_fingerprint(conn, None)
}

#[cfg(unix)]
fn stage_with_owner(
    source: &Path,
    source_owner: u32,
    anchor: &Path,
    storage_owner: u32,
) -> Result<std::path::PathBuf, String> {
    // Check the fixed root storage anchor before creating anything. The
    // staging name is unique and never the broker's live `sidecar` name.
    let anchor_meta = fs::symlink_metadata(anchor).map_err(|error| error.to_string())?;
    if !anchor.is_absolute()
        || !anchor_meta.is_dir()
        || anchor_meta.file_type().is_symlink()
        || anchor_meta.uid() != storage_owner
        || anchor_meta.mode() & 0o077 != 0
    {
        return Err("cutover storage root is not owner-only".into());
    }
    if fs::symlink_metadata(anchor.join("sidecar")).is_ok() {
        return Err("broker sidecar already exists; refusing a second live copy".into());
    }
    let initial = source_artifacts(source, source_owner)?;
    let conn = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("Failed to open v29 cutover source: {error}"))?;
    broker_main_file_must_be_named(&conn)?;
    let before = source_artifacts(source, source_owner)?;
    if before[0] != initial[0] {
        return Err("cutover source main file replaced during open".into());
    }
    let journal: String = conn
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if !journal.eq_ignore_ascii_case("wal") {
        return Err("cutover source must use SQLite WAL mode".into());
    }
    let expected = complete_v29_fingerprint(&conn)?;
    let payload_identities =
        super::broker_payload_custody::capture_source_identities(&conn, source, source_owner)?;
    let stage = anchor.join(format!("sidecar-stage-{}", uuid::Uuid::new_v4()));
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&stage)
        .map_err(|error| error.to_string())?;
    let copy = stage.join("pid-identity.db");
    let result = (|| {
        conn.execute("VACUUM INTO ?1", [copy.to_string_lossy().as_ref()])
            .map_err(|error| format!("Failed to snapshot v29 WAL state: {error}"))?;
        #[cfg(test)]
        after_snapshot_hook();
        broker_main_file_must_be_named(&conn)?;
        let after = source_artifacts(source, source_owner)?;
        if !source_artifacts_unchanged(&before, &after) {
            return Err(format!(
                "cutover source artifact changed during snapshot: {before:?} -> {after:?}"
            ));
        }
        if complete_v29_fingerprint(&conn)? != expected {
            return Err("cutover source rows changed during snapshot".into());
        }
        fs::set_permissions(&copy, fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())?;
        let copied = Connection::open_with_flags(&copy, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| error.to_string())?;
        drop(copied);
        super::broker_payload_custody::stage(
            &conn,
            &copy,
            source,
            &stage,
            &anchor.join("sidecar"),
            source_owner,
            storage_owner,
            &payload_identities,
        )?;
        let state_path = source.with_file_name("state.db");
        if state_path.exists() {
            write_state_source_binding(&stage, &state_path, source_owner, storage_owner)?;
        }
        let copied = Connection::open_with_flags(&copy, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| error.to_string())?;
        let projected = super::broker_payload_custody::projected_fingerprint(
            &conn,
            Some(&anchor.join("sidecar")),
        )?;
        if complete_v29_fingerprint(&copied)? != projected {
            return Err("cutover copy differs beyond explicit payload transformations".into());
        }
        drop(copied);
        if !source_artifacts_unchanged(&before, &source_artifacts(source, source_owner)?)
            || complete_v29_fingerprint(&conn)? != expected
        {
            return Err("cutover source changed during payload custody".into());
        }
        File::open(&copy)
            .and_then(|file| file.sync_all())
            .map_err(|error| error.to_string())?;
        File::open(&stage)
            .and_then(|dir| dir.sync_all())
            .map_err(|error| error.to_string())?;
        File::open(anchor)
            .and_then(|dir| dir.sync_all())
            .map_err(|error| error.to_string())?;
        Ok(stage.clone())
    })();
    if result.is_err() {
        // The live name was never touched. Leave a failed root-only stage for
        // explicit installer inspection; startup ignores it.
        return result;
    }
    result
}

#[cfg(target_os = "linux")]
fn publish_with_owner(
    source: &Path,
    source_owner: u32,
    stage: &Path,
    anchor: &Path,
    storage_owner: u32,
) -> Result<String, String> {
    use std::os::unix::ffi::OsStrExt;
    if stage.parent() != Some(anchor)
        || !stage
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("sidecar-stage-"))
    {
        return Err("cutover stage is not under the fixed broker state root".into());
    }
    let stage_meta = fs::symlink_metadata(stage).map_err(|error| error.to_string())?;
    if !stage_meta.is_dir()
        || stage_meta.file_type().is_symlink()
        || stage_meta.uid() != storage_owner
        || stage_meta.mode() & 0o077 != 0
    {
        return Err("cutover stage directory is not owner-only".into());
    }
    let stage_db = stage.join("pid-identity.db");
    let stage_meta = fs::symlink_metadata(&stage_db).map_err(|error| error.to_string())?;
    if !stage_meta.is_file()
        || stage_meta.file_type().is_symlink()
        || stage_meta.uid() != storage_owner
        || stage_meta.nlink() != 1
        || stage_meta.mode() & 0o077 != 0
    {
        return Err("cutover stage database is not owner-only".into());
    }
    // The stage must contain exactly the single consistent SQLite copy.
    // A stale WAL or another entry could change the read after validation.
    let entries = fs::read_dir(stage).map_err(|error| error.to_string())?;
    for entry in entries {
        if ![
            std::ffi::OsStr::new("pid-identity.db"),
            std::ffi::OsStr::new(MAILBOX_PAYLOAD_DIRECTORY),
            std::ffi::OsStr::new("payload-custody.json"),
        ]
        .contains(
            &entry
                .map_err(|error| error.to_string())?
                .file_name()
                .as_os_str(),
        ) {
            return Err("cutover stage has unexpected artifacts".into());
        }
    }
    let before = source_artifacts(source, source_owner)?;
    let source_conn = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| error.to_string())?;
    broker_main_file_must_be_named(&source_conn)?;
    complete_v29_fingerprint(&source_conn)?;
    let source_hash = super::broker_payload_custody::projected_fingerprint(
        &source_conn,
        Some(&anchor.join("sidecar")),
    )?;
    let copy_conn = Connection::open_with_flags(&stage_db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| error.to_string())?;
    if complete_v29_fingerprint(&copy_conn)? != source_hash {
        return Err("cutover source and staged rows/schema differ".into());
    }
    super::broker_payload_custody::verify(
        &source_conn,
        &stage_db,
        source,
        stage,
        &anchor.join("sidecar"),
        source_owner,
        storage_owner,
    )?;
    drop(copy_conn);
    if !source_artifacts_unchanged(&before, &source_artifacts(source, source_owner)?) {
        return Err("cutover source changed before publication".into());
    }
    broker_main_file_must_be_named(&source_conn)?;
    let target_dir = anchor.join("sidecar");
    let old =
        std::ffi::CString::new(stage.as_os_str().as_bytes()).map_err(|error| error.to_string())?;
    let new = std::ffi::CString::new(target_dir.as_os_str().as_bytes())
        .map_err(|error| error.to_string())?;
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            old.as_ptr(),
            libc::AT_FDCWD,
            new.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result != 0 {
        return Err(format!(
            "cutover publication refused: {}",
            std::io::Error::last_os_error()
        ));
    }
    File::open(anchor)
        .and_then(|dir| dir.sync_all())
        .map_err(|error| error.to_string())?;
    activate_with_owner(&target_dir.join("pid-identity.db"), storage_owner, anchor)
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
    let mut conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|error| format!("Failed to open broker sidecar: {error}"))?;
    authority.validate_opened_target()?;
    configure_writable_sidecar_connection(&conn)?;
    if schema::sidecar_version(&conn)? == 32 {
        schema::validate_broker_owned(&conn)?;
        // Serialize a v32 upgrade on the retained connection. A second opener
        // sees the committed v33 shape and does not repeat the DDL.
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        if schema::sidecar_version(&tx)? == 32 {
            for definition in [
                schema::BROKER_EXACT_SOURCE_PROJECTION_SCHEMA,
                schema::BROKER_EXACT_SOURCE_PROJECTION_IMMUTABLE,
                schema::BROKER_EXACT_SOURCE_PROJECTION_RETAIN,
            ] {
                tx.execute_batch(definition).map_err(|e| e.to_string())?;
            }
            tx.pragma_update(None, "user_version", schema::BROKER_OWNED_VERSION)
                .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
    }
    let generation = schema::validate_broker_owned(&conn)?;
    super::broker_payload_custody::check_manifest_marker(path, owner)?;
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
        state_source: read_state_source_binding(path, owner)?,
    })
}

#[cfg(unix)]
pub(super) fn activate_with_owner(
    path: &Path,
    owner: u32,
    anchor: &Path,
) -> Result<String, String> {
    check_storage(path, owner, anchor)?;
    let authority = MailboxAuthorityFence::acquire_exclusive(path).map_err(|e| e.to_string())?;
    let original = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| error.to_string())?;
    let version = schema::sidecar_version(&original)?;
    match version {
        // Only an independently staged copy may reach this call from the old
        // cutover path. Validate its historical shape before migrating it;
        // the default v29 island is never opened writable here.
        29 => schema::validate_exact_v29(&original)?,
        31 => schema::validate_exact_current(&original)?,
        30 => return Err("persisted broker-owned v30 sidecar requires separate disposition; in-place migration is unsupported".into()),
        _ => return Err(format!("broker activation requires ordinary v29 copy or v31 fresh sidecar, found version {version}")),
    }
    super::broker_payload_custody::verify_activation(&original, path, owner)?;
    drop(original);
    harden_artifacts(path, owner)?;
    let mut mailbox = MailboxDb::open_with_authority(&authority)?;
    schema::validate_exact_current(&mailbox.conn)?;
    super::broker_payload_custody::verify_activation(&mailbox.conn, path, owner)?;
    let generation = uuid::Uuid::new_v4().to_string();
    let tx = mailbox
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| format!("Failed to start broker cutover: {error}"))?;
    tx.execute_batch(schema::BROKER_AUTHORITY_SCHEMA)
        .map_err(|error| format!("Failed to create broker authority: {error}"))?;
    tx.execute_batch(schema::BROKER_OWNER_SCHEMA)
        .map_err(|error| format!("Failed to create broker owner provenance: {error}"))?;
    for definition in [
        schema::BROKER_PREPARED_OWNER_SCHEMA,
        schema::BROKER_PREPARED_OWNER_IMMUTABLE,
        schema::BROKER_PREPARED_OWNER_RETAIN,
        schema::BROKER_OWNER_RELEASE_SCHEMA,
        schema::BROKER_SOURCE_EFFECT_GRANT_SCHEMA,
        schema::BROKER_EXACT_SOURCE_PROJECTION_SCHEMA,
        schema::BROKER_EXACT_SOURCE_PROJECTION_IMMUTABLE,
        schema::BROKER_EXACT_SOURCE_PROJECTION_RETAIN,
        schema::BROKER_SOURCE_EVIDENCE_SCHEMA,
        schema::BROKER_FRESH_SOURCE_ADMISSION_SCHEMA,
        schema::BROKER_SOURCE_EVIDENCE_UPDATE_GUARD,
        schema::BROKER_SOURCE_EVIDENCE_RETAIN,
        schema::BROKER_FRESH_SOURCE_ADMISSION_IMMUTABLE,
        schema::BROKER_FRESH_SOURCE_ADMISSION_RETAIN,
        schema::BROKER_OWNER_RELEASE_IMMUTABLE,
        schema::BROKER_OWNER_RELEASE_RETAIN,
        schema::BROKER_OWNER_RELEASE_EXACT,
        schema::BROKER_PREPARED_OWNER_NO_RUNNING,
    ] {
        tx.execute_batch(definition)
            .map_err(|error| format!("Failed to create broker prepared owner: {error}"))?;
    }
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

    // Construct an exact historical source from the current fixture builder.
    // The production cutover only reads this file and migrates its staged copy.
    #[cfg(unix)]
    fn pin_v29_fixture(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "DROP TRIGGER completion_uncertain_input_preserve;
             DROP TABLE completion_uncertain_input;
             DROP INDEX idx_mailbox_deliverable_session_live;
             DROP INDEX idx_mailbox_deliverable_target_live;
             DROP INDEX idx_mailbox_deliverable_global;
             PRAGMA user_version=29;",
        )
        .unwrap();
        conn.execute_batch(include_str!("migrations/0022_live_history_barrier.sql"))
            .unwrap();
        schema::validate_exact_v29(&conn).unwrap();
    }

    #[cfg(target_os = "linux")]
    fn payload_cutover_fixture() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        std::path::PathBuf,
        MailboxRow,
        MailboxRow,
    ) {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("old");
        let broker_root = root.path().join("broker");
        fs::create_dir(&old).unwrap();
        fs::create_dir(&broker_root).unwrap();
        fs::set_permissions(&broker_root, fs::Permissions::from_mode(0o700)).unwrap();
        let source = old.join("pid-identity.db");
        let mut db = MailboxDb::open(&source).unwrap();
        let mail = match db
            .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
                session_id: "recipient",
                handle: "completion-one",
                payload_json: r#"{"protocol":"source-retention-release-v1","body":"held"}"#,
                owner_invocation_uuid: Some("owner-invocation"),
                matched_os_pid: None,
                matched_os_boot_id: None,
                matched_os_pid_starttime_ticks: None,
                matched_chain_index: None,
                state_dir: "state",
                meta_path: "meta",
                log_path: "log",
                rc_path: "rc",
                rc: 0,
            })
            .unwrap()
        {
            EnqueueResult::Inserted(row) => row,
            other => panic!("{other:?}"),
        };
        let input = match db
            .enqueue_submitted_input(&SubmittedInputEnqueue {
                submission_token: "submission-one",
                target: InboxTarget {
                    kind: InboxTargetKind::Session,
                    id: "recipient",
                },
                input: b"new input bytes",
            })
            .unwrap()
        {
            EnqueueResult::Inserted(row) => row,
            other => panic!("{other:?}"),
        };
        db.conn
            .execute(
                "INSERT INTO completion_event(event_id,kind,state,delivery_mode,
            state_dir,meta_path,log_path,rc_path,rc,payload_json,payload_file_path,
            payload_sha256,payload_byte_len,payload_retention_policy,created_at,triggered_at)
            VALUES('completion-one',?1,'triggered','async','state','meta','log','rc',0,
            ?2,?3,?4,?5,?6,?7,?7)",
                params![
                    AGENT_BASH_COMPLETE_KIND,
                    mail.payload_json,
                    mail.payload_file_path,
                    mail.payload_sha256,
                    mail.payload_byte_len,
                    mail.payload_retention_policy,
                    now_rfc3339()
                ],
            )
            .unwrap();
        db.conn
            .execute_batch(
                "PRAGMA wal_autocheckpoint=0;
            CREATE TABLE custody_probe(value TEXT NOT NULL);
            INSERT INTO custody_probe VALUES('committed WAL fact');",
            )
            .unwrap();
        drop(db);
        pin_v29_fixture(&source);
        (root, source, broker_root, mail, input)
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn offline_payload_custody_publishes_files_with_full_row_continuity() {
        let (root, source, broker_root, mail, input) = payload_cutover_fixture();
        let uid = unsafe { libc::geteuid() };
        let db_only_root = root.path().join("db-only");
        let db_only = db_only_root.join("sidecar");
        fs::create_dir(&db_only_root).unwrap();
        fs::create_dir(&db_only).unwrap();
        fs::set_permissions(&db_only_root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&db_only, fs::Permissions::from_mode(0o700)).unwrap();
        let unsafe_db = db_only.join("pid-identity.db");
        Connection::open(&source)
            .unwrap()
            .execute("VACUUM INTO ?1", [unsafe_db.to_str().unwrap()])
            .unwrap();
        fs::set_permissions(&unsafe_db, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(
            activate_with_owner(&unsafe_db, uid, &db_only_root)
                .unwrap_err()
                .contains("payload")
        );
        let stage = stage_with_owner(&source, uid, &broker_root, uid).unwrap();
        assert!(!broker_root.join("sidecar").exists());
        let stage_db = Connection::open_with_flags(
            stage.join("pid-identity.db"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let value: String = stage_db
            .query_row("SELECT value FROM custody_probe", [], |r| r.get(0))
            .unwrap();
        assert_eq!(value, "committed WAL fact");
        drop(stage_db);
        let generation = publish_with_owner(&source, uid, &stage, &broker_root, uid).unwrap();
        let target = broker_root.join("sidecar/pid-identity.db");
        let mut broker = open_with_owner(&target, uid, &broker_root).unwrap();
        let row = broker
            .read_exact_mailbox_row(&generation, "recipient", mail.seq)
            .unwrap()
            .unwrap()
            .row;
        assert_eq!(row.handle, mail.handle);
        assert_eq!(row.owner_invocation_uuid, mail.owner_invocation_uuid);
        assert_eq!(row.payload_sha256, mail.payload_sha256);
        assert_eq!(row.payload_byte_len, mail.payload_byte_len);
        assert!(row.delivered_at.is_none());
        assert!(
            row.payload_file_path
                .as_ref()
                .unwrap()
                .starts_with(broker_root.join("sidecar").to_str().unwrap())
        );
        broker
            .mailbox
            .payloads()
            .verify_mailbox_row_payload(&row)
            .unwrap();
        let event = broker
            .mailbox
            .completion_event("completion-one")
            .unwrap()
            .unwrap();
        assert_eq!(event.payload_file_path, row.payload_file_path);
        let input_row = broker
            .read_exact_mailbox_row(&generation, "recipient", input.seq)
            .unwrap()
            .unwrap()
            .row;
        broker
            .mailbox
            .payloads()
            .verify_mailbox_row_payload(&input_row)
            .unwrap();
        assert_eq!(input_row.handle, input.handle);
        assert_eq!(input_row.payload_sha256, input.payload_sha256);
        assert_eq!(
            input_row.state_dir,
            Path::new(input_row.payload_file_path.as_ref().unwrap())
                .parent()
                .unwrap()
                .to_str()
                .unwrap()
        );
        let bytes = fs::read(row.payload_file_path.as_ref().unwrap()).unwrap();
        let new_mail = AgentBashCompleteEnqueue {
            session_id: "recipient",
            handle: "new-broker-mail",
            payload_json: "ignored",
            owner_invocation_uuid: Some("owner-invocation"),
            matched_os_pid: None,
            matched_os_boot_id: None,
            matched_os_pid_starttime_ticks: None,
            matched_chain_index: None,
            state_dir: "state",
            meta_path: "meta",
            log_path: "log",
            rc_path: "rc",
            rc: 0,
        };
        assert!(
            broker
                .enqueue_notification_bytes("wrong", new_mail, b"{}")
                .is_err()
        );
        assert!(
            broker
                .enqueue_notification_bytes(&generation, new_mail, b"{}")
                .is_err()
        );
        let new_bytes = br#"{"protocol":"source-retention-release-v1","handle":"new-broker-mail"}"#;
        let new_row = match broker
            .enqueue_notification_bytes(&generation, new_mail, new_bytes)
            .unwrap()
        {
            EnqueueResult::Inserted(row) => row,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            fs::read(new_row.payload_file_path.as_ref().unwrap()).unwrap(),
            new_bytes
        );
        assert!(
            new_row
                .payload_file_path
                .as_ref()
                .unwrap()
                .starts_with(broker_root.join("sidecar").to_str().unwrap())
        );
        assert!(matches!(
            broker
                .enqueue_notification_bytes(&generation, new_mail, new_bytes)
                .unwrap(),
            EnqueueResult::AlreadyEnqueued(_)
        ));
        drop(broker);
        let old_file = mail.payload_file_path.as_ref().unwrap();
        fs::set_permissions(old_file, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(old_file, b"retired source mutation").unwrap();
        let restarted = open_with_owner(&target, uid, &broker_root).unwrap();
        let resumed = restarted
            .read_exact_mailbox_row(&generation, "recipient", mail.seq)
            .unwrap()
            .unwrap()
            .row;
        assert_eq!(fs::read(resumed.payload_file_path.unwrap()).unwrap(), bytes);
        assert!(stage_with_owner(&source, uid, &broker_root, uid).is_err());
        assert_eq!(
            fs::read_dir(broker_root.join("sidecar/inbox-payloads/v1/sha256"))
                .unwrap()
                .flat_map(|r| fs::read_dir(r.unwrap().path()).unwrap())
                .filter(|r| r
                    .as_ref()
                    .is_ok_and(|entry| entry.file_name().to_string_lossy().len() == 64))
                .count(),
            3
        );
        drop(root);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn offline_payload_custody_rejects_missing_modified_symlink_hardlink_and_copy_swap() {
        use std::os::unix::fs::symlink;
        for attack in ["missing", "modified", "symlink", "hardlink", "copy-swap"] {
            let (_root, source, broker_root, mail, _input) = payload_cutover_fixture();
            let path = PathBuf::from(mail.payload_file_path.unwrap());
            match attack {
                "missing" => fs::remove_file(&path).unwrap(),
                "modified" => {
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
                    fs::write(&path, b"modified payload").unwrap();
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
                }
                "symlink" => {
                    let old = path.with_extension("original");
                    fs::rename(&path, &old).unwrap();
                    symlink(&old, &path).unwrap();
                }
                "hardlink" => {
                    fs::hard_link(&path, path.with_extension("second-link")).unwrap();
                }
                "copy-swap" => {
                    AFTER_SNAPSHOT_HOOK.with(|slot| {
                        *slot.borrow_mut() = Some(Box::new(move || {
                            let old = path.with_extension("original");
                            fs::rename(&path, &old).unwrap();
                            fs::copy(&old, &path).unwrap();
                            fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
                        }));
                    });
                }
                _ => unreachable!(),
            }
            let uid = unsafe { libc::geteuid() };
            assert!(
                stage_with_owner(&source, uid, &broker_root, uid).is_err(),
                "{attack}"
            );
            assert!(!broker_root.join("sidecar").exists(), "{attack}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn offline_payload_custody_checks_exact_digest_length_handle_and_protocol() {
        for attack in [
            "same-length-digest",
            "short-length",
            "bad-handle",
            "bad-protocol",
        ] {
            let (_root, source, broker_root, mail, _input) = payload_cutover_fixture();
            let path = PathBuf::from(mail.payload_file_path.unwrap());
            match attack {
                "same-length-digest" => {
                    let mut bytes = fs::read(&path).unwrap();
                    let last = bytes.len() - 2;
                    bytes[last] = if bytes[last] == b'X' { b'Y' } else { b'X' };
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
                    fs::write(&path, bytes).unwrap();
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
                }
                "short-length" => {
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
                    fs::write(&path, b"short").unwrap();
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
                }
                "bad-handle" | "bad-protocol" => {
                    let mut db = MailboxDb::open(&source).unwrap();
                    let payload = if attack == "bad-handle" {
                        r#"{"protocol":"source-retention-release-v1","handle":"someone-else"}"#
                    } else {
                        r#"{"handle":"bad-protocol"}"#
                    };
                    let handle = if attack == "bad-handle" {
                        "bad-handle"
                    } else {
                        "bad-protocol"
                    };
                    db.enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
                        session_id: "recipient",
                        handle,
                        payload_json: payload,
                        owner_invocation_uuid: None,
                        matched_os_pid: None,
                        matched_os_boot_id: None,
                        matched_os_pid_starttime_ticks: None,
                        matched_chain_index: None,
                        state_dir: "state",
                        meta_path: "meta",
                        log_path: "log",
                        rc_path: "rc",
                        rc: 0,
                    })
                    .unwrap();
                }
                _ => unreachable!(),
            }
            let uid = unsafe { libc::geteuid() };
            assert!(
                stage_with_owner(&source, uid, &broker_root, uid).is_err(),
                "{attack}"
            );
            assert!(!broker_root.join("sidecar").exists(), "{attack}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn offline_payload_custody_detects_swap_during_copy_and_stale_stage() {
        let (_root, source, broker_root, mail, _input) = payload_cutover_fixture();
        let path = PathBuf::from(mail.payload_file_path.unwrap());
        crate::mailbox::broker_payload_custody::set_after_payload_open_hook(move || {
            let old = path.with_extension("original");
            fs::rename(&path, &old).unwrap();
            fs::copy(&old, &path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
        });
        let uid = unsafe { libc::geteuid() };
        assert!(stage_with_owner(&source, uid, &broker_root, uid).is_err());
        assert!(!broker_root.join("sidecar").exists());

        let (_root, source, broker_root, mail, _input) = payload_cutover_fixture();
        let stage = stage_with_owner(&source, uid, &broker_root, uid).unwrap();
        let sha = mail.payload_sha256.as_ref().unwrap();
        let staged = stage
            .join("inbox-payloads/v1/sha256")
            .join(&sha[..2])
            .join(sha);
        let old = staged.with_extension("original");
        fs::rename(&staged, &old).unwrap();
        fs::copy(&old, &staged).unwrap();
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o400)).unwrap();
        fs::remove_file(old).unwrap();
        assert!(publish_with_owner(&source, uid, &stage, &broker_root, uid).is_err());
        assert!(!broker_root.join("sidecar").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn offline_payload_custody_refuses_changed_source_before_publish_and_recovers_after_rename() {
        let (_root, source, broker_root, mail, _input) = payload_cutover_fixture();
        let uid = unsafe { libc::geteuid() };
        let stage = stage_with_owner(&source, uid, &broker_root, uid).unwrap();
        let old_file = mail.payload_file_path.as_ref().unwrap();
        fs::set_permissions(old_file, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(old_file, b"changed source").unwrap();
        assert!(publish_with_owner(&source, uid, &stage, &broker_root, uid).is_err());
        assert!(!broker_root.join("sidecar").exists());

        let (_root, source, broker_root, mail, _input) = payload_cutover_fixture();
        let stage = stage_with_owner(&source, uid, &broker_root, uid).unwrap();
        let target_dir = broker_root.join("sidecar");
        fs::rename(&stage, &target_dir).unwrap();
        File::open(&broker_root).unwrap().sync_all().unwrap();
        let target = target_dir.join("pid-identity.db");
        assert!(open_with_owner(&target, uid, &broker_root).is_err());
        fs::set_permissions(
            mailbox_authority_path(&target),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        let generation = activate_with_owner(&target, uid, &broker_root).unwrap();
        let broker = open_with_owner(&target, uid, &broker_root).unwrap();
        let row = broker
            .read_exact_mailbox_row(&generation, "recipient", mail.seq)
            .unwrap()
            .unwrap()
            .row;
        broker
            .mailbox
            .payloads()
            .verify_mailbox_row_payload(&row)
            .unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mailbox_readback_is_exact_bounded_and_uses_retained_v30_connection() {
        let root = tempfile::tempdir().unwrap();
        let old_dir = root.path().join("old");
        fs::create_dir(&old_dir).unwrap();
        let source = old_dir.join("pid-identity.db");
        let mut old = MailboxDb::open(&source).unwrap();
        let enqueue = |db: &mut MailboxDb, session_id: &str, handle: &str| {
            let input = AgentBashCompleteEnqueue {
                session_id,
                handle,
                payload_json: r#"{"protocol":"source-retention-release-v1","body":"unread"}"#,
                owner_invocation_uuid: Some("source-invocation"),
                matched_os_pid: None,
                matched_os_boot_id: None,
                matched_os_pid_starttime_ticks: None,
                matched_chain_index: None,
                state_dir: "state",
                meta_path: "meta",
                log_path: "log",
                rc_path: "rc",
                rc: 0,
            };
            match db.enqueue_agent_bash_complete(&input).unwrap() {
                EnqueueResult::Inserted(row) => row,
                other => panic!("unexpected enqueue: {other:?}"),
            }
        };
        let first = enqueue(&mut old, "recipient", "first");
        let sibling = enqueue(&mut old, "sibling", "foreign");
        let second = enqueue(&mut old, "recipient", "second");
        drop(old);
        pin_v29_fixture(&source);

        let broker_root = root.path().join("broker");
        fs::create_dir(&broker_root).unwrap();
        fs::set_permissions(&broker_root, fs::Permissions::from_mode(0o700)).unwrap();
        let uid = unsafe { libc::geteuid() };
        let stage = stage_with_owner(&source, uid, &broker_root, uid).unwrap();
        let generation = publish_with_owner(&source, uid, &stage, &broker_root, uid).unwrap();
        let directory = broker_root.join("sidecar");
        let target = directory.join("pid-identity.db");
        let broker = open_with_owner(&target, uid, &broker_root).unwrap();

        assert!(
            broker
                .read_exact_mailbox_row("wrong", "recipient", first.seq)
                .is_err()
        );
        assert!(
            broker
                .read_exact_mailbox_row(&generation, "recipient", sibling.seq)
                .unwrap()
                .is_none()
        );
        let read = broker
            .read_exact_mailbox_row(&generation, "recipient", first.seq)
            .unwrap()
            .unwrap();
        assert_eq!(read.source_generation, generation);
        assert_eq!(read.row.handle, "first");
        assert_eq!(
            read.row.owner_invocation_uuid.as_deref(),
            Some("source-invocation")
        );
        assert_eq!(read.row.payload_sha256, first.payload_sha256);
        assert_eq!(read.row.payload_byte_len, first.payload_byte_len);
        assert!(read.row.delivered_at.is_none());
        broker
            .mailbox
            .payloads()
            .verify_mailbox_row_payload(&read.row)
            .unwrap();
        let page = broker
            .read_pending_mailbox_page(&generation, "recipient", 0, 1)
            .unwrap();
        assert_eq!(
            page.iter().map(|item| item.row.seq).collect::<Vec<_>>(),
            vec![first.seq]
        );
        let page = broker
            .read_pending_mailbox_page(&generation, "recipient", first.seq, 1)
            .unwrap();
        assert_eq!(
            page.iter().map(|item| item.row.seq).collect::<Vec<_>>(),
            vec![second.seq]
        );
        assert!(
            broker
                .read_pending_mailbox_page(&generation, "recipient", 0, 65)
                .is_err()
        );

        drop(broker);
        let mut retired = MailboxDb::open(&source).unwrap();
        retired
            .acknowledge_range("recipient", first.seq, first.seq, "retired")
            .unwrap();
        drop(retired);
        let broker = open_with_owner(&target, uid, &broker_root).unwrap();
        assert!(
            broker
                .read_exact_mailbox_row(&generation, "recipient", first.seq)
                .unwrap()
                .unwrap()
                .row
                .delivered_at
                .is_none()
        );
        fs::rename(&target, directory.join("moved.db")).unwrap();
        assert!(
            broker
                .read_exact_mailbox_row(&generation, "recipient", first.seq)
                .is_err()
        );
        drop(broker);
        assert!(open_with_owner(&target, uid, &broker_root).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn recipient_selection_uses_retained_pending_payload_without_ack() {
        let (_root, source, broker_root, mail, _input) = payload_cutover_fixture();
        let uid = unsafe { libc::geteuid() };
        let stage = stage_with_owner(&source, uid, &broker_root, uid).unwrap();
        let generation = publish_with_owner(&source, uid, &stage, &broker_root, uid).unwrap();
        let target = broker_root.join("sidecar/pid-identity.db");
        let state_path = source.with_file_name("state.db");
        drop(StateDb::open(&state_path).unwrap());
        write_state_source_binding(&broker_root.join("sidecar"), &state_path, uid, uid).unwrap();
        let mut broker = open_with_owner(&target, uid, &broker_root).unwrap();
        let owner = CompletionDomainOwner {
            protocol: PROTOCOL.into(),
            domain_id: broker.domain_id().unwrap(),
            supervisor_authority_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: uuid::Uuid::new_v4().to_string(),
            guardian_identity: SourceProcessIdentity {
                pid: 1,
                boot_id: uuid::Uuid::new_v4().to_string(),
                starttime_ticks: 1,
            },
            driver_identity: SourceProcessIdentity {
                pid: 2,
                boot_id: uuid::Uuid::new_v4().to_string(),
                starttime_ticks: 2,
            },
            endpoint: "fixture".into(),
        };
        let root_id = uuid::Uuid::new_v4().to_string();
        assert!(
            broker
                .read_bounded_recipient_selection("wrong", &root_id, &owner)
                .is_err()
        );
        let first = broker
            .read_bounded_recipient_selection(&generation, &root_id, &owner)
            .unwrap();
        let candidate = first.candidate.unwrap();
        assert_eq!(candidate.session_id, "recipient");
        assert_eq!(candidate.seq, mail.seq);
        assert_eq!(candidate.payload_sha256, mail.payload_sha256.unwrap());
        assert_eq!(candidate.payload_byte_len, mail.payload_byte_len.unwrap());
        assert_eq!(
            broker
                .read_exact_mailbox_row(&generation, "recipient", mail.seq)
                .unwrap()
                .unwrap()
                .row
                .delivered_at,
            None
        );
        drop(broker);
        // The retired user-owned copy cannot change this broker selection.
        let mut retired = MailboxDb::open(&source).unwrap();
        retired
            .acknowledge_range("recipient", mail.seq, mail.seq, "retired")
            .unwrap();
        drop(retired);
        let mut broker = open_with_owner(&target, uid, &broker_root).unwrap();
        assert_eq!(
            broker
                .read_bounded_recipient_selection(&generation, &root_id, &owner)
                .unwrap()
                .candidate
                .unwrap()
                .seq,
            mail.seq
        );
        let retained_path = broker
            .read_exact_mailbox_row(&generation, "recipient", mail.seq)
            .unwrap()
            .unwrap()
            .row
            .payload_file_path
            .unwrap();
        let corrupted = vec![b'x'; fs::metadata(&retained_path).unwrap().len() as usize];
        fs::set_permissions(&retained_path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&retained_path, corrupted).unwrap();
        fs::set_permissions(&retained_path, fs::Permissions::from_mode(0o400)).unwrap();
        assert!(
            broker
                .read_bounded_recipient_selection(&generation, &root_id, &owner)
                .unwrap_err()
                .contains("integrity mismatch")
        );
    }

    #[cfg(unix)]
    #[test]
    fn source_effect_obligations_retain_spent_gap_restart_wal_and_orphans() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
            .unwrap();
        conn.execute_batch("CREATE TABLE broker_prepared_owner(root_id TEXT PRIMARY KEY,owner_generation TEXT NOT NULL,source_generation TEXT NOT NULL);")
            .unwrap();
        conn.execute_batch(schema::BROKER_SOURCE_EFFECT_GRANT_SCHEMA)
            .unwrap();
        conn.execute_batch(schema::BROKER_SOURCE_EVIDENCE_SCHEMA)
            .unwrap();
        let root = uuid::Uuid::new_v4().to_string();
        let other = uuid::Uuid::new_v4().to_string();
        let generation = uuid::Uuid::new_v4().to_string();
        let owner = uuid::Uuid::new_v4().to_string();
        for id in [&root, &other] {
            conn.execute("INSERT INTO broker_prepared_owner(root_id,owner_generation,source_generation) VALUES(?1,?2,?3)", params![id,owner,generation]).unwrap();
        }
        let insert = |root_id: &str, phase: &str| {
            let grant = uuid::Uuid::new_v4().to_string();
            let registration = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO broker_source_effect_grant
                 (grant_id,source_generation,root_id,owner_generation,driver_identity,
                  authority_ordinal,registration_id,registration_digest,registration_bytes,
                  listener_revision,listener_json,phase,revision)
                 VALUES(?1,?2,?3,?4,'{}',1,?5,'digest',X'01',1,'{}',?6,1)",
                params![grant, generation, root_id, owner, registration, phase],
            )
            .unwrap();
            (grant, registration)
        };
        let (_reserved, _) = insert(&root, "reserved");
        let (spent, registration) = insert(&root, "consumed");
        let _sibling = insert(&other, "consumed");
        let before = source_effect_obligations_on(&conn, &generation, &root, &owner).unwrap();
        assert_eq!(before.reserved, 1);
        assert_eq!(before.consumed_without_evidence, 1);
        assert_eq!(before.unsettled(), 2);
        assert!(source_effect_obligations_on(&conn, "stale", &root, &owner).is_err());
        assert_eq!(
            source_effect_obligations_on(&conn, &generation, &other, &owner)
                .unwrap()
                .consumed_without_evidence,
            1
        );
        let wal_reader = Connection::open(&path).unwrap();
        assert_eq!(
            source_effect_obligations_on(&wal_reader, &generation, &root, &owner)
                .unwrap()
                .consumed_without_evidence,
            1
        );
        drop(wal_reader);
        drop(conn);
        let conn = Connection::open(&path).unwrap();
        let after = source_effect_obligations_on(&conn, &generation, &root, &owner).unwrap();
        assert_eq!(after.consumed_without_evidence, 1);
        conn.execute(
            "INSERT INTO broker_source_evidence
             (grant_id,source_generation,registration_id,seal_json,phase,revision)
             VALUES(?1,?2,?3,NULL,'unknown',1)",
            params![spent, generation, registration],
        )
        .unwrap();
        assert_eq!(
            source_effect_obligations_on(&conn, &generation, &root, &owner)
                .unwrap()
                .unknown,
            1
        );
        conn.execute(
            "UPDATE broker_source_evidence SET source_generation='stale' WHERE grant_id=?1",
            [&spent],
        )
        .unwrap();
        assert!(source_effect_obligations_on(&conn, &generation, &root, &owner).is_err());
        conn.execute(
            "UPDATE broker_source_evidence SET source_generation=?2 WHERE grant_id=?1",
            params![spent, generation],
        )
        .unwrap();
        let orphan_grant = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO broker_source_effect_grant
             (grant_id,source_generation,root_id,owner_generation,driver_identity,
              authority_ordinal,registration_id,registration_digest,registration_bytes,
              listener_revision,listener_json,phase,revision)
             VALUES(?1,?2,?3,?4,'{}',1,?5,'digest',X'01',1,'{}','consumed',2)",
            params![
                orphan_grant,
                generation,
                uuid::Uuid::new_v4().to_string(),
                owner,
                uuid::Uuid::new_v4().to_string()
            ],
        )
        .unwrap();
        assert!(source_effect_obligations_on(&conn, &generation, &root, &owner).is_err());
        conn.execute(
            "DELETE FROM broker_source_effect_grant WHERE grant_id=?1",
            [&orphan_grant],
        )
        .unwrap();
        conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        conn.execute(
            "INSERT INTO broker_source_evidence
             (grant_id,source_generation,registration_id,seal_json,phase,revision)
             VALUES(?1,?2,?3,NULL,'unknown',1)",
            params![
                uuid::Uuid::new_v4().to_string(),
                generation,
                uuid::Uuid::new_v4().to_string()
            ],
        )
        .unwrap();
        assert!(source_effect_obligations_on(&conn, &generation, &root, &owner).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn prepared_owner_requires_atomic_exact_release_before_running() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("sidecar");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.join("pid-identity.db");
        let mut db = MailboxDb::open(&path).unwrap();
        let domain = db.completion_continuation_domain().unwrap().unwrap();
        let live = crate::pid_identity::read_current_process_identity().unwrap();
        let old = super::super::CompletionDomainOwner {
            protocol: PROTOCOL.into(),
            domain_id: domain.clone(),
            supervisor_authority_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: uuid::Uuid::new_v4().to_string(),
            guardian_identity: SourceProcessIdentity {
                pid: live.os_pid,
                boot_id: live.os_boot_id.clone(),
                starttime_ticks: live.os_pid_starttime_ticks,
            },
            driver_identity: SourceProcessIdentity {
                pid: live.os_pid,
                boot_id: live.os_boot_id,
                starttime_ticks: live.os_pid_starttime_ticks,
            },
            endpoint: "/old/copied-owner".into(),
        };
        db.publish_completion_owner_with_kernel_root(&old, Some(&uuid::Uuid::new_v4().to_string()))
            .unwrap();
        drop(db);
        for artifact in [path.clone(), mailbox_authority_path(&path)] {
            fs::set_permissions(artifact, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let uid = unsafe { libc::geteuid() };
        let source_generation = activate_with_owner(&path, uid, root.path()).unwrap();
        let mut broker = open_with_owner(&path, uid, root.path()).unwrap();
        let boot_id = uuid::Uuid::new_v4().to_string();
        let stamp = |host_pid, pidns_ino| PreparedProcessStamp {
            host_pid,
            boot_id: boot_id.clone(),
            starttime_ticks: host_pid as u64 + 100,
            pidns_dev: 1,
            pidns_ino,
        };
        let prepared = PreparedBrokerOwner {
            source_generation: source_generation.clone(),
            root_id: uuid::Uuid::new_v4().to_string(),
            owner_uid: uid,
            domain_id: domain,
            supervisor_authority_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: uuid::Uuid::new_v4().to_string(),
            endpoint: "/prepared/owner".into(),
            entry: stamp(101, 1),
            guardian: stamp(102, 1),
            driver: stamp(103, 1),
            root_init: stamp(104, 2),
            joined_child: stamp(105, 2),
        };
        let mut copied = prepared.clone();
        copied.owner_generation = old.owner_generation.clone();
        assert!(broker.prepare_exact_owner(&copied).is_err());
        let mut stale = prepared.clone();
        stale.source_generation = uuid::Uuid::new_v4().to_string();
        assert!(broker.prepare_exact_owner(&stale).is_err());
        let mut sibling = prepared.clone();
        sibling.joined_child.pidns_ino = sibling.entry.pidns_ino;
        assert!(broker.prepare_exact_owner(&sibling).is_err());
        let mut reused_pid = prepared.clone();
        reused_pid.driver.host_pid = reused_pid.guardian.host_pid;
        assert!(broker.prepare_exact_owner(&reused_pid).is_err());
        assert_eq!(broker.prepare_exact_owner(&prepared).unwrap(), prepared);
        assert_eq!(
            broker
                .read_root_source_effect_obligations(&prepared.root_id, &prepared.root_init)
                .unwrap(),
            BrokerSourceEffectObligations::default()
        );
        let mut stale_init = prepared.root_init.clone();
        stale_init.starttime_ticks += 1;
        assert!(
            broker
                .read_root_source_effect_obligations(&prepared.root_id, &stale_init)
                .is_err()
        );
        assert!(
            broker
                .read_root_source_effect_obligations(
                    &uuid::Uuid::new_v4().to_string(),
                    &prepared.root_init
                )
                .is_err()
        );
        assert!(broker.prepare_exact_owner(&prepared).is_err());
        let mut same_root = prepared.clone();
        same_root.owner_generation = uuid::Uuid::new_v4().to_string();
        assert!(broker.prepare_exact_owner(&same_root).is_err());
        let mut premature_running = old.clone();
        premature_running.owner_generation = prepared.owner_generation.clone();
        premature_running.supervisor_authority_id = prepared.supervisor_authority_id.clone();
        assert!(
            broker
                .mailbox_mut()
                .publish_completion_owner_with_kernel_root(
                    &premature_running,
                    Some(&prepared.root_id)
                )
                .is_err()
        );
        // A direct v18 publisher cannot use a release row without the exact
        // broker provenance marker in the same transaction.
        let direct_owner = super::super::CompletionDomainOwner {
            protocol: PROTOCOL.into(),
            domain_id: prepared.domain_id.clone(),
            supervisor_authority_id: prepared.supervisor_authority_id.clone(),
            owner_generation: prepared.owner_generation.clone(),
            guardian_identity: SourceProcessIdentity {
                pid: i64::from(prepared.guardian.host_pid),
                boot_id: prepared.guardian.boot_id.clone(),
                starttime_ticks: prepared.guardian.starttime_ticks as i64,
            },
            driver_identity: SourceProcessIdentity {
                pid: i64::from(prepared.driver.host_pid),
                boot_id: prepared.driver.boot_id.clone(),
                starttime_ticks: prepared.driver.starttime_ticks as i64,
            },
            endpoint: prepared.endpoint.clone(),
        };
        {
            let tx = broker
                .mailbox
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            tx.execute(
                "INSERT INTO broker_owner_release(owner_generation,source_generation,root_id,
                 release_id,guardian_identity,driver_identity,committed_at)
                 VALUES(?1,?2,?3,?4,?5,?6,'fixture')",
                params![
                    prepared.owner_generation,
                    prepared.source_generation,
                    prepared.root_id,
                    uuid::Uuid::new_v4().to_string(),
                    serde_json::to_string(&direct_owner.guardian_identity).unwrap(),
                    serde_json::to_string(&direct_owner.driver_identity).unwrap(),
                ],
            )
            .unwrap();
            assert!(MailboxDb::publish_completion_owner_on(
                &tx,
                &direct_owner,
                Some(&prepared.root_id),
            )
            .is_err());
        }
        assert!(
            broker
                .read_exact_prepared_owner("stale", &prepared.root_id, &prepared.owner_generation)
                .is_err()
        );
        assert!(
            broker
                .read_exact_prepared_owner(
                    &source_generation,
                    &uuid::Uuid::new_v4().to_string(),
                    &prepared.owner_generation,
                )
                .is_err()
        );
        assert!(
            broker
                .read_exact_continuation(
                    &source_generation,
                    &prepared.root_id,
                    &prepared.domain_id,
                    &prepared.supervisor_authority_id,
                    &prepared.owner_generation,
                    None,
                )
                .is_err()
        );
        assert_eq!(
            broker
                .mailbox
                .conn
                .query_row(
                    "SELECT count(*) FROM completion_continuation_owner WHERE generation=?1",
                    [&prepared.owner_generation],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0,
        );
        broker
            .mailbox
            .conn
            .execute_batch(
                "CREATE TRIGGER broker_release_fail BEFORE INSERT ON broker_completion_owner
                BEGIN SELECT RAISE(ABORT,'injected marker failure'); END",
            )
            .unwrap();
        assert!(broker.commit_exact_prepared_release(&prepared).is_err());
        for (table, key) in [
            ("completion_continuation_owner", "generation"),
            ("broker_completion_owner", "owner_generation"),
            ("broker_owner_release", "owner_generation"),
        ] {
            let count: i64 = broker
                .mailbox
                .conn
                .query_row(
                    &format!("SELECT count(*) FROM {table} WHERE {key}=?1"),
                    [&prepared.owner_generation],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 0, "partial release in {table}");
        }
        assert!(
            broker
                .read_exact_release(
                    &source_generation,
                    &prepared.root_id,
                    &prepared.owner_generation
                )
                .is_err()
        );
        broker
            .mailbox
            .conn
            .execute_batch("DROP TRIGGER broker_release_fail")
            .unwrap();
        broker
            .mailbox
            .conn
            .execute_batch(
                "CREATE TRIGGER broker_running_fail BEFORE INSERT ON completion_continuation_owner
                BEGIN SELECT RAISE(ABORT,'injected running failure'); END",
            )
            .unwrap();
        assert!(broker.commit_exact_prepared_release(&prepared).is_err());
        assert!(
            broker
                .read_exact_release(
                    &source_generation,
                    &prepared.root_id,
                    &prepared.owner_generation,
                )
                .is_err()
        );
        broker
            .mailbox
            .conn
            .execute_batch("DROP TRIGGER broker_running_fail")
            .unwrap();
        let mut altered = prepared.clone();
        altered.driver.starttime_ticks += 1;
        assert!(broker.commit_exact_prepared_release(&altered).is_err());
        let released = broker.commit_exact_prepared_release(&prepared).unwrap();
        assert_eq!(released.prepared, prepared);
        assert_eq!(released.owner.owner_generation, prepared.owner_generation);
        assert!(broker.commit_exact_prepared_release(&prepared).is_err());
        broker
            .mailbox
            .conn
            .execute(
                "INSERT INTO broker_source_effect_grant
             (grant_id,source_generation,root_id,owner_generation,driver_identity,
              authority_ordinal,registration_id,registration_digest,registration_bytes,
              listener_revision,listener_json,phase,revision)
             VALUES(?1,?2,?3,?4,'{}',1,?5,'digest',X'01',1,'{}','consumed',2)",
                params![
                    uuid::Uuid::new_v4().to_string(),
                    source_generation,
                    prepared.root_id,
                    prepared.owner_generation,
                    uuid::Uuid::new_v4().to_string()
                ],
            )
            .unwrap();
        drop(broker);
        let broker = open_with_owner(&path, uid, root.path()).unwrap();
        assert_eq!(
            broker
                .read_root_source_effect_obligations(&prepared.root_id, &prepared.root_init)
                .unwrap()
                .consumed_without_evidence,
            1
        );
        assert_eq!(
            broker
                .read_exact_prepared_owner(
                    &source_generation,
                    &prepared.root_id,
                    &prepared.owner_generation,
                )
                .unwrap(),
            prepared,
        );
        assert_eq!(
            broker
                .read_exact_release(
                    &source_generation,
                    &prepared.root_id,
                    &prepared.owner_generation
                )
                .unwrap(),
            released,
        );
        broker
            .mailbox
            .conn
            .execute_batch("DROP TRIGGER broker_prepared_owner_no_running")
            .unwrap();
        drop(broker);
        assert!(open_with_owner(&path, uid, root.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn quiesced_v29_copy_activates_once_and_direct_writers_refuse_v30() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("sidecar");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.join("pid-identity.db");
        drop(MailboxDb::open(&path).unwrap());
        pin_v29_fixture(&path);
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
        fs::remove_file(directory.join("payload-custody.json")).unwrap();
        assert!(open_with_owner(&path, uid, root.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn persisted_v30_broker_sidecar_refuses_without_rewriting_generation() {
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
        let generation = activate_with_owner(&path, uid, root.path()).unwrap();
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "DROP TRIGGER completion_uncertain_input_preserve;
             DROP TABLE completion_uncertain_input;
             DROP INDEX idx_mailbox_deliverable_session_live;
             DROP INDEX idx_mailbox_deliverable_target_live;
             DROP INDEX idx_mailbox_deliverable_global;
             PRAGMA user_version=30;",
        )
        .unwrap();
        conn.execute_batch(include_str!("migrations/0022_live_history_barrier.sql"))
            .unwrap();
        drop(conn);
        for refused in [
            open_with_owner(&path, uid, root.path()).err().unwrap(),
            activate_with_owner(&path, uid, root.path()).err().unwrap(),
        ] {
            assert!(refused.contains("persisted broker-owned v30"), "{refused}");
        }
        let conn = Connection::open(&path).unwrap();
        assert_eq!(schema::sidecar_version(&conn).unwrap(), 30);
        let retained: String = conn
            .query_row(
                "SELECT source_generation FROM broker_sidecar_authority WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained, generation);
        assert!(MailboxDb::open(&path).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn broker_storage_rejects_exposed_directory_and_symlinked_main_or_wal() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("sidecar");
        fs::create_dir(&directory).unwrap();
        // Establish the exposed-directory case under restrictive test umasks.
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
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
    fn retained_v32_upgrade_adds_exact_projection_without_losing_wal() {
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
        let generation = activate_with_owner(&path, uid, root.path()).unwrap();
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "PRAGMA wal_autocheckpoint=0;
             DROP TRIGGER broker_exact_source_projection_immutable;
             DROP TRIGGER broker_exact_source_projection_retain;
             DROP TABLE broker_exact_source_projection;
             CREATE TABLE retained_v32_wal(value TEXT NOT NULL);
             INSERT INTO retained_v32_wal VALUES('committed');
             PRAGMA user_version=32;",
        )
        .unwrap();
        assert_eq!(schema::sidecar_version(&conn).unwrap(), 32);
        let upgraded = open_with_owner(&path, uid, root.path()).unwrap();
        assert_eq!(upgraded.source_generation(), generation);
        assert_eq!(schema::sidecar_version(&upgraded.mailbox.conn).unwrap(), 33);
        let value: String = upgraded
            .mailbox
            .conn
            .query_row("SELECT value FROM retained_v32_wal", [], |r| r.get(0))
            .unwrap();
        assert_eq!(value, "committed");
        schema::validate_broker_owned(&upgraded.mailbox.conn).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn committed_v29_wal_content_survives_quiesced_copy_and_broker_restart() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.pid-identity.db");
        drop(MailboxDb::open(&source).unwrap());
        pin_v29_fixture(&source);
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
    fn offline_stage_preserves_committed_wal_and_requires_separate_publication() {
        let root = tempfile::tempdir().unwrap();
        let source_dir = root.path().join("old");
        let broker_root = root.path().join("broker");
        fs::create_dir(&source_dir).unwrap();
        fs::create_dir(&broker_root).unwrap();
        fs::set_permissions(&broker_root, fs::Permissions::from_mode(0o700)).unwrap();
        let source = source_dir.join("pid-identity.db");
        drop(MailboxDb::open(&source).unwrap());
        pin_v29_fixture(&source);
        let writer = Connection::open(&source).unwrap();
        writer.execute_batch("PRAGMA wal_autocheckpoint=0; CREATE TABLE cutover_probe(value TEXT); INSERT INTO cutover_probe VALUES('committed WAL row');").unwrap();
        let uid = unsafe { libc::geteuid() };
        let stage = stage_with_owner(&source, uid, &broker_root, uid).unwrap();
        assert!(!broker_root.join("sidecar").exists());
        let staged = Connection::open_with_flags(
            stage.join("pid-identity.db"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        assert_eq!(
            staged
                .query_row("SELECT value FROM cutover_probe", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "committed WAL row"
        );
        drop(staged);
        assert!(stage_with_owner(&source, uid, &broker_root, uid).is_ok());
        // A crash after an installer atomically publishes the complete
        // directory, but before activation, is a refusal on broker restart.
        let target_dir = broker_root.join("sidecar");
        fs::rename(&stage, &target_dir).unwrap();
        File::open(&broker_root).unwrap().sync_all().unwrap();
        let target = target_dir.join("pid-identity.db");
        assert!(open_with_owner(&target, uid, &broker_root).is_err());
        // The installed broker sets umask(077) before any authority lock.
        // This unprivileged unit process need not have that umask.
        fs::set_permissions(
            mailbox_authority_path(&target),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        assert!(stage_with_owner(&source, uid, &broker_root, uid).is_err());
        let generation = activate_with_owner(&target, uid, &broker_root).unwrap();
        assert_eq!(
            open_with_owner(&target, uid, &broker_root)
                .unwrap()
                .source_generation(),
            generation
        );
        writer
            .execute(
                "INSERT INTO cutover_probe VALUES('retired source only')",
                [],
            )
            .unwrap();
        let broker = open_with_owner(&target, uid, &broker_root).unwrap();
        let count: i64 = broker
            .mailbox
            .conn
            .query_row("SELECT count(*) FROM cutover_probe", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[cfg(unix)]
    #[test]
    fn offline_stage_rejects_source_replacement_and_bad_storage() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("pid-identity.db");
        drop(MailboxDb::open(&source).unwrap());
        pin_v29_fixture(&source);
        let broker_root = root.path().join("broker");
        fs::create_dir(&broker_root).unwrap();
        // This first attempt intentionally uses an exposed broker directory.
        fs::set_permissions(&broker_root, fs::Permissions::from_mode(0o755)).unwrap();
        let uid = unsafe { libc::geteuid() };
        assert!(stage_with_owner(&source, uid, &broker_root, uid).is_err());
        fs::set_permissions(&broker_root, fs::Permissions::from_mode(0o700)).unwrap();
        let link = root.path().join("linked-broker");
        symlink(&broker_root, &link).unwrap();
        assert!(stage_with_owner(&source, uid, &link, uid).is_err());
        let replacement = root.path().join("replacement");
        fs::rename(&source, &replacement).unwrap();
        symlink(&replacement, &source).unwrap();
        assert!(stage_with_owner(&source, uid, &broker_root, uid).is_err());
        fs::remove_file(&source).unwrap();
        fs::rename(&replacement, &source).unwrap();
        let stage = stage_with_owner(&source, uid, &broker_root, uid).unwrap();
        assert!(stage.join("pid-identity.db").exists());
    }

    #[cfg(unix)]
    #[test]
    fn offline_stage_refuses_a_source_path_swap_during_copy() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("pid-identity.db");
        drop(MailboxDb::open(&source).unwrap());
        pin_v29_fixture(&source);
        let broker_root = root.path().join("broker");
        fs::create_dir(&broker_root).unwrap();
        fs::set_permissions(&broker_root, fs::Permissions::from_mode(0o700)).unwrap();
        let swapped = source.clone();
        AFTER_SNAPSHOT_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let old = swapped.with_extension("retired");
                fs::rename(&swapped, &old).unwrap();
                fs::copy(&old, &swapped).unwrap();
            }));
        });
        let uid = unsafe { libc::geteuid() };
        assert!(stage_with_owner(&source, uid, &broker_root, uid).is_err());
        assert!(!broker_root.join("sidecar").exists());
    }

    #[cfg(unix)]
    #[test]
    fn failed_copy_leaves_v29_source_as_the_only_live_database() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("pid-identity.db");
        drop(MailboxDb::open(&source).unwrap());
        pin_v29_fixture(&source);
        let broker_root = root.path().join("broker");
        fs::create_dir(&broker_root).unwrap();
        fs::set_permissions(&broker_root, fs::Permissions::from_mode(0o700)).unwrap();
        let copied_root = broker_root.clone();
        AFTER_SNAPSHOT_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let stage = fs::read_dir(&copied_root)
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path();
                fs::write(stage.join("pid-identity.db"), b"broken copy").unwrap();
            }));
        });
        let uid = unsafe { libc::geteuid() };
        assert!(stage_with_owner(&source, uid, &broker_root, uid).is_err());
        assert!(!broker_root.join("sidecar").exists());
        let historical =
            Connection::open_with_flags(&source, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        schema::validate_exact_v29(&historical).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn publication_refuses_stale_snapshot_and_activates_only_matching_v29() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("pid-identity.db");
        drop(MailboxDb::open(&source).unwrap());
        pin_v29_fixture(&source);
        let writer = Connection::open(&source).unwrap();
        writer.execute_batch("CREATE TABLE cutover_probe(value TEXT); INSERT INTO cutover_probe VALUES('first');").unwrap();
        let broker_root = root.path().join("broker");
        fs::create_dir(&broker_root).unwrap();
        fs::set_permissions(&broker_root, fs::Permissions::from_mode(0o700)).unwrap();
        let uid = unsafe { libc::geteuid() };
        let stale = stage_with_owner(&source, uid, &broker_root, uid).unwrap();
        writer
            .execute("INSERT INTO cutover_probe VALUES('second')", [])
            .unwrap();
        assert!(publish_with_owner(&source, uid, &stale, &broker_root, uid).is_err());
        assert!(!broker_root.join("sidecar").exists());
        let stage = stage_with_owner(&source, uid, &broker_root, uid).unwrap();
        let generation = publish_with_owner(&source, uid, &stage, &broker_root, uid).unwrap();
        assert!(!stage.exists());
        let target = broker_root.join("sidecar/pid-identity.db");
        let broker = open_with_owner(&target, uid, &broker_root).unwrap();
        assert_eq!(broker.source_generation(), generation);
        assert_eq!(
            broker
                .mailbox
                .conn
                .query_row("SELECT count(*) FROM cutover_probe", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert!(publish_with_owner(&source, uid, &stale, &broker_root, uid).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn publication_refuses_mispermissioned_or_symlinked_stage() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("pid-identity.db");
        drop(MailboxDb::open(&source).unwrap());
        pin_v29_fixture(&source);
        let broker_root = root.path().join("broker");
        fs::create_dir(&broker_root).unwrap();
        fs::set_permissions(&broker_root, fs::Permissions::from_mode(0o700)).unwrap();
        let uid = unsafe { libc::geteuid() };
        let stage = stage_with_owner(&source, uid, &broker_root, uid).unwrap();
        fs::set_permissions(&stage, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(publish_with_owner(&source, uid, &stage, &broker_root, uid).is_err());
        fs::set_permissions(&stage, fs::Permissions::from_mode(0o700)).unwrap();
        fs::remove_file(stage.join("pid-identity.db")).unwrap();
        symlink(&source, stage.join("pid-identity.db")).unwrap();
        assert!(publish_with_owner(&source, uid, &stage, &broker_root, uid).is_err());
        assert!(!broker_root.join("sidecar").exists());
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
        let copied_snapshot = broker
            .mailbox_mut()
            .accept_exact_native_attempt(&attempt, &owner, &root_id)
            .unwrap();
        broker
            .mailbox_mut()
            .bind_exact_native_grant(
                &copied_snapshot,
                &uuid::Uuid::new_v4().to_string(),
                &"c".repeat(64),
            )
            .unwrap();
        assert!(
            broker
                .read_exact_native_grant_v30(
                    &generation,
                    &root_id,
                    &owner,
                    &copied_snapshot,
                    &uuid::Uuid::new_v4().to_string(),
                    &"c".repeat(64)
                )
                .is_err(),
            "retained copied v29 owner cannot authorize native K"
        );
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
        let ungranted_source = super::super::ContinuationAttempt {
            attempt_id: uuid::Uuid::new_v4().to_string(),
            operation: "source_recovery".into(),
            source_registration_id: Some(uuid::Uuid::new_v4().to_string()),
            ..new_attempt.clone()
        };
        assert!(
            broker
                .reserve_exact_attempt(&new_owner, &root_id, &ungranted_source)
                .unwrap_err()
                .contains("exact registration/listener file custody and a one-use effect grant")
        );
        assert!(
            broker
                .accept_exact_attempt(&new_owner, &root_id, &ungranted_source)
                .unwrap_err()
                .contains("physical effect custody")
        );
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
        let (new_snapshot, new_accepted) = broker
            .accept_exact_attempt(&new_owner, &root_id, &new_attempt)
            .unwrap();
        assert_eq!(new_accepted.phase.as_deref(), Some("accepted"));
        assert!(
            broker
                .bind_exact_native_grant_v30(
                    &generation,
                    &root_id,
                    &new_owner,
                    &new_snapshot,
                    &uuid::Uuid::new_v4().to_string(),
                    &"d".repeat(64),
                )
                .is_err(),
            "a transport row is not an accepted provider invocation"
        );
        broker.mailbox_mut().conn.execute(
            "INSERT INTO session_wake_claim(session_id,claim_token,claimed_at,reason,auto_wake_count) VALUES('native-session','native-claim','2026-09-23T00:00:00Z','fixture',1)",
            [],
        ).unwrap();
        let native_attempt = super::super::ContinuationAttempt {
            attempt_id: uuid::Uuid::new_v4().to_string(),
            operation: "activation".into(),
            session_id: Some("native-session".into()),
            claim_token: Some("native-claim".into()),
            result_path: "/fixture/native-result".into(),
            ..new_attempt.clone()
        };
        broker
            .reserve_exact_attempt(&new_owner, &root_id, &native_attempt)
            .unwrap();
        let (native_snapshot, _) = broker
            .accept_exact_attempt(&new_owner, &root_id, &native_attempt)
            .unwrap();
        let native_grant = uuid::Uuid::new_v4().to_string();
        let request_digest = "d".repeat(64);
        let native = broker
            .bind_exact_native_grant_v30(
                &generation,
                &root_id,
                &new_owner,
                &native_snapshot,
                &native_grant,
                &request_digest,
            )
            .unwrap();
        assert_eq!(native.source_generation, generation);
        assert_eq!(native.binding.grant_id, native_grant);
        assert_eq!(
            broker
                .bind_exact_native_grant_v30(
                    &generation,
                    &root_id,
                    &new_owner,
                    &native_snapshot,
                    &native_grant,
                    &request_digest,
                )
                .unwrap(),
            native,
            "lost bind reply reads the same grant"
        );
        let restarted = open_with_owner(&target, uid, root.path()).unwrap();
        assert_eq!(
            restarted
                .read_exact_native_grant_v30(
                    &generation,
                    &root_id,
                    &new_owner,
                    &native_snapshot,
                    &native_grant,
                    &request_digest,
                )
                .unwrap(),
            Some(native.clone()),
            "broker restart reads one retained State binding"
        );
        assert!(
            broker
                .read_exact_native_grant_v30(
                    "retired-generation",
                    &root_id,
                    &new_owner,
                    &native_snapshot,
                    &native_grant,
                    &request_digest,
                )
                .is_err()
        );
        assert!(
            broker
                .read_exact_native_grant_v30(
                    &generation,
                    &uuid::Uuid::new_v4().to_string(),
                    &new_owner,
                    &native_snapshot,
                    &native_grant,
                    &request_digest,
                )
                .is_err()
        );
        let mut wrong_snapshot = native_snapshot.clone();
        wrong_snapshot.attempt.result_path = "/fixture/sibling-result".into();
        assert!(
            broker
                .read_exact_native_grant_v30(
                    &generation,
                    &root_id,
                    &new_owner,
                    &wrong_snapshot,
                    &native_grant,
                    &request_digest,
                )
                .is_err()
        );
        assert!(
            broker
                .read_exact_native_grant_v30(
                    &generation,
                    &root_id,
                    &new_owner,
                    &native_snapshot,
                    &uuid::Uuid::new_v4().to_string(),
                    &request_digest,
                )
                .is_err()
        );
        assert!(
            broker
                .accept_exact_attempt(&new_owner, &root_id, &new_attempt)
                .is_err(),
            "acceptance replay must not advance twice"
        );
        assert!(
            broker
                .revoke_exact_unaccepted_attempt(&new_owner, &root_id, &new_attempt)
                .is_err(),
            "accepted work cannot be withdrawn"
        );
        let withdrawn_attempt = super::super::ContinuationAttempt {
            attempt_id: uuid::Uuid::new_v4().to_string(),
            result_path: "/fixture/withdrawn-result".into(),
            ..new_attempt.clone()
        };
        broker
            .reserve_exact_attempt(&new_owner, &root_id, &withdrawn_attempt)
            .unwrap();
        let withdrawn = broker
            .revoke_exact_unaccepted_attempt(&new_owner, &root_id, &withdrawn_attempt)
            .unwrap();
        assert_eq!(withdrawn.phase.as_deref(), Some("never_started"));
        assert_eq!(withdrawn.revision, Some(2));
        assert!(
            broker
                .revoke_exact_unaccepted_attempt(&new_owner, &root_id, &withdrawn_attempt)
                .is_err(),
            "withdrawal replay cannot advance twice"
        );
        assert_eq!(
            read(
                &broker,
                &generation,
                &root_id,
                &new_owner.owner_generation,
                Some(&new_attempt.attempt_id)
            )
            .unwrap(),
            new_accepted
        );
        // The retained broker is the SQLite caller. Activation must compare
        // the owner row with the broker-verified driver incarnation, rather
        // than with the broker process's own PID.
        let mut remote_driver = std::process::Command::new("sleep")
            .arg("20")
            .spawn()
            .unwrap();
        let live_driver =
            crate::pid_identity::read_live_process_identity(i64::from(remote_driver.id()))
                .unwrap()
                .unwrap();
        let mut remote_owner = new_owner.clone();
        remote_owner.owner_generation = uuid::Uuid::new_v4().to_string();
        remote_owner.driver_identity = SourceProcessIdentity {
            pid: live_driver.os_pid,
            boot_id: live_driver.os_boot_id,
            starttime_ticks: live_driver.os_pid_starttime_ticks,
        };
        broker.publish_exact_owner(&remote_owner, &root_id).unwrap();
        broker.mailbox.conn.execute(
            "INSERT INTO session_wake_claim(session_id,claim_token,claimed_at,reason,auto_wake_count)
             VALUES('remote-session','remote-claim','2026-09-23T00:00:00Z','fixture',1)",
            [],
        ).unwrap();
        let remote_attempt = super::super::ContinuationAttempt {
            attempt_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: remote_owner.owner_generation.clone(),
            operation: "activation".into(),
            request_sha256: "c".repeat(64),
            source_registration_id: None,
            source_listener_revision: None,
            session_id: Some("remote-session".into()),
            claim_token: Some("remote-claim".into()),
            result_path: "/fixture/remote-result".into(),
        };
        let remote_reserved = broker
            .reserve_exact_attempt(&remote_owner, &root_id, &remote_attempt)
            .unwrap();
        assert!(remote_reserved.claim_present);
        let remote_withdrawn = broker
            .revoke_exact_unaccepted_attempt(&remote_owner, &root_id, &remote_attempt)
            .unwrap();
        assert!(!remote_withdrawn.claim_present);
        assert_eq!(remote_withdrawn.phase.as_deref(), Some("never_started"));
        let _ = remote_driver.kill();
        let _ = remote_driver.wait();
        drop(broker);

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
