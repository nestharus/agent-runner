/// Immutable selected-W debt for one original async recipient. It is evidence
/// for a later wake decision, not a successor grant or a delivered assertion.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshBashWakeObligation {
    pub request_id: String,
    pub session_id: String,
    pub seq: i64,
    pub source_id: String,
    pub attempt_id: String,
    pub lane_id: String,
    pub source_generation: String,
    pub root_id: String,
    pub owner_generation: String,
    pub original_identity: FreshRecipientIdentity,
    pub payload_sha256: String,
    pub payload_byte_len: i64,
    pub recorded_at: String,
}

fn fresh_bash_wake_obligation_schema_count(state: &Connection) -> Result<i64, String> {
    state.query_row(
        "SELECT count(*) FROM sqlite_master WHERE name IN
         ('fresh_bash_wake_obligation','fresh_bash_wake_obligation_no_update',
          'fresh_bash_wake_obligation_no_delete')",
        [],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

fn verify_fresh_bash_wake_obligation_schema(state: &Connection) -> Result<(), String> {
    if fresh_bash_wake_obligation_schema_count(state)? != 3 {
        return Err("fresh Bash wake obligation schema is incomplete".into());
    }
    verify_fresh_sql_objects(
        state,
        FRESH_BASH_WAKE_OBLIGATION_SCHEMA,
        "fresh_bash_wake_obligation",
        &[
            "fresh_bash_wake_obligation_no_update",
            "fresh_bash_wake_obligation_no_delete",
        ],
    )
}

impl FreshV30Lane {
    /// Called only after W, the original attachment and the exact retained
    /// mailbox row/source have all been independently joined. An interrupted
    /// insert is repaired by the same source request ID and exact readback.
    fn record_bash_wake_obligation(
        &self,
        event: &FreshBashSourceEvent,
        session: &FreshV30Session,
        seq: i64,
        original: &FreshRecipientIdentity,
        sha: &str,
        len: i64,
    ) -> Result<FreshBashWakeObligation, String> {
        let (root, actor) = self.released_handoff_for_root(&event.root_id)?;
        if actor != *original || root.d_key != session.request_id {
            return Err("fresh Bash wake obligation differs from original D/J".into());
        }
        if event.lane_id != self.identity.lane_id
            || event.source_generation != self.identity.source_generation
            || event.owner_generation != root.old_release.prepared.owner_generation
        {
            return Err("fresh Bash wake obligation differs from selected W/owner".into());
        }
        if self.recipient_binding(session, original)?
            != (event.root_id.clone(), event.owner_generation.clone())
        {
            return Err("fresh Bash wake obligation differs from original attachment".into());
        }
        let (source, attempt) = self.accepted_source_for_row(
            session,
            seq,
            sha,
            len,
            &event.root_id,
            &event.owner_generation,
        )?;
        if source != event.source_id || attempt != event.attempt_id {
            return Err("fresh Bash wake obligation differs from selected W".into());
        }
        let intended = FreshBashWakeObligation {
            request_id: event.request_id.clone(),
            session_id: session.session_id.clone(),
            seq,
            source_id: source,
            attempt_id: attempt,
            lane_id: event.lane_id.clone(),
            source_generation: event.source_generation.clone(),
            root_id: event.root_id.clone(),
            owner_generation: event.owner_generation.clone(),
            original_identity: original.clone(),
            payload_sha256: sha.into(),
            payload_byte_len: len,
            recorded_at: Utc::now().to_rfc3339(),
        };
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        state.execute_batch("PRAGMA synchronous=FULL")
            .map_err(|e| e.to_string())?;
        state.execute(
            "INSERT OR IGNORE INTO fresh_bash_wake_obligation VALUES
             (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                intended.request_id,
                intended.session_id,
                intended.seq,
                intended.source_id,
                intended.attempt_id,
                intended.lane_id,
                intended.source_generation,
                intended.root_id,
                intended.owner_generation,
                Self::recipient_identity_json(original)?,
                intended.payload_sha256,
                intended.payload_byte_len,
                intended.recorded_at,
            ],
        )
        .map_err(|e| format!("fresh Bash wake obligation conflicts: {e}"))?;
        let stored = self.read_bash_wake_obligation(&event.request_id)?
            .ok_or("fresh Bash wake obligation absent after insert")?;
        if stored.request_id != intended.request_id
            || stored.session_id != intended.session_id
            || stored.seq != intended.seq
            || stored.source_id != intended.source_id
            || stored.attempt_id != intended.attempt_id
            || stored.lane_id != intended.lane_id
            || stored.source_generation != intended.source_generation
            || stored.root_id != intended.root_id
            || stored.owner_generation != intended.owner_generation
            || stored.original_identity != intended.original_identity
            || stored.payload_sha256 != intended.payload_sha256
            || stored.payload_byte_len != intended.payload_byte_len
        {
            return Err("fresh Bash wake obligation exact retry changed".into());
        }
        Ok(stored)
    }

    pub fn read_bash_wake_obligation(
        &self,
        request_id: &str,
    ) -> Result<Option<FreshBashWakeObligation>, String> {
        validate_request_id(request_id)?;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let stored: Option<(String, i64, String, String, String, String, String, String, String, String, i64, String)> = state
            .query_row(
                "SELECT session_id,seq,source_id,attempt_id,lane_id,source_generation,
                        root_id,owner_generation,original_identity,payload_sha256,
                        payload_byte_len,recorded_at
                 FROM fresh_bash_wake_obligation WHERE request_id=?1",
                [request_id],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,
                        r.get(6)?,r.get(7)?,r.get(8)?,r.get(9)?,r.get(10)?,r.get(11)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some(row) = stored else { return Ok(None) };
        let obligation = FreshBashWakeObligation {
            request_id: request_id.into(), session_id: row.0, seq: row.1,
            source_id: row.2, attempt_id: row.3, lane_id: row.4,
            source_generation: row.5, root_id: row.6, owner_generation: row.7,
            original_identity: serde_json::from_str(&row.8).map_err(|e| e.to_string())?,
            payload_sha256: row.9, payload_byte_len: row.10, recorded_at: row.11,
        };
        let event = self.selected_private_bash_event(request_id)?;
        let (root, original) = self.released_handoff_for_root(&event.root_id)?;
        let session = self.read_session(&root.d_key)?.ok_or("wake D absent")?;
        if obligation.original_identity != original
            || obligation.session_id != session.session_id
            || obligation.source_id != event.source_id
            || obligation.attempt_id != event.attempt_id
            || obligation.lane_id != event.lane_id
            || obligation.source_generation != event.source_generation
            || obligation.root_id != event.root_id
            || obligation.owner_generation != event.owner_generation
            || self.recipient_binding(&session, &original)?
                != (event.root_id.clone(), event.owner_generation.clone())
        {
            return Err("fresh Bash wake obligation D/J/W attachment changed".into());
        }
        let notification: bool = state
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM fresh_bash_notify_request
                 WHERE request_id=?1 AND source_id=?2 AND attempt_id=?3
                   AND listener_session_id=?4 AND listener_invocation_uuid=?5
                   AND recipient_identity=?6 AND root_id=?7 AND owner_generation=?8)",
                params![
                    request_id,
                    obligation.source_id,
                    obligation.attempt_id,
                    obligation.session_id,
                    root.invocation_uuid,
                    Self::recipient_identity_json(&original)?,
                    obligation.root_id,
                    obligation.owner_generation,
                ],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !notification {
            return Err("fresh Bash wake obligation has no original notify request".into());
        }
        let query = format!(
            "SELECT {MAILBOX_ROW_COLUMNS} FROM mailbox WHERE session_id=?1 AND seq=?2"
        );
        let mailbox = self
            .sidecar
            .mailbox()
            .conn
            .query_row(&query, params![obligation.session_id, obligation.seq], map_mailbox_row)
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or("fresh Bash wake mailbox row absent")?;
        if mailbox.kind != AGENT_BASH_COMPLETE_KIND
            || mailbox.handle != obligation.source_id
            || mailbox.owner_invocation_uuid.as_deref() != Some(root.invocation_uuid.as_str())
            || mailbox.payload_sha256.as_deref() != Some(obligation.payload_sha256.as_str())
            || mailbox.payload_byte_len != Some(obligation.payload_byte_len)
            || mailbox.payload_retention_policy.as_deref() != Some("until_terminal_disposition")
        {
            return Err("fresh Bash wake mailbox row changed".into());
        }
        let (source, attempt) = self.accepted_source_for_row(
            &session, obligation.seq, &obligation.payload_sha256,
            obligation.payload_byte_len, &obligation.root_id,
            &obligation.owner_generation,
        )?;
        if source != obligation.source_id || attempt != obligation.attempt_id {
            return Err("fresh Bash wake obligation row/source changed".into());
        }
        Ok(Some(obligation))
    }
}
