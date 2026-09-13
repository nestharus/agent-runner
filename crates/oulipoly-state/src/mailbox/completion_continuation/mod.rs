//! Sidecar continuation projection and per-attempt authority. State admission
//! remains authoritative; this projection may be repaired only from that ledger.
//! Declared roles: accessor, validator, mapper, orchestration.
use super::*;
use crate::completion_continuation::{AdmittedSourceBinding, PROTOCOL, SourceProcessIdentity};
use serde::{Deserialize, Serialize};
mod attempts;
mod source;
pub(super) use source::{accept_on, bound_event, reject_unbound_v2_trigger, retained_payload};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionDomainOwner {
    pub protocol: String,
    pub domain_id: String,
    pub owner_generation: String,
    pub guardian_identity: SourceProcessIdentity,
    pub driver_identity: SourceProcessIdentity,
    pub endpoint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuationAttempt {
    pub attempt_id: String,
    pub owner_generation: String,
    pub operation: String,
    pub request_sha256: String,
    pub source_registration_id: Option<String>,
    pub source_listener_revision: Option<i64>,
    pub session_id: Option<String>,
    pub claim_token: Option<String>,
    pub result_path: String,
}

impl MailboxDb {
    /// Nonmutating capability lookup. Version integers alone are not lineage.
    pub fn completion_continuation_domain(&self) -> Result<Option<String>, String> {
        domain_on(&self.conn)
    }

    /// Native independent-root bootstrap uses this only for a previously absent
    /// sidecar. Existing legacy domains require separately authorized transition.
    pub fn open_completion_continuation_domain(path: &Path) -> Result<Self, String> {
        if path.exists() {
            let probe = Self::open_read_only(path)?;
            if probe.completion_continuation_domain()?.is_none() {
                return Err("unsupported_transition_required: existing domain has no completion-continuation-v2 lineage".into());
            }
            drop(probe);
            return Self::open(path);
        }
        let authority =
            MailboxAuthorityFence::acquire_exclusive(path).map_err(|e| e.to_string())?;
        if authority.path().exists() {
            return Err(
                "completion domain appeared during bootstrap; retry independent entry".into(),
            );
        }
        // Publish only a complete fresh domain. A crash during either schema
        // transaction leaves the real name absent, never a misleading legacy17.
        let staging_directory = tempfile::Builder::new()
            .prefix(".completion-fresh-")
            .tempdir_in(authority.path().parent().ok_or("domain parent absent")?)
            .map_err(|e| e.to_string())?;
        let staging = staging_directory.path().join(
            authority
                .path()
                .file_name()
                .ok_or("domain filename absent")?,
        );
        let mut staged = Self::open(&staging)?;
        #[cfg(feature = "age360-fault-fixtures")]
        crate::completion_continuation::age360_fault_barrier("fresh-schema17");
        initialize_fresh_domain(&mut staged.conn)?;
        staged
            .conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")
            .map_err(|e| e.to_string())?;
        drop(staged);
        std::fs::File::open(&staging)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        // Namespace election excludes every supported writer of the final name.
        std::fs::rename(&staging, authority.path()).map_err(|e| e.to_string())?;
        std::fs::File::open(authority.path().parent().ok_or("domain parent absent")?)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        Self::open_with_owned_authority(authority)
    }

    pub fn retain_completion_context(
        &self,
        identity: &SourceProcessIdentity,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO completion_continuation_context(identity) VALUES(?1)",
                [serde_json::to_string(identity).map_err(|e| e.to_string())?],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn release_completion_context(
        &self,
        identity: &SourceProcessIdentity,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "DELETE FROM completion_continuation_context WHERE identity=?1",
                [serde_json::to_string(identity).map_err(|e| e.to_string())?],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn completion_contexts(&self) -> Result<Vec<SourceProcessIdentity>, String> {
        let mut statement = self
            .conn
            .prepare("SELECT identity FROM completion_continuation_context")
            .map_err(|e| e.to_string())?;
        let identities = statement
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        identities
            .map(|r| {
                serde_json::from_str(&r.map_err(|e| e.to_string())?).map_err(|e| e.to_string())
            })
            .collect()
    }

    pub fn completion_continuation_owner(&self) -> Result<Option<CompletionDomainOwner>, String> {
        if domain_on(&self.conn)?.is_none() {
            return Ok(None);
        }
        self.conn.query_row("SELECT domain_id,generation,guardian_identity,driver_identity,endpoint FROM completion_continuation_owner WHERE phase='running'", [], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?)))
            .optional().map_err(|e| e.to_string())?.map(|(domain_id,owner_generation,guardian,driver,endpoint)| Ok(CompletionDomainOwner {
                protocol: PROTOCOL.into(), domain_id, owner_generation,
                guardian_identity: serde_json::from_str(&guardian).map_err(|e| e.to_string())?,
                driver_identity: serde_json::from_str(&driver).map_err(|e| e.to_string())?, endpoint,
            })).transpose()
    }

    pub(crate) fn close_idle_continuation_generation(
        &mut self,
        owner: &CompletionDomainOwner,
        admitted: &[AdmittedSourceBinding],
    ) -> Result<bool, String> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        if domain_on(&tx)?.as_deref() != Some(&owner.domain_id) {
            return Err("completion retirement domain conflict".into());
        }
        for binding in admitted {
            let source = binding.registration()?;
            if source.domain_id != owner.domain_id {
                continue;
            }
            let retained:Option<Vec<u8>>=tx.query_row("SELECT binding FROM completion_continuation_source WHERE registration_id=?1 AND phase='accepted'",[source.registration_id],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
            let Some(retained) = retained else {
                return Ok(false);
            };
            if !AdmittedSourceBinding::decode(&retained)?.same_source(binding) {
                return Ok(false);
            }
        }
        let pending:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM completion_event_listener WHERE acknowledged_at IS NULL) OR EXISTS(SELECT 1 FROM mailbox WHERE delivered_at IS NULL) OR EXISTS(SELECT 1 FROM completion_continuation_attempt WHERE phase NOT IN ('drained','never_started'))",[],|r|r.get(0)).map_err(|e|e.to_string())?;
        if pending {
            return Ok(false);
        }
        let changed=tx.execute("UPDATE completion_continuation_owner SET phase='closing' WHERE generation=?1 AND domain_id=?2 AND phase='running' AND guardian_identity=?3 AND driver_identity=?4",params![owner.owner_generation,owner.domain_id,serde_json::to_string(&owner.guardian_identity).map_err(|e|e.to_string())?,serde_json::to_string(&owner.driver_identity).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(changed == 1)
    }

    /// The independently rooted guardian holds the domain lifetime election.
    /// Replacing a generation preserves all predecessor physical debt.
    pub fn publish_completion_continuation_owner(
        &mut self,
        owner: &CompletionDomainOwner,
    ) -> Result<(), String> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        if owner.protocol != PROTOCOL || domain_on(&tx)?.as_deref() != Some(&owner.domain_id) {
            return Err("completion owner domain/protocol conflict".into());
        }
        tx.execute(
            "UPDATE completion_continuation_owner SET phase='lost' WHERE phase='running'",
            [],
        )
        .map_err(|e| e.to_string())?;
        // Reserved is committed before acceptance, and acceptance before any fork.
        // Replacement can revoke that unspent authority, but never accepted debt.
        tx.execute("UPDATE completion_continuation_attempt SET phase='never_started',revision=revision+1,integrated=1,drain_receipt='replacement_revoked_unaccepted' WHERE phase='reserved' AND revision=1 AND custodian_identity IS NULL",[]).map_err(|e|e.to_string())?;
        tx.execute("DELETE FROM session_wake_claim WHERE EXISTS(SELECT 1 FROM completion_continuation_attempt a WHERE a.operation='activation' AND a.session_id=session_wake_claim.session_id AND a.claim_token=session_wake_claim.claim_token AND a.phase='never_started')",[]).map_err(|e|e.to_string())?;
        tx.execute("UPDATE completion_continuation_attempt SET phase='unknown_custody',revision=revision+1 WHERE phase IN ('accepted','starting','running')", []).map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO completion_continuation_owner VALUES(?1,?2,'running',?3,?4,?5)",
            params![
                owner.owner_generation,
                owner.domain_id,
                serde_json::to_string(&owner.guardian_identity).map_err(|e| e.to_string())?,
                serde_json::to_string(&owner.driver_identity).map_err(|e| e.to_string())?,
                owner.endpoint
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
}

fn initialize_fresh_domain(conn: &mut Connection) -> Result<(), String> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    tx.execute_batch(include_str!(
        "../migrations/0018_completion_continuation.sql"
    ))
    .map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO completion_continuation_domain VALUES(1,?1,'main-native-completion-v2')",
        [uuid::Uuid::new_v4().to_string()],
    )
    .map_err(|e| e.to_string())?;
    tx.pragma_update(None, "user_version", 18)
        .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

fn domain_on(conn: &Connection) -> Result<Option<String>, String> {
    let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='completion_continuation_domain')", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    if !exists {
        return Ok(None);
    }
    validate_schema_on(conn)?;
    conn.query_row("SELECT domain_id FROM completion_continuation_domain WHERE singleton=1 AND lineage='main-native-completion-v2'", [], |r| r.get(0)).optional().map_err(|e| e.to_string())
}

