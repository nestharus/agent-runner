use super::*;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

const ATTACH_PROTOCOL: &str = NATIVE_WORKER_ATTACH_PROTOCOL;
const Q_PROTOCOL: &str = NATIVE_KERNEL_Q_PROTOCOL;

fn canonical_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value)
}

fn sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn host_identity(value: &SourceProcessIdentity) -> bool {
    value.pid > 0 && value.starttime_ticks > 0 && canonical_uuid(&value.boot_id)
}

impl MailboxDb {
    /// Record broker K's exact worker and work PID1 while its pre-exec gate is
    /// held. The caller must deliver authenticated broker evidence; State only
    /// validates the structure and its durable v28 binding. A successful attach
    /// is never evidence that K replied or that the gate was released.
    pub fn attach_broker_native_worker(
        &mut self,
        binding: &NativeGrantBinding,
        evidence: &BrokerNativeAttachEvidence,
    ) -> Result<NativeWorkerAttach, String> {
        if evidence.protocol != ATTACH_PROTOCOL
            || !evidence.gate_held_before_release
            || evidence.attempt_id != binding.attempt_id
            || evidence.grant_id != binding.grant_id
            || evidence.kernel_root_id != binding.kernel_root_id
            || evidence.work_id.is_empty()
            || evidence.work_id.len() > 256
            || evidence.work_id.contains('\0')
            || !canonical_uuid(&evidence.work_incarnation_id)
            || !canonical_uuid(&evidence.broker_incarnation_id)
            || evidence.worker_entrypoint != NATIVE_ROOT_WORKER_ENTRY
            || !sha256(&evidence.runner_image_sha256)
            || !host_identity(&evidence.worker_identity)
            || !host_identity(&evidence.pid1_identity)
            || evidence.worker_identity == evidence.pid1_identity
            || evidence.worker_identity.boot_id != evidence.pid1_identity.boot_id
            || evidence.work_pid_namespace_inode <= 0
            || !sha256(&evidence.attach_receipt_sha256)
        {
            return Err("invalid broker native attach evidence".into());
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        if attempts::read_native_grant_binding_on(&tx, &binding.attempt_id)?.as_ref()
            != Some(binding)
        {
            return Err("native attach binding conflict".into());
        }
        tx.execute(
            "INSERT INTO completion_native_worker_attach(
                attempt_id,grant_id,protocol,gate_held_before_release,kernel_root_id,
                work_id,work_incarnation_id,broker_incarnation_id,
                worker_entrypoint,runner_image_sha256,worker_identity,pid1_identity,
                work_pid_namespace_inode,attach_receipt_sha256)
             VALUES(?1,?2,?3,1,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                binding.attempt_id,
                binding.grant_id,
                evidence.protocol,
                evidence.kernel_root_id,
                evidence.work_id,
                evidence.work_incarnation_id,
                evidence.broker_incarnation_id,
                evidence.worker_entrypoint,
                evidence.runner_image_sha256,
                serde_json::to_string(&evidence.worker_identity).map_err(|e| e.to_string())?,
                serde_json::to_string(&evidence.pid1_identity).map_err(|e| e.to_string())?,
                evidence.work_pid_namespace_inode,
                evidence.attach_receipt_sha256,
            ],
        )
        .map_err(|error| error.to_string())?;
        let expected = NativeWorkerAttach {
            attempt_id: binding.attempt_id.clone(),
            grant_id: binding.grant_id.clone(),
            evidence: evidence.clone(),
        };
        if read_attach_on(&tx, &binding.attempt_id)?.as_ref() != Some(&expected) {
            return Err("native attach readback conflict".into());
        }
        tx.commit().map_err(|error| error.to_string())?;
        Ok(expected)
    }

    /// Indexed exact-attempt readback for a lost attach response. Presence
    /// alone does not resolve a lost K response or prove gate release.
    pub fn native_worker_attach(
        &self,
        attempt_id: &str,
    ) -> Result<Option<NativeWorkerAttach>, String> {
        read_attach_on(&self.conn, attempt_id)
    }

