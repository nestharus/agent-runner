/// One immutable choice of offer identity for an exact selected W. The
/// decision reserves no process and grants no delivery authority.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshBashWakeSuccessorDecision {
    pub wake_request_id: String,
    pub offer_request_id: String,
    pub obligation: FreshBashWakeObligation,
    pub decided_at: String,
}

fn fresh_bash_wake_successor_decision_schema_count(state: &Connection) -> Result<i64, String> {
    state.query_row(
        "SELECT count(*) FROM sqlite_master WHERE name IN
         ('fresh_bash_wake_successor_decision',
          'fresh_bash_wake_successor_decision_no_update',
          'fresh_bash_wake_successor_decision_no_delete')",
        [],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

fn verify_fresh_bash_wake_successor_decision_schema(state: &Connection) -> Result<(), String> {
    if fresh_bash_wake_successor_decision_schema_count(state)? != 3 {
        return Err("fresh Bash wake successor decision schema is incomplete".into());
    }
    verify_fresh_sql_objects(
        state,
        FRESH_BASH_WAKE_SUCCESSOR_DECISION_SCHEMA,
        "fresh_bash_wake_successor_decision",
        &[
            "fresh_bash_wake_successor_decision_no_update",
            "fresh_bash_wake_successor_decision_no_delete",
        ],
    )
}

impl FreshV30Lane {
    /// The serving Broker calls this only for the socket's pinned original
    /// D-bound peer. A reply loss recovers the same immutable choice. A failed
    /// launch after this commit remains unknown; it cannot choose a new peer.
    pub fn decide_bash_wake_successor(
        &self,
        wake_request_id: &str,
        offer_request_id: &str,
        original: &FreshRecipientIdentity,
    ) -> Result<FreshBashWakeSuccessorDecision, String> {
        validate_request_id(wake_request_id)?;
        validate_request_id(offer_request_id)?;
        if let Some(existing) = self.read_bash_wake_successor_decision(wake_request_id, original)? {
            if existing.offer_request_id != offer_request_id {
                return Err("fresh Bash wake successor already has another decision".into());
            }
            return Ok(existing);
        }
        let obligation = self
            .read_bash_wake_obligation(wake_request_id)?
            .ok_or("selected Bash wake obligation absent")?;
        if obligation.original_identity != *original {
            return Err("wake decision requires the original D-bound recipient".into());
        }
        let (root, actor) = self.released_handoff_for_root(&obligation.root_id)?;
        if actor != *original {
            return Err("wake decision original process changed".into());
        }
        let session = self.read_session(&root.d_key)?.ok_or("wake decision D absent")?;
        let terminal = self.read_private_root_terminal(&root, original, &session)?;
        // The exact member this W belongs to, whether the root has one child
        // or a set; no other member's row can stand in for it.
        let member = terminal
            .member(wake_request_id)
            .ok_or("selected Bash wake is not a member of its root")?;
        if member.notification_state != "pending_f"
            || member.listener_policy.as_deref() != Some("notify")
            || member.mailbox_seq != Some(obligation.seq)
            || member.delivery_payload_sha256.as_deref()
                != Some(obligation.payload_sha256.as_str())
        {
            return Err("selected Bash wake is not pending one original F".into());
        }
        let decision = FreshBashWakeSuccessorDecision {
            wake_request_id: wake_request_id.into(),
            offer_request_id: offer_request_id.into(),
            obligation,
            decided_at: Utc::now().to_rfc3339(),
        };
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        state.execute_batch("PRAGMA synchronous=FULL").map_err(|e| e.to_string())?;
        state.execute(
            "INSERT OR IGNORE INTO fresh_bash_wake_successor_decision VALUES (?1,?2,?3,?4)",
            params![
                decision.wake_request_id,
                decision.offer_request_id,
                serde_json::to_string(&decision.obligation).map_err(|e| e.to_string())?,
                decision.decided_at,
            ],
        )
        .map_err(|e| format!("fresh Bash wake successor decision conflicts: {e}"))?;
        let stored = self
            .read_bash_wake_successor_decision(wake_request_id, original)?
            .ok_or("fresh Bash wake successor decision absent after commit")?;
        if stored.offer_request_id != decision.offer_request_id
            || stored.obligation != decision.obligation
        {
            return Err("fresh Bash wake successor decision changed on retry".into());
        }
        Ok(stored)
    }

    pub fn read_bash_wake_successor_decision(
        &self,
        wake_request_id: &str,
        original: &FreshRecipientIdentity,
    ) -> Result<Option<FreshBashWakeSuccessorDecision>, String> {
        validate_request_id(wake_request_id)?;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let row: Option<(String, String, String)> = state
            .query_row(
                "SELECT offer_request_id,obligation_json,decided_at
                 FROM fresh_bash_wake_successor_decision WHERE wake_request_id=?1",
                [wake_request_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((offer_request_id, obligation_json, decided_at)) = row else {
            return Ok(None);
        };
        validate_request_id(&offer_request_id)?;
        let stored: FreshBashWakeObligation =
            serde_json::from_str(&obligation_json).map_err(|e| e.to_string())?;
        let current = self
            .read_bash_wake_obligation(wake_request_id)?
            .ok_or("fresh Bash wake successor decision lost W")?;
        if stored != current || current.original_identity != *original {
            return Err("fresh Bash wake successor decision changed W/original".into());
        }
        Ok(Some(FreshBashWakeSuccessorDecision {
            wake_request_id: wake_request_id.into(),
            offer_request_id,
            obligation: current,
            decided_at,
        }))
    }

    pub fn read_bash_wake_successor_decision_for_offer(
        &self,
        offer_request_id: &str,
    ) -> Result<Option<FreshBashWakeSuccessorDecision>, String> {
        validate_request_id(offer_request_id)?;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let wake_request_id: Option<String> = state
            .query_row(
                "SELECT wake_request_id FROM fresh_bash_wake_successor_decision
                 WHERE offer_request_id=?1",
                [offer_request_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some(wake_request_id) = wake_request_id else {
            return Ok(None);
        };
        let obligation = self
            .read_bash_wake_obligation(&wake_request_id)?
            .ok_or("fresh Bash wake successor decision lost W")?;
        let decision = self
            .read_bash_wake_successor_decision(
                &wake_request_id,
                &obligation.original_identity,
            )?
            .ok_or("fresh Bash wake successor decision absent")?;
        if decision.offer_request_id != offer_request_id {
            return Err("fresh Bash wake successor offer decision changed".into());
        }
        Ok(Some(decision))
    }
}