pub fn activation_request_sha256(
    session_id: &str,
    runtime: Option<&SessionMetadataRow>,
    token: &str,
    count: i64,
) -> String {
    crate::completion_continuation::sha256(
        serde_json::json!([
            "completion-native-resume-v2",
            session_id,
            token,
            count,
            runtime.and_then(|r| r.model_name.as_deref()),
            runtime.and_then(|r| r.models_dir.as_deref()),
            runtime.and_then(|r| r.invocation_uuid.as_deref()),
            runtime.and_then(|r| r.provider_name.as_deref())
        ])
        .to_string()
        .as_bytes(),
    )
}
pub(super) use attempts::{
    admit_launcher_on, bind_generation_on, cancel_unaccepted_activation_on, reserve_activation_on,
};

pub(super) fn validate_schema_on(conn: &Connection) -> Result<(), String> {
    type Definition = (String, String, String);
    static EXPECTED: std::sync::OnceLock<Result<Vec<Definition>, String>> =
        std::sync::OnceLock::new();
    fn definitions(conn: &Connection) -> Result<Vec<Definition>, String> {
        let mut statement=conn.prepare("SELECT type,name,sql FROM sqlite_master WHERE name LIKE 'completion_continuation_%' AND sql IS NOT NULL ORDER BY type,name").map_err(|e|e.to_string())?;
        statement
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|e| e.to_string())?
            .map(|r| r.map_err(|e| e.to_string()))
            .collect()
    }
    let expected = EXPECTED
        .get_or_init(|| {
            let expected = Connection::open_in_memory().map_err(|e| e.to_string())?;
            expected
                .execute_batch("CREATE TABLE session_wake_claim(session_id TEXT,claim_token TEXT);")
                .map_err(|e| e.to_string())?;
            expected
                .execute_batch(include_str!(
                    "../migrations/0018_completion_continuation.sql"
                ))
                .map_err(|e| e.to_string())?;
            definitions(&expected)
        })
        .as_ref()
        .map_err(Clone::clone)?;
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if version != 18 || definitions(conn)? != *expected {
        return Err(
            "unsupported_transition_required: completion domain schema lineage differs".into(),
        );
    }
    Ok(())
}

