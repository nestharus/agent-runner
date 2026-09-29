#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshSuccessorOffer {
    pub offer_request_id: String,
    pub generation: String,
    pub session_id: String,
    pub seq: i64,
    pub source_id: String,
    pub attempt_id: String,
    pub lane_id: String,
    pub source_generation: String,
    pub root_id: String,
    pub owner_generation: String,
    pub original_identity: FreshRecipientIdentity,
    pub successor_identity: FreshRecipientIdentity,
    pub payload_sha256: String,
    pub payload_byte_len: i64,
    pub offered_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshSuccessorAdmission {
    pub offer: FreshSuccessorOffer,
    pub admitted_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshSuccessorDeliveryReadback {
    pub grant: FreshDeliveryReadback,
    pub generation: String,
    pub offer_request_id: String,
    pub receipt_sha256: Option<String>,
}

pub struct FreshSuccessorDelivery {
    pub readback: FreshSuccessorDeliveryReadback,
    pub delivery_token: String,
    pub payload: Vec<u8>,
    pub receipt_path: String,
}

/// Written by the receiving Runner from the actual F reply, with exclusive
/// creation and file/parent fsync. Broker reads this file; a State lookup is
/// never treated as a receiver receipt.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshSuccessorReceiverReceipt {
    pub grant_id: String,
    pub delivery_request_id: String,
    pub offer_request_id: String,
    pub generation: String,
    pub session_id: String,
    pub seq: i64,
    pub source_id: String,
    pub attempt_id: String,
    pub successor_identity: FreshRecipientIdentity,
    pub payload_sha256: String,
    pub payload_byte_len: i64,
    pub delivery_token_sha256: String,
}

impl FreshV30Lane {
    pub fn admitted_successor(
        &self,
        offer_request_id: &str,
        actor: &FreshRecipientIdentity,
    ) -> Result<FreshSuccessorAdmission, String> {
        let admission = self.read_successor_admission(offer_request_id, actor)?
            .ok_or("successor admission absent")?;
        if admission.offer.successor_identity != *actor {
            return Err("pinned peer is not admitted successor".into());
        }
        Ok(admission)
    }

    fn successor_receipt_root(&self) -> Result<std::path::PathBuf, String> {
        let broker_root = self.state_path.parent().and_then(|p| p.parent())
            .ok_or("fresh broker root absent")?;
        let parent = broker_root.parent().ok_or("fresh broker root parent absent")?;
        let name = broker_root.file_name().and_then(|n| n.to_str())
            .ok_or("fresh broker root name absent")?;
        Ok(parent.join(format!("{name}-successor-receipts")))
    }

    fn checked_successor_receipt_directory(&self, uid: u32) -> Result<std::path::PathBuf, String> {
        let root = self.successor_receipt_root()?;
        let root_meta = fs::symlink_metadata(&root).map_err(|e| e.to_string())?;
        if !root_meta.is_dir() || root_meta.file_type().is_symlink() || root_meta.uid() != 0
            || root_meta.mode() & 0o777 != 0o711 {
            return Err("successor receipt root changed".into());
        }
        let parent = root.join(uid.to_string());
        let meta = fs::symlink_metadata(&parent).map_err(|e| e.to_string())?;
        if !meta.is_dir() || meta.file_type().is_symlink() || meta.uid() != uid
            || meta.mode() & 0o777 != 0o700 {
            return Err("successor receipt directory changed".into());
        }
        Ok(parent)
    }

    pub fn successor_receipt_path(&self, grant_id: &str, uid: u32) -> Result<std::path::PathBuf, String> {
        validate_request_id(grant_id)?;
        Ok(self.checked_successor_receipt_directory(uid)?.join(format!("{grant_id}.json")))
    }

