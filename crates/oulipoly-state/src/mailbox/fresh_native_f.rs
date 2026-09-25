use base64::Engine as _;
use std::os::unix::fs::FileTypeExt;

/// Private pre-send input plan. The Tail token is supplied by the original
/// Runner after a typed native Tail read; this record itself is not a receipt.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshNativeFPrepareRequest {
    pub preparation_request_id: String,
    pub delivery_request_id: String,
    pub grant_id: String,
    pub delivery_token: String,
    pub runtime_generation_id: String,
    pub provider_instance_id: String,
    pub settings_id: String,
    pub envelope_nonce: String,
    pub tail_resume_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshNativeFPreparation {
    pub preparation_request_id: String,
    pub delivery_request_id: String,
    pub grant_id: String,
    pub delivery_token_sha256: String,
    pub recipient_identity: FreshRecipientIdentity,
    pub root_id: String,
    pub owner_generation: String,
    pub session_id: String,
    pub seq: i64,
    pub source_id: String,
    pub attempt_id: String,
    pub lane_id: String,
    pub source_generation: String,
    pub payload_sha256: String,
    pub payload_byte_len: i64,
    pub runtime_generation_id: String,
    pub runtime_spawn_invocation_uuid: String,
    pub pty_control_path: String,
    pub pty_control_device: u64,
    pub pty_control_inode: u64,
    pub provider_account: String,
    pub provider_instance_id: String,
    pub settings_id: String,
    pub provider_session_id: String,
    pub envelope_nonce: String,
    pub envelope_text: String,
    pub envelope_sha256: String,
    pub tail_resume_token: String,
    pub prepared_at: String,
}

fn nonempty_bounded(name: &str, value: &str, max: usize) -> Result<(), String> {
    if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(format!("invalid native F {name}"));
    }
    Ok(())
}

impl FreshV30Lane {
    /// Same-key readback is independent of current endpoint liveness. It can
    /// recover a lost reply without selecting a replacement input effect.
    pub fn read_native_f_preparation(
        &self,
        request_id: &str,
        recipient: &FreshRecipientIdentity,
    ) -> Result<Option<FreshNativeFPreparation>, String> {
        validate_request_id(request_id)?;
        let recipient_json = Self::recipient_identity_json(recipient)?;
        let record: Option<String> = self.sidecar.mailbox().conn.query_row(
            "SELECT record_json FROM fresh_native_f_preparation
             WHERE preparation_request_id=?1 AND recipient_identity=?2",
            params![request_id, recipient_json], |row| row.get(0),
        ).optional().map_err(|e| e.to_string())?;
        record.map(|json| serde_json::from_str(&json).map_err(|e| e.to_string())).transpose()
    }

