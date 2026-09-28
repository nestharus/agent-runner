/// The broker reserves this before sending the second turn to the selected
/// Codex thread. Every field is checked against fresh State and the original
/// native ACK; the old resident PTY F tables are not involved.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshHeadlessNativeFRequest {
    pub bash_request_id: String,
    pub delivery_request_id: String,
    pub delivery_token: String,
    pub recipient: FreshRecipientIdentity,
    pub original_source_grant_id: String,
    pub selected_k_json: String,
    pub nonce: String,
    pub envelope: String,
    pub anchor_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshHeadlessNativeFAttempt {
    pub candidate: FreshNativeKFCandidate,
    pub bash_request_id: String,
    pub delivery_request_id: String,
    pub delivery_token_sha256: String,
    pub recipient: FreshRecipientIdentity,
    pub original_source_grant_id: String,
    pub selected_k_json: String,
    pub nonce: String,
    pub envelope: String,
    pub envelope_sha256: String,
    pub anchor_sha256: String,
    pub reserved_at: String,
}

/// These are the facts observed from the provider-owned rollout after the
/// exact user input and one assistant response in the completed second turn.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshHeadlessNativeFProof {
    pub bash_request_id: String,
    pub delivery_request_id: String,
    pub delivery_token: String,
    pub recipient: FreshRecipientIdentity,
    pub original_source_grant_id: String,
    pub selected_k_json: String,
    pub fresh_grant_id: String,
    pub native_session_id: String,
    pub first_turn_id: String,
    pub turn_id: String,
    pub nonce: String,
    pub envelope_sha256: String,
    pub user_item_id: String,
    pub assistant_response_sha256: String,
    pub receipt_json: String,
    pub receipt_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshHeadlessNativeFAck {
    pub proof: FreshHeadlessNativeFProof,
    pub basis: String,
    pub acknowledged_at: String,
}

fn expected_headless_native_f_receipt(
    candidate: &FreshNativeKFCandidate,
    proof: &FreshHeadlessNativeFProof,
) -> Result<serde_json::Value, String> {
    let selected: serde_json::Value = serde_json::from_str(&proof.selected_k_json)
        .map_err(|_| "headless native F selected K invalid")?;
    Ok(serde_json::json!({
        "format": "age319-native-f-turn-receipt/v1",
        "selected_k": selected,
        "fresh_grant_id": candidate.fresh.grant_id,
        "delivery_request_id": proof.delivery_request_id,
        "session_id": candidate.fresh.session_id,
        "row_seq": candidate.fresh.seq,
        "source_id": candidate.fresh.source_id,
        "payload_sha256": candidate.fresh.payload_sha256,
        "turn_id": proof.turn_id,
        "first_turn_id": proof.first_turn_id,
        "nonce": proof.nonce,
        "envelope_sha256": proof.envelope_sha256,
        "user_item_id": proof.user_item_id,
        "assistant_response_sha256": proof.assistant_response_sha256,
        "status": "completed",
        "fresh_row_acknowledged": false,
    }))
}

fn validate_headless_native_f_proof(
    candidate: &FreshNativeKFCandidate,
    attempt: &FreshHeadlessNativeFAttempt,
    proof: &FreshHeadlessNativeFProof,
) -> Result<(), String> {
    let expected_response = format!("AGE319_F_ACK {}", proof.delivery_token);
    let receipt: serde_json::Value = serde_json::from_str(&proof.receipt_json)
        .map_err(|_| "headless native F receipt invalid")?;
    let expected_receipt = expected_headless_native_f_receipt(candidate, proof)?;
    if candidate != &attempt.candidate
        || candidate.fresh.phase != "submitted"
        || candidate.fresh.grant_id != proof.fresh_grant_id
        || candidate.native_session_id != proof.native_session_id
        || candidate.original_turn_id != proof.first_turn_id
        || proof.turn_id == proof.first_turn_id
        || proof.turn_id.is_empty()
        || proof.user_item_id.is_empty()
        || proof.user_item_id.len() > 256
        || proof.nonce != attempt.nonce
        || proof.envelope_sha256 != attempt.envelope_sha256
        || attempt.envelope_sha256 != sha256_hex(attempt.envelope.as_bytes())
        || proof.assistant_response_sha256 != sha256_hex(expected_response.as_bytes())
        || proof.receipt_sha256 != sha256_hex(proof.receipt_json.as_bytes())
        || receipt != expected_receipt
        || attempt.delivery_token_sha256 != sha256_hex(proof.delivery_token.as_bytes())
        || attempt.bash_request_id != proof.bash_request_id
        || attempt.delivery_request_id != proof.delivery_request_id
        || attempt.original_source_grant_id != proof.original_source_grant_id
        || attempt.selected_k_json != proof.selected_k_json
        || attempt.recipient != proof.recipient
    {
        return Err("headless native F proof differs from reserved second turn".into());
    }
    Ok(())
}