    /// Record broker Q only for the exact previously attached work and PID1.
    /// The broker transport must authenticate this PID1 terminal/reap and
    /// namespace-drain observation. State cannot obtain that kernel proof from
    /// a claimed PID, an ACK, or a root-worker result.
    pub fn settle_broker_native_kernel_q(
        &mut self,
        attached: &NativeWorkerAttach,
        evidence: &BrokerNativeKernelQEvidence,
    ) -> Result<NativeKernelQSettlement, String> {
        if evidence.protocol != Q_PROTOCOL
            || evidence.attempt_id != attached.attempt_id
            || evidence.grant_id != attached.grant_id
            || evidence.kernel_root_id != attached.evidence.kernel_root_id
            || evidence.work_id != attached.evidence.work_id
            || evidence.work_incarnation_id != attached.evidence.work_incarnation_id
            || !evidence.pid1_reaped
            || evidence.remaining_work_processes != 0
            || evidence.pid1_wait_status < 0
            || !sha256(&evidence.terminal_receipt_sha256)
            || evidence.terminal_receipt_sha256 == attached.evidence.attach_receipt_sha256
            || !canonical_uuid(&evidence.observing_broker_incarnation_id)
            || evidence.worker_identity != attached.evidence.worker_identity
            || evidence.pid1_identity != attached.evidence.pid1_identity
            || evidence.work_pid_namespace_inode != attached.evidence.work_pid_namespace_inode
        {
            return Err("invalid broker native kernel Q evidence".into());
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        if read_attach_on(&tx, &attached.attempt_id)?.as_ref() != Some(attached) {
            return Err("native kernel Q attach conflict".into());
        }
        tx.execute(
            "INSERT INTO completion_native_kernel_q(
                attempt_id,grant_id,protocol,kernel_root_id,work_id,work_incarnation_id,observing_broker_incarnation_id,
                worker_identity,pid1_identity,work_pid_namespace_inode,pid1_wait_status,
                remaining_work_processes,pid1_reaped,terminal_receipt_sha256)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,0,1,?12)",
            params![
                attached.attempt_id,
                attached.grant_id,
                evidence.protocol,
                evidence.kernel_root_id,
                attached.evidence.work_id,
                attached.evidence.work_incarnation_id,
                evidence.observing_broker_incarnation_id,
                serde_json::to_string(&evidence.worker_identity).map_err(|e| e.to_string())?,
                serde_json::to_string(&evidence.pid1_identity).map_err(|e| e.to_string())?,
                evidence.work_pid_namespace_inode,
                evidence.pid1_wait_status,
                evidence.terminal_receipt_sha256,
            ],
        )
        .map_err(|error| error.to_string())?;
        let expected = NativeKernelQSettlement {
            attempt_id: attached.attempt_id.clone(),
            grant_id: attached.grant_id.clone(),
            work_id: attached.evidence.work_id.clone(),
            work_incarnation_id: attached.evidence.work_incarnation_id.clone(),
            evidence: evidence.clone(),
        };
        if read_q_on(&tx, &attached.attempt_id)?.as_ref() != Some(&expected) {
            return Err("native kernel Q readback conflict".into());
        }
        tx.commit().map_err(|error| error.to_string())?;
        Ok(expected)
    }

    /// Indexed exact-attempt readback of Q debt or settlement. This never
    /// closes the continuation claim or certifies a worker result.
    pub fn native_kernel_q(
        &self,
        attempt_id: &str,
    ) -> Result<Option<NativeKernelQSettlement>, String> {
        read_q_on(&self.conn, attempt_id)
    }
}

