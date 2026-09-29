/// The physical receiver file is named by the Broker grant and authenticated
/// socket UID. Its digest and inode are rechecked whenever ACK is read.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshOriginalReceiptIdentity {
    pub grant_id: String,
    pub receipt_path: String,
    pub recipient_uid: u32,
    pub receipt_sha256: String,
    pub receipt_device: i64,
    pub receipt_inode: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshOriginalReceiverReceipt {
    pub delivery_request_id: String,
    pub grant: FreshDeliveryReadback,
    pub recipient_identity: FreshRecipientIdentity,
    pub recipient_uid: u32,
    pub payload_base64: String,
    pub delivery_token_sha256: String,
}

impl FreshV30Lane {
    pub fn original_manual_ack_exists_for_grant(&self, grant_id: &str) -> Result<bool, String> {
        validate_request_id(grant_id)?;
        self.sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM fresh_recipient_ack_evidence
             WHERE grant_id=?1 AND basis='manual_ack')",
                [grant_id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())
    }
    pub fn original_manual_ack_exists_for_root(&self, root_id: &str) -> Result<bool, String> {
        self.sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM fresh_recipient_ack_evidence e
             JOIN fresh_recipient_grant g ON g.grant_id=e.grant_id
             WHERE g.root_id=?1 AND e.basis='manual_ack')",
                [root_id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())
    }
    fn original_receipt_root(&self) -> Result<std::path::PathBuf, String> {
        let broker_root = self
            .state_path
            .parent()
            .and_then(|p| p.parent())
            .ok_or("fresh broker root absent")?;
        let parent = broker_root
            .parent()
            .ok_or("fresh broker root parent absent")?;
        let name = broker_root
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("fresh broker root name absent")?;
        Ok(parent.join(format!("{name}-original-receipts")))
    }

    fn checked_original_receipt_directory(&self, uid: u32) -> Result<std::path::PathBuf, String> {
        let root = self.original_receipt_root()?;
        let meta = fs::symlink_metadata(&root).map_err(|e| e.to_string())?;
        if !meta.is_dir()
            || meta.file_type().is_symlink()
            || meta.uid() != 0
            || meta.mode() & 0o777 != 0o711
        {
            return Err("original receipt root changed".into());
        }
        let directory = root.join(uid.to_string());
        let meta = fs::symlink_metadata(&directory).map_err(|e| e.to_string())?;
        if !meta.is_dir()
            || meta.file_type().is_symlink()
            || meta.uid() != uid
            || meta.mode() & 0o777 != 0o700
        {
            return Err("original receipt UID directory changed".into());
        }
        Ok(directory)
    }

    pub fn original_receipt_path(
        &self,
        grant_id: &str,
        uid: u32,
    ) -> Result<std::path::PathBuf, String> {
        validate_request_id(grant_id)?;
        Ok(self
            .checked_original_receipt_directory(uid)?
            .join(format!("{grant_id}.json")))
    }

    pub fn ensure_original_receipt_directory(&self, uid: u32) -> Result<(), String> {
        require_root()?;
        let root = self.original_receipt_root()?;
        let parent = root.parent().ok_or("original receipt root parent absent")?;
        let meta = fs::symlink_metadata(parent).map_err(|e| e.to_string())?;
        if !meta.is_dir()
            || meta.file_type().is_symlink()
            || meta.uid() != 0
            || meta.mode() & 0o022 != 0
        {
            return Err("original receipt root ancestry untrusted".into());
        }
        match fs::create_dir(&root) {
            Ok(()) => {
                fs::set_permissions(&root, fs::Permissions::from_mode(0o711))
                    .map_err(|e| e.to_string())?;
                File::open(&root)
                    .and_then(|f| f.sync_all())
                    .map_err(|e| e.to_string())?;
                File::open(parent)
                    .and_then(|f| f.sync_all())
                    .map_err(|e| e.to_string())?;
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.to_string()),
        }
        let root_meta = fs::symlink_metadata(&root).map_err(|e| e.to_string())?;
        if !root_meta.is_dir()
            || root_meta.file_type().is_symlink()
            || root_meta.uid() != 0
            || root_meta.mode() & 0o777 != 0o711
        {
            return Err("original receipt root changed".into());
        }
        let directory = root.join(uid.to_string());
        match fs::create_dir(&directory) {
            Ok(()) => {
                fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
                    .map_err(|e| e.to_string())?;
                let c_path = std::ffi::CString::new(directory.as_os_str().as_bytes())
                    .map_err(|e| e.to_string())?;
                if unsafe { libc::chown(c_path.as_ptr(), uid, u32::MAX) } != 0 {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                File::open(&directory)
                    .and_then(|f| f.sync_all())
                    .map_err(|e| e.to_string())?;
                File::open(&root)
                    .and_then(|f| f.sync_all())
                    .map_err(|e| e.to_string())?;
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.to_string()),
        }
        self.checked_original_receipt_directory(uid).map(|_| ())
    }

    fn original_grant_material(
        &self,
        grant_id: &str,
        recipient: &FreshRecipientIdentity,
        uid: u32,
    ) -> Result<(FreshDeliveryReadback, String, String), String> {
        let grant = self
            .read_recipient_delivery(grant_id, recipient)?
            .ok_or("original F grant or peer absent")?;
        let identity = Self::recipient_identity_json(recipient)?;
        let bound: Option<(String, String, u32)> = self
            .sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT g.delivery_request_id,g.delivery_token,u.recipient_uid
             FROM fresh_recipient_grant g JOIN fresh_original_grant_uid u ON u.grant_id=g.grant_id
             WHERE g.grant_id=?1 AND g.recipient_identity=?2 AND u.recipient_identity=?2",
                params![grant_id, identity],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let (request, token, bound_uid) = bound.ok_or("original F UID binding absent")?;
        if uid != bound_uid {
            return Err("original F receiving UID changed".into());
        }
        Ok((grant, request, token))
    }

    fn read_original_receipt_file(
        &self,
        grant: &FreshDeliveryReadback,
        request: &str,
        token: &str,
        recipient: &FreshRecipientIdentity,
        uid: u32,
    ) -> Result<(String, i64, i64, Vec<u8>), String> {
        let path = self.original_receipt_path(&grant.grant_id, uid)?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)
            .map_err(|e| format!("original receiver receipt absent: {e}"))?;
        let meta = file.metadata().map_err(|e| e.to_string())?;
        if !meta.is_file()
            || meta.uid() != uid
            || meta.nlink() != 1
            || meta.mode() & 0o777 != 0o400
            || meta.len() > 64 * 1024 * 1024
        {
            return Err("original receiver receipt file untrusted".into());
        }
        let mut bytes = Vec::new();
        file.take(64 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        let named = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if bytes.len() as u64 != meta.len()
            || named.dev() != meta.dev()
            || named.ino() != meta.ino()
            || named.file_type().is_symlink()
        {
            return Err("original receiver receipt changed during read".into());
        }
        let receipt: FreshOriginalReceiverReceipt =
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        let mut expected = grant.clone();
        expected.phase = receipt.grant.phase.clone();
        if !matches!(receipt.grant.phase.as_str(), "unknown" | "submitted")
            || receipt.grant != expected
            || receipt.delivery_request_id != request
            || receipt.recipient_identity != *recipient
            || receipt.recipient_uid != uid
            || receipt.delivery_token_sha256 != sha256_hex(token.as_bytes())
        {
            return Err("original receiver receipt differs from F/peer/token".into());
        }
        let payload = base64::engine::general_purpose::STANDARD
            .decode(receipt.payload_base64.as_bytes())
            .map_err(|e| e.to_string())?;
        if i64::try_from(payload.len()).ok() != Some(grant.payload_byte_len)
            || sha256_hex(&payload) != grant.payload_sha256
        {
            return Err("original receiver receipt bytes differ from F row".into());
        }
        Ok((
            sha256_hex(&bytes),
            meta.dev() as i64,
            meta.ino() as i64,
            payload,
        ))
    }

    pub fn certify_original_receipt(
        &mut self,
        grant_id: &str,
        recipient: &FreshRecipientIdentity,
        uid: u32,
    ) -> Result<FreshOriginalReceiptIdentity, String> {
        let (grant, request, token) = self.original_grant_material(grant_id, recipient, uid)?;
        if grant.phase == "acked" {
            return Err("original receipt already ACKed".into());
        }
        let (sha, device, inode, payload) =
            self.read_original_receipt_file(&grant, &request, &token, recipient, uid)?;
        if payload != self.lookup_payload(&grant.lane_id, &grant.session_id, grant.seq)? {
            return Err("original receiver receipt differs from retained F bytes".into());
        }
        let identity = Self::recipient_identity_json(recipient)?;
        self.sidecar
            .mailbox()
            .conn
            .execute(
                "INSERT INTO fresh_original_receipt
             (grant_id,delivery_request_id,session_id,seq,source_id,attempt_id,lane_id,
              source_generation,root_id,owner_generation,recipient_identity,recipient_uid,
              payload_sha256,payload_byte_len,delivery_token_sha256,receipt_sha256,
              receipt_device,receipt_inode,certified_at)
             SELECT g.grant_id,g.delivery_request_id,g.session_id,g.seq,g.source_id,g.attempt_id,
                    g.lane_id,g.source_generation,g.root_id,g.owner_generation,
                    g.recipient_identity,u.recipient_uid,g.payload_sha256,g.payload_byte_len,
                    ?4,?5,?6,?7,?8
             FROM fresh_recipient_grant g JOIN fresh_original_grant_uid u ON u.grant_id=g.grant_id
             JOIN fresh_recipient_row_source r ON r.session_id=g.session_id AND r.seq=g.seq
             WHERE g.grant_id=?1 AND g.recipient_identity=?2 AND u.recipient_uid=?3
               AND g.phase IN ('unknown','submitted') AND r.source_id=g.source_id
               AND r.attempt_id=g.attempt_id AND r.payload_sha256=g.payload_sha256
               AND r.payload_byte_len=g.payload_byte_len
             ON CONFLICT(grant_id) DO NOTHING",
                params![
                    grant_id,
                    identity,
                    uid,
                    sha256_hex(token.as_bytes()),
                    sha,
                    device,
                    inode,
                    Utc::now().to_rfc3339()
                ],
            )
            .map_err(|e| e.to_string())?;
        self.read_original_receipt(grant_id, recipient, uid)?
            .ok_or("original receipt certification absent".into())
    }

    pub fn read_original_receipt(
        &self,
        grant_id: &str,
        recipient: &FreshRecipientIdentity,
        uid: u32,
    ) -> Result<Option<FreshOriginalReceiptIdentity>, String> {
        let (grant, request, token) = self.original_grant_material(grant_id, recipient, uid)?;
        let identity = Self::recipient_identity_json(recipient)?;
        let durable: Option<(String, i64, i64)> = self
            .sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT c.receipt_sha256,c.receipt_device,c.receipt_inode
             FROM fresh_original_receipt c
             JOIN fresh_recipient_grant g ON g.grant_id=c.grant_id
             JOIN fresh_original_grant_uid u ON u.grant_id=c.grant_id
             JOIN fresh_recipient_row_source r ON r.session_id=c.session_id AND r.seq=c.seq
             WHERE c.grant_id=?1 AND c.delivery_request_id=?2
               AND c.session_id=g.session_id AND c.seq=g.seq
               AND c.source_id=g.source_id AND c.attempt_id=g.attempt_id
               AND c.lane_id=g.lane_id AND c.source_generation=g.source_generation
               AND c.root_id=g.root_id AND c.owner_generation=g.owner_generation
               AND c.recipient_identity=?3 AND c.recipient_identity=g.recipient_identity
               AND c.recipient_identity=u.recipient_identity
               AND c.recipient_uid=?4 AND c.recipient_uid=u.recipient_uid
               AND c.payload_sha256=g.payload_sha256 AND c.payload_byte_len=g.payload_byte_len
               AND c.delivery_token_sha256=?5
               AND r.source_id=c.source_id AND r.attempt_id=c.attempt_id
               AND r.payload_sha256=c.payload_sha256 AND r.payload_byte_len=c.payload_byte_len",
                params![
                    grant_id,
                    request,
                    identity,
                    uid,
                    sha256_hex(token.as_bytes())
                ],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((sha, device, inode)) = durable else {
            return Ok(None);
        };
        if grant.phase == "acked" {
            let locked: bool = self
                .sidecar
                .mailbox()
                .conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM fresh_original_ack_receipt a
                 JOIN fresh_original_receipt c ON c.grant_id=a.grant_id
                 JOIN fresh_recipient_ack_evidence e ON e.grant_id=a.grant_id
                 JOIN fresh_recipient_grant g ON g.grant_id=a.grant_id
                 WHERE a.grant_id=?1 AND a.delivery_request_id=?2
                   AND a.recipient_uid=?3 AND a.receipt_sha256=?4
                   AND a.receipt_device=?5 AND a.receipt_inode=?6
                   AND a.acknowledged_at=e.acknowledged_at
                   AND e.basis='manual_ack' AND e.delegation_id IS NULL
                   AND e.delivery_request_id=a.delivery_request_id
                   AND e.delivery_token_sha256=?7
                   AND c.delivery_request_id=a.delivery_request_id
                   AND c.recipient_uid=a.recipient_uid
                   AND c.receipt_sha256=a.receipt_sha256
                   AND c.receipt_device=a.receipt_device
                   AND c.receipt_inode=a.receipt_inode
                   AND c.delivery_token_sha256=e.delivery_token_sha256
                   AND g.phase='acked' AND g.acknowledged_at=a.acknowledged_at)",
                    params![
                        grant_id,
                        request,
                        uid,
                        sha,
                        device,
                        inode,
                        sha256_hex(token.as_bytes())
                    ],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if !locked {
                return Err("original ACK receipt immutable join absent".into());
            }
        }
        let (observed_sha, observed_device, observed_inode, _) =
            self.read_original_receipt_file(&grant, &request, &token, recipient, uid)?;
        if (observed_sha, observed_device, observed_inode) != (sha.clone(), device, inode) {
            return Err("original receiver receipt changed after certification".into());
        }
        Ok(Some(FreshOriginalReceiptIdentity {
            grant_id: grant_id.into(),
            receipt_path: self
                .original_receipt_path(grant_id, uid)?
                .display()
                .to_string(),
            recipient_uid: uid,
            receipt_sha256: sha,
            receipt_device: device,
            receipt_inode: inode,
        }))
    }
}
