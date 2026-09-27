use super::*;

/// Only the Broker constructs this after read_v2_recipient_custody has
/// revalidated the original H/W/Q source and its pending listener row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrokerV2RecipientBinding {
    pub source_grant_id: String,
    pub source_id: String,
    pub registration_id: String,
    pub source_generation: String,
    pub root_id: String,
    pub owner_generation: String,
    pub listener_id: String,
    pub session_id: String,
    pub owner_invocation_uuid: String,
    pub row_seq: i64,
    pub payload_sha256: String,
    pub payload_byte_len: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrokerV2RecipientGrant {
    pub grant_id: String,
    pub binding: BrokerV2RecipientBinding,
    pub root_init_identity: String,
    pub recipient_identity: String,
    pub delivery_token: String,
    pub phase: String,
    pub ack_response_sha256: Option<String>,
}

impl BrokerSidecar {
    fn verify_v2_recipient_row(
        &self,
        binding: &BrokerV2RecipientBinding,
        phase: &str,
    ) -> Result<Vec<u8>, String> {
        self.check_mailbox_read(&binding.source_generation)?;
        let row = self
            .read_exact_mailbox_row(
                &binding.source_generation,
                &binding.session_id,
                binding.row_seq,
            )?
            .ok_or("v2 recipient row absent")?
            .row;
        if row.kind != AGENT_BASH_COMPLETE_KIND
            || row.owner_invocation_uuid.as_deref() != Some(&binding.owner_invocation_uuid)
            || row.payload_sha256.as_deref() != Some(&binding.payload_sha256)
            || row.payload_byte_len != Some(binding.payload_byte_len)
            || row.payload_retention_policy.as_deref() != Some(MAILBOX_PAYLOAD_RETENTION_POLICY)
            || (phase == "acked") != row.delivered_at.is_some()
        {
            return Err("v2 recipient row identity or disposition changed".into());
        }
        let listener: Option<(String, String, String, Option<String>)> = self
            .mailbox
            .conn
            .query_row(
                "SELECT event_id,listener_id,owner_invocation_uuid,acknowledged_at
             FROM completion_event_listener WHERE mailbox_seq=?1
             AND session_id=?2 AND listener_id=?3 AND active=1",
                params![binding.row_seq, binding.session_id, binding.listener_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let (event_id, listener_id, invocation, acknowledged_at) =
            listener.ok_or("v2 original listener absent")?;
        if listener_id != binding.listener_id
            || invocation != binding.owner_invocation_uuid
            || (phase == "acked") != acknowledged_at.is_some()
        {
            return Err("v2 original listener identity or ACK changed".into());
        }
        let accepted: bool = self
            .mailbox
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM broker_completion_source_acceptance a
             JOIN broker_source_effect_grant g ON g.grant_id=a.grant_id
             JOIN completion_continuation_source s ON s.registration_id=a.registration_id
             WHERE a.grant_id=?1 AND a.registration_id=?2 AND a.source_generation=?3
             AND g.root_id=?4 AND g.owner_generation=?5 AND s.source_id=?6
             AND s.event_id=?7 AND s.payload_sha256=?8
             AND s.payload_byte_len=?9 AND s.phase='accepted')",
                params![
                    binding.source_grant_id,
                    binding.registration_id,
                    binding.source_generation,
                    binding.root_id,
                    binding.owner_generation,
                    binding.source_id,
                    event_id,
                    binding.payload_sha256,
                    binding.payload_byte_len,
                ],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !accepted {
            return Err("v2 recipient source acceptance changed".into());
        }
        self.mailbox.payloads().verify_mailbox_row_payload(&row)?;
        let bytes = self
            .mailbox
            .completion_recovery_payload(&event_id)?
            .ok_or("v2 accepted payload absent")?;
        if bytes.len() as i64 != binding.payload_byte_len
            || sha256_hex(&bytes) != binding.payload_sha256
        {
            return Err("v2 accepted payload differs from recipient row".into());
        }
        Ok(bytes)
    }

    pub fn reserve_v2_recipient_grant(
        &mut self,
        binding: &BrokerV2RecipientBinding,
        root_init_identity: &str,
        recipient_identity: &str,
    ) -> Result<BrokerV2RecipientGrant, String> {
        let root_init: PreparedProcessStamp =
            serde_json::from_str(root_init_identity).map_err(|_| "invalid v2 root PID1 stamp")?;
        let recipient: PreparedProcessStamp =
            serde_json::from_str(recipient_identity).map_err(|_| "invalid v2 wake stamp")?;
        if !root_init.valid()
            || !recipient.valid()
            || root_init.boot_id != recipient.boot_id
            || (root_init.pidns_dev, root_init.pidns_ino)
                != (recipient.pidns_dev, recipient.pidns_ino)
            || root_init.host_pid == recipient.host_pid
            || binding.row_seq <= 0
            || binding.payload_byte_len < 0
            || self
                .mailbox
                .conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM broker_v2_recipient_grant
                 WHERE source_grant_id=?1 OR row_seq=?2)",
                    params![binding.source_grant_id, binding.row_seq],
                    |r| r.get::<_, bool>(0),
                )
                .map_err(|e| e.to_string())?
        {
            return Err("v2 recipient grant already exists or identity invalid".into());
        }
        self.verify_v2_recipient_row(binding, "reserved")?;
        let grant_id = Uuid::new_v4().to_string();
        let delivery_token = Uuid::new_v4().to_string();
        let changed = self
            .mailbox
            .conn
            .execute(
                "INSERT INTO broker_v2_recipient_grant
             (grant_id,source_grant_id,source_generation,root_id,owner_generation,
              registration_id,source_id,listener_id,session_id,owner_invocation_uuid,row_seq,
              payload_sha256,payload_byte_len,root_init_identity,recipient_identity,
              delivery_token,phase,created_at)
             SELECT ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,'reserved',?17
             WHERE EXISTS(SELECT 1 FROM mailbox m JOIN completion_event_listener l
               ON l.mailbox_seq=m.seq
               WHERE m.seq=?11 AND m.session_id=?9 AND m.delivered_at IS NULL
               AND m.payload_sha256=?12 AND m.payload_byte_len=?13
               AND l.listener_id=?8 AND l.owner_invocation_uuid=?10
               AND l.active=1 AND l.acknowledged_at IS NULL)",
                params![
                    grant_id,
                    binding.source_grant_id,
                    binding.source_generation,
                    binding.root_id,
                    binding.owner_generation,
                    binding.registration_id,
                    binding.source_id,
                    binding.listener_id,
                    binding.session_id,
                    binding.owner_invocation_uuid,
                    binding.row_seq,
                    binding.payload_sha256,
                    binding.payload_byte_len,
                    root_init_identity,
                    recipient_identity,
                    delivery_token,
                    now_rfc3339(),
                ],
            )
            .map_err(|e| format!("v2 recipient grant refused: {e}"))?;
        if changed != 1 {
            return Err("v2 recipient row changed before grant".into());
        }
        self.read_v2_recipient_grant(&binding.source_grant_id)?
            .ok_or("v2 recipient grant vanished".into())
    }

    pub fn read_v2_recipient_grant(
        &self,
        source_grant_id: &str,
    ) -> Result<Option<BrokerV2RecipientGrant>, String> {
        let grant: Option<BrokerV2RecipientGrant> = self
            .mailbox
            .conn
            .query_row(
                "SELECT grant_id,source_grant_id,source_generation,root_id,owner_generation,
                registration_id,source_id,listener_id,session_id,owner_invocation_uuid,row_seq,
                payload_sha256,payload_byte_len,root_init_identity,recipient_identity,
                delivery_token,phase,ack_response_sha256
             FROM broker_v2_recipient_grant WHERE source_grant_id=?1",
                [source_grant_id],
                |r| {
                    Ok(BrokerV2RecipientGrant {
                        grant_id: r.get(0)?,
                        binding: BrokerV2RecipientBinding {
                            source_grant_id: r.get(1)?,
                            source_generation: r.get(2)?,
                            root_id: r.get(3)?,
                            owner_generation: r.get(4)?,
                            registration_id: r.get(5)?,
                            source_id: r.get(6)?,
                            listener_id: r.get(7)?,
                            session_id: r.get(8)?,
                            owner_invocation_uuid: r.get(9)?,
                            row_seq: r.get(10)?,
                            payload_sha256: r.get(11)?,
                            payload_byte_len: r.get(12)?,
                        },
                        root_init_identity: r.get(13)?,
                        recipient_identity: r.get(14)?,
                        delivery_token: r.get(15)?,
                        phase: r.get(16)?,
                        ack_response_sha256: r.get(17)?,
                    })
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(grant) = &grant {
            self.verify_v2_recipient_row(&grant.binding, &grant.phase)?;
        }
        Ok(grant)
    }

    pub fn v2_recipient_payload(&self, grant: &BrokerV2RecipientGrant) -> Result<Vec<u8>, String> {
        let current = self
            .read_v2_recipient_grant(&grant.binding.source_grant_id)?
            .ok_or("v2 recipient grant absent")?;
        if current != *grant {
            return Err("v2 recipient grant changed".into());
        }
        self.verify_v2_recipient_row(&grant.binding, &grant.phase)
    }

    pub fn begin_v2_recipient_send(
        &mut self,
        grant: &BrokerV2RecipientGrant,
    ) -> Result<(), String> {
        if self
            .read_v2_recipient_grant(&grant.binding.source_grant_id)?
            .as_ref()
            != Some(grant)
            || grant.phase != "reserved"
        {
            return Err("v2 send grant is not reserved".into());
        }
        if self
            .mailbox
            .conn
            .execute(
                "UPDATE broker_v2_recipient_grant SET phase='unknown',sending_at=?2
             WHERE grant_id=?1 AND phase='reserved'",
                params![grant.grant_id, now_rfc3339()],
            )
            .map_err(|e| e.to_string())?
            != 1
        {
            return Err("v2 send attempt changed".into());
        }
        Ok(())
    }

    pub fn mark_v2_recipient_submitted(
        &mut self,
        source_grant_id: &str,
        recipient_identity: &str,
    ) -> Result<(), String> {
        let grant = self
            .read_v2_recipient_grant(source_grant_id)?
            .ok_or("v2 recipient grant absent")?;
        if grant.phase != "unknown" || grant.recipient_identity != recipient_identity {
            return Err("v2 recipient submission identity or phase changed".into());
        }
        if self
            .mailbox
            .conn
            .execute(
                "UPDATE broker_v2_recipient_grant SET phase='submitted',submitted_at=?2
             WHERE grant_id=?1 AND phase='unknown' AND recipient_identity=?3",
                params![grant.grant_id, now_rfc3339(), recipient_identity],
            )
            .map_err(|e| e.to_string())?
            != 1
        {
            return Err("v2 recipient submission changed".into());
        }
        Ok(())
    }

    pub fn acknowledge_v2_recipient(
        &mut self,
        source_grant_id: &str,
        recipient_identity: &str,
        token: &str,
        response_sha256: &str,
    ) -> Result<BrokerV2RecipientGrant, String> {
        let grant = self
            .read_v2_recipient_grant(source_grant_id)?
            .ok_or("v2 recipient grant absent")?;
        if grant.phase != "submitted"
            || grant.recipient_identity != recipient_identity
            || grant.delivery_token != token
            || response_sha256.len() != 64
            || !response_sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("v2 explicit ACK actor, token or phase refused".into());
        }
        let now = now_rfc3339();
        let tx = self
            .mailbox
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let row = tx
            .execute(
                "UPDATE mailbox SET delivered_at=?2,delivered_by_invocation_uuid=?3,
             delivery_attempts=delivery_attempts+1,delivery_error=NULL
             WHERE seq=?1 AND session_id=?4 AND delivered_at IS NULL
             AND payload_sha256=?5 AND payload_byte_len=?6 AND kind='agent_bash_complete'",
                params![
                    grant.binding.row_seq,
                    now,
                    grant.binding.owner_invocation_uuid,
                    grant.binding.session_id,
                    grant.binding.payload_sha256,
                    grant.binding.payload_byte_len
                ],
            )
            .map_err(|e| e.to_string())?;
        let listener = tx
            .execute(
                "UPDATE completion_event_listener SET acknowledged_at=?2,
             acknowledgement_reason='explicit_v2_wake_ack',retirement_pending=0
             WHERE mailbox_seq=?1 AND listener_id=?3 AND session_id=?4
             AND owner_invocation_uuid=?5 AND active=1 AND acknowledged_at IS NULL",
                params![
                    grant.binding.row_seq,
                    now,
                    grant.binding.listener_id,
                    grant.binding.session_id,
                    grant.binding.owner_invocation_uuid
                ],
            )
            .map_err(|e| e.to_string())?;
        let updated = tx
            .execute(
                "UPDATE broker_v2_recipient_grant SET phase='acked',acknowledged_at=?2,
             ack_response_sha256=?3 WHERE grant_id=?1 AND phase='submitted'
             AND recipient_identity=?4 AND delivery_token=?5",
                params![
                    grant.grant_id,
                    now,
                    response_sha256,
                    recipient_identity,
                    token
                ],
            )
            .map_err(|e| e.to_string())?;
        if (row, listener, updated) != (1, 1, 1) {
            return Err("v2 recipient ACK attachment changed".into());
        }
        tx.commit().map_err(|e| e.to_string())?;
        self.read_v2_recipient_grant(source_grant_id)?
            .ok_or("v2 recipient ACK vanished".into())
    }
}
