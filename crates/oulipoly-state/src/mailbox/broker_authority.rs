//! Root-owned sidecar cutover boundary. This opens the same State database;
//! there is no second acceptance ledger. Migration of the quiesced v29 bytes
//! into this directory is a separate prerequisite to activation.
use super::*;
#[cfg(unix)]
use sha2::{Digest, Sha256};
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

impl BrokerSidecar {
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

    /// Prepare an inert, root-only SQLite snapshot of an exact v29 source.
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

    /// Publish only a previously validated snapshot under an installed
    /// quiescence proof. The directory rename never replaces an existing
    /// sidecar. A crash after rename leaves v29 at the fixed name and broker
    /// startup refuses until the installer resumes activation explicitly.
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
fn require_root_owned_ancestors(path: &Path) -> Result<(), String> {
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
    let mut digest = Sha256::new();
    let mut schema_rows = conn
        .prepare("SELECT type,name,tbl_name,COALESCE(sql,'') FROM sqlite_master ORDER BY type,name")
        .map_err(|error| error.to_string())?;
    let rows = schema_rows
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|error| error.to_string())?;
    let mut tables = Vec::new();
    for row in rows {
        let (kind, name, parent, sql) = row.map_err(|error| error.to_string())?;
        for field in [&kind, &name, &parent, &sql] {
            digest.update((field.len() as u64).to_le_bytes());
            digest.update(field.as_bytes());
        }
        if kind == "table" {
            tables.push(name);
        }
    }
    tables.sort();
    for table in tables {
        // SQLite's own identifier came from sqlite_master; quote it as an
        // identifier rather than interpolating it as SQL syntax.
        let quoted = format!("\"{}\"", table.replace('"', "\"\""));
        let statement = conn
            .prepare(&format!("SELECT * FROM {quoted}"))
            .map_err(|error| error.to_string())?;
        let columns = statement.column_count();
        let order = (1..=columns)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let mut statement = conn
            .prepare(&format!("SELECT * FROM {quoted} ORDER BY {order}"))
            .map_err(|error| error.to_string())?;
        digest.update((table.len() as u64).to_le_bytes());
        digest.update(table.as_bytes());
        let mut rows = statement.query([]).map_err(|error| error.to_string())?;
        while let Some(row) = rows.next().map_err(|error| error.to_string())? {
            digest.update([0xff]);
            for index in 0..columns {
                use rusqlite::types::ValueRef;
                let (tag, bytes): (u8, Vec<u8>) =
                    match row.get_ref(index).map_err(|error| error.to_string())? {
                        ValueRef::Null => (0, Vec::new()),
                        ValueRef::Integer(value) => (1, value.to_le_bytes().to_vec()),
                        ValueRef::Real(value) => (2, value.to_bits().to_le_bytes().to_vec()),
                        ValueRef::Text(value) => (3, value.to_vec()),
                        ValueRef::Blob(value) => (4, value.to_vec()),
                    };
                digest.update([tag]);
                digest.update((bytes.len() as u64).to_le_bytes());
                digest.update(bytes);
            }
        }
    }
    Ok(digest.finalize().into())
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
        if complete_v29_fingerprint(&copied)? != expected {
            return Err("cutover copy schema or rows differ from v29 source".into());
        }
        drop(copied);
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
        if entry.map_err(|error| error.to_string())?.file_name()
            != std::ffi::OsStr::new("pid-identity.db")
        {
            return Err("cutover stage has unexpected artifacts".into());
        }
    }
    let before = source_artifacts(source, source_owner)?;
    let source_conn = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| error.to_string())?;
    broker_main_file_must_be_named(&source_conn)?;
    let source_hash = complete_v29_fingerprint(&source_conn)?;
    let copy_conn = Connection::open_with_flags(&stage_db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| error.to_string())?;
    if complete_v29_fingerprint(&copy_conn)? != source_hash {
        return Err("cutover source and staged rows/schema differ".into());
    }
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
    for definition in [
        schema::BROKER_PREPARED_OWNER_SCHEMA,
        schema::BROKER_PREPARED_OWNER_IMMUTABLE,
        schema::BROKER_PREPARED_OWNER_RETAIN,
        schema::BROKER_OWNER_RELEASE_SCHEMA,
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
        drop(broker);
        let broker = open_with_owner(&path, uid, root.path()).unwrap();
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
    fn offline_stage_preserves_committed_wal_and_requires_separate_publication() {
        let root = tempfile::tempdir().unwrap();
        let source_dir = root.path().join("old");
        let broker_root = root.path().join("broker");
        fs::create_dir(&source_dir).unwrap();
        fs::create_dir(&broker_root).unwrap();
        fs::set_permissions(&broker_root, fs::Permissions::from_mode(0o700)).unwrap();
        let source = source_dir.join("pid-identity.db");
        drop(MailboxDb::open(&source).unwrap());
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
        let broker_root = root.path().join("broker");
        fs::create_dir(&broker_root).unwrap();
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
        assert!(MailboxDb::open(&source).is_ok());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn publication_refuses_stale_snapshot_and_activates_only_matching_v29() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("pid-identity.db");
        drop(MailboxDb::open(&source).unwrap());
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
