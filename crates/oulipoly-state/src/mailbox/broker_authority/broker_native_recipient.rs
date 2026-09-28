use super::*;

/// A selected native Codex K and its own thread, never the broker-image wake
/// actor. The token is one-use and the original source row is held by State.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrokerNativeRecipientGrant {
    pub grant_id: String,
    pub binding: BrokerV2RecipientBinding,
    pub selected_k_json: String,
    pub native_session_id: String,
    pub envelope_sha256: String,
    pub delivery_token: String,
    pub phase: String,
    pub turn_id: Option<String>,
    pub turn_receipt_sha256: Option<String>,
    pub ack_response_sha256: Option<String>,
}

fn digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

impl BrokerSidecar {
    pub fn read_native_recipient_grant(
        &self,
        source_grant_id: &str,
    ) -> Result<Option<BrokerNativeRecipientGrant>, String> {
        let grant = self
            .mailbox
            .conn
            .query_row(
                "SELECT grant_id,source_grant_id,source_generation,root_id,owner_generation,
             registration_id,source_id,listener_id,session_id,owner_invocation_uuid,row_seq,
             payload_sha256,payload_byte_len,selected_k_json,native_session_id,
             envelope_sha256,delivery_token,phase,turn_id,turn_receipt_sha256,ack_response_sha256
             FROM broker_native_recipient_grant WHERE source_grant_id=?1",
                [source_grant_id],
                |r| {
                    Ok(BrokerNativeRecipientGrant {
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
                        selected_k_json: r.get(13)?,
                        native_session_id: r.get(14)?,
                        envelope_sha256: r.get(15)?,
                        delivery_token: r.get(16)?,
                        phase: r.get(17)?,
                        turn_id: r.get(18)?,
                        turn_receipt_sha256: r.get(19)?,
                        ack_response_sha256: r.get(20)?,
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

    pub fn reserve_native_recipient_grant(
        &mut self,
        binding: &BrokerV2RecipientBinding,
        selected_k_json: &str,
        native_session_id: &str,
        envelope_sha256: &str,
        delivery_token: &str,
    ) -> Result<BrokerNativeRecipientGrant, String> {
        let selected: serde_json::Value = serde_json::from_str(selected_k_json)
            .map_err(|_| "invalid selected native K readback")?;
        if selected
            .pointer("/native_session_id")
            .and_then(|v| v.as_str())
            != Some(native_session_id)
            || selected
                .pointer("/grant_id")
                .and_then(|v| v.as_str())
                .is_none()
            || native_session_id == binding.session_id
            || !digest(envelope_sha256)
            || Uuid::parse_str(delivery_token)
                .map_err(|_| "invalid native ACK token")?
                .to_string()
                != delivery_token
        {
            return Err("native recipient binding, session or token invalid".into());
        }
        self.verify_v2_recipient_row(binding, "reserved")?;
        let collision: bool = self.mailbox.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM broker_v2_recipient_grant WHERE source_grant_id=?1 OR row_seq=?2)
                 OR EXISTS(SELECT 1 FROM broker_native_recipient_grant WHERE source_grant_id=?1 OR row_seq=?2)",
            params![binding.source_grant_id, binding.row_seq], |r| r.get(0),
        ).map_err(|e| e.to_string())?;
        if collision {
            return Err("original row already has a recipient grant".into());
        }
        let grant_id = Uuid::new_v4().to_string();
        let changed = self
            .mailbox
            .conn
            .execute(
                "INSERT INTO broker_native_recipient_grant
             (grant_id,source_grant_id,source_generation,root_id,owner_generation,
              registration_id,source_id,listener_id,session_id,owner_invocation_uuid,row_seq,
              payload_sha256,payload_byte_len,selected_k_json,native_session_id,
              envelope_sha256,delivery_token,phase,created_at)
             SELECT ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,'reserved',?18
             WHERE EXISTS(SELECT 1 FROM mailbox m JOIN completion_event_listener l
               ON l.mailbox_seq=m.seq WHERE m.seq=?11 AND m.session_id=?9
               AND m.delivered_at IS NULL AND m.payload_sha256=?12 AND m.payload_byte_len=?13
               AND l.listener_id=?8 AND l.owner_invocation_uuid=?10
               AND l.active=1 AND l.acknowledged_at IS NULL)
               AND NOT EXISTS(SELECT 1 FROM broker_v2_recipient_grant
                   WHERE source_grant_id=?2 OR row_seq=?11)",
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
                    selected_k_json,
                    native_session_id,
                    envelope_sha256,
                    delivery_token,
                    now_rfc3339()
                ],
            )
            .map_err(|e| format!("native recipient reservation refused: {e}"))?;
        if changed != 1 {
            return Err("original row changed before native reservation".into());
        }
        self.read_native_recipient_grant(&binding.source_grant_id)?
            .ok_or("native grant vanished".into())
    }

    pub fn begin_native_recipient_send(
        &mut self,
        grant: &BrokerNativeRecipientGrant,
    ) -> Result<(), String> {
        if grant.phase != "reserved"
            || self
                .read_native_recipient_grant(&grant.binding.source_grant_id)?
                .as_ref()
                != Some(grant)
        {
            return Err("native recipient send is not reserved".into());
        }
        let changed = self
            .mailbox
            .conn
            .execute(
                "UPDATE broker_native_recipient_grant SET phase='unknown',sending_at=?2
             WHERE grant_id=?1 AND phase='reserved'",
                params![grant.grant_id, now_rfc3339()],
            )
            .map_err(|e| e.to_string())?;
        if changed != 1 {
            return Err("native recipient send changed".into());
        }
        Ok(())
    }

    pub fn mark_native_recipient_submitted(
        &mut self,
        grant: &BrokerNativeRecipientGrant,
        turn_id: &str,
        receipt_sha256: &str,
    ) -> Result<(), String> {
        if grant.phase != "reserved" || turn_id.is_empty() || !digest(receipt_sha256) {
            return Err("native turn receipt identity invalid".into());
        }
        let current = self
            .read_native_recipient_grant(&grant.binding.source_grant_id)?
            .ok_or("native recipient grant absent")?;
        if current.phase != "unknown"
            || current.grant_id != grant.grant_id
            || current.binding != grant.binding
            || current.selected_k_json != grant.selected_k_json
            || current.envelope_sha256 != grant.envelope_sha256
            || current.delivery_token != grant.delivery_token
        {
            return Err("native turn submission lineage changed".into());
        }
        let changed = self
            .mailbox
            .conn
            .execute(
                "UPDATE broker_native_recipient_grant SET phase='submitted',submitted_at=?2,
             turn_id=?3,turn_receipt_sha256=?4 WHERE grant_id=?1 AND phase='unknown'
             AND selected_k_json=?5 AND envelope_sha256=?6 AND delivery_token=?7",
                params![
                    grant.grant_id,
                    now_rfc3339(),
                    turn_id,
                    receipt_sha256,
                    grant.selected_k_json,
                    grant.envelope_sha256,
                    grant.delivery_token
                ],
            )
            .map_err(|e| e.to_string())?;
        if changed != 1 {
            return Err("native turn submission changed".into());
        }
        Ok(())
    }

    pub fn acknowledge_native_recipient(
        &mut self,
        source_grant_id: &str,
        selected_k_json: &str,
        token: &str,
        turn_id: &str,
        receipt_sha256: &str,
        response_sha256: &str,
    ) -> Result<BrokerNativeRecipientGrant, String> {
        let grant = self
            .read_native_recipient_grant(source_grant_id)?
            .ok_or("native grant absent")?;
        if grant.phase == "acked"
            && grant.selected_k_json == selected_k_json
            && grant.delivery_token == token
            && grant.turn_id.as_deref() == Some(turn_id)
            && grant.turn_receipt_sha256.as_deref() == Some(receipt_sha256)
            && grant.ack_response_sha256.as_deref() == Some(response_sha256)
        {
            return Ok(grant);
        } // A lost transaction reply reads one settlement.
        if grant.phase != "submitted"
            || grant.selected_k_json != selected_k_json
            || grant.delivery_token != token
            || grant.turn_id.as_deref() != Some(turn_id)
            || grant.turn_receipt_sha256.as_deref() != Some(receipt_sha256)
            || !digest(response_sha256)
        {
            return Err("native assistant ACK identity, receipt or phase refused".into());
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
             acknowledgement_reason='explicit_native_codex_assistant_ack',retirement_pending=0
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
                "UPDATE broker_native_recipient_grant SET phase='acked',acknowledged_at=?2,
             ack_response_sha256=?3 WHERE grant_id=?1 AND phase='submitted'
             AND selected_k_json=?4 AND delivery_token=?5 AND turn_id=?6
             AND turn_receipt_sha256=?7",
                params![
                    grant.grant_id,
                    now,
                    response_sha256,
                    selected_k_json,
                    token,
                    turn_id,
                    receipt_sha256
                ],
            )
            .map_err(|e| e.to_string())?;
        if (row, listener, updated) != (1, 1, 1) {
            return Err("native original row/listener/grant changed".into());
        }
        tx.commit().map_err(|e| e.to_string())?;
        self.read_native_recipient_grant(source_grant_id)?
            .ok_or("native ACK vanished".into())
    }
}
