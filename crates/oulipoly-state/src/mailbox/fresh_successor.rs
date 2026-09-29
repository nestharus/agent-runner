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

impl FreshV30Lane {
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