impl FreshV30Lane {
    pub fn read_headless_native_f_attempt(
        &self,
        delivery_request_id: &str,
        recipient: &FreshRecipientIdentity,
    ) -> Result<Option<FreshHeadlessNativeFAttempt>, String> {
        validate_request_id(delivery_request_id)?;
        let json: Option<String> = self.sidecar.mailbox().conn.query_row(
            "SELECT record_json FROM fresh_headless_native_f_attempt
             WHERE delivery_request_id=?1 AND recipient_identity=?2",
            params![delivery_request_id, Self::recipient_identity_json(recipient)?],
            |row| row.get(0),
        ).optional().map_err(|e| e.to_string())?;
        json.map(|json| serde_json::from_str(&json).map_err(|e| e.to_string())).transpose()
    }

    pub fn reserve_headless_native_f_attempt(
        &mut self,
        request: &FreshHeadlessNativeFRequest,
    ) -> Result<FreshHeadlessNativeFAttempt, String> {
        if self.read_headless_native_f_attempt(&request.delivery_request_id, &request.recipient)?.is_some() {
            return Err("headless native F attempt already reserved; no replay".into());
        }
        validate_request_id(&request.nonce)?;
        let candidate = self.attest_native_k_f_candidate(
            &request.bash_request_id, &request.delivery_request_id,
            &request.delivery_token, &request.recipient,
            &request.original_source_grant_id, &request.selected_k_json,
        )?;
        if candidate.fresh.phase != "submitted"
            || request.anchor_sha256.len() != 64
            || !request.anchor_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("headless native F has no submitted grant or rollout anchor".into());
        }
        let payload = self.lookup_payload(
            &candidate.fresh.lane_id, &candidate.fresh.session_id, candidate.fresh.seq,
        )?;
        let selected: serde_json::Value = serde_json::from_str(&request.selected_k_json)
            .map_err(|e| e.to_string())?;
        let expected = serde_json::json!({
            "format": "age319-native-f-input/v1",
            "instruction": format!("Read this fresh notification and respond with exactly AGE319_F_ACK {}. Do not use tools or add any other text.", request.delivery_token),
            "nonce": request.nonce,
            "delivery_request_id": request.delivery_request_id,
            "delivery_token": request.delivery_token,
            "selected_k": selected,
            "first_turn_id": candidate.original_turn_id,
            "fresh": candidate.fresh,
            "payload_base64": base64::engine::general_purpose::STANDARD.encode(&payload),
        });
        let actual: serde_json::Value = serde_json::from_str(&request.envelope)
            .map_err(|e| e.to_string())?;
        if actual != expected {
            return Err("headless native F envelope differs from exact grant and payload".into());
        }
        let record = FreshHeadlessNativeFAttempt {
            candidate,
            bash_request_id: request.bash_request_id.clone(),
            delivery_request_id: request.delivery_request_id.clone(),
            delivery_token_sha256: sha256_hex(request.delivery_token.as_bytes()),
            recipient: request.recipient.clone(),
            original_source_grant_id: request.original_source_grant_id.clone(),
            selected_k_json: request.selected_k_json.clone(),
            nonce: request.nonce.clone(),
            envelope: request.envelope.clone(),
            envelope_sha256: sha256_hex(request.envelope.as_bytes()),
            anchor_sha256: request.anchor_sha256.clone(),
            reserved_at: Utc::now().to_rfc3339(),
        };
        self.sidecar.mailbox_mut().conn.execute(
            "INSERT INTO fresh_headless_native_f_attempt VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![record.candidate.fresh.grant_id, record.delivery_request_id,
                Self::recipient_identity_json(&record.recipient)?, record.candidate.native_session_id,
                record.candidate.original_turn_id, record.nonce, record.envelope_sha256,
                serde_json::to_string(&record).map_err(|e| e.to_string())?, record.reserved_at],
        ).map_err(|e| format!("headless native F reservation refused: {e}"))?;
        Ok(record)
    }

    /// Read only. This is the recovery path after a post-commit reply loss;
    /// it never retries the ACK or sends another provider turn.
    pub fn read_headless_native_f_ack(
        &self,
        delivery_request_id: &str,
        recipient: &FreshRecipientIdentity,
    ) -> Result<Option<FreshHeadlessNativeFAck>, String> {
        validate_request_id(delivery_request_id)?;
        let records: Option<(String, String, String, String, String, String)> = self.sidecar.mailbox().conn.query_row(
            "SELECT a.record_json,t.record_json,a.turn_id,a.assistant_response_sha256,
                    a.receipt_sha256,a.acknowledged_at FROM fresh_headless_native_f_ack a
             JOIN fresh_headless_native_f_attempt t ON t.grant_id=a.grant_id
             JOIN fresh_recipient_grant g ON g.grant_id=a.grant_id
             JOIN mailbox m ON m.session_id=g.session_id AND m.seq=g.seq
             JOIN fresh_recipient_row_source r ON r.session_id=g.session_id AND r.seq=g.seq
             WHERE a.delivery_request_id=?1 AND a.recipient_identity=?2
               AND t.delivery_request_id=a.delivery_request_id
               AND t.recipient_identity=a.recipient_identity
               AND t.native_session_id=a.native_session_id
               AND g.delivery_request_id=a.delivery_request_id
               AND g.recipient_identity=a.recipient_identity
               AND g.phase='acked' AND g.acknowledged_at=a.acknowledged_at
               AND m.delivered_at=a.acknowledged_at
               AND m.delivered_by_invocation_uuid=a.grant_id AND m.delivery_attempts=1
               AND m.payload_sha256=g.payload_sha256 AND m.payload_byte_len=g.payload_byte_len
               AND r.source_id=g.source_id AND r.attempt_id=g.attempt_id
               AND r.payload_sha256=g.payload_sha256 AND r.payload_byte_len=g.payload_byte_len",
            params![delivery_request_id, Self::recipient_identity_json(recipient)?],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        ).optional().map_err(|e| e.to_string())?;
        records.map(|(ack_json, attempt_json, turn, response, receipt, at)| {
            let ack: FreshHeadlessNativeFAck = serde_json::from_str(&ack_json).map_err(|e| e.to_string())?;
            let attempt: FreshHeadlessNativeFAttempt = serde_json::from_str(&attempt_json).map_err(|e| e.to_string())?;
            validate_headless_native_f_proof(&attempt.candidate, &attempt, &ack.proof)?;
            if ack.basis != "native_codex_f_assistant_ack"
                || ack.acknowledged_at != at
                || ack.proof.turn_id != turn
                || ack.proof.assistant_response_sha256 != response
                || ack.proof.receipt_sha256 != receipt
                || ack.proof.delivery_request_id != delivery_request_id
                || ack.proof.recipient != *recipient
            {
                return Err("headless native F ACK record differs from settlement".into());
            }
            Ok(ack)
        }).transpose()
    }

    /// One SQLite transaction changes exactly the F row and grant and adds
    /// retained native evidence. The original v2 row is read-only here.
    pub fn acknowledge_headless_native_f(
        &mut self,
        proof: &FreshHeadlessNativeFProof,
    ) -> Result<FreshHeadlessNativeFAck, String> {
        validate_request_id(&proof.turn_id)?;
        let candidate = self.attest_native_k_f_candidate(
            &proof.bash_request_id, &proof.delivery_request_id,
            &proof.delivery_token, &proof.recipient,
            &proof.original_source_grant_id, &proof.selected_k_json,
        )?;
        let attempt = self.read_headless_native_f_attempt(
            &proof.delivery_request_id, &proof.recipient,
        )?.ok_or("headless native F attempt absent; no ACK")?;
        validate_headless_native_f_proof(&candidate, &attempt, proof)?;
        let identity = Self::recipient_identity_json(&proof.recipient)?;
        let now = Utc::now().to_rfc3339();
        let ack = FreshHeadlessNativeFAck {
            proof: proof.clone(), basis: "native_codex_f_assistant_ack".into(),
            acknowledged_at: now.clone(),
        };
        let tx = self.sidecar.mailbox_mut().conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let changed = tx.execute(
            "UPDATE mailbox SET delivered_at=?3,delivered_by_invocation_uuid=?4,
               delivery_attempts=delivery_attempts+1,delivery_error=NULL
             WHERE session_id=?1 AND seq=?2 AND delivered_at IS NULL AND delivery_attempts=0
               AND payload_sha256=?5 AND payload_byte_len=?6
               AND EXISTS (SELECT 1 FROM fresh_recipient_row_source r
                 WHERE r.session_id=?1 AND r.seq=?2 AND r.source_id=?7 AND r.attempt_id=?8
                   AND r.payload_sha256=?5 AND r.payload_byte_len=?6)
               AND EXISTS (SELECT 1 FROM fresh_headless_native_f_attempt t
                 WHERE t.grant_id=?4 AND t.delivery_request_id=?9
                   AND t.recipient_identity=?10 AND t.envelope_sha256=?11)",
            params![candidate.fresh.session_id, candidate.fresh.seq, now,
                candidate.fresh.grant_id, candidate.fresh.payload_sha256,
                candidate.fresh.payload_byte_len, candidate.fresh.source_id,
                candidate.fresh.attempt_id, proof.delivery_request_id, identity,
                proof.envelope_sha256],
        ).map_err(|e| e.to_string())?;
        if changed != 1 { return Err("headless native F row/source already settled or changed".into()); }
        if tx.execute(
            "UPDATE fresh_recipient_grant SET phase='acked',acknowledged_at=?2
             WHERE grant_id=?1 AND delivery_token=?3 AND recipient_identity=?4
               AND phase='submitted' AND delivery_request_id=?5
               AND session_id=?6 AND seq=?7 AND source_id=?8 AND attempt_id=?9
               AND payload_sha256=?10 AND payload_byte_len=?11",
            params![candidate.fresh.grant_id, now, proof.delivery_token, identity,
                proof.delivery_request_id, candidate.fresh.session_id, candidate.fresh.seq,
                candidate.fresh.source_id, candidate.fresh.attempt_id,
                candidate.fresh.payload_sha256, candidate.fresh.payload_byte_len],
        ).map_err(|e| e.to_string())? != 1 {
            return Err("headless native F grant changed before ACK".into());
        }
        tx.execute(
            "INSERT INTO fresh_headless_native_f_ack VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![candidate.fresh.grant_id, proof.delivery_request_id, identity,
                proof.native_session_id, proof.turn_id, proof.user_item_id,
                proof.assistant_response_sha256, proof.receipt_sha256, ack.basis,
                serde_json::to_string(&ack).map_err(|e| e.to_string())?, now],
        ).map_err(|e| format!("headless native F evidence duplicate or absent: {e}"))?;
        tx.commit().map_err(|e| e.to_string())?;
        self.read_headless_native_f_ack(&proof.delivery_request_id, &proof.recipient)?
            .ok_or("headless native F ACK readback absent".into())
    }
}