    fn ensure_successor_receipt_directory(&self, uid: u32) -> Result<(), String> {
        require_root()?;
        let root = self.successor_receipt_root()?;
        let parent = root.parent().ok_or("successor receipt root parent absent")?;
        let parent_meta = fs::symlink_metadata(parent).map_err(|e| e.to_string())?;
        if !parent_meta.is_dir() || parent_meta.file_type().is_symlink()
            || parent_meta.uid() != 0 || parent_meta.mode() & 0o022 != 0 {
            return Err("successor receipt root ancestry untrusted".into());
        }
        match fs::create_dir(&root) {
            Ok(()) => {
                fs::set_permissions(&root, fs::Permissions::from_mode(0o711))
                    .map_err(|e| e.to_string())?;
                File::open(&root).and_then(|f| f.sync_all()).map_err(|e| e.to_string())?;
                File::open(parent).and_then(|f| f.sync_all()).map_err(|e| e.to_string())?;
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.to_string()),
        }
        let root_meta = fs::symlink_metadata(&root).map_err(|e| e.to_string())?;
        if !root_meta.is_dir() || root_meta.file_type().is_symlink() || root_meta.uid() != 0
            || root_meta.mode() & 0o777 != 0o711 {
            return Err("successor receipt root changed".into());
        }
        let user_dir = root.join(uid.to_string());
        match fs::create_dir(&user_dir) {
            Ok(()) => {
                fs::set_permissions(&user_dir, fs::Permissions::from_mode(0o700))
                    .map_err(|e| e.to_string())?;
                let c_path = std::ffi::CString::new(user_dir.as_os_str().as_bytes())
                    .map_err(|e| e.to_string())?;
                if unsafe { libc::chown(c_path.as_ptr(), uid, u32::MAX) } != 0 {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                File::open(&user_dir).and_then(|f| f.sync_all()).map_err(|e| e.to_string())?;
                File::open(&root).and_then(|f| f.sync_all()).map_err(|e| e.to_string())?;
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.to_string()),
        }
        self.checked_successor_receipt_directory(uid).map(|_| ())
    }

    fn successor_grant_by_request(
        &self, delivery_request_id: &str, actor: &FreshRecipientIdentity, uid: u32,
    ) -> Result<Option<(FreshSuccessorDeliveryReadback, String)>, String> {
        validate_request_id(delivery_request_id)?;
        let identity = Self::recipient_identity_json(actor)?;
        self.sidecar.mailbox().conn.query_row(
            "SELECT g.grant_id,g.session_id,g.seq,g.source_id,g.attempt_id,g.lane_id,
                    g.source_generation,g.root_id,g.owner_generation,g.payload_sha256,
                    g.payload_byte_len,g.phase,g.generation,g.offer_request_id,g.delivery_token,
                    r.receipt_sha256
             FROM fresh_successor_grant g LEFT JOIN fresh_successor_receipt r
               ON r.grant_id=g.grant_id
             WHERE g.delivery_request_id=?1 AND g.successor_identity=?2 AND g.recipient_uid=?3",
            params![delivery_request_id,identity,uid],
            |r| Ok((
                FreshSuccessorDeliveryReadback {
                    grant: FreshDeliveryReadback {
                        grant_id:r.get(0)?,session_id:r.get(1)?,seq:r.get(2)?,
                        source_id:r.get(3)?,attempt_id:r.get(4)?,lane_id:r.get(5)?,
                        source_generation:r.get(6)?,root_id:r.get(7)?,
                        owner_generation:r.get(8)?,payload_sha256:r.get(9)?,
                        payload_byte_len:r.get(10)?,phase:r.get(11)?,
                    },
                    generation:r.get(12)?,offer_request_id:r.get(13)?,
                    receipt_sha256:r.get(15)?,
                },r.get(14)?
            )),
        ).optional().map_err(|e| e.to_string())
    }

    pub fn read_successor_delivery(
        &self, delivery_request_id: &str, actor: &FreshRecipientIdentity, uid: u32,
    ) -> Result<Option<FreshSuccessorDeliveryReadback>, String> {
        let Some((read, _)) = self.successor_grant_by_request(delivery_request_id, actor, uid)? else {
            return Ok(None);
        };
        let admission = self.admitted_successor(&read.offer_request_id, actor)?;
        self.verify_successor_grant(&read, &admission)?;
        Ok(Some(read))
    }

    fn verify_successor_grant(
        &self, read: &FreshSuccessorDeliveryReadback,
        admission: &FreshSuccessorAdmission,
    ) -> Result<(), String> {
        let offer = &admission.offer;
        let g = &read.grant;
        if read.generation != offer.generation
            || read.offer_request_id != offer.offer_request_id
            || (g.session_id.as_str(),g.seq,g.source_id.as_str(),g.attempt_id.as_str(),
                g.lane_id.as_str(),g.source_generation.as_str(),g.root_id.as_str(),
                g.owner_generation.as_str(),g.payload_sha256.as_str(),g.payload_byte_len)
             != (offer.session_id.as_str(),offer.seq,offer.source_id.as_str(),
                 offer.attempt_id.as_str(),offer.lane_id.as_str(),
                 offer.source_generation.as_str(),offer.root_id.as_str(),
                 offer.owner_generation.as_str(),offer.payload_sha256.as_str(),
                 offer.payload_byte_len)
        {
            return Err("successor grant differs from cross-store admission".into());
        }
        let payload = self.lookup_payload(&g.lane_id, &g.session_id, g.seq)?;
        if i64::try_from(payload.len()).ok() != Some(g.payload_byte_len)
            || sha256_hex(&payload) != g.payload_sha256 {
            return Err("successor grant payload changed".into());
        }
        Ok(())
    }

    pub fn submit_successor_delivery(
        &mut self, offer_request_id: &str, delivery_request_id: &str,
        actor: &FreshRecipientIdentity, uid: u32,
    ) -> Result<FreshSuccessorDelivery, String> {
        validate_request_id(delivery_request_id)?;
        let admission = self.admitted_successor(offer_request_id, actor)?;
        if let Some((existing, _)) = self.successor_grant_by_request(delivery_request_id, actor, uid)? {
            self.verify_successor_grant(&existing, &admission)?;
            return Err("successor F already granted; use exact readback/recovery".into());
        }
        let offer = &admission.offer;
        let session = self.read_session_for_successor(offer)?;
        let (sha,len,attempt) = self.require_pending_successor_row(
            &session,offer.seq,&offer.source_id,&offer.root_id,&offer.owner_generation,
            Some(&offer.generation)
        )?;
        if (sha.as_str(),len,attempt.as_str()) !=
            (offer.payload_sha256.as_str(),offer.payload_byte_len,offer.attempt_id.as_str()) {
            return Err("successor F row/source differs from admission".into());
        }
        let payload = self.lookup_payload(&offer.lane_id,&offer.session_id,offer.seq)?;
        if i64::try_from(payload.len()).ok() != Some(len) || sha256_hex(&payload) != sha {
            return Err("successor F bytes differ from admitted payload".into());
        }
        self.ensure_successor_receipt_directory(uid)?;
        let grant_id = Uuid::new_v4().to_string();
        let token = Uuid::new_v4().to_string();
        let identity = Self::recipient_identity_json(actor)?;
        let changed = self.sidecar.mailbox().conn.execute(
            "INSERT INTO fresh_successor_grant
             (grant_id,delivery_request_id,delivery_token,generation,offer_request_id,
              session_id,seq,source_id,attempt_id,lane_id,source_generation,root_id,
              owner_generation,successor_identity,recipient_uid,payload_sha256,payload_byte_len,phase,created_at)
             SELECT ?1,?2,?3,a.generation,a.offer_request_id,a.session_id,a.seq,
                    a.source_id,a.attempt_id,a.lane_id,a.source_generation,a.root_id,
                    a.owner_generation,a.successor_identity,?10,a.payload_sha256,
                    a.payload_byte_len,'unknown',?5
             FROM fresh_successor_admission a
             JOIN mailbox m ON m.session_id=a.session_id AND m.seq=a.seq
             JOIN fresh_recipient_row_source r ON r.session_id=a.session_id AND r.seq=a.seq
             WHERE a.offer_request_id=?4 AND a.successor_identity=?6
               AND a.generation=?7 AND a.payload_sha256=?8 AND a.payload_byte_len=?9
               AND m.delivered_at IS NULL AND m.payload_sha256=a.payload_sha256
               AND m.payload_byte_len=a.payload_byte_len
               AND m.payload_retention_policy='until_terminal_disposition'
               AND r.source_id=a.source_id AND r.attempt_id=a.attempt_id
               AND r.payload_sha256=a.payload_sha256 AND r.payload_byte_len=a.payload_byte_len
               AND NOT EXISTS (SELECT 1 FROM fresh_recipient_grant g
                 WHERE g.session_id=a.session_id AND g.seq=a.seq)",
            params![grant_id,delivery_request_id,token,offer_request_id,
                Utc::now().to_rfc3339(),identity,offer.generation,sha,len,uid],
        ).map_err(|e| format!("successor F grant refused: {e}"))?;
        if changed != 1 { return Err("successor F exact row changed before grant".into()); }
        let (readback, _) = self.successor_grant_by_request(delivery_request_id, actor, uid)?
            .ok_or("successor F grant readback absent")?;
        self.verify_successor_grant(&readback, &admission)?;
        Ok(FreshSuccessorDelivery {
            receipt_path:self.successor_receipt_path(&grant_id, uid)?
                .to_string_lossy().into_owned(),
            readback,delivery_token:token,payload,
        })
    }

    pub fn recover_successor_delivery(
        &self, delivery_request_id: &str, actor: &FreshRecipientIdentity, uid: u32,
    ) -> Result<FreshSuccessorDelivery, String> {
        let (readback, token) = self.successor_grant_by_request(delivery_request_id, actor, uid)?
            .ok_or("successor F request has no durable grant")?;
        let admission = self.admitted_successor(&readback.offer_request_id, actor)?;
        self.verify_successor_grant(&readback, &admission)?;
        if readback.grant.phase == "acked" {
            return Err("successor F already ACKed".into());
        }
        let payload = self.lookup_payload(
            &readback.grant.lane_id,&readback.grant.session_id,readback.grant.seq
        )?;
        Ok(FreshSuccessorDelivery {
            receipt_path:self.successor_receipt_path(&readback.grant.grant_id, uid)?
                .to_string_lossy().into_owned(),
            readback,delivery_token:token,payload,
        })
    }

    pub fn mark_successor_submitted(&mut self, grant_id: &str) -> Result<(), String> {
        if self.sidecar.mailbox_mut().conn.execute(
            "UPDATE fresh_successor_grant SET phase='submitted',submitted_at=?2
             WHERE grant_id=?1 AND phase='unknown'",
            params![grant_id,Utc::now().to_rfc3339()],
        ).map_err(|e| e.to_string())? != 1 {
            return Err("successor F submission state changed".into());
        }
        Ok(())
    }

    fn read_successor_receipt_file(
        &self, read: &FreshSuccessorDeliveryReadback, token: &str,
        actor: &FreshRecipientIdentity, uid: u32,
    ) -> Result<(String, i64, i64), String> {
        use std::os::unix::fs::MetadataExt as _;
        let path = self.successor_receipt_path(&read.grant.grant_id, uid)?;
        let file = OpenOptions::new().read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path).map_err(|e| format!("successor receiver receipt absent: {e}"))?;
        let meta = file.metadata().map_err(|e| e.to_string())?;
        if !meta.is_file() || meta.uid() != uid || meta.nlink() != 1
            || meta.mode() & 0o777 != 0o400 || meta.len() > 8192 {
            return Err("successor receiver receipt file untrusted".into());
        }
        let mut bytes = Vec::new();
        file.take(8193).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        if bytes.len() as u64 != meta.len() {
            return Err("successor receiver receipt changed during read".into());
        }
        let receipt: FreshSuccessorReceiverReceipt =
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        let g = &read.grant;
        if receipt != (FreshSuccessorReceiverReceipt {
            grant_id:g.grant_id.clone(),
            delivery_request_id:self.sidecar.mailbox().conn.query_row(
                "SELECT delivery_request_id FROM fresh_successor_grant WHERE grant_id=?1",
                [&g.grant_id],|r| r.get(0)).map_err(|e| e.to_string())?,
            offer_request_id:read.offer_request_id.clone(),
            generation:read.generation.clone(),
            session_id:g.session_id.clone(),seq:g.seq,
            source_id:g.source_id.clone(),attempt_id:g.attempt_id.clone(),
            successor_identity:actor.clone(),payload_sha256:g.payload_sha256.clone(),
            payload_byte_len:g.payload_byte_len,
            delivery_token_sha256:sha256_hex(token.as_bytes()),
        }) {
            return Err("successor receiver receipt does not match F/peer/token".into());
        }
        Ok((sha256_hex(&bytes),meta.dev() as i64,meta.ino() as i64))
    }

