/// The broker supplies this from a pinned SO_PEERCRED process. It is never
/// accepted from the recipient request as an identity assertion.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshRecipientIdentity {
    pub host_pid: i32,
    pub boot_id: String,
    pub starttime_ticks: u64,
    pub pidns_dev: u64,
    pub pidns_ino: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FreshDeliveryReadback {
    pub grant_id: String,
    pub session_id: String,
    pub seq: i64,
    pub source_id: String,
    pub attempt_id: String,
    pub lane_id: String,
    pub source_generation: String,
    pub root_id: String,
    pub owner_generation: String,
    pub payload_sha256: String,
    pub payload_byte_len: i64,
    pub phase: String,
}

pub struct FreshDeliverySubmission {
    pub readback: FreshDeliveryReadback,
    pub delivery_token: String,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FreshAckDelegation {
    pub delegation_id: String,
    pub session_id: String,
    pub grant_ids: Vec<String>,
}

impl FreshV30Lane {
    fn recipient_identity_json(identity: &FreshRecipientIdentity) -> Result<String, String> {
        if identity.host_pid <= 0 || identity.starttime_ticks == 0 || identity.boot_id.is_empty() {
            return Err("invalid pinned recipient identity".into());
        }
        serde_json::to_string(identity).map_err(|e| e.to_string())
    }

    fn recipient_binding(
        &self,
        session: &FreshV30Session,
        identity: &FreshRecipientIdentity,
    ) -> Result<(String, String), String> {
        self.require_session(session)?;
        let identity_json = Self::recipient_identity_json(identity)?;
        let binding: (String, String) = self
            .sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT root_id,owner_generation FROM fresh_recipient_binding
             WHERE session_id=?1 AND recipient_identity=?2 AND source_generation=?3",
                params![
                    session.session_id,
                    identity_json,
                    self.identity.source_generation
                ],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or("recipient is absent, offline or wrong for fresh session")?;
        self.require_state_recipient_attachment(
            &session.session_id,
            &identity_json,
            &binding.0,
            &binding.1,
        )?;
        Ok(binding)
    }

    fn require_state_recipient_attachment(
        &self,
        session_id: &str,
        identity_json: &str,
        root: &str,
        owner: &str,
    ) -> Result<(), String> {
        let state = self.sidecar.bound_state()?;
        let connection =
            Connection::open_with_flags(state.path(), OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| e.to_string())?;
        let attached: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM fresh_lane_recipient_attachment
             WHERE session_id=?1 AND lane_id=?2 AND source_generation=?3
             AND root_id=?4 AND owner_generation=?5 AND recipient_identity=?6
             AND attached_at!='')",
                params![
                    session_id,
                    self.identity.lane_id,
                    self.identity.source_generation,
                    root,
                    owner,
                    identity_json
                ],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !attached {
            return Err("fresh State has no exact recipient attachment".into());
        }
        Ok(())
    }