#[cfg(test)]
mod headless_native_f_tests {
    use super::*;

    #[test]
    fn second_turn_proof_rejects_wrong_token_source_payload_k_and_response() {
        let recipient = FreshRecipientIdentity {
            host_pid: 10, boot_id: "boot".into(), starttime_ticks: 20,
            pidns_dev: 30, pidns_ino: 40,
        };
        let fresh = FreshDeliveryReadback {
            grant_id: "fresh-grant".into(), session_id: "session".into(), seq: 2,
            source_id: "source".into(), attempt_id: "bash-attempt".into(),
            lane_id: "lane".into(), source_generation: "generation".into(),
            root_id: "root".into(), owner_generation: "owner".into(),
            payload_sha256: "payload".into(), payload_byte_len: 7,
            phase: "submitted".into(),
        };
        let candidate = FreshNativeKFCandidate {
            fresh, fresh_physical_grant_id: "physical".into(),
            fresh_parent_work_grant_id: "work".into(),
            original_native_grant_id: "native".into(),
            original_source_grant_id: "original".into(), original_row_seq: 1,
            native_session_id: "thread".into(), original_turn_id: "first".into(),
            original_turn_receipt_sha256: "first-receipt".into(),
        };
        let envelope = "exact F envelope".to_owned();
        let attempt = FreshHeadlessNativeFAttempt {
            candidate: candidate.clone(), bash_request_id: "bash-request".into(),
            delivery_request_id: "delivery-request".into(),
            delivery_token_sha256: sha256_hex(b"token"), recipient: recipient.clone(),
            original_source_grant_id: "original".into(), selected_k_json: "{}".into(),
            nonce: "nonce".into(), envelope_sha256: sha256_hex(envelope.as_bytes()),
            envelope, anchor_sha256: "anchor".into(), reserved_at: "now".into(),
        };
        let mut proof = FreshHeadlessNativeFProof {
            bash_request_id: attempt.bash_request_id.clone(),
            delivery_request_id: attempt.delivery_request_id.clone(),
            delivery_token: "token".into(), recipient,
            original_source_grant_id: attempt.original_source_grant_id.clone(),
            selected_k_json: attempt.selected_k_json.clone(),
            fresh_grant_id: candidate.fresh.grant_id.clone(),
            native_session_id: candidate.native_session_id.clone(),
            first_turn_id: candidate.original_turn_id.clone(), turn_id: "second".into(),
            nonce: attempt.nonce.clone(), envelope_sha256: attempt.envelope_sha256.clone(),
            user_item_id: "provider-user".into(),
            assistant_response_sha256: sha256_hex(b"AGE319_F_ACK token"),
            receipt_json: String::new(), receipt_sha256: String::new(),
        };
        proof.receipt_json = format!("{}\n", expected_headless_native_f_receipt(&candidate, &proof).unwrap());
        proof.receipt_sha256 = sha256_hex(proof.receipt_json.as_bytes());
        assert!(validate_headless_native_f_proof(&candidate, &attempt, &proof).is_ok());
        let mut wrong = proof.clone();
        wrong.delivery_token = "wrong".into();
        assert!(validate_headless_native_f_proof(&candidate, &attempt, &wrong).is_err());
        wrong = proof.clone();
        wrong.selected_k_json = "another K".into();
        assert!(validate_headless_native_f_proof(&candidate, &attempt, &wrong).is_err());
        wrong = proof.clone();
        wrong.turn_id = "first".into();
        assert!(validate_headless_native_f_proof(&candidate, &attempt, &wrong).is_err());
        wrong = proof.clone();
        wrong.user_item_id.clear();
        assert!(validate_headless_native_f_proof(&candidate, &attempt, &wrong).is_err());
        wrong = proof.clone();
        wrong.assistant_response_sha256 = sha256_hex(b"AGE319_F_ACK wrong");
        assert!(validate_headless_native_f_proof(&candidate, &attempt, &wrong).is_err());
        let mut changed = candidate.clone();
        changed.fresh.source_id = "altered source".into();
        assert!(validate_headless_native_f_proof(&changed, &attempt, &proof).is_err());
        changed = candidate.clone();
        changed.fresh.payload_sha256 = "altered payload".into();
        assert!(validate_headless_native_f_proof(&changed, &attempt, &proof).is_err());
    }
}