    pub fn certify_successor_receipt(
        &mut self, delivery_request_id: &str, actor: &FreshRecipientIdentity, uid: u32,
    ) -> Result<String, String> {
        let (read,token) = self.successor_grant_by_request(delivery_request_id,actor,uid)?
            .ok_or("successor F grant absent for receipt")?;
        let admission = self.admitted_successor(&read.offer_request_id,actor)?;
        self.verify_successor_grant(&read,&admission)?;
        if read.grant.phase == "acked" { return Err("successor receipt already ACKed".into()); }
        let (digest,device,inode) = self.read_successor_receipt_file(&read,&token,actor,uid)?;
        let identity = Self::recipient_identity_json(actor)?;
        self.sidecar.mailbox().conn.execute(
            "INSERT INTO fresh_successor_receipt
             (grant_id,delivery_request_id,generation,session_id,seq,source_id,attempt_id,
              successor_identity,payload_sha256,payload_byte_len,delivery_token_sha256,
              receipt_sha256,receipt_device,receipt_inode,certified_at)
             SELECT g.grant_id,g.delivery_request_id,g.generation,g.session_id,g.seq,
                    g.source_id,g.attempt_id,g.successor_identity,g.payload_sha256,
                    g.payload_byte_len,?4,?5,?6,?7,?8
             FROM fresh_successor_grant g
             JOIN fresh_successor_admission a ON a.generation=g.generation
             JOIN fresh_recipient_row_source r ON r.session_id=g.session_id AND r.seq=g.seq
             WHERE g.delivery_request_id=?1 AND g.successor_identity=?2
               AND g.grant_id=?3 AND g.phase IN ('unknown','submitted')
               AND a.offer_request_id=g.offer_request_id AND a.session_id=g.session_id
               AND a.seq=g.seq AND a.source_id=g.source_id AND a.attempt_id=g.attempt_id
               AND a.successor_identity=g.successor_identity
               AND a.payload_sha256=g.payload_sha256 AND a.payload_byte_len=g.payload_byte_len
               AND r.source_id=g.source_id AND r.attempt_id=g.attempt_id
               AND r.payload_sha256=g.payload_sha256 AND r.payload_byte_len=g.payload_byte_len
             ON CONFLICT(grant_id) DO NOTHING",
            params![delivery_request_id,identity,read.grant.grant_id,
                sha256_hex(token.as_bytes()),digest,device,inode,Utc::now().to_rfc3339()],
        ).map_err(|e| e.to_string())?;
        let persisted: Option<(String,i64,i64)> = self.sidecar.mailbox().conn.query_row(
            "SELECT receipt_sha256,receipt_device,receipt_inode FROM fresh_successor_receipt
             WHERE grant_id=?1 AND delivery_request_id=?2 AND generation=?3
               AND successor_identity=?4 AND payload_sha256=?5 AND payload_byte_len=?6
               AND delivery_token_sha256=?7",
            params![read.grant.grant_id,delivery_request_id,read.generation,identity,
                read.grant.payload_sha256,read.grant.payload_byte_len,
                sha256_hex(token.as_bytes())],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
        ).optional().map_err(|e| e.to_string())?;
        if persisted != Some((digest.clone(),device,inode)) {
            return Err("successor receipt certification conflicts with durable record".into());
        }
        Ok(digest)
    }