    /// Freeze exactly one F grant into one native input plan. The caller must
    /// capture a complete provider-native Tail page before invoking this API.
    /// No PTY write, receipt, or ACK is performed here.
    pub fn prepare_native_f_input(
        &mut self,
        request: &FreshNativeFPrepareRequest,
        recipient: &FreshRecipientIdentity,
    ) -> Result<FreshNativeFPreparation, String> {
        for id in [
            &request.preparation_request_id,
            &request.delivery_request_id,
            &request.grant_id,
            &request.delivery_token,
            &request.runtime_generation_id,
            &request.envelope_nonce,
        ] {
            validate_request_id(id)?;
        }
        nonempty_bounded("provider instance", &request.provider_instance_id, 1024)?;
        nonempty_bounded("settings", &request.settings_id, 1024)?;
        nonempty_bounded("Tail anchor", &request.tail_resume_token, 4096)?;
        let token_sha = sha256_hex(request.delivery_token.as_bytes());
        if let Some(existing) = self.read_native_f_preparation(
            &request.preparation_request_id, recipient,
        )? {
            if existing.delivery_request_id == request.delivery_request_id
                && existing.grant_id == request.grant_id
                && existing.delivery_token_sha256 == token_sha
                && existing.runtime_generation_id == request.runtime_generation_id
                && existing.provider_instance_id == request.provider_instance_id
                && existing.settings_id == request.settings_id
                && existing.envelope_nonce == request.envelope_nonce
                && existing.tail_resume_token == request.tail_resume_token
            {
                return Ok(existing);
            }
            return Err("native F preparation key conflicts with immutable record".into());
        }
        let identity_json = Self::recipient_identity_json(recipient)?;
        let grant: (String, i64, String, String, String, String, String, String, String, i64, String) =
            self.sidecar.mailbox().conn.query_row(
                "SELECT session_id,seq,source_id,attempt_id,lane_id,source_generation,
                        root_id,owner_generation,payload_sha256,payload_byte_len,phase
                 FROM fresh_recipient_grant WHERE grant_id=?1 AND delivery_request_id=?2
                   AND delivery_token=?3 AND recipient_identity=?4",
                params![request.grant_id, request.delivery_request_id,
                    request.delivery_token, identity_json],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,
                       r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?,r.get(9)?,r.get(10)?)),
            ).map_err(|_| "native F exact original grant/token absent".to_string())?;
        let (session_id, seq, source_id, attempt_id, lane_id, source_generation,
            root_id, owner_generation, payload_sha256, payload_byte_len, phase) = grant;
        if phase == "acked" || lane_id != self.identity.lane_id
            || source_generation != self.identity.source_generation {
            return Err("native F grant is ACKed or belongs to another lane".into());
        }
        let binding: (String, String) = self.sidecar.mailbox().conn.query_row(
            "SELECT root_id,owner_generation FROM fresh_recipient_binding
             WHERE session_id=?1 AND recipient_identity=?2 AND source_generation=?3",
            params![session_id, identity_json, source_generation],
            |r| Ok((r.get(0)?,r.get(1)?)),
        ).map_err(|_| "native F original recipient binding absent".to_string())?;
        if binding != (root_id.clone(), owner_generation.clone()) {
            return Err("native F original root binding changed".into());
        }
        self.require_state_recipient_attachment(
            &session_id, &identity_json, &root_id, &owner_generation,
        )?;
        let exact_session: FreshV30Session = self.sidecar.mailbox().conn.query_row(
            "SELECT lane_id,source_generation,session_id,request_id,allocation_id
             FROM fresh_lane_session WHERE session_id=?1",
            [&session_id],
            |r| Ok(FreshV30Session { lane_id:r.get(0)?, source_generation:r.get(1)?,
                session_id:r.get(2)?, request_id:r.get(3)?, allocation_id:r.get(4)? }),
        ).map_err(|_| "native F session absent".to_string())?;
        let accepted = self.accepted_source_for_row(
            &exact_session, seq, &payload_sha256, payload_byte_len,
            &root_id, &owner_generation,
        )?;
        if accepted != (source_id.clone(), attempt_id.clone()) {
            return Err("native F accepted W source changed".into());
        }
        let payload = self.lookup_payload(&lane_id, &session_id, seq)?;
        if i64::try_from(payload.len()).ok() != Some(payload_byte_len)
            || sha256_hex(&payload) != payload_sha256 {
            return Err("native F retained W payload changed".into());
        }
        let generation = match self.sidecar.mailbox().runtime_lifecycle_reader()
            .session_generation_projection(&session_id).map_err(|e| e.to_string())? {
            SessionGenerationProjection::One(row) => row,
            _ => return Err("native F resident PTY generation absent or ambiguous".into()),
        };
        if generation.generation_id.to_string() != request.runtime_generation_id
            || generation.lifecycle_state != RuntimeLifecycleState::Running
            || generation.runtime_mode != "pty_interactive"
            || generation.session_id.as_deref() != Some(session_id.as_str())
            || generation.provider_name.is_empty() {
            return Err("native F selected resident PTY generation mismatched".into());
        }
        let path = generation.pty_control_path.as_deref()
            .filter(|path| !path.is_empty()).ok_or("native F PTY endpoint absent")?;
        let socket_meta = fs::symlink_metadata(path)
            .map_err(|_| "native F PTY endpoint absent")?;
        if !socket_meta.file_type().is_socket() || socket_meta.ino() == 0 {
            return Err("native F PTY endpoint is not a socket".into());
        }
        let ExactProcessEvidence::Recorded(creator) = &generation.creator_process_evidence else {
            return Err("native F PTY creator identity absent".into());
        };
        if creator.os_pid != i64::from(recipient.host_pid)
            || creator.os_boot_id != recipient.boot_id
            || i64::try_from(recipient.starttime_ticks).ok()
                != Some(creator.os_pid_starttime_ticks)
        {
            return Err("native F PTY creator is not original root actor".into());
        }
        let ExactProcessEvidence::Recorded(provider_process) = &generation.exact_process_evidence else {
            return Err("native F resident provider identity absent".into());
        };
        if pid_identity::read_live_process_identity(provider_process.os_pid)?
            != Some(provider_process.clone()) {
            return Err("native F resident provider process changed or exited".into());
        }
        if payload.len() > 192 * 1024 {
            return Err("native F payload exceeds bounded input envelope".into());
        }
        let encoded = base64::engine::general_purpose::STANDARD.encode(&payload);
        let envelope_text = format!(
            "[Oulipoly native F v1]\nnonce: {}\nsource: {}\nsession: {}\nrow: {}\npayload-sha256: {}\npayload-base64: {}\n[/Oulipoly native F v1]",
            request.envelope_nonce, source_id, session_id, seq, payload_sha256, encoded,
        );
        let record = FreshNativeFPreparation {
            preparation_request_id: request.preparation_request_id.clone(),
            delivery_request_id: request.delivery_request_id.clone(),
            grant_id: request.grant_id.clone(),
            delivery_token_sha256: token_sha,
            recipient_identity: recipient.clone(), root_id, owner_generation,
            session_id: session_id.clone(), seq, source_id, attempt_id, lane_id,
            source_generation, payload_sha256, payload_byte_len,
            runtime_generation_id: request.runtime_generation_id.clone(),
            runtime_spawn_invocation_uuid: generation.spawn_invocation_uuid,
            pty_control_path: path.to_string(),
            pty_control_device: socket_meta.dev(), pty_control_inode: socket_meta.ino(),
            provider_account: generation.provider_name,
            provider_instance_id: request.provider_instance_id.clone(),
            settings_id: request.settings_id.clone(), provider_session_id: session_id,
            envelope_nonce: request.envelope_nonce.clone(),
            envelope_sha256: sha256_hex(envelope_text.as_bytes()), envelope_text,
            tail_resume_token: request.tail_resume_token.clone(),
            prepared_at: Utc::now().to_rfc3339(),
        };
        let json = serde_json::to_string(&record).map_err(|e| e.to_string())?;
        self.sidecar.mailbox_mut().conn.execute(
            "INSERT INTO fresh_native_f_preparation
             (preparation_request_id,grant_id,delivery_request_id,delivery_token_sha256,
              recipient_identity,root_id,owner_generation,session_id,seq,source_id,attempt_id,
              lane_id,source_generation,payload_sha256,payload_byte_len,runtime_generation_id,
              runtime_spawn_invocation_uuid,pty_control_path,pty_control_device,pty_control_inode,provider_account,
              provider_instance_id,settings_id,provider_session_id,envelope_nonce,
              envelope_text,envelope_sha256,tail_resume_token,record_json,prepared_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,
                    ?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30)",
            params![record.preparation_request_id,record.grant_id,record.delivery_request_id,
                record.delivery_token_sha256,identity_json,record.root_id,record.owner_generation,
                record.session_id,record.seq,record.source_id,record.attempt_id,record.lane_id,
                record.source_generation,record.payload_sha256,record.payload_byte_len,
                record.runtime_generation_id,record.runtime_spawn_invocation_uuid,
                record.pty_control_path,
                i64::try_from(record.pty_control_device).map_err(|_| "native F socket device overflow")?,
                i64::try_from(record.pty_control_inode).map_err(|_| "native F socket inode overflow")?,
                record.provider_account,record.provider_instance_id,
                record.settings_id,record.provider_session_id,record.envelope_nonce,
                record.envelope_text,record.envelope_sha256,record.tail_resume_token,
                json,record.prepared_at],
        ).map_err(|e| format!("native F preparation refused: {e}"))?;
        Ok(record)
    }
}
