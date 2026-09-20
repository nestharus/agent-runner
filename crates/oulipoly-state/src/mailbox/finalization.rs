//! Retention references, not delivery authority. Exact membership/target validation
//! remains in mailbox settlement. References outlive consumer ACK, but not their
//! prepared finalizer (including unwinding) or a provably dead owner process.
use super::*;

pub struct DeliveryFinalizationGuard {
    db: MailboxDb,
    token: String,
}

impl Drop for DeliveryFinalizationGuard {
    fn drop(&mut self) {
        if let Err(error) = self.db.conn.execute(
            "DELETE FROM mailbox_delivery_finalizers WHERE token = ?1",
            params![self.token],
        ) {
            tracing::warn!(%error, "Failed to release delivery finalization reference; dead-owner maintenance will reclaim it");
        }
    }
}

impl MailboxDb {
    /// Acquire before registering an attempt, and hold until its finalization is
    /// complete or abandoned. This pins history only, not submission or settlement.
    pub fn retain_delivery_finalization(
        &self,
        attempt_id: &str,
    ) -> Result<DeliveryFinalizationGuard, String> {
        if attempt_id.trim().is_empty() {
            return Err("delivery finalization requires an attempt identity".into());
        }
        let identity = current_runtime_creator_identity().map_err(|err| err.to_string())?;
        // Keep the normal canonical namespace authority as long as the reference.
        let db = MailboxDb::open(&self.path)?;
        let token = uuid::Uuid::new_v4().to_string();
        db.conn.execute(
            "INSERT INTO mailbox_delivery_finalizers
             (token, attempt_id, os_pid, os_boot_id, os_pid_starttime_ticks, checked_order)
             VALUES (?1, ?2, ?3, ?4, ?5,
                     (SELECT COALESCE(MAX(checked_order), 0) + 1 FROM mailbox_delivery_finalizers))",
            params![token, attempt_id, identity.os_pid, identity.os_boot_id,
                    identity.os_pid_starttime_ticks],
        ).map_err(|err| format!("Failed to retain delivery finalization: {err}"))?;
        Ok(DeliveryFinalizationGuard { db, token })
    }
}

pub(super) fn reap_abandoned_finalizers(conn: &mut Connection, limit: i64) -> Result<(), String> {
    reap_abandoned_finalizers_with(
        conn,
        limit,
        pid_identity::observe_finalizer_process_identity,
    )
}

pub(super) fn reap_abandoned_finalizers_with(
    conn: &mut Connection,
    limit: i64,
    observe: impl Fn(i64) -> pid_identity::FinalizerProcessIdentityObservation,
) -> Result<(), String> {
    // Rotate live/uncertain owners to the tail so they cannot starve crashed
    // owners. Work is bounded per maintenance call, not a lease expiry that could
    // erase evidence underneath a slow but live finalizer.
    let owners = {
        let mut stmt = conn
            .prepare(
                "SELECT token, os_pid, os_boot_id, os_pid_starttime_ticks
             FROM mailbox_delivery_finalizers ORDER BY checked_order, token LIMIT ?1",
            )
            .map_err(|err| err.to_string())?;
        stmt.query_map(params![limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                ProcessIdentity {
                    os_pid: row.get(1)?,
                    os_boot_id: row.get(2)?,
                    os_pid_starttime_ticks: row.get(3)?,
                },
            ))
        })
        .map_err(|err| err.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| err.to_string())?
    };
    let observations = owners
        .into_iter()
        .map(|(token, owner)| {
            let dead = match observe(owner.os_pid) {
                pid_identity::FinalizerProcessIdentityObservation::Dead
                | pid_identity::FinalizerProcessIdentityObservation::ExactExited(_) => true,
                pid_identity::FinalizerProcessIdentityObservation::ExactLive(live) => live != owner,
                _ => false, // uncertain is not authority to discard evidence
            };
            (token, dead)
        })
        .collect::<Vec<_>>();
    if observations.is_empty() {
        return Ok(());
    }
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|err| err.to_string())?;
    for (token, dead) in observations {
        if dead {
            tx.execute(
                "DELETE FROM mailbox_delivery_finalizers WHERE token = ?1",
                params![token],
            )
        } else {
            tx.execute(
                "UPDATE mailbox_delivery_finalizers SET checked_order =
                (SELECT COALESCE(MAX(checked_order), 0) + 1 FROM mailbox_delivery_finalizers)
                WHERE token = ?1",
                params![token],
            )
        }
        .map_err(|err| format!("Failed to maintain finalizer references: {err}"))?;
    }
    tx.commit().map_err(|err| err.to_string())
}

impl MailboxDb {
    /// Durable history pin; not an ACK and not authority to launch or settle.
    pub fn retain_completed_delivery(
        &self,
        invocation_uuid: &str,
        attempt: &str,
        session: &str,
        chain: &str,
        seqs: &[i64],
    ) -> Result<(), String> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        exact_delivery_attempt_pending_on(&tx, attempt, session, Some(chain), seqs)?;
        let bound: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM mailbox_delivery_attempts WHERE attempt_id=?1 AND delivery_invocation_uuid=?2)",
            params![attempt, invocation_uuid], |r|r.get(0)).map_err(|e|e.to_string())?;
        if !bound {
            return Err("completed_turn_delivery_owner_mismatch".into());
        }
        tx.execute(
            "INSERT OR IGNORE INTO mailbox_completed_turn_pins VALUES (?1,?2)",
            params![invocation_uuid, attempt],
        )
        .map_err(|e| e.to_string())?;
        let recorded: String = tx
            .query_row(
                "SELECT attempt_id FROM mailbox_completed_turn_pins WHERE invocation_uuid=?1",
                [invocation_uuid],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if recorded != attempt {
            return Err("completed_turn_history_pin_conflict".into());
        }
        tx.commit().map_err(|e| e.to_string())
    }
}