    fn accepted_source_for_row(
        &self,
        session: &FreshV30Session,
        seq: i64,
        sha: &str,
        len: i64,
        root: &str,
        owner: &str,
    ) -> Result<(String, String), String> {
        let source: Option<(String, String, String, String)> = self
            .sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT s.source_id,s.attempt_id,s.state_admission_id,s.registration_digest
                 FROM fresh_recipient_row_source r
                 JOIN fresh_recipient_source s ON s.source_id=r.source_id
                 WHERE r.session_id=?1 AND r.seq=?2 AND r.payload_sha256=?3
                   AND r.payload_byte_len=?4 AND r.attempt_id=s.attempt_id
                   AND s.source_generation=?5 AND s.lane_id=?6
                   AND s.root_id=?7 AND s.owner_generation=?8",
                params![
                    session.session_id,
                    seq,
                    sha,
                    len,
                    self.identity.source_generation,
                    self.identity.lane_id,
                    root,
                    owner
                ],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let (source_id, attempt_id, admission_id, digest) =
            source.ok_or("fresh row lacks exact admitted source/attempt provenance")?;
        let state = self.sidecar.bound_state()?;
        let connection =
            Connection::open_with_flags(state.path(), OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| e.to_string())?;
        let accepted: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM fresh_lane_accepted_source
             WHERE source_id=?1 AND attempt_id=?2 AND state_admission_id=?3
             AND registration_digest=?4 AND source_generation=?5 AND lane_id=?6
             AND root_id=?7 AND owner_generation=?8 AND accepted_at!='')",
                params![
                    source_id,
                    attempt_id,
                    admission_id,
                    digest,
                    self.identity.source_generation,
                    self.identity.lane_id,
                    root,
                    owner
                ],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !accepted {
            return Err("fresh State has no exact accepted source/attempt".into());
        }
        Ok((source_id, attempt_id))
    }

    /// One selected row, exact retained bytes and a durable one-use token.
    /// `unknown` is committed before transport; a lost reply cannot cause a
    /// second grant or silently count as recipient consumption.
    pub fn submit_recipient_delivery(
        &mut self,
        delivery_request_id: &str,
        session: &FreshV30Session,
        recipient: &FreshRecipientIdentity,
    ) -> Result<FreshDeliverySubmission, String> {
        validate_request_id(delivery_request_id)?;
        if self
            .read_recipient_delivery_by_request(delivery_request_id, recipient)?
            .is_some()
        {
            return Err("fresh delivery request already consumed; use exact readback".into());
        }
        let (root, owner) = self.recipient_binding(session, recipient)?;
        if self
            .sidecar
            .mailbox()
            .notifications_paused(&session.session_id)?
        {
            return Err("fresh recipient notifications paused".into());
        }
        let query = format!(
            "SELECT {MAILBOX_ROW_COLUMNS} FROM mailbox
             WHERE session_id=?1 AND delivered_at IS NULL
               AND (target_kind IS NULL OR (target_kind='session' AND target_id=?1))
               AND {DELIVERABLE_MAILBOX_ERROR_PREDICATE}
               AND NOT EXISTS (SELECT 1 FROM fresh_recipient_grant g
                 WHERE g.session_id=mailbox.session_id AND g.seq=mailbox.seq)
             ORDER BY seq LIMIT 1"
        );
        let row = self
            .sidecar
            .mailbox()
            .conn
            .query_row(&query, [&session.session_id], map_mailbox_row)
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or("no ungranted pending fresh recipient row")?;
        let sha = row
            .payload_sha256
            .as_deref()
            .ok_or("fresh recipient row has no retained payload digest")?;
        let len = row
            .payload_byte_len
            .ok_or("fresh recipient row has no retained payload length")?;
        if len < 0 || row.payload_file_path.is_none() {
            return Err("fresh recipient row lacks retained payload bytes".into());
        }
        let (source_id, attempt_id) =
            self.accepted_source_for_row(session, row.seq, sha, len, &root, &owner)?;
        self.sidecar
            .mailbox()
            .payloads()
            .verify_mailbox_row_payload(&row)?;
        let payload =
            fs::read(row.payload_file_path.as_ref().unwrap()).map_err(|e| e.to_string())?;
        if i64::try_from(payload.len()).ok() != Some(len) || sha256_hex(&payload) != sha {
            return Err("fresh recipient payload changed after verification".into());
        }
        let identity_json = Self::recipient_identity_json(recipient)?;
        let grant_id = Uuid::new_v4().to_string();
        let delivery_token = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let tx = self
            .sidecar
            .mailbox_mut()
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let changed = tx
            .execute(
                "INSERT INTO fresh_recipient_grant
             (grant_id,delivery_request_id,delivery_token,session_id,seq,source_id,attempt_id,
              recipient_identity,payload_sha256,payload_byte_len,phase,created_at,
              lane_id,source_generation,root_id,owner_generation)
             SELECT ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'unknown',?11,
               ?17,?18,?12,?13
             WHERE EXISTS (SELECT 1 FROM mailbox WHERE session_id=?4 AND seq=?5
               AND delivered_at IS NULL AND payload_sha256=?9 AND payload_byte_len=?10
               AND kind=?14 AND handle=?15 AND payload_file_path=?16
               AND payload_retention_policy='until_terminal_disposition')
               AND EXISTS (SELECT 1 FROM fresh_recipient_binding WHERE session_id=?4
                 AND recipient_identity=?8 AND root_id=?12 AND owner_generation=?13)
               AND EXISTS (SELECT 1 FROM fresh_recipient_row_source WHERE session_id=?4
                 AND seq=?5 AND source_id=?6 AND attempt_id=?7 AND payload_sha256=?9
                 AND payload_byte_len=?10)",
                params![
                    grant_id,
                    delivery_request_id,
                    delivery_token,
                    session.session_id,
                    row.seq,
                    source_id,
                    attempt_id,
                    identity_json,
                    sha,
                    len,
                    now,
                    root,
                    owner,
                    row.kind,
                    row.handle,
                    row.payload_file_path,
                    self.identity.lane_id,
                    self.identity.source_generation
                ],
            )
            .map_err(|e| format!("fresh delivery grant refused: {e}"))?;
        if changed != 1 {
            return Err("fresh recipient row changed before grant".into());
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(FreshDeliverySubmission {
            readback: FreshDeliveryReadback {
                grant_id,
                session_id: session.session_id.clone(),
                seq: row.seq,
                source_id,
                attempt_id,
                lane_id: self.identity.lane_id.clone(),
                source_generation: self.identity.source_generation.clone(),
                root_id: root,
                owner_generation: owner,
                payload_sha256: sha.into(),
                payload_byte_len: len,
                phase: "unknown".into(),
            },
            delivery_token,
            payload,
        })
    }

    pub fn read_recipient_delivery(
        &self,
        grant_id: &str,
        recipient: &FreshRecipientIdentity,
    ) -> Result<Option<FreshDeliveryReadback>, String> {
        validate_request_id(grant_id)?;
        let identity_json = Self::recipient_identity_json(recipient)?;
        self.sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT grant_id,session_id,seq,source_id,attempt_id,
                    lane_id,source_generation,root_id,owner_generation,
                    payload_sha256,payload_byte_len,phase FROM fresh_recipient_grant
             WHERE grant_id=?1 AND recipient_identity=?2",
                params![grant_id, identity_json],
                |r| {
                    Ok(FreshDeliveryReadback {
                        grant_id: r.get(0)?,
                        session_id: r.get(1)?,
                        seq: r.get(2)?,
                        source_id: r.get(3)?,
                        attempt_id: r.get(4)?,
                        lane_id: r.get(5)?,
                        source_generation: r.get(6)?,
                        root_id: r.get(7)?,
                        owner_generation: r.get(8)?,
                        payload_sha256: r.get(9)?,
                        payload_byte_len: r.get(10)?,
                        phase: r.get(11)?,
                    })
                },
            )
            .optional()
            .map_err(|e| e.to_string())
    }

    /// Readback uses the caller's persisted delivery request ID. A lost
    /// submission reply therefore does not require a leaked grant ID.
    pub fn read_recipient_delivery_by_request(
        &self,
        delivery_request_id: &str,
        recipient: &FreshRecipientIdentity,
    ) -> Result<Option<FreshDeliveryReadback>, String> {
        validate_request_id(delivery_request_id)?;
        let identity_json = Self::recipient_identity_json(recipient)?;
        let id: Option<String> = self
            .sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT grant_id FROM fresh_recipient_grant
             WHERE delivery_request_id=?1 AND recipient_identity=?2",
                params![delivery_request_id, identity_json],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        id.map(|id| {
            self.read_recipient_delivery(&id, recipient)
                .and_then(|grant| grant.ok_or("fresh recipient grant vanished".into()))
        })
        .transpose()
    }

    /// Recover exact bytes after a lost reply without creating or consuming a
    /// second grant. The token is returned only together with verified bytes.
    pub fn recover_recipient_delivery_by_request(
        &self,
        delivery_request_id: &str,
        recipient: &FreshRecipientIdentity,
    ) -> Result<FreshDeliverySubmission, String> {
        let readback = self
            .read_recipient_delivery_by_request(delivery_request_id, recipient)?
            .ok_or("fresh delivery request has no durable grant")?;
        if readback.phase == "acked" {
            return Err("fresh delivery already acknowledged".into());
        }
        let identity_json = Self::recipient_identity_json(recipient)?;
        let token: String = self
            .sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT delivery_token FROM fresh_recipient_grant WHERE grant_id=?1
             AND delivery_request_id=?2 AND recipient_identity=?3 AND phase!='acked'",
                params![readback.grant_id, delivery_request_id, identity_json],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let payload =
            self.lookup_payload(&self.identity.lane_id, &readback.session_id, readback.seq)?;
        if i64::try_from(payload.len()).ok() != Some(readback.payload_byte_len)
            || sha256_hex(&payload) != readback.payload_sha256
        {
            return Err("recovered fresh payload differs from grant".into());
        }
        Ok(FreshDeliverySubmission {
            readback,
            delivery_token: token,
            payload,
        })
    }

    /// A complete socket write proves submission to the socket only. It does
    /// not ACK the recipient. A failed write leaves durable unknown debt.
    pub fn mark_recipient_submitted(&mut self, grant_id: &str) -> Result<(), String> {
        let changed = self
            .sidecar
            .mailbox_mut()
            .conn
            .execute(
                "UPDATE fresh_recipient_grant SET phase='submitted',submitted_at=?2
             WHERE grant_id=?1 AND phase='unknown'",
                params![grant_id, Utc::now().to_rfc3339()],
            )
            .map_err(|e| e.to_string())?;
        if changed != 1 {
            return Err("fresh delivery submission state changed".into());
        }
        Ok(())
    }

    /// Only the pinned original recipient can ACK with the exact delivery
    /// token. Age and an audit label never confer ACK authority.
    pub fn acknowledge_recipient_delivery(
        &mut self,
        grant_id: &str,
        token: &str,
        recipient: &FreshRecipientIdentity,
    ) -> Result<FreshDeliveryReadback, String> {
        validate_request_id(grant_id)?;
        validate_request_id(token)?;
        let identity_json = Self::recipient_identity_json(recipient)?;
        let grant = self.read_recipient_delivery(grant_id, recipient)?
            .ok_or("fresh delivery grant or recipient absent")?;
        let payload = self.lookup_payload(&grant.lane_id, &grant.session_id, grant.seq)?;
        if grant.lane_id != self.identity.lane_id
            || i64::try_from(payload.len()).ok() != Some(grant.payload_byte_len)
            || sha256_hex(&payload) != grant.payload_sha256
        {
            return Err("fresh ACK attachment/source/payload changed".into());
        }
        let tx = self
            .sidecar
            .mailbox_mut()
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let (session, seq, sha, len): (String, i64, String, i64) = tx
            .query_row(
                "SELECT session_id,seq,payload_sha256,payload_byte_len
                 FROM fresh_recipient_grant WHERE grant_id=?1
             AND delivery_token=?2 AND recipient_identity=?3 AND phase IN ('unknown','submitted')",
                params![grant_id, token, identity_json],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or("fresh delivery token or recipient refused")?;
        let now = Utc::now().to_rfc3339();
        if tx
            .execute(
                "UPDATE mailbox SET delivered_at=?3,delivered_by_invocation_uuid=?4,
              delivery_attempts=delivery_attempts+1,delivery_error=NULL
             WHERE session_id=?1 AND seq=?2 AND delivered_at IS NULL
               AND payload_sha256=?5 AND payload_byte_len=?6",
                params![session, seq, now, grant_id, sha, len],
            )
            .map_err(|e| e.to_string())?
            != 1
        {
            return Err("fresh recipient row already acknowledged or changed".into());
        }
        if tx.execute(
            "UPDATE fresh_recipient_grant SET phase='acked',acknowledged_at=?2 WHERE grant_id=?1",
            params![grant_id, now],
        ).map_err(|e| e.to_string())? != 1 {
            return Err("fresh recipient grant changed before ACK".into());
        }
        let token_sha = sha256_hex(token.as_bytes());
        let evidence = tx.execute(
            "INSERT INTO fresh_recipient_ack_evidence
             (grant_id,delivery_request_id,delivery_token_sha256,session_id,seq,
              source_id,attempt_id,recipient_identity,payload_sha256,payload_byte_len,
              basis,delegation_id,acknowledged_at)
             SELECT g.grant_id,g.delivery_request_id,?4,g.session_id,g.seq,
                    g.source_id,g.attempt_id,g.recipient_identity,g.payload_sha256,
                    g.payload_byte_len,'manual_ack',NULL,?5
             FROM fresh_recipient_grant g
             JOIN mailbox m ON m.session_id=g.session_id AND m.seq=g.seq
             JOIN fresh_recipient_row_source r ON r.session_id=g.session_id AND r.seq=g.seq
             WHERE g.grant_id=?1 AND g.delivery_token=?2 AND g.recipient_identity=?3
               AND g.phase='acked' AND g.acknowledged_at=?5
               AND m.delivered_at=?5 AND m.delivered_by_invocation_uuid=?1
               AND m.payload_sha256=g.payload_sha256 AND m.payload_byte_len=g.payload_byte_len
               AND r.source_id=g.source_id AND r.attempt_id=g.attempt_id
               AND r.payload_sha256=g.payload_sha256 AND r.payload_byte_len=g.payload_byte_len",
            params![grant_id,token,identity_json,token_sha,now],
        ).map_err(|e| e.to_string())?;
        if evidence != 1 { return Err("fresh ACK lacks exact row/source/grant/token".into()); }
        tx.commit().map_err(|e| e.to_string())?;
        self.read_recipient_delivery(grant_id, recipient)?
            .ok_or("ACK readback absent".into())
    }

    /// Local manual lookup requires an explicit lane and exact row. It does
    /// not submit, notify, ACK, or infer a route from `(session,seq)` alone.
    pub fn lookup_payload(
        &self,
        lane_id: &str,
        session_id: &str,
        seq: i64,
    ) -> Result<Vec<u8>, String> {
        if lane_id != self.identity.lane_id || seq <= 0 {
            return Err("wrong or unqualified fresh payload lookup".into());
        }
        let allocation: Option<String> = self
            .sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT request_id FROM fresh_lane_session WHERE session_id=?1
             AND lane_id=?2 AND source_generation=?3",
                params![session_id, lane_id, self.identity.source_generation],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let session = self
            .read_session(&allocation.ok_or("fresh session absent from exact lane")?)?
            .ok_or("fresh session allocation vanished")?;
        let row: Option<(String, i64, String)> = self
            .sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT payload_sha256,payload_byte_len,payload_file_path FROM mailbox
             WHERE session_id=?1 AND seq=?2",
                params![session_id, seq],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let (sha, len, path) = row.ok_or("fresh payload row absent")?;
        let source_binding: Option<(String, String, String)> = self
            .sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT root_id,owner_generation,recipient_identity FROM fresh_recipient_binding
             WHERE session_id=?1 AND source_generation=?2",
                params![session_id, self.identity.source_generation],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let (root, owner, recipient_identity) =
            source_binding.ok_or("fresh recipient binding absent")?;
        self.require_state_recipient_attachment(session_id, &recipient_identity, &root, &owner)?;
        self.accepted_source_for_row(&session, seq, &sha, len, &root, &owner)?;
        if len < 0
            || self
                .sidecar
                .mailbox()
                .payloads()
                .payload_path_for_sha256(&sha)?
                != Path::new(&path)
        {
            return Err("fresh payload address changed".into());
        }
        let metadata = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() != len as u64
            || !metadata.permissions().readonly()
        {
            return Err("fresh payload file identity invalid".into());
        }
        let bytes = fs::read(path).map_err(|e| e.to_string())?;
        if i64::try_from(bytes.len()).ok() != Some(len) || sha256_hex(&bytes) != sha {
            return Err("fresh payload digest or length mismatch".into());
        }
        Ok(bytes)
    }

    /// The original recipient explicitly designates exact grant IDs and one
    /// pinned cleanup actor. No ranges or inferred intervening rows exist.
    pub fn delegate_ack_batch(
        &mut self,
        grant_ids: &[String],
        owner: &FreshRecipientIdentity,
        delegate: &FreshRecipientIdentity,
    ) -> Result<FreshAckDelegation, String> {
        if grant_ids.is_empty() || grant_ids.len() > 32 {
            return Err("delegated ACK batch must contain 1..32 exact grants".into());
        }
        let owner_json = Self::recipient_identity_json(owner)?;
        let delegate_json = Self::recipient_identity_json(delegate)?;
        if owner_json == delegate_json {
            return Err("self-delegation is unnecessary".into());
        }
        let mut unique = std::collections::HashSet::new();
        let mut session = None;
        for grant_id in grant_ids {
            validate_request_id(grant_id)?;
            if !unique.insert(grant_id) {
                return Err("duplicate delegated grant".into());
            }
            let grant = self
                .read_recipient_delivery(grant_id, owner)?
                .ok_or("delegated grant does not belong to recipient")?;
            let payload = self.lookup_payload(&grant.lane_id, &grant.session_id, grant.seq)?;
            if grant.lane_id != self.identity.lane_id
                || i64::try_from(payload.len()).ok() != Some(grant.payload_byte_len)
                || sha256_hex(&payload) != grant.payload_sha256
            {
                return Err("delegated ACK attachment/source/payload changed".into());
            }
            if grant.phase == "acked" {
                return Err("delegated grant already acknowledged".into());
            }
            if session.as_deref().is_some_and(|s| s != grant.session_id) {
                return Err("delegated batch crosses recipient sessions".into());
            }
            session = Some(grant.session_id);
        }
        let session_id = session.unwrap();
        let delegation_id = Uuid::new_v4().to_string();
        let tx = self
            .sidecar
            .mailbox_mut()
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO fresh_recipient_ack_delegation
             (delegation_id,session_id,owner_identity,delegate_identity,created_at)
             VALUES(?1,?2,?3,?4,?5)",
            params![
                delegation_id,
                session_id,
                owner_json,
                delegate_json,
                Utc::now().to_rfc3339()
            ],
        )
        .map_err(|e| e.to_string())?;
        for id in grant_ids {
            let changed = tx
                .execute(
                    "INSERT INTO fresh_recipient_ack_delegation_item(delegation_id,grant_id)
                 SELECT ?1,grant_id FROM fresh_recipient_grant WHERE grant_id=?2
                 AND session_id=?3 AND recipient_identity=?4 AND phase!='acked'",
                    params![delegation_id, id, session_id, owner_json],
                )
                .map_err(|e| e.to_string())?;
            if changed != 1 {
                return Err("delegated batch changed before commit".into());
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(FreshAckDelegation {
            delegation_id,
            session_id,
            grant_ids: grant_ids.to_vec(),
        })
    }

    /// The delegate can consume only the exact approved batch, atomically.
    pub fn acknowledge_delegated_batch(
        &mut self,
        delegation_id: &str,
        delegate: &FreshRecipientIdentity,
    ) -> Result<FreshAckDelegation, String> {
        validate_request_id(delegation_id)?;
        let delegate_json = Self::recipient_identity_json(delegate)?;
        let retained = self.sidecar.mailbox().conn.prepare(
            "SELECT g.session_id,g.seq,g.payload_sha256,g.payload_byte_len
             FROM fresh_recipient_ack_delegation d
             JOIN fresh_recipient_ack_delegation_item i ON i.delegation_id=d.delegation_id
             JOIN fresh_recipient_grant g ON g.grant_id=i.grant_id
             WHERE d.delegation_id=?1 AND d.delegate_identity=?2 AND d.consumed_at IS NULL"
        ).map_err(|e| e.to_string())?.query_map(params![delegation_id,delegate_json], |r| {
            Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,
                r.get::<_,String>(2)?,r.get::<_,i64>(3)?))
        }).map_err(|e| e.to_string())?.collect::<Result<Vec<_>,_>>()
            .map_err(|e| e.to_string())?;
        if retained.is_empty() || retained.len() > 32 {
            return Err("delegated ACK authority or rows absent".into());
        }
        for (session, seq, sha, len) in &retained {
            let payload = self.lookup_payload(&self.identity.lane_id, session, *seq)?;
            if i64::try_from(payload.len()).ok() != Some(*len) || sha256_hex(&payload) != *sha {
                return Err("delegated ACK attachment/source/payload changed".into());
            }
        }
        let tx = self
            .sidecar
            .mailbox_mut()
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let session_id: String = tx
            .query_row(
                "SELECT session_id FROM fresh_recipient_ack_delegation WHERE delegation_id=?1
             AND delegate_identity=?2 AND consumed_at IS NULL",
                params![delegation_id, delegate_json],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or("delegated ACK authority absent")?;
        let grants = tx
            .prepare(
                "SELECT g.grant_id,g.seq,g.payload_sha256,g.payload_byte_len,g.delivery_token
             FROM fresh_recipient_ack_delegation_item i
             JOIN fresh_recipient_grant g ON g.grant_id=i.grant_id
             WHERE i.delegation_id=?1 AND g.session_id=?2 AND g.phase!='acked'
             ORDER BY g.seq LIMIT 33",
            )
            .map_err(|e| e.to_string())?
            .query_map(params![delegation_id, session_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let total: i64 = tx
            .query_row(
                "SELECT count(*) FROM fresh_recipient_ack_delegation_item WHERE delegation_id=?1",
                [delegation_id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if grants.is_empty() || grants.len() > 32 || total != grants.len() as i64 {
            return Err("delegated ACK batch is incomplete or changed".into());
        }
        let now = Utc::now().to_rfc3339();
        for (grant_id, seq, sha, len, token) in &grants {
            if tx
                .execute(
                    "UPDATE mailbox SET delivered_at=?3,delivered_by_invocation_uuid=?4,
                  delivery_attempts=delivery_attempts+1,delivery_error=NULL
                 WHERE session_id=?1 AND seq=?2 AND delivered_at IS NULL
                   AND payload_sha256=?5 AND payload_byte_len=?6",
                    params![session_id, seq, now, delegation_id, sha, len],
                )
                .map_err(|e| e.to_string())?
                != 1
            {
                return Err("delegated ACK row changed".into());
            }
            if tx.execute(
                "UPDATE fresh_recipient_grant SET phase='acked',acknowledged_at=?2 WHERE grant_id=?1",
                params![grant_id, now],
            ).map_err(|e| e.to_string())? != 1 {
                return Err("delegated fresh grant changed before ACK".into());
            }
            let token_sha = sha256_hex(token.as_bytes());
            let evidence = tx.execute(
                "INSERT INTO fresh_recipient_ack_evidence
                 (grant_id,delivery_request_id,delivery_token_sha256,session_id,seq,
                  source_id,attempt_id,recipient_identity,payload_sha256,payload_byte_len,
                  basis,delegation_id,acknowledged_at)
                 SELECT g.grant_id,g.delivery_request_id,?4,g.session_id,g.seq,
                        g.source_id,g.attempt_id,g.recipient_identity,g.payload_sha256,
                        g.payload_byte_len,'delegated_manual_ack',?3,?5
                 FROM fresh_recipient_grant g
                 JOIN mailbox m ON m.session_id=g.session_id AND m.seq=g.seq
                 JOIN fresh_recipient_row_source r ON r.session_id=g.session_id AND r.seq=g.seq
                 JOIN fresh_recipient_ack_delegation_item i ON i.grant_id=g.grant_id
                 JOIN fresh_recipient_ack_delegation d ON d.delegation_id=i.delegation_id
                 WHERE g.grant_id=?1 AND g.delivery_token=?2 AND d.delegation_id=?3
                   AND d.session_id=g.session_id AND d.delegate_identity=?6
                   AND g.phase='acked' AND g.acknowledged_at=?5
                   AND m.delivered_at=?5 AND m.delivered_by_invocation_uuid=?3
                   AND m.payload_sha256=g.payload_sha256 AND m.payload_byte_len=g.payload_byte_len
                   AND r.source_id=g.source_id AND r.attempt_id=g.attempt_id
                   AND r.payload_sha256=g.payload_sha256 AND r.payload_byte_len=g.payload_byte_len",
                params![grant_id,token,delegation_id,token_sha,now,delegate_json],
            ).map_err(|e| e.to_string())?;
            if evidence != 1 { return Err("delegated ACK lacks exact row/source/grant/token".into()); }
        }
        tx.execute(
            "UPDATE fresh_recipient_ack_delegation SET consumed_at=?2 WHERE delegation_id=?1
             AND consumed_at IS NULL",
            params![delegation_id, now],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(FreshAckDelegation {
            delegation_id: delegation_id.into(),
            session_id,
            grant_ids: grants.into_iter().map(|(id, _, _, _, _)| id).collect(),
        })
    }
}
