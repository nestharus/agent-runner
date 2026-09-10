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