impl MailboxDb {
    /// Exact postcommit bookkeeping only: no scheduler or process launch. The
    /// marker and claim release share a transaction, making a lost response
    /// replayable without releasing a successor's claim.
    pub fn finish_completed_turn_bookkeeping(
        &mut self,
        session: &str,
        invocation: &str,
        settlement: &str,
        original_claim: Option<&str>,
        exit_code: i32,
    ) -> Result<(), String> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let recorded: Option<(String,String,Option<String>)> = tx.query_row(
            "SELECT settlement_id,session_id,claim_token FROM mailbox_completed_turn_tails WHERE invocation_uuid=?1",
            [invocation], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|e|e.to_string())?;
        if let Some((id, target, token)) = recorded {
            if id != settlement || target != session || token.as_deref() != original_claim {
                return Err("completed_turn_tail_identity_conflict".into());
            }
            return tx.commit().map_err(|e| e.to_string());
        }
        let current = wake_claim_tx(&tx, session)?;
        if current.as_ref().map(|c| c.claim_token.as_str()) != original_claim {
            // Original native drain legitimately removes its claim. Absence by
            // itself is not proof; require the exact integrated activation and
            // terminal runtime relationship, not the recovery process's env.
            let drained: bool = if current.is_none() && original_claim.is_some() {
                tx.query_row("SELECT EXISTS(SELECT 1 FROM completion_continuation_attempt a JOIN runtime_generation g ON g.generation_uuid=a.runtime_generation_uuid AND g.spawn_invocation_uuid=a.spawn_invocation_uuid WHERE a.operation='activation' AND a.session_id=?1 AND a.spawn_invocation_uuid=?2 AND a.claim_token=?3 AND a.phase='drained' AND a.integrated=1 AND a.drain_receipt IS NOT NULL AND g.lifecycle_state='exited')",
                    params![session,invocation,original_claim],|r|r.get(0)).map_err(|e|e.to_string())?
            } else {
                false
            };
            if !drained {
                return Err("completed_turn_tail_authority_conflict".into());
            }
        }
        if let Some(claim) = &current
            && claim.wake_invocation_uuid.as_deref() != Some(invocation)
        {
            return Err("completed_turn_tail_claim_owner_conflict".into());
        }
        let (generations, exited):(i64,i64) = tx.query_row(
            "SELECT count(*),COALESCE(sum(lifecycle_state='exited'),0) FROM runtime_generation WHERE session_id=?1 AND spawn_invocation_uuid=?2",
            params![session,invocation],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|e|e.to_string())?;
        if generations > 0 && (generations != 1 || exited != 1) {
            return Err("completed_turn_original_runtime_not_exited".into());
        }
        if generations == 0 {
            settle_runtime_compatibility_row(
                &tx,
                LegacyRuntimeProjectionSettlement {
                    session_id: session,
                    invocation_uuid: invocation,
                    last_exit_code: Some(exit_code),
                },
                &now_rfc3339(),
            )?;
        }
        if let Some(claim) = current {
            // Existing native trigger refuses deletion while physical custody
            // remains outstanding. This operation cannot manufacture its drain.
            let changed = tx.execute("DELETE FROM session_wake_claim WHERE session_id=?1 AND claim_token=?2 AND wake_invocation_uuid=?3",
                params![session,claim.claim_token,invocation]).map_err(|e|e.to_string())?;
            if changed != 1 {
                return Err("completed_turn_tail_claim_changed".into());
            }
        }
        tx.execute("INSERT INTO mailbox_completed_turn_tails(invocation_uuid,settlement_id,session_id,claim_token) VALUES (?1,?2,?3,?4)",
            params![invocation,settlement,session,original_claim]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod completed_tests {
    use super::*;
    #[test]
    fn exact_tail_release_replays_without_releasing_successor() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.conn.execute("INSERT INTO session_wake_claim(session_id,claim_token,claimed_at,wake_invocation_uuid,reason,auto_wake_count) VALUES ('s','old','now','original','fixture',1)",[]).unwrap();
        db.finish_completed_turn_bookkeeping("s", "original", "settled", Some("old"), 0)
            .unwrap();
        assert!(db.wake_session_reader().wake_claim("s").unwrap().is_none());
        db.conn.execute("INSERT INTO session_wake_claim(session_id,claim_token,claimed_at,wake_invocation_uuid,reason,auto_wake_count) VALUES ('s','new','now','successor','fixture',2)",[]).unwrap();
        db.finish_completed_turn_bookkeeping("s", "original", "settled", Some("old"), 0)
            .unwrap();
        assert_eq!(
            db.wake_session_reader()
                .wake_claim("s")
                .unwrap()
                .unwrap()
                .claim_token,
            "new"
        );
        assert!(
            db.finish_completed_turn_bookkeeping("s", "original", "different", Some("old"), 0)
                .is_err()
        );
    }
    #[test]
    fn absent_or_foreign_original_claim_does_not_grant_tail_authority() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        assert!(
            db.finish_completed_turn_bookkeeping("s", "original", "settled", Some("lost"), 0)
                .unwrap_err()
                .contains("authority_conflict")
        );
        db.conn.execute("INSERT INTO session_wake_claim(session_id,claim_token,claimed_at,wake_invocation_uuid,reason,auto_wake_count) VALUES ('s','old','now','foreign','fixture',1)",[]).unwrap();
        assert!(
            db.finish_completed_turn_bookkeeping("s", "original", "settled", Some("old"), 0)
                .unwrap_err()
                .contains("claim_owner_conflict")
        );
        assert_eq!(
            db.conn
                .query_row(
                    "SELECT count(*) FROM mailbox_completed_turn_tails",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }
}