    pub fn read_successor_receipt(
        &self, delivery_request_id: &str, actor: &FreshRecipientIdentity, uid: u32,
    ) -> Result<Option<String>, String> {
        let Some((read,token)) = self.successor_grant_by_request(delivery_request_id,actor,uid)?
            else { return Ok(None); };
        let admission = self.admitted_successor(&read.offer_request_id,actor)?;
        self.verify_successor_grant(&read,&admission)?;
        let identity = Self::recipient_identity_json(actor)?;
        let durable: Option<(String,i64,i64)> = self.sidecar.mailbox().conn.query_row(
            "SELECT receipt_sha256,receipt_device,receipt_inode FROM fresh_successor_receipt
             WHERE grant_id=?1 AND delivery_request_id=?2 AND generation=?3
               AND session_id=?4 AND seq=?5 AND source_id=?6 AND attempt_id=?7
               AND successor_identity=?8 AND payload_sha256=?9 AND payload_byte_len=?10
               AND delivery_token_sha256=?11",
            params![read.grant.grant_id,delivery_request_id,read.generation,
                read.grant.session_id,read.grant.seq,read.grant.source_id,
                read.grant.attempt_id,identity,read.grant.payload_sha256,
                read.grant.payload_byte_len,sha256_hex(token.as_bytes())],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
        ).optional().map_err(|e| e.to_string())?;
        if let Some((sha,dev,ino)) = &durable {
            if self.read_successor_receipt_file(&read,&token,actor,uid)? !=
                (sha.clone(),*dev,*ino) {
                return Err("successor receiver receipt changed after certification".into());
            }
        }
        Ok(durable.map(|v| v.0))
    }