fn read_attach_on(
    conn: &Connection,
    attempt_id: &str,
) -> Result<Option<NativeWorkerAttach>, String> {
    conn.query_row(
        "SELECT attempt_id,grant_id,protocol,gate_held_before_release,kernel_root_id,
                work_id,work_incarnation_id,broker_incarnation_id,
                worker_entrypoint,runner_image_sha256,worker_identity,pid1_identity,
                work_pid_namespace_inode,attach_receipt_sha256
         FROM completion_native_worker_attach WHERE attempt_id=?1",
        [attempt_id],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, bool>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, String>(8)?,
                r.get::<_, String>(9)?,
                r.get::<_, String>(10)?,
                r.get::<_, String>(11)?,
                r.get::<_, i64>(12)?,
                r.get::<_, String>(13)?,
            ))
        },
    )
    .optional()
    .map_err(|e| e.to_string())?
    .map(
        |(
            attempt_id,
            grant_id,
            protocol,
            gate_held_before_release,
            kernel_root_id,
            work_id,
            work_incarnation_id,
            broker_incarnation_id,
            worker_entrypoint,
            runner_image_sha256,
            worker,
            pid1,
            inode,
            digest,
        )| {
            Ok(NativeWorkerAttach {
                attempt_id: attempt_id.clone(),
                grant_id: grant_id.clone(),
                evidence: BrokerNativeAttachEvidence {
                    protocol,
                    gate_held_before_release,
                    attempt_id: attempt_id.clone(),
                    grant_id: grant_id.clone(),
                    kernel_root_id,
                    work_id,
                    work_incarnation_id,
                    broker_incarnation_id,
                    worker_entrypoint,
                    runner_image_sha256,
                    worker_identity: serde_json::from_str(&worker).map_err(|e| e.to_string())?,
                    pid1_identity: serde_json::from_str(&pid1).map_err(|e| e.to_string())?,
                    work_pid_namespace_inode: inode,
                    attach_receipt_sha256: digest,
                },
            })
        },
    )
    .transpose()
}

fn read_q_on(
    conn: &Connection,
    attempt_id: &str,
) -> Result<Option<NativeKernelQSettlement>, String> {
    conn.query_row(
        "SELECT attempt_id,grant_id,protocol,kernel_root_id,work_id,work_incarnation_id,observing_broker_incarnation_id,
                worker_identity,pid1_identity,work_pid_namespace_inode,pid1_wait_status,
                remaining_work_processes,pid1_reaped,terminal_receipt_sha256
         FROM completion_native_kernel_q WHERE attempt_id=?1",
        [attempt_id],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, String>(8)?,
                r.get::<_, i64>(9)?,
                r.get::<_, i64>(10)?,
                r.get::<_, i64>(11)?,
                r.get::<_, bool>(12)?,
                r.get::<_, String>(13)?,
            ))
        },
    )
    .optional()
    .map_err(|e| e.to_string())?
    .map(
        |(
            attempt_id,
            grant_id,
            protocol,
            kernel_root_id,
            work_id,
            work_incarnation_id,
            broker_incarnation_id,
            worker,
            pid1,
            inode,
            status,
            remaining,
            reaped,
            digest,
        )| {
            Ok(NativeKernelQSettlement {
                attempt_id: attempt_id.clone(),
                grant_id: grant_id.clone(),
                work_id: work_id.clone(),
                work_incarnation_id: work_incarnation_id.clone(),
                evidence: BrokerNativeKernelQEvidence {
                    protocol,
                    attempt_id: attempt_id.clone(),
                    grant_id: grant_id.clone(),
                    kernel_root_id,
                    work_id: work_id.clone(),
                    work_incarnation_id: work_incarnation_id.clone(),
                    observing_broker_incarnation_id: broker_incarnation_id,
                    worker_identity: serde_json::from_str(&worker).map_err(|e| e.to_string())?,
                    pid1_identity: serde_json::from_str(&pid1).map_err(|e| e.to_string())?,
                    work_pid_namespace_inode: inode,
                    pid1_wait_status: status,
                    pid1_reaped: reaped,
                    remaining_work_processes: remaining,
                    terminal_receipt_sha256: digest,
                },
            })
        },
    )
    .transpose()
}