pub(super) fn mark_activation_running_on(
    tx: &Transaction<'_>,
    generation: &RuntimeGenerationId,
) -> Result<(), String> {
    if domain_on(tx)?.is_none() {
        return Ok(());
    }
    tx.execute("UPDATE completion_continuation_attempt SET phase='running',revision=revision+1 WHERE runtime_generation_uuid=?1 AND phase='starting'",[generation.to_string()]).map_err(|e|e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, MailboxDb, CompletionDomainOwner) {
        let dir = tempfile::tempdir().unwrap();
        let mut db =
            MailboxDb::open_completion_continuation_domain(&dir.path().join("pid-identity.db"))
                .unwrap();
        let live = crate::pid_identity::read_live_process_identity(i64::from(std::process::id()))
            .unwrap()
            .unwrap();
        let identity = SourceProcessIdentity {
            pid: live.os_pid,
            boot_id: live.os_boot_id,
            starttime_ticks: live.os_pid_starttime_ticks,
        };
        let owner = CompletionDomainOwner {
            protocol: PROTOCOL.into(),
            domain_id: db.completion_continuation_domain().unwrap().unwrap(),
            owner_generation: uuid::Uuid::new_v4().to_string(),
            guardian_identity: identity.clone(),
            driver_identity: identity,
            endpoint: "/fixture/owner.sock".into(),
        };
        db.publish_completion_continuation_owner(&owner).unwrap();
        (dir, db, owner)
    }
    fn reservation(db: &mut MailboxDb, owner: &CompletionDomainOwner) -> ContinuationAttempt {
        db.conn.execute("INSERT INTO session_wake_claim(session_id,claim_token,claimed_at,reason,auto_wake_count) VALUES('session','token','2026-09-12T00:00:00Z','fixture',1)",[]).unwrap();
        let attempt = ContinuationAttempt {
            attempt_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: owner.owner_generation.clone(),
            operation: "activation".into(),
            request_sha256: "a".repeat(64),
            source_registration_id: None,
            source_listener_revision: None,
            session_id: Some("session".into()),
            claim_token: Some("token".into()),
            result_path: "/fixture/result.json".into(),
        };
        db.reserve_continuation_attempt(&attempt).unwrap();
        attempt
    }
    #[test]
    fn completion_continuation_existing_legacy_domain_is_not_migrated_by_open_or_probe() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let db = MailboxDb::open(&path).unwrap();
        assert_eq!(db.completion_continuation_domain().unwrap(), None);
        drop(db);
        assert!(MailboxDb::open_completion_continuation_domain(&path).is_err());
        let db = MailboxDb::open(&path).unwrap();
        assert_eq!(
            db.conn
                .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            17
        );
        assert!(db.list_pending("ordinary").unwrap().is_empty());
    }
    #[test]
    fn source_recovery_population_and_exclusion_survive_repeated_owner_replacement() {
        let (_dir, mut db, mut owner) = fixture();
        let source = |owner: &CompletionDomainOwner, registration: &str| ContinuationAttempt {
            attempt_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: owner.owner_generation.clone(),
            operation: "source_recovery".into(),
            request_sha256: "a".repeat(64),
            source_registration_id: Some(registration.into()),
            source_listener_revision: Some(1),
            session_id: None,
            claim_token: None,
            result_path: "/fixture/result.json".into(),
        };
        for n in 0..4 {
            let request = source(&owner, &format!("source-{n}"));
            db.reserve_continuation_attempt(&request).unwrap();
            db.accept_continuation_attempt(&request).unwrap();
            owner.owner_generation = uuid::Uuid::new_v4().to_string();
            db.publish_completion_continuation_owner(&owner).unwrap();
            assert!(
                db.reserve_continuation_attempt(&source(&owner, &format!("source-{n}")))
                    .is_err()
            );
        }
        assert!(
            db.reserve_continuation_attempt(&source(&owner, "fifth-distinct-source"))
                .is_err()
        );
        assert_eq!(db.pending_continuation_attempts().unwrap().len(), 4);
    }

    #[test]
    fn adopting_receipt_requires_exact_original_boundary_and_replays_across_replacement() {
        let (_dir, mut db, owner) = fixture();
        let request = reservation(&mut db, &owner);
        db.accept_continuation_attempt(&request).unwrap();
        db.attach_continuation_custodian_with_adopter(
            &request,
            &owner.driver_identity,
            Some(&owner.guardian_identity),
        )
        .unwrap();
        db.advance_continuation_attempt(
            &request,
            3,
            "accepted",
            "starting",
            &owner.driver_identity,
        )
        .unwrap();
        let replacement = CompletionDomainOwner {
            owner_generation: uuid::Uuid::new_v4().to_string(),
            ..owner.clone()
        };
        db.publish_completion_continuation_owner(&replacement)
            .unwrap();
        let mut receipt = serde_json::json!({"attempt_id":request.attempt_id,"custodian":owner.driver_identity,"adopter":owner.guardian_identity,"custodian_wait_status":9,"owned_children":"ECHILD"});
        receipt["adopter"]["pid"] = 999999.into();
        assert!(
            db.discharge_adopted_continuation_attempt(&request, &receipt.to_string())
                .is_err()
        );
        assert!(
            db.wake_session_reader()
                .wake_claim("session")
                .unwrap()
                .is_some()
        );
        receipt["adopter"] = serde_json::to_value(&owner.guardian_identity).unwrap();
        db.discharge_adopted_continuation_attempt(&request, &receipt.to_string())
            .unwrap();
        db.discharge_adopted_continuation_attempt(&request, &receipt.to_string())
            .unwrap();
        assert!(
            db.wake_session_reader()
                .wake_claim("session")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn completion_continuation_schema_fingerprint_rejects_same_version_mutation() {
        let (_dir, db, _owner) = fixture();
        db.conn
            .execute_batch("DROP TRIGGER completion_continuation_claim_delete")
            .unwrap();
        assert!(db.completion_continuation_domain().is_err());
    }
    #[test]
    fn completion_continuation_accepted_and_unknown_attempts_fence_manual_release_and_new_owner() {
        let (_dir, mut db, owner) = fixture();
        let attempt = reservation(&mut db, &owner);
        db.accept_continuation_attempt(&attempt).unwrap();
        assert!(!matches!(
            db.wake_sessions()
                .release_wake_claim_for_manual_resume("session", "token"),
            Ok(true)
        ));
        db.attach_continuation_custodian(&attempt, &owner.driver_identity)
            .unwrap();
        db.advance_continuation_attempt(
            &attempt,
            3,
            "accepted",
            "starting",
            &owner.driver_identity,
        )
        .unwrap();
        let replacement = CompletionDomainOwner {
            owner_generation: uuid::Uuid::new_v4().to_string(),
            ..owner.clone()
        };
        db.publish_completion_continuation_owner(&replacement)
            .unwrap();
        assert!(
            db.advance_continuation_attempt(
                &attempt,
                4,
                "starting",
                "running",
                &owner.driver_identity
            )
            .is_err()
        );
        assert!(!matches!(
            db.wake_sessions()
                .release_wake_claim_for_manual_resume("session", "token"),
            Ok(true)
        ));
        assert!(
            !db.close_idle_continuation_generation(&replacement, &[])
                .unwrap()
        );
        let mut wrong = owner.driver_identity.clone();
        wrong.starttime_ticks += 1;
        assert!(
            db.discharge_continuation_attempt(&attempt, &wrong, "actual-drain")
                .is_err()
        );
        db.discharge_continuation_attempt(&attempt, &owner.driver_identity, "actual-drain")
            .unwrap();
        db.discharge_continuation_attempt(&attempt, &owner.driver_identity, "actual-drain")
            .unwrap();
        assert!(
            db.wake_session_reader()
                .wake_claim("session")
                .unwrap()
                .is_none()
        );
        assert!(
            db.close_idle_continuation_generation(&replacement, &[])
                .unwrap()
        );
    }
    #[test]
    fn same_driver_revokes_only_exact_unaccepted_reservation() {
        let (_dir, mut db, owner) = fixture();
        let attempt = reservation(&mut db, &owner);
        let mut wrong = attempt.clone();
        wrong.result_path.push_str("-wrong");
        assert!(db.revoke_unaccepted_continuation_attempt(&wrong).is_err());
        db.revoke_unaccepted_continuation_attempt(&attempt).unwrap();
        assert!(
            !db.pending_continuation_attempts()
                .unwrap()
                .iter()
                .any(|a| a.attempt_id == attempt.attempt_id)
        );
        let mut accepted = attempt.clone();
        accepted.attempt_id = uuid::Uuid::new_v4().to_string();
        db.reserve_continuation_attempt(&accepted).unwrap();
        db.accept_continuation_attempt(&accepted).unwrap();
        assert!(
            db.revoke_unaccepted_continuation_attempt(&accepted)
                .is_err()
        );
        assert!(
            db.pending_continuation_attempts()
                .unwrap()
                .iter()
                .any(|a| a.attempt_id == accepted.attempt_id)
        );
    }
    #[test]
    fn completion_continuation_attempt_envelope_and_never_forked_receipt_are_exact() {
        let (_dir, mut db, owner) = fixture();
        let attempt = reservation(&mut db, &owner);
        let mut changed = attempt.clone();
        changed.result_path.push_str("-changed");
        assert!(db.accept_continuation_attempt(&changed).is_err());
        db.accept_continuation_attempt(&attempt).unwrap();
        assert!(
            db.record_continuation_never_forked(&changed, "fork_failed")
                .is_err()
        );
        db.record_continuation_never_forked(&attempt, "fork_failed")
            .unwrap();
        db.record_continuation_never_forked(&attempt, "fork_failed")
            .unwrap();
        assert!(
            db.record_continuation_never_forked(&attempt, "different_reason")
                .is_err()
        );
        assert!(
            db.wake_session_reader()
                .wake_claim("session")
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn completion_continuation_unreleased_announcement_does_not_claim_no_fork() {
        let (_dir, mut db, owner) = fixture();
        let attempt = reservation(&mut db, &owner);
        db.accept_continuation_attempt(&attempt).unwrap();
        db.record_continuation_unreleased_before_announcement(
            &attempt,
            "original wait, endpoint EOF, unsent grant",
        )
        .unwrap();
        db.record_continuation_unreleased_before_announcement(
            &attempt,
            "original wait, endpoint EOF, unsent grant",
        )
        .unwrap();
        assert!(
            db.record_continuation_never_forked(
                &attempt,
                "original wait, endpoint EOF, unsent grant"
            )
            .is_err()
        );
        let receipt: String = db
            .conn
            .query_row(
                "SELECT drain_receipt FROM completion_continuation_attempt WHERE attempt_id=?1",
                [&attempt.attempt_id],
                |r| r.get(0),
            )
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&receipt).unwrap();
        assert_eq!(value["gate"], "unreleased_announcement_eof");
        assert!(
            db.wake_session_reader()
                .wake_claim("session")
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn completion_continuation_unreleased_gate_settles_predecessor_unknown_without_ack() {
        let (_dir, mut db, owner) = fixture();
        let attempt = reservation(&mut db, &owner);
        db.accept_continuation_attempt(&attempt).unwrap();
        db.attach_continuation_custodian(&attempt, &owner.driver_identity)
            .unwrap();
        let replacement = CompletionDomainOwner {
            owner_generation: uuid::Uuid::new_v4().to_string(),
            ..owner.clone()
        };
        db.publish_completion_continuation_owner(&replacement)
            .unwrap();
        let mut wrong = owner.driver_identity.clone();
        wrong.starttime_ticks += 1;
        assert!(
            db.cancel_unreleased_continuation_gate(&attempt, &wrong, "gate_eof_echild")
                .is_err()
        );
        db.cancel_unreleased_continuation_gate(&attempt, &owner.driver_identity, "gate_eof_echild")
            .unwrap();
        db.cancel_unreleased_continuation_gate(&attempt, &owner.driver_identity, "gate_eof_echild")
            .unwrap();
        assert!(
            db.wake_session_reader()
                .wake_claim("session")
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn completion_continuation_replacement_revokes_only_unaccepted_reservation() {
        let (_dir, mut db, owner) = fixture();
        let attempt = reservation(&mut db, &owner);
        let replacement = CompletionDomainOwner {
            owner_generation: uuid::Uuid::new_v4().to_string(),
            ..owner
        };
        db.publish_completion_continuation_owner(&replacement)
            .unwrap();
        assert!(db.accept_continuation_attempt(&attempt).is_err());
        assert!(
            db.wake_session_reader()
                .wake_claim("session")
                .unwrap()
                .is_none()
        );
        let phase: String = db
            .conn
            .query_row(
                "SELECT phase FROM completion_continuation_attempt WHERE attempt_id=?1",
                [attempt.attempt_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(phase, "never_started");
    }
}
