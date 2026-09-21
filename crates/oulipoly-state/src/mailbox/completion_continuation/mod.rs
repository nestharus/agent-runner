//! Sidecar continuation projection and per-attempt authority. State admission
//! remains authoritative; this projection may be repaired only from that ledger.
//! Declared roles: accessor, validator, mapper, orchestration.
use super::*;
use crate::completion_continuation::{AdmittedSourceBinding, PROTOCOL, SourceProcessIdentity};
use serde::{Deserialize, Serialize};
mod attempts;
pub(super) use attempts::native_original_drain_on;
mod notification;
mod source;
pub use notification::{
    CompletionNotificationRequest, NotificationDeliveryEvidence, NotificationDisposition,
    NotificationPolicy,
};
pub(super) use notification::{
    reconcile_on as reconcile_notification_on,
    register_listener_on as register_notification_listener_on,
};
pub(super) use source::{accept_on, bound_event, reject_unbound_v2_trigger, retained_payload};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionDomainOwner {
    pub protocol: String,
    pub domain_id: String,
    /// Stable process-tree authority. This survives driver succession but is
    /// not reused by a later independent root after retirement.
    pub supervisor_authority_id: String,
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

    /// Authorized schema entry, not a probe. Existing domains use the same
    /// ordered additive upgrade as ordinary writable opens. Pair callers must
    /// finish State migration first and start recovery only after both succeed.
    pub fn open_completion_continuation_domain(path: &Path) -> Result<Self, String> {
        if path.exists() {
            return Self::open(path);
        }
        let authority =
            MailboxAuthorityFence::acquire_exclusive(path).map_err(|e| e.to_string())?;
        if authority.path().exists() {
            return Err(
                "completion domain appeared during bootstrap; retry independent entry".into(),
            );
        }
        // Publish only a complete fresh domain. A crash during schema
        // construction leaves the real name absent, never a partial domain.
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
        let staged = Self::open(&staging)?;
        // Retain the existing fault key; staging now contains the atomic v18
        // schema and identity, but still has not published the final name.
        #[cfg(feature = "age360-fault-fixtures")]
        crate::completion_continuation::age360_fault_barrier("fresh-schema17");
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
        self.conn.query_row("SELECT domain_id,supervisor_authority_id,generation,guardian_identity,driver_identity,endpoint FROM completion_continuation_owner WHERE phase='running'", [], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?)))
            .optional().map_err(|e| e.to_string())?.map(|(domain_id,supervisor_authority_id,owner_generation,guardian,driver,endpoint)| Ok(CompletionDomainOwner {
                protocol: PROTOCOL.into(), domain_id, supervisor_authority_id, owner_generation,
                guardian_identity: serde_json::from_str(&guardian).map_err(|e| e.to_string())?,
                driver_identity: serde_json::from_str(&driver).map_err(|e| e.to_string())?, endpoint,
            })).transpose()
    }

    /// The sidecar continuity head is the exact durable cursor for State repair.
    /// Recovery reads only the append-only suffix after this ordinal; accepted
    /// historical bindings before it are not part of the hot working set.
    pub fn completion_continuity_repair_ordinal(&self) -> Result<i64, String> {
        Ok(completion_continuity_head_on(&self.conn)?.map_or(0, |head| head.authority_ordinal))
    }

    /// Return only source images that still lack an accepted completion.  The
    /// partial index added with sidecar v21 keeps terminal source history out of
    /// this bounded recovery selection.
    pub fn unaccepted_completion_continuations(
        &self,
        supervisor_authority_id: &str,
        limit: usize,
    ) -> Result<Vec<AdmittedSourceBinding>, String> {
        let limit = i64::try_from(limit).map_err(|_| "completion recovery limit overflow")?;
        let mut statement = self
            .conn
            .prepare(
                "WITH RECURSIVE supervisor_scope(authority_id) AS (
                    SELECT ?1
                    UNION
                    SELECT inheritance.predecessor_authority_id
                    FROM completion_supervisor_inheritance inheritance
                    JOIN supervisor_scope scope
                      ON inheritance.authority_id=scope.authority_id)
                 SELECT binding FROM completion_continuation_source
                 WHERE supervisor_authority_id IN (
                    SELECT authority_id FROM supervisor_scope)
                   AND phase='registered'
                 ORDER BY registration_id LIMIT ?2",
            )
            .map_err(|error| error.to_string())?;
        statement
            .query_map(params![supervisor_authority_id, limit], |row| {
                row.get::<_, Vec<u8>>(0)
            })
            .map_err(|error| error.to_string())?
            .map(|row| AdmittedSourceBinding::decode(&row.map_err(|error| error.to_string())?))
            .collect()
    }

    pub(crate) fn close_idle_continuation_generation(
        &mut self,
        owner: &CompletionDomainOwner,
        state_head: Option<&CompletionContinuityHead>,
    ) -> Result<bool, String> {
        // This connection belongs to this retirement attempt only. While State's
        // writer is held, sidecar contention is a refusal to retire, not a wait.
        // The caller drops State and the guardian retries from fresh premises.
        self.conn
            .busy_timeout(std::time::Duration::ZERO)
            .map_err(|e| e.to_string())?;
        let tx = match self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
        {
            Ok(tx) => tx,
            Err(e) if sqlite_error_is_contention(&e) => return Ok(false),
            Err(e) => return Err(e.to_string()),
        };
        if domain_on(&tx)?.as_deref() != Some(&owner.domain_id) {
            return Err("completion retirement domain conflict".into());
        }
        // The append-only heads are the exact durable statement that every
        // State admission through this point was projected. Do not decode all
        // successful historical bindings while both databases are writer-fenced.
        if completion_continuity_head_on(&tx)?.as_ref() != state_head {
            return Ok(false);
        }
        if let Some(head) = state_head
            && sidecar_generation_on(&tx)? != head.sidecar_generation
        {
            return Ok(false);
        }
        let pending_sql = format!("WITH RECURSIVE supervisor_scope(authority_id) AS (
SELECT ?1 UNION SELECT inheritance.predecessor_authority_id
FROM completion_supervisor_inheritance inheritance JOIN supervisor_scope scope
ON inheritance.authority_id=scope.authority_id)
SELECT EXISTS(SELECT 1 FROM completion_continuation_source INDEXED BY completion_continuation_source_unaccepted
WHERE supervisor_authority_id IN (SELECT authority_id FROM supervisor_scope) AND phase='registered') OR EXISTS(SELECT 1 FROM completion_event_listener INDEXED BY idx_completion_event_listener_retirement_pending WHERE retirement_pending=1) OR EXISTS(SELECT 1 FROM mailbox INDEXED BY idx_mailbox_deliverable_global WHERE delivered_at IS NULL AND {}) OR EXISTS(SELECT 1 FROM completion_continuation_attempt INDEXED BY completion_continuation_attempt_unresolved WHERE supervisor_authority_id IN (SELECT authority_id FROM supervisor_scope) AND phase NOT IN ('drained','never_started'))", super::DELIVERABLE_MAILBOX_ERROR_PREDICATE);
        let pending: bool = tx
            .query_row(&pending_sql, [&owner.supervisor_authority_id], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if pending {
            return Ok(false);
        }
        let changed=tx.execute("UPDATE completion_continuation_owner SET phase='closing' WHERE generation=?1 AND domain_id=?2 AND phase='running' AND guardian_identity=?3 AND driver_identity=?4",params![owner.owner_generation,owner.domain_id,serde_json::to_string(&owner.guardian_identity).map_err(|e|e.to_string())?,serde_json::to_string(&owner.driver_identity).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
        if changed == 1 {
            tx.execute(
                "UPDATE completion_supervisor_authority SET phase='retired'
                 WHERE authority_id=?1 AND domain_id=?2",
                params![owner.supervisor_authority_id, owner.domain_id],
            )
            .map_err(|e| e.to_string())?;
        }
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
        let authority = uuid::Uuid::parse_str(&owner.supervisor_authority_id)
            .map_err(|_| "completion supervisor authority is not a UUID")?;
        if authority.to_string() != owner.supervisor_authority_id {
            return Err("completion supervisor authority is not canonical".into());
        }
        let encoded_guardian =
            serde_json::to_string(&owner.guardian_identity).map_err(|e| e.to_string())?;
        let current_root: Option<(String, String)> = tx
            .query_row(
                "SELECT supervisor_authority_id,guardian_identity
                 FROM completion_continuation_owner WHERE phase='running'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if current_root.as_ref().is_some_and(|(authority, guardian)| {
            authority == &owner.supervisor_authority_id && guardian != &encoded_guardian
        }) {
            return Err("completion supervisor authority cannot move to a different root".into());
        }
        let continues_root = current_root.as_ref().is_some_and(|(authority, guardian)| {
            authority == &owner.supervisor_authority_id && guardian == &encoded_guardian
        });
        tx.execute(
            "INSERT OR IGNORE INTO completion_supervisor_authority(
                authority_id,domain_id,phase,created_by_generation,guardian_identity)
             VALUES(?1,?2,'active',?3,?4)",
            params![
                owner.supervisor_authority_id,
                owner.domain_id,
                owner.owner_generation,
                encoded_guardian
            ],
        )
        .map_err(|e| e.to_string())?;
        let (authority_domain, authority_guardian): (String, Option<String>) = tx
            .query_row(
                "SELECT domain_id,guardian_identity FROM completion_supervisor_authority
                 WHERE authority_id=?1",
                [&owner.supervisor_authority_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| e.to_string())?;
        if authority_domain != owner.domain_id {
            return Err("completion supervisor authority domain conflict".into());
        }
        if authority_guardian.as_deref() != Some(encoded_guardian.as_str()) {
            return Err("completion supervisor authority cannot move to a different root".into());
        }
        tx.execute(
            "UPDATE completion_supervisor_authority SET phase='active'
             WHERE authority_id=?1",
            [&owner.supervisor_authority_id],
        )
        .map_err(|e| e.to_string())?;
        // Adoption is explicit and durable. Only roots that still own unresolved
        // attempts or unaccepted sources enter this tree; terminal predecessors
        // never become part of its recovery scope.
        if !continues_root {
            tx.execute(
                "WITH unresolved(authority_id) AS (
                SELECT supervisor_authority_id
                FROM completion_continuation_attempt
                WHERE domain_id=?2 AND phase NOT IN ('drained','never_started')
                UNION
                SELECT supervisor_authority_id
                FROM completion_continuation_source
                WHERE domain_id=?2 AND phase='registered')
             INSERT OR IGNORE INTO completion_supervisor_inheritance(
                authority_id,predecessor_authority_id,inherited_by_generation)
             SELECT ?1,authority_id,?3 FROM unresolved WHERE authority_id!=?1",
                params![
                    owner.supervisor_authority_id,
                    owner.domain_id,
                    owner.owner_generation
                ],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.execute(
            "UPDATE completion_continuation_owner SET phase='lost' WHERE domain_id=?1 AND phase='running'",
            [&owner.domain_id],
        )
        .map_err(|e| e.to_string())?;
        if !continues_root {
            tx.execute(
                "UPDATE completion_supervisor_authority SET phase='retired'
             WHERE domain_id=?1 AND authority_id!=?2 AND phase='active'",
                params![owner.domain_id, owner.supervisor_authority_id],
            )
            .map_err(|e| e.to_string())?;
        }
        // Reserved is committed before acceptance, and acceptance before any fork.
        // Replacement can revoke that unspent authority, but never accepted debt.
        // Capture only the exact claims in the unresolved working set. The
        // v18 physical-custody trigger correctly forbids deleting a claim until
        // its attempt is terminal, so revoke attempts first and delete these
        // primary-key claims immediately afterward in the same transaction.
        let revoked_claims: Vec<(String, String)> = {
            let mut statement = tx
                .prepare(
                    "WITH RECURSIVE supervisor_scope(authority_id) AS (
SELECT ?1 UNION SELECT inheritance.predecessor_authority_id
FROM completion_supervisor_inheritance inheritance JOIN supervisor_scope scope
ON inheritance.authority_id=scope.authority_id)
SELECT session_id,claim_token FROM completion_continuation_attempt
WHERE supervisor_authority_id IN (SELECT authority_id FROM supervisor_scope)
AND phase NOT IN ('drained','never_started') AND operation='activation'
AND phase='reserved' AND revision=1 AND custodian_identity IS NULL",
                )
                .map_err(|e| e.to_string())?;
            statement
                .query_map([&owner.supervisor_authority_id], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .map_err(|e| e.to_string())?
                .map(|row| row.map_err(|e| e.to_string()))
                .collect::<Result<_, _>>()?
        };
        tx.execute("WITH RECURSIVE supervisor_scope(authority_id) AS (
SELECT ?1 UNION SELECT inheritance.predecessor_authority_id
FROM completion_supervisor_inheritance inheritance JOIN supervisor_scope scope
ON inheritance.authority_id=scope.authority_id)
UPDATE completion_continuation_attempt SET phase='never_started',revision=revision+1,integrated=1,drain_receipt='replacement_revoked_unaccepted' WHERE supervisor_authority_id IN (SELECT authority_id FROM supervisor_scope) AND phase NOT IN ('drained','never_started') AND phase='reserved' AND revision=1 AND custodian_identity IS NULL",[&owner.supervisor_authority_id]).map_err(|e|e.to_string())?;
        for (session_id, claim_token) in revoked_claims {
            tx.execute(
                "DELETE FROM session_wake_claim WHERE session_id=?1 AND claim_token=?2",
                params![session_id, claim_token],
            )
            .map_err(|e| e.to_string())?;
        }
        if !continues_root {
            tx.execute("WITH RECURSIVE supervisor_scope(authority_id) AS (
SELECT ?1 UNION SELECT inheritance.predecessor_authority_id
FROM completion_supervisor_inheritance inheritance JOIN supervisor_scope scope
ON inheritance.authority_id=scope.authority_id)
UPDATE completion_continuation_attempt SET phase='unknown_custody',revision=revision+1 WHERE supervisor_authority_id IN (SELECT authority_id FROM supervisor_scope) AND phase NOT IN ('drained','never_started') AND phase IN ('accepted','starting','running')", [&owner.supervisor_authority_id]).map_err(|e| e.to_string())?;
        }
        tx.execute(
            "INSERT INTO completion_continuation_owner(
                generation,domain_id,phase,guardian_identity,driver_identity,
                endpoint,supervisor_authority_id)
             VALUES(?1,?2,'running',?3,?4,?5,?6)",
            params![
                owner.owner_generation,
                owner.domain_id,
                encoded_guardian,
                serde_json::to_string(&owner.driver_identity).map_err(|e| e.to_string())?,
                owner.endpoint,
                owner.supervisor_authority_id
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
}

pub(in crate::mailbox) fn domain_on(conn: &Connection) -> Result<Option<String>, String> {
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
        let mut statement = conn
            .prepare(
                "SELECT type,name,sql FROM sqlite_master
                 WHERE sql IS NOT NULL AND (
                    name LIKE 'completion_continuation_%'
                    OR name LIKE 'completion_supervisor_%'
                    OR name LIKE 'completion_owner_supervisor_%'
                    OR name LIKE 'completion_source_supervisor_%'
                    OR name LIKE 'completion_attempt_supervisor_%')
                 ORDER BY type,name",
            )
            .map_err(|e| e.to_string())?;
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
                .execute_batch("CREATE TABLE session_wake_claim(session_id TEXT,claim_token TEXT); CREATE TABLE completion_event_listener(event_id TEXT,listener_id TEXT,acknowledged_at TEXT,acknowledgement_reason TEXT,mailbox_seq INTEGER,PRIMARY KEY(event_id,listener_id));")
                .map_err(|e| e.to_string())?;
            expected
                .execute_batch(include_str!(
                    "../migrations/0018_completion_continuation.sql"
                ))
                .map_err(|e| e.to_string())?;
            expected.execute_batch(include_str!("../migrations/0019_notification_settlement.sql"))
                .map_err(|e| e.to_string())?;
            expected
                .execute_batch(include_str!(
                    "../migrations/0021_completion_recovery_working_set.sql"
                ))
                .map_err(|e| e.to_string())?;
            expected
                .execute_batch(include_str!(
                    "../migrations/0022_completion_native_runtime.sql"
                ))
                .map_err(|e| e.to_string())?;
            definitions(&expected)
        })
        .as_ref()
        .map_err(Clone::clone)?;
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if version != super::schema::CURRENT_VERSION || definitions(conn)? != *expected {
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
            supervisor_authority_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: uuid::Uuid::new_v4().to_string(),
            guardian_identity: identity.clone(),
            driver_identity: identity,
            endpoint: "/fixture/owner.sock".into(),
        };
        db.publish_completion_continuation_owner(&owner).unwrap();
        (dir, db, owner)
    }

    fn explain(db: &MailboxDb, sql: &str, supervisor_authority_id: &str) -> Vec<String> {
        let mut statement = db.conn.prepare(sql).unwrap();
        if statement.parameter_count() == 1 {
            statement
                .query_map([supervisor_authority_id], |row| row.get::<_, String>(3))
                .unwrap()
                .map(|row| row.unwrap())
                .collect()
        } else {
            statement
                .query_map(params![supervisor_authority_id, 16_i64], |row| {
                    row.get::<_, String>(3)
                })
                .unwrap()
                .map(|row| row.unwrap())
                .collect()
        }
    }

    #[test]
    fn completion_recovery_queries_exclude_terminal_history_by_construction() {
        let (_dir, mut db, owner) = fixture();
        let tx = db.conn.transaction().unwrap();
        {
            let mut insert = tx
                .prepare(
                    "INSERT INTO completion_continuation_attempt(
                        attempt_id,domain_id,owner_generation,operation,request_sha256,
                        phase,result_path,integrated,drain_receipt,supervisor_authority_id)
                     VALUES(?1,?2,?3,'transport',?4,'never_started',?5,1,'fixture',?6)",
                )
                .unwrap();
            for ordinal in 0..5_000 {
                insert
                    .execute(params![
                        format!("terminal-{ordinal:05}"),
                        owner.domain_id,
                        owner.owner_generation,
                        "a".repeat(64),
                        format!("/fixture/terminal-{ordinal:05}.json"),
                        owner.supervisor_authority_id
                    ])
                    .unwrap();
            }
        }
        tx.commit().unwrap();

        let unresolved = reservation(&mut db, &owner);
        assert_eq!(
            db.pending_continuation_attempts_for_supervisor(&owner.supervisor_authority_id, 1,)
                .unwrap(),
            vec![unresolved]
        );

        let attempts = explain(
            &db,
            "EXPLAIN QUERY PLAN WITH RECURSIVE supervisor_scope(authority_id) AS (
                SELECT ?1 UNION SELECT inheritance.predecessor_authority_id
                FROM completion_supervisor_inheritance inheritance
                JOIN supervisor_scope scope ON inheritance.authority_id=scope.authority_id)
             SELECT attempt_id,owner_generation,operation,request_sha256,
                source_registration_id,source_listener_revision,session_id,claim_token,result_path
             FROM completion_continuation_attempt
             WHERE supervisor_authority_id IN (SELECT authority_id FROM supervisor_scope)
               AND phase NOT IN ('drained','never_started')
             ORDER BY owner_generation,attempt_id LIMIT ?2",
            &owner.supervisor_authority_id,
        );
        assert!(
            attempts
                .iter()
                .any(|detail| detail.contains("completion_continuation_attempt_unresolved")),
            "unresolved recovery must not scan terminal history: {attempts:?}"
        );

        let sources = explain(
            &db,
            "EXPLAIN QUERY PLAN WITH RECURSIVE supervisor_scope(authority_id) AS (
                SELECT ?1 UNION SELECT inheritance.predecessor_authority_id
                FROM completion_supervisor_inheritance inheritance
                JOIN supervisor_scope scope ON inheritance.authority_id=scope.authority_id)
             SELECT binding FROM completion_continuation_source
             WHERE supervisor_authority_id IN (SELECT authority_id FROM supervisor_scope)
               AND phase='registered'
             ORDER BY registration_id LIMIT ?2",
            &owner.supervisor_authority_id,
        );
        assert!(
            sources
                .iter()
                .any(|detail| detail.contains("completion_continuation_source_unaccepted")),
            "source recovery must not scan accepted history: {sources:?}"
        );

        let unspent = explain(
            &db,
            "EXPLAIN QUERY PLAN WITH RECURSIVE supervisor_scope(authority_id) AS (
                SELECT ?1 UNION SELECT inheritance.predecessor_authority_id
                FROM completion_supervisor_inheritance inheritance
                JOIN supervisor_scope scope ON inheritance.authority_id=scope.authority_id)
             UPDATE completion_continuation_attempt
             SET phase='never_started',revision=revision+1,integrated=1,
                 drain_receipt='replacement_revoked_unaccepted'
             WHERE supervisor_authority_id IN (SELECT authority_id FROM supervisor_scope)
               AND phase NOT IN ('drained','never_started')
               AND phase='reserved' AND revision=1
               AND custodian_identity IS NULL",
            &owner.supervisor_authority_id,
        );
        assert!(
            unspent
                .iter()
                .any(|detail| detail.contains("completion_continuation_attempt_unresolved")),
            "owner replacement must not scan settled attempts: {unspent:?}"
        );

        let native_runtime = db
            .conn
            .prepare(
                "EXPLAIN QUERY PLAN SELECT EXISTS(
                    SELECT 1 FROM completion_continuation_attempt
                        INDEXED BY completion_continuation_attempt_native_runtime
                    WHERE operation='activation' AND domain_id=?1
                      AND runtime_generation_uuid=?2 AND spawn_invocation_uuid=?3
                 )",
            )
            .unwrap()
            .query_map(
                params![owner.domain_id, "runtime-generation", "invocation"],
                |row| row.get::<_, String>(3),
            )
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            native_runtime.iter().any(|detail| {
                detail.contains("completion_continuation_attempt_native_runtime")
            }),
            "native runtime identity entered attempt history: {native_runtime:?}"
        );
    }

    #[test]
    fn new_root_inherits_only_unresolved_predecessor_authority() {
        let (_dir, mut db, first) = fixture();
        let attempt = reservation(&mut db, &first);
        db.accept_continuation_attempt(&attempt).unwrap();
        db.attach_continuation_custodian(&attempt, &first.driver_identity)
            .unwrap();

        let second = CompletionDomainOwner {
            supervisor_authority_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: uuid::Uuid::new_v4().to_string(),
            ..first.clone()
        };
        db.publish_completion_continuation_owner(&second).unwrap();
        assert_eq!(
            db.pending_continuation_attempts_for_supervisor(&second.supervisor_authority_id, 16,)
                .unwrap(),
            vec![attempt.clone()]
        );
        let inherited: bool = db
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM completion_supervisor_inheritance
                 WHERE authority_id=?1 AND predecessor_authority_id=?2)",
                params![
                    second.supervisor_authority_id,
                    first.supervisor_authority_id
                ],
                |row| row.get(0),
            )
            .unwrap();
        assert!(inherited);

        db.discharge_continuation_attempt(&attempt, &first.driver_identity, "fixture-drain")
            .unwrap();
        let third = CompletionDomainOwner {
            supervisor_authority_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: uuid::Uuid::new_v4().to_string(),
            ..second.clone()
        };
        db.publish_completion_continuation_owner(&third).unwrap();
        assert!(
            db.pending_continuation_attempts_for_supervisor(&third.supervisor_authority_id, 16,)
                .unwrap()
                .is_empty()
        );
        let inherited_count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM completion_supervisor_inheritance
                 WHERE authority_id=?1",
                [&third.supervisor_authority_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(inherited_count, 0, "settled history is not adopted");
    }

    #[test]
    fn driver_succession_keeps_one_root_authority_and_known_worker_custody() {
        let (_dir, mut db, first) = fixture();
        let attempt = reservation(&mut db, &first);
        db.accept_continuation_attempt(&attempt).unwrap();
        db.attach_continuation_custodian_with_adopter(
            &attempt,
            &first.driver_identity,
            Some(&first.guardian_identity),
        )
        .unwrap();
        db.advance_continuation_attempt(
            &attempt,
            3,
            "accepted",
            "starting",
            &first.driver_identity,
        )
        .unwrap();

        let successor = CompletionDomainOwner {
            owner_generation: uuid::Uuid::new_v4().to_string(),
            endpoint: "/fixture/replacement.sock".into(),
            ..first.clone()
        };
        db.publish_completion_continuation_owner(&successor)
            .unwrap();

        let (phase, supervisor): (String, String) = db
            .conn
            .query_row(
                "SELECT phase,supervisor_authority_id
                 FROM completion_continuation_attempt WHERE attempt_id=?1",
                [&attempt.attempt_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(phase, "starting");
        assert_eq!(supervisor, first.supervisor_authority_id);
        assert_eq!(
            db.conn
                .query_row(
                    "SELECT COUNT(*) FROM completion_supervisor_inheritance
                     WHERE authority_id=?1",
                    [&first.supervisor_authority_id],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0,
            "driver succession must not inherit or supersede its own root"
        );
        assert_eq!(
            db.completion_continuation_owner().unwrap().unwrap(),
            successor
        );
    }

    #[test]
    fn supervisor_authority_cannot_move_to_another_guardian_incarnation() {
        let (_dir, mut db, first) = fixture();
        let mut foreign = first.clone();
        foreign.owner_generation = uuid::Uuid::new_v4().to_string();
        foreign.guardian_identity.starttime_ticks += 1;
        assert!(
            db.publish_completion_continuation_owner(&foreign)
                .unwrap_err()
                .contains("cannot move to a different root")
        );

        db.conn
            .execute(
                "UPDATE completion_continuation_owner SET phase='lost'
                 WHERE generation=?1",
                [&first.owner_generation],
            )
            .unwrap();
        assert!(
            db.publish_completion_continuation_owner(&foreign)
                .unwrap_err()
                .contains("cannot move to a different root"),
            "retiring the live owner must not make its durable authority reusable"
        );
    }

    #[test]
    fn unresolved_supervisor_walk_is_bounded_and_reaches_later_rows() {
        let (_dir, db, owner) = fixture();
        for ordinal in 0..5 {
            db.conn
                .execute(
                    "INSERT INTO completion_continuation_attempt(
                        attempt_id,domain_id,owner_generation,operation,request_sha256,
                        phase,result_path,supervisor_authority_id)
                     VALUES(?1,?2,?3,'transport',?4,'unknown_custody',?5,?6)",
                    params![
                        format!("page-{ordinal}"),
                        owner.domain_id,
                        owner.owner_generation,
                        "a".repeat(64),
                        format!("/fixture/page-{ordinal}.json"),
                        owner.supervisor_authority_id
                    ],
                )
                .unwrap();
        }

        let first = db
            .pending_continuation_attempts_for_supervisor_after(
                &owner.supervisor_authority_id,
                None,
                2,
            )
            .unwrap();
        let second = db
            .pending_continuation_attempts_for_supervisor_after(
                &owner.supervisor_authority_id,
                Some((
                    &first.last().unwrap().owner_generation,
                    &first.last().unwrap().attempt_id,
                )),
                2,
            )
            .unwrap();
        let third = db
            .pending_continuation_attempts_for_supervisor_after(
                &owner.supervisor_authority_id,
                Some((
                    &second.last().unwrap().owner_generation,
                    &second.last().unwrap().attempt_id,
                )),
                2,
            )
            .unwrap();
        let observed = first
            .into_iter()
            .chain(second)
            .chain(third)
            .map(|attempt| attempt.attempt_id)
            .collect::<Vec<_>>();
        assert_eq!(observed, ["page-0", "page-1", "page-2", "page-3", "page-4"]);
    }
    #[test]
    fn completion_continuation_unknown_listener_is_not_a_response_only_waiver() {
        let (_dir, mut db, owner) = fixture();
        db.register_completion_event(CompletionEventRegistrationInput {
            event_id: "legacy-unknown",
            delivery_mode: "sync",
            owner_session_id: Some("session"),
            owner_invocation_uuid: Some("owner"),
            state_dir: "/offline",
            meta_path: "/offline/meta",
            log_path: "/offline/log",
            rc_path: "/offline/rc",
        })
        .unwrap();
        assert_eq!(
            db.completion_notification_diagnostics("legacy-unknown")
                .unwrap()[0]["disposition"],
            "unknown"
        );
        assert!(!db.close_idle_continuation_generation(&owner, None).unwrap());
        assert!(
            db.completion_event_listeners("legacy-unknown").unwrap()[0]
                .acknowledged_at
                .is_none()
        );
    }

    #[test]
    fn idle_close_ignores_large_explicitly_retired_listener_history() {
        let (_dir, mut db, owner) = fixture();
        db.conn
            .execute(
                "INSERT INTO completion_event(
                    event_id,kind,state,delivery_mode,state_dir,meta_path,log_path,rc_path,created_at)
                 VALUES('retired-history','agent_bash_complete','pending','sync',
                    '/fixture','/fixture/meta','/fixture/log','/fixture/rc',?1)",
                [now_rfc3339()],
            )
            .unwrap();
        let tx = db.conn.transaction().unwrap();
        {
            let mut insert = tx
                .prepare(
                    "INSERT INTO completion_event_listener(
                        event_id,listener_id,session_id,owner_invocation_uuid,
                        active,created_at,retirement_pending)
                     VALUES('retired-history',?1,?2,?1,0,?3,0)",
                )
                .unwrap();
            let created_at = now_rfc3339();
            for ordinal in 0..5_000 {
                insert
                    .execute(params![
                        format!("retired-listener-{ordinal:05}"),
                        format!("retired-session-{ordinal:05}"),
                        created_at
                    ])
                    .unwrap();
            }
        }
        tx.commit().unwrap();

        let plan = db
            .conn
            .prepare(
                "EXPLAIN QUERY PLAN SELECT EXISTS(
                    SELECT 1 FROM completion_event_listener
                         INDEXED BY idx_completion_event_listener_retirement_pending
                    WHERE retirement_pending=1)",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            plan.iter().any(|detail| {
                detail.contains("idx_completion_event_listener_retirement_pending")
            }),
            "idle-close listener probe escaped its live projection: {plan:?}"
        );
        assert!(db.close_idle_continuation_generation(&owner, None).unwrap());
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
    fn original_birth_attachment_retry_closes_current_generation_start_authority() {
        let (_dir, mut db, owner) = fixture();
        let attempt = reservation(&mut db, &owner);
        db.accept_continuation_attempt(&attempt).unwrap();
        let driver = &owner.driver_identity;
        db.attach_original_continuation_custody(&attempt, driver, driver, driver)
            .unwrap();
        assert!(
            db.advance_continuation_attempt(&attempt, 3, "accepted", "starting", driver)
                .is_err()
        );
        let phase: String = db
            .conn
            .query_row(
                "SELECT phase FROM completion_continuation_attempt WHERE attempt_id=?1",
                [&attempt.attempt_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(phase, "unknown_custody");
    }

    #[test]
    fn original_birth_attachment_survives_succession_without_grant_authority() {
        let (_dir, mut db, owner) = fixture();
        let attempt = reservation(&mut db, &owner);
        db.accept_continuation_attempt(&attempt).unwrap();
        let mut successor = owner.clone();
        successor.owner_generation = uuid::Uuid::new_v4().to_string();
        db.publish_completion_continuation_owner(&successor)
            .unwrap();
        let driver = &owner.driver_identity;
        let mut wrong = driver.clone();
        wrong.starttime_ticks += 1;
        assert!(
            db.attach_original_continuation_custody(&attempt, &wrong, driver, driver)
                .is_err()
        );
        let mut altered = attempt.clone();
        altered.request_sha256 = "b".repeat(64);
        assert!(
            db.attach_original_continuation_custody(&altered, driver, driver, driver)
                .is_err()
        );
        db.attach_original_continuation_custody(&attempt, driver, driver, driver)
            .unwrap();
        db.attach_original_continuation_custody(&attempt, driver, driver, driver)
            .unwrap();
        assert!(
            db.attach_original_continuation_custody(&attempt, driver, &wrong, driver)
                .is_err()
        );
        assert!(
            db.attach_original_continuation_custody(&attempt, driver, driver, &wrong)
                .is_err()
        );
        assert!(
            db.advance_continuation_attempt(&attempt, 3, "accepted", "starting", driver)
                .is_err()
        );
        let (phase, revision, integrated): (String, i64, i64) = db.conn.query_row(
            "SELECT phase,revision,integrated FROM completion_continuation_attempt WHERE attempt_id=?1", [&attempt.attempt_id], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(phase, "unknown_custody");
        // Same-root driver succession no longer fabricates a custody-loss
        // revision. The original legacy attachment itself still closes grant
        // authority and records the one unknown-custody transition.
        assert_eq!(revision, 3);
        assert_eq!(integrated, 0);
    }

    #[test]
    fn completion_continuation_legacy_probe_is_nonmutating() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let db = MailboxDb::open(&path).unwrap();
        super::super::schema::remove_continuation_schema_for_legacy_fixture(&db.conn);
        db.conn.pragma_update(None, "user_version", 17).unwrap();
        drop(db);
        let before = std::fs::read(&path).unwrap();
        let probe = MailboxDb::open_read_only(&path).unwrap();
        assert_eq!(probe.completion_continuation_domain().unwrap(), None);
        assert_eq!(
            probe
                .conn
                .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            17
        );
        drop(probe);
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn completion_continuation_populated_v17_upgrade_preserves_legacy_claim_and_domain_on_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let db = MailboxDb::open(&path).unwrap();
        super::super::schema::remove_continuation_schema_for_legacy_fixture(&db.conn);
        db.conn.pragma_update(None, "user_version", 17).unwrap();
        db.conn.execute(
            "INSERT INTO session_wake_claim(session_id,claim_token,claimed_at,reason,auto_wake_count) VALUES('legacy-session','legacy-token','2026-09-12T00:00:00Z','retained-before-upgrade',3)",
            [],
        ).unwrap();
        drop(db);

        let upgraded = MailboxDb::open_completion_continuation_domain(&path).unwrap();
        let domain = upgraded.completion_continuation_domain().unwrap().unwrap();
        assert!(upgraded.completion_continuation_owner().unwrap().is_none());
        assert!(upgraded.pending_continuation_attempts().unwrap().is_empty());
        let claim: (String, String, i64) = upgraded.conn.query_row(
            "SELECT claim_token,reason,auto_wake_count FROM session_wake_claim WHERE session_id='legacy-session'",
            [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).unwrap();
        assert_eq!(
            claim,
            ("legacy-token".into(), "retained-before-upgrade".into(), 3)
        );
        assert_eq!(
            upgraded
                .conn
                .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            super::super::schema::CURRENT_VERSION
        );
        drop(upgraded);

        let reopened = MailboxDb::open_completion_continuation_domain(&path).unwrap();
        assert_eq!(
            reopened.completion_continuation_domain().unwrap(),
            Some(domain)
        );
        assert!(
            reopened
                .wake_session_reader()
                .wake_claim("legacy-session")
                .unwrap()
                .is_some()
        );
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
    fn legacy_claim_without_v2_attempt_is_retained_on_launcher_refusal() {
        // Final-system custody boundary, not a migration test: neither a current
        // domain nor a live owner can manufacture an old launcher's v2 attempt.
        let (_dir, mut db, owner) = fixture();
        db.conn.execute("INSERT INTO session_wake_claim(session_id,claim_token,claimed_at,reason,auto_wake_count) VALUES('legacy-session','legacy-token','2026-09-13T00:00:00Z','fixture',1)", []).unwrap();
        let child = ProcessIdentity {
            os_pid: owner.driver_identity.pid,
            os_boot_id: owner.driver_identity.boot_id,
            os_pid_starttime_ticks: owner.driver_identity.starttime_ticks,
        };
        let tx = db.conn.transaction().unwrap();
        let error = admit_launcher_on(&tx, "legacy-session", "legacy-token", &child).unwrap_err();
        assert!(
            error.contains("unsupported_legacy_activation_recovery"),
            "{error}"
        );
        tx.commit().unwrap();
        assert!(db.pending_continuation_attempts().unwrap().is_empty());
        assert!(
            db.wake_session_reader()
                .wake_claim("legacy-session")
                .unwrap()
                .is_some()
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
    fn completion_supervisor_schema_fingerprint_rejects_missing_authority_fence() {
        let (_dir, db, _owner) = fixture();
        db.conn
            .execute_batch("DROP TRIGGER completion_attempt_supervisor_authority_insert")
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
            !db.close_idle_continuation_generation(&replacement, None)
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
            db.close_idle_continuation_generation(&replacement, None)
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
        // Act1: reserved activation revocation must atomically retire its claim.
        assert!(
            db.wake_session_reader()
                .wake_claim("session")
                .unwrap()
                .is_none()
        );
        let accepted = reservation(&mut db, &owner);
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
    fn storage_accepted_revocation_rejects_without_requesting_a_writer() {
        let (_dir, mut db, owner) = fixture();
        let attempt = reservation(&mut db, &owner);
        db.accept_continuation_attempt(&attempt).unwrap();
        let blocker = Connection::open(db.path()).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        for _ in 0..2 {
            let error = db
                .revoke_unaccepted_continuation_attempt(&attempt)
                .unwrap_err();
            assert_eq!(error, "reservation no longer unaccepted under this driver");
            assert!(
                db.wake_session_reader()
                    .wake_claim("session")
                    .unwrap()
                    .is_some()
            );
        }
        blocker.execute_batch("ROLLBACK").unwrap();
        assert_eq!(db.pending_continuation_attempts().unwrap(), vec![attempt]);
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
    fn root_supervisor_can_settle_only_its_exact_unforked_attempt() {
        let (_dir, mut db, owner) = fixture();
        let attempt = reservation(&mut db, &owner);
        db.accept_continuation_attempt(&attempt).unwrap();
        let mut wrong = owner.guardian_identity.clone();
        wrong.starttime_ticks += 1;
        assert!(
            db.record_supervisor_never_forked(&attempt, &wrong, "worker spawn failed")
                .is_err()
        );
        db.record_supervisor_never_forked(
            &attempt,
            &owner.guardian_identity,
            "worker spawn failed",
        )
        .unwrap();
        db.record_supervisor_never_forked(
            &attempt,
            &owner.guardian_identity,
            "worker spawn failed",
        )
        .unwrap();
        assert!(
            db.record_supervisor_never_forked(
                &attempt,
                &owner.guardian_identity,
                "different failure",
            )
            .is_err()
        );
        let receipt: String = db
            .conn
            .query_row(
                "SELECT drain_receipt FROM completion_continuation_attempt WHERE attempt_id=?1",
                [&attempt.attempt_id],
                |row| row.get(0),
            )
            .unwrap();
        let receipt: serde_json::Value = serde_json::from_str(&receipt).unwrap();
        assert_eq!(receipt["gate"], "root_worker_not_forked");
        assert_eq!(
            receipt["supervisor"],
            serde_json::json!(owner.guardian_identity)
        );
        assert!(
            db.wake_session_reader()
                .wake_claim("session")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn root_supervisor_distinguishes_forked_but_unreleased_worker() {
        let (_dir, mut db, owner) = fixture();
        let attempt = reservation(&mut db, &owner);
        db.accept_continuation_attempt(&attempt).unwrap();
        db.record_supervisor_unreleased_worker(
            &attempt,
            &owner.guardian_identity,
            4242,
            "worker identity unavailable",
        )
        .unwrap();
        db.record_supervisor_unreleased_worker(
            &attempt,
            &owner.guardian_identity,
            4242,
            "worker identity unavailable",
        )
        .unwrap();
        assert!(
            db.record_supervisor_unreleased_worker(
                &attempt,
                &owner.guardian_identity,
                4243,
                "worker identity unavailable",
            )
            .is_err()
        );
        let receipt: String = db
            .conn
            .query_row(
                "SELECT drain_receipt FROM completion_continuation_attempt
                 WHERE attempt_id=?1",
                [&attempt.attempt_id],
                |row| row.get(0),
            )
            .unwrap();
        let receipt: serde_json::Value = serde_json::from_str(&receipt).unwrap();
        assert_eq!(
            receipt["gate"],
            "root_worker_forked_execution_grant_not_sent"
        );
        assert_eq!(receipt["worker_pid"], 4242);
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