    pub fn acknowledge_successor_delivery(
        &mut self, delivery_request_id: &str, token: &str,
        actor: &FreshRecipientIdentity, uid: u32,
    ) -> Result<FreshSuccessorDeliveryReadback, String> {
        validate_request_id(token)?;
        let (read,stored_token) = self.successor_grant_by_request(delivery_request_id,actor,uid)?
            .ok_or("successor F grant absent for ACK")?;
        if read.grant.phase == "acked" || stored_token != token {
            return Err("successor ACK token consumed or wrong".into());
        }
        let admission = self.admitted_successor(&read.offer_request_id,actor)?;
        self.verify_successor_grant(&read,&admission)?;
        let receipt = self.read_successor_receipt(delivery_request_id,actor,uid)?
            .ok_or("successor receiver receipt absent; ACK refused")?;
        let identity = Self::recipient_identity_json(actor)?;
        let now = Utc::now().to_rfc3339();
        let tx = self.sidecar.mailbox_mut().conn.transaction_with_behavior(
            TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        let exact: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM fresh_successor_grant g
             JOIN fresh_successor_admission a ON a.generation=g.generation
             JOIN fresh_successor_receipt c ON c.grant_id=g.grant_id
             JOIN fresh_recipient_row_source r ON r.session_id=g.session_id AND r.seq=g.seq
             WHERE g.delivery_request_id=?1 AND g.delivery_token=?2
               AND g.successor_identity=?3 AND g.grant_id=?4 AND g.phase IN ('unknown','submitted')
               AND a.offer_request_id=g.offer_request_id AND a.session_id=g.session_id
               AND a.seq=g.seq AND a.source_id=g.source_id AND a.attempt_id=g.attempt_id
               AND a.successor_identity=g.successor_identity
               AND a.payload_sha256=g.payload_sha256 AND a.payload_byte_len=g.payload_byte_len
               AND c.generation=g.generation AND c.delivery_request_id=g.delivery_request_id
               AND c.session_id=g.session_id AND c.seq=g.seq AND c.source_id=g.source_id
               AND c.attempt_id=g.attempt_id AND c.successor_identity=g.successor_identity
               AND c.payload_sha256=g.payload_sha256 AND c.payload_byte_len=g.payload_byte_len
               AND c.delivery_token_sha256=?5 AND c.receipt_sha256=?6
               AND r.source_id=g.source_id AND r.attempt_id=g.attempt_id
               AND r.payload_sha256=g.payload_sha256 AND r.payload_byte_len=g.payload_byte_len)",
            params![delivery_request_id,token,identity,read.grant.grant_id,
                sha256_hex(token.as_bytes()),receipt],
            |r| r.get(0),
        ).map_err(|e| e.to_string())?;
        if !exact { return Err("successor ACK evidence does not join admitted F".into()); }
        if tx.execute(
            "UPDATE mailbox SET delivered_at=?3,delivered_by_invocation_uuid=?4,
              delivery_attempts=delivery_attempts+1,delivery_error=NULL
             WHERE session_id=?1 AND seq=?2 AND delivered_at IS NULL
               AND payload_sha256=?5 AND payload_byte_len=?6",
            params![read.grant.session_id,read.grant.seq,now,read.grant.grant_id,
                read.grant.payload_sha256,read.grant.payload_byte_len],
        ).map_err(|e| e.to_string())? != 1 {
            return Err("successor ACK row changed or already delivered".into());
        }
        if tx.execute(
            "UPDATE fresh_successor_grant SET phase='acked',acknowledged_at=?2
             WHERE grant_id=?1 AND phase IN ('unknown','submitted')",
            params![read.grant.grant_id,now],
        ).map_err(|e| e.to_string())? != 1 {
            return Err("successor F grant changed before ACK".into());
        }
        if tx.execute(
            "INSERT INTO fresh_successor_ack_evidence
             (grant_id,delivery_request_id,generation,session_id,seq,source_id,attempt_id,
              successor_identity,payload_sha256,payload_byte_len,delivery_token_sha256,
              receipt_sha256,acknowledged_at)
             SELECT g.grant_id,g.delivery_request_id,g.generation,g.session_id,g.seq,
                    g.source_id,g.attempt_id,g.successor_identity,g.payload_sha256,
                    g.payload_byte_len,c.delivery_token_sha256,c.receipt_sha256,?2
             FROM fresh_successor_grant g JOIN fresh_successor_receipt c
               ON c.grant_id=g.grant_id
             JOIN mailbox m ON m.session_id=g.session_id AND m.seq=g.seq
             WHERE g.grant_id=?1 AND g.phase='acked' AND g.acknowledged_at=?2
               AND m.delivered_at=?2 AND m.delivered_by_invocation_uuid=?1
               AND c.receipt_sha256=?3",
            params![read.grant.grant_id,now,receipt],
        ).map_err(|e| e.to_string())? != 1 {
            return Err("successor ACK immutable evidence absent".into());
        }
        tx.commit().map_err(|e| e.to_string())?;
        self.read_successor_ack(delivery_request_id,actor,uid)?
            .ok_or("successor ACK readback absent".into())
    }

    pub fn read_successor_ack(
        &self, delivery_request_id: &str, actor: &FreshRecipientIdentity, uid: u32,
    ) -> Result<Option<FreshSuccessorDeliveryReadback>, String> {
        let Some(read) = self.read_successor_delivery(delivery_request_id,actor,uid)?
            else { return Ok(None); };
        if read.grant.phase != "acked" { return Ok(None); }
        let receipt = self.read_successor_receipt(delivery_request_id,actor,uid)?
            .ok_or("successor ACK lacks durable receiver receipt")?;
        let identity = Self::recipient_identity_json(actor)?;
        let exact: bool = self.sidecar.mailbox().conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM fresh_successor_ack_evidence e
             JOIN fresh_successor_grant g ON g.grant_id=e.grant_id
             JOIN fresh_successor_receipt c ON c.grant_id=e.grant_id
             JOIN fresh_successor_admission a ON a.generation=e.generation
             JOIN fresh_recipient_row_source r ON r.session_id=e.session_id AND r.seq=e.seq
             JOIN mailbox m ON m.session_id=e.session_id AND m.seq=e.seq
             WHERE e.delivery_request_id=?1 AND e.grant_id=?2 AND e.generation=?3
               AND e.session_id=?4 AND e.seq=?5 AND e.source_id=?6 AND e.attempt_id=?7
               AND e.successor_identity=?8 AND e.payload_sha256=?9 AND e.payload_byte_len=?10
               AND e.receipt_sha256=?11 AND e.receipt_sha256=c.receipt_sha256
               AND e.delivery_token_sha256=c.delivery_token_sha256
               AND g.delivery_request_id=e.delivery_request_id AND g.generation=e.generation
               AND g.session_id=e.session_id AND g.seq=e.seq
               AND g.source_id=e.source_id AND g.attempt_id=e.attempt_id
               AND g.successor_identity=e.successor_identity
               AND g.payload_sha256=e.payload_sha256 AND g.payload_byte_len=e.payload_byte_len
               AND g.phase='acked' AND g.acknowledged_at=e.acknowledged_at
               AND a.offer_request_id=g.offer_request_id
               AND a.session_id=e.session_id AND a.seq=e.seq
               AND a.source_id=e.source_id AND a.attempt_id=e.attempt_id
               AND a.successor_identity=e.successor_identity
               AND a.payload_sha256=e.payload_sha256 AND a.payload_byte_len=e.payload_byte_len
               AND r.source_id=e.source_id AND r.attempt_id=e.attempt_id
               AND r.payload_sha256=e.payload_sha256 AND r.payload_byte_len=e.payload_byte_len
               AND m.delivered_at=e.acknowledged_at
               AND m.delivered_by_invocation_uuid=e.grant_id
               AND m.payload_sha256=e.payload_sha256 AND m.payload_byte_len=e.payload_byte_len)",
            params![delivery_request_id,read.grant.grant_id,read.generation,
                read.grant.session_id,read.grant.seq,read.grant.source_id,read.grant.attempt_id,
                identity,read.grant.payload_sha256,read.grant.payload_byte_len,receipt],
            |r| r.get(0),
        ).map_err(|e| e.to_string())?;
        if !exact { return Err("successor ACK readback cross-store join incomplete".into()); }
        Ok(Some(read))
    }

    /// The Broker supplies `successor` from its pinned socket peer. The offer
    /// identifies a process generation but gives it no delivery authority.
    pub fn offer_successor(
        &self,
        request_id: &str,
        session: &FreshV30Session,
        seq: i64,
        source_id: &str,
        successor: &FreshRecipientIdentity,
    ) -> Result<FreshSuccessorOffer, String> {
        validate_request_id(request_id)?;
        if let Some(existing) = self.read_successor_offer(request_id, successor)? {
            if existing.session_id != session.session_id
                || existing.seq != seq
                || existing.source_id != source_id
            {
                return Err("successor offer request changed its exact row/source".into());
            }
            return Ok(existing);
        }
        self.require_session(session)?;
        let original_json: String = self
            .sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT recipient_identity FROM fresh_recipient_binding WHERE session_id=?1
             AND source_generation=?2",
                params![session.session_id, self.identity.source_generation],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or("fresh original recipient binding absent")?;
        let original: FreshRecipientIdentity =
            serde_json::from_str(&original_json).map_err(|e| e.to_string())?;
        if *successor == original {
            return Err("successor must be a distinct process generation".into());
        }
        let (root, owner) = self.recipient_binding(session, &original)?;
        let (sha, len, attempt) =
            self.require_pending_successor_row(session, seq, source_id, &root, &owner, None)?;
        let offer = FreshSuccessorOffer {
            offer_request_id: request_id.into(),
            generation: Uuid::new_v4().to_string(),
            session_id: session.session_id.clone(),
            seq,
            source_id: source_id.into(),
            attempt_id: attempt,
            lane_id: self.identity.lane_id.clone(),
            source_generation: self.identity.source_generation.clone(),
            root_id: root,
            owner_generation: owner,
            original_identity: original,
            successor_identity: successor.clone(),
            payload_sha256: sha,
            payload_byte_len: len,
            offered_at: Utc::now().to_rfc3339(),
        };
        self.sidecar
            .mailbox()
            .conn
            .execute(
                "INSERT INTO fresh_successor_offer VALUES
             (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
                params![
                    offer.offer_request_id,
                    offer.generation,
                    offer.session_id,
                    offer.seq,
                    offer.source_id,
                    offer.attempt_id,
                    offer.lane_id,
                    offer.source_generation,
                    offer.root_id,
                    offer.owner_generation,
                    original_json,
                    Self::recipient_identity_json(successor)?,
                    offer.payload_sha256,
                    offer.payload_byte_len,
                    offer.offered_at
                ],
            )
            .map_err(|e| format!("successor offer conflicts with an existing generation: {e}"))?;
        self.read_successor_offer(request_id, successor)?
            .ok_or("successor offer readback absent".into())
    }

    pub fn read_successor_offer(
        &self,
        request_id: &str,
        successor: &FreshRecipientIdentity,
    ) -> Result<Option<FreshSuccessorOffer>, String> {
        validate_request_id(request_id)?;
        let identity = Self::recipient_identity_json(successor)?;
        let row = self
            .sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT generation,session_id,seq,source_id,attempt_id,lane_id,
                    source_generation,root_id,owner_generation,original_identity,
                    payload_sha256,payload_byte_len,offered_at
             FROM fresh_successor_offer WHERE offer_request_id=?1 AND successor_identity=?2",
                params![request_id, identity],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, String>(7)?,
                        r.get::<_, String>(8)?,
                        r.get::<_, String>(9)?,
                        r.get::<_, String>(10)?,
                        r.get::<_, i64>(11)?,
                        r.get::<_, String>(12)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;
        row.map(|r| {
            Ok(FreshSuccessorOffer {
                offer_request_id: request_id.into(),
                generation: r.0,
                session_id: r.1,
                seq: r.2,
                source_id: r.3,
                attempt_id: r.4,
                lane_id: r.5,
                source_generation: r.6,
                root_id: r.7,
                owner_generation: r.8,
                original_identity: serde_json::from_str(&r.9).map_err(|e| e.to_string())?,
                successor_identity: successor.clone(),
                payload_sha256: r.10,
                payload_byte_len: r.11,
                offered_at: r.12,
            })
        })
        .transpose()
    }

    pub fn read_successor_offer_for_root(
        &self,
        request_id: &str,
        original: &FreshRecipientIdentity,
    ) -> Result<(String, FreshSuccessorOffer), String> {
        validate_request_id(request_id)?;
        let successor_json: String = self
            .sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT successor_identity FROM fresh_successor_offer
             WHERE offer_request_id=?1 AND original_identity=?2",
                params![request_id, Self::recipient_identity_json(original)?],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or("successor offer absent for original root")?;
        let successor: FreshRecipientIdentity =
            serde_json::from_str(&successor_json).map_err(|e| e.to_string())?;
        let offer = self
            .read_successor_offer(request_id, &successor)?
            .ok_or("successor offer readback absent")?;
        let session = self.read_session_for_successor(&offer)?;
        Ok((session.request_id, offer))
    }

    fn require_pending_successor_row(
        &self,
        session: &FreshV30Session,
        seq: i64,
        source_id: &str,
        root: &str,
        owner: &str,
        accepted_generation: Option<&str>,
    ) -> Result<(String, i64, String), String> {
        if self
            .sidecar
            .mailbox()
            .notifications_paused(&session.session_id)?
        {
            return Err("fresh recipient notifications paused".into());
        }
        let query = format!(
            "SELECT {MAILBOX_ROW_COLUMNS} FROM mailbox WHERE session_id=?1 AND seq=?2
             AND delivered_at IS NULL AND (target_kind IS NULL OR
             (target_kind='session' AND target_id=?1))
             AND {DELIVERABLE_MAILBOX_ERROR_PREDICATE}"
        );
        let row = self
            .sidecar
            .mailbox()
            .conn
            .query_row(&query, params![session.session_id, seq], map_mailbox_row)
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or("successor exact pending row absent or already ACKed")?;
        let sha = row
            .payload_sha256
            .as_deref()
            .ok_or("successor row payload digest absent")?;
        let len = row
            .payload_byte_len
            .ok_or("successor row payload length absent")?;
        if len < 0 || row.payload_file_path.is_none() {
            return Err("successor row retained payload absent".into());
        }
        let (accepted_source, attempt) =
            self.accepted_source_for_row(session, seq, sha, len, root, owner)?;
        if accepted_source != source_id {
            return Err("successor pending row/source mismatch".into());
        }
        let granted: bool = self
            .sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM fresh_recipient_grant
             WHERE session_id=?1 AND seq=?2)",
                params![session.session_id, seq],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if granted {
            return Err("successor row already granted or ACKed".into());
        }
        let admitted: Option<String> = self
            .state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?
            .query_row(
                "SELECT generation FROM fresh_lane_successor_admission
                WHERE session_id=?1 AND seq=?2",
                params![session.session_id, seq],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if admitted
            .as_deref()
            .is_some_and(|generation| Some(generation) != accepted_generation)
        {
            return Err("successor row already has an admitted generation".into());
        }
        self.sidecar
            .mailbox()
            .payloads()
            .verify_mailbox_row_payload(&row)?;
        Ok((sha.into(), len, attempt))
    }

    /// Called only after the Broker independently verifies the offered process
    /// is still live and is the same installed Runner image. State is written
    /// first; an exact retry repairs a lost sidecar write without a new grant.
    pub fn admit_successor(
        &self,
        request_id: &str,
        original: &FreshRecipientIdentity,
    ) -> Result<FreshSuccessorAdmission, String> {
        validate_request_id(request_id)?;
        let identity = Self::recipient_identity_json(original)?;
        let successor_json: String = self
            .sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT successor_identity FROM fresh_successor_offer
             WHERE offer_request_id=?1 AND original_identity=?2",
                params![request_id, identity],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or("successor offer absent for original root")?;
        let successor: FreshRecipientIdentity =
            serde_json::from_str(&successor_json).map_err(|e| e.to_string())?;
        let offer = self
            .read_successor_offer(request_id, &successor)?
            .ok_or("successor offer readback absent")?;
        let session = self.read_session_for_successor(&offer)?;
        // The serving Broker verifies this D-bound root on its pinned socket
        // before calling this storage transition.
        if self.recipient_binding(&session, original)?
            != (offer.root_id.clone(), offer.owner_generation.clone())
        {
            return Err("successor original root binding changed".into());
        }
        let (sha, len, attempt) = self.require_pending_successor_row(
            &session,
            offer.seq,
            &offer.source_id,
            &offer.root_id,
            &offer.owner_generation,
            Some(&offer.generation),
        )?;
        if (sha, len, attempt)
            != (
                offer.payload_sha256.clone(),
                offer.payload_byte_len,
                offer.attempt_id.clone(),
            )
        {
            return Err("successor offer row/source changed before admission".into());
        }
        let now = Utc::now().to_rfc3339();
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        state
            .execute_batch("PRAGMA synchronous=FULL")
            .map_err(|e| e.to_string())?;
        state
            .execute(
                "INSERT INTO fresh_lane_successor_admission VALUES
             (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
             ON CONFLICT(generation) DO NOTHING",
                params![
                    offer.generation,
                    offer.offer_request_id,
                    offer.session_id,
                    offer.seq,
                    offer.source_id,
                    offer.attempt_id,
                    offer.lane_id,
                    offer.source_generation,
                    offer.root_id,
                    offer.owner_generation,
                    identity,
                    successor_json,
                    offer.payload_sha256,
                    offer.payload_byte_len,
                    now
                ],
            )
            .map_err(|e| format!("fresh State successor row already admitted: {e}"))?;
        let state_admission = self
            .read_successor_state(&offer.generation)?
            .ok_or("fresh State successor admission absent")?;
        if state_admission.offer != offer {
            return Err("fresh State successor admission conflicts with offer".into());
        }
        self.sidecar
            .mailbox()
            .conn
            .execute(
                "INSERT INTO fresh_successor_admission
             SELECT generation,offer_request_id,session_id,seq,source_id,attempt_id,
                    lane_id,source_generation,root_id,owner_generation,original_identity,
                    successor_identity,payload_sha256,payload_byte_len,?2
             FROM fresh_successor_offer WHERE offer_request_id=?1
             ON CONFLICT(generation) DO NOTHING",
                params![request_id, state_admission.admitted_at],
            )
            .map_err(|e| format!("broker successor row already admitted: {e}"))?;
        self.read_successor_admission(request_id, original)?
            .ok_or("successor admission readback absent".into())
    }

    fn read_session_for_successor(
        &self,
        offer: &FreshSuccessorOffer,
    ) -> Result<FreshV30Session, String> {
        let request: String = self
            .sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT request_id FROM fresh_lane_session WHERE session_id=?1
             AND lane_id=?2 AND source_generation=?3",
                params![offer.session_id, offer.lane_id, offer.source_generation],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or("successor fresh session absent")?;
        self.read_session(&request)?
            .ok_or("successor fresh session admission absent".into())
    }

    fn read_successor_state(
        &self,
        generation: &str,
    ) -> Result<Option<FreshSuccessorAdmission>, String> {
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let row = state
            .query_row(
                "SELECT offer_request_id,admitted_at FROM fresh_lane_successor_admission
             WHERE generation=?1",
                [generation],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((request, admitted_at)) = row else {
            return Ok(None);
        };
        let successor_json: String = state
            .query_row(
                "SELECT successor_identity FROM fresh_lane_successor_admission
             WHERE generation=?1",
                [generation],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let successor: FreshRecipientIdentity =
            serde_json::from_str(&successor_json).map_err(|e| e.to_string())?;
        let offer = self
            .read_successor_offer(&request, &successor)?
            .ok_or("fresh State successor has no Broker offer")?;
        let exact: bool = state
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM fresh_lane_successor_admission WHERE
             generation=?1 AND offer_request_id=?2 AND session_id=?3 AND seq=?4
             AND source_id=?5 AND attempt_id=?6 AND lane_id=?7 AND source_generation=?8
             AND root_id=?9 AND owner_generation=?10 AND original_identity=?11
             AND successor_identity=?12 AND payload_sha256=?13 AND payload_byte_len=?14)",
                params![
                    offer.generation,
                    offer.offer_request_id,
                    offer.session_id,
                    offer.seq,
                    offer.source_id,
                    offer.attempt_id,
                    offer.lane_id,
                    offer.source_generation,
                    offer.root_id,
                    offer.owner_generation,
                    Self::recipient_identity_json(&offer.original_identity)?,
                    successor_json,
                    offer.payload_sha256,
                    offer.payload_byte_len
                ],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !exact {
            return Err("fresh State successor binding differs from Broker offer".into());
        }
        Ok(Some(FreshSuccessorAdmission { offer, admitted_at }))
    }

    /// Broker-only repair predicate: a committed State row proves that a
    /// previous exact live-root admission reached its first durable fence.
    pub fn successor_state_committed_for_root(
        &self,
        request_id: &str,
        original: &FreshRecipientIdentity,
    ) -> Result<bool, String> {
        let (_, offer) = self.read_successor_offer_for_root(request_id, original)?;
        Ok(self.read_successor_state(&offer.generation)?.is_some())
    }

    /// The original root or exact successor can recover the admission after a
    /// lost reply or Broker restart. A half-written cross-store transition is
    /// reported as unknown until the original root retries `admit_successor`.
    pub fn read_successor_admission(
        &self,
        request_id: &str,
        actor: &FreshRecipientIdentity,
    ) -> Result<Option<FreshSuccessorAdmission>, String> {
        validate_request_id(request_id)?;
        let actor_json = Self::recipient_identity_json(actor)?;
        let row: Option<(String, String, String)> = self
            .sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT generation,original_identity,successor_identity FROM fresh_successor_offer
             WHERE offer_request_id=?1 AND (original_identity=?2 OR successor_identity=?2)",
                params![request_id, actor_json],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((generation, original_json, _successor_json)) = row else {
            return Ok(None);
        };
        let state = self.read_successor_state(&generation)?;
        let sidecar: Option<String> = self
            .sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT admitted_at FROM fresh_successor_admission
             WHERE generation=?1 AND offer_request_id=?2",
                params![generation, request_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        match (state, sidecar) {
            (None,None) => Ok(None),
            (Some(admission),Some(at)) if at == admission.admitted_at => {
                let original: FreshRecipientIdentity = serde_json::from_str(&original_json)
                    .map_err(|e| e.to_string())?;
                let session = self.read_session_for_successor(&admission.offer)?;
                self.recipient_binding(&session, &original)?;
                let exact: bool = self.sidecar.mailbox().conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM fresh_successor_admission WHERE
                     generation=?1 AND session_id=?2 AND seq=?3 AND source_id=?4
                     AND attempt_id=?5 AND lane_id=?6 AND source_generation=?7
                     AND root_id=?8 AND owner_generation=?9 AND original_identity=?10
                     AND successor_identity=?11 AND payload_sha256=?12
                     AND payload_byte_len=?13)",
                    params![admission.offer.generation,admission.offer.session_id,
                        admission.offer.seq,admission.offer.source_id,admission.offer.attempt_id,
                        admission.offer.lane_id,admission.offer.source_generation,
                        admission.offer.root_id,admission.offer.owner_generation,
                        Self::recipient_identity_json(&admission.offer.original_identity)?,
                        Self::recipient_identity_json(&admission.offer.successor_identity)?,
                        admission.offer.payload_sha256,admission.offer.payload_byte_len],
                    |r| r.get(0),
                ).map_err(|e| e.to_string())?;
                if !exact { return Err("broker successor admission differs from State".into()) }
                Ok(Some(admission))
            }
            _ => Err("successor admission cross-store transition incomplete; original root retry required".into()),
        }
    }
}
