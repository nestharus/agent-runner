fn fresh_bash_notify_schema_count(state: &Connection) -> Result<i64, String> {
    state
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE
         (type='table' AND name='fresh_bash_notify_request') OR
         (type='trigger' AND name IN ('fresh_bash_notify_request_no_update',
          'fresh_bash_notify_request_no_delete'))",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())
}

fn verify_fresh_bash_notify_schema(state: &Connection) -> Result<(), String> {
    if fresh_bash_notify_schema_count(state)? != 3 {
        return Err("fresh Bash notification schema is incomplete".into());
    }
    verify_fresh_sql_objects(
        state,
        FRESH_BASH_NOTIFY_SCHEMA,
        "fresh_bash_notify_request",
        &[
            "fresh_bash_notify_request_no_update",
            "fresh_bash_notify_request_no_delete",
        ],
    )?;
    Ok(())
}

impl FreshV30Lane {
    fn selected_private_bash_event(
        &self,
        request_id: &str,
    ) -> Result<FreshBashSourceEvent, String> {
        validate_request_id(request_id)?;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let receipt: String = state
            .query_row(
                "SELECT receipt_json FROM fresh_bash_selected_event WHERE request_id=?1",
                [request_id],
                |r| r.get(0),
            )
            .map_err(|_| "fresh Bash source W absent or unknown".to_string())?;
        let event: FreshBashSourceEvent =
            serde_json::from_str(&receipt).map_err(|e| e.to_string())?;
        self.verify_captured_bash_event(&event)?;
        let accepted: bool = state
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM fresh_lane_accepted_source WHERE source_id=?1
             AND attempt_id=?2 AND state_admission_id=?3 AND registration_digest=?4
             AND source_generation=?5 AND lane_id=?6 AND root_id=?7 AND owner_generation=?8)",
                params![
                    event.source_id,
                    event.attempt_id,
                    event.state_admission_id,
                    event.registration_digest,
                    event.source_generation,
                    event.lane_id,
                    event.root_id,
                    event.owner_generation
                ],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !accepted {
            return Err("fresh Bash W has no exact State acceptance".into());
        }
        Ok(event)
    }

    /// Called only for a broker-challenged, pinned original root actor. The
    /// private C registration is response-only unless this explicit request is
    /// committed. The State request and attachment are one durable fence;
    /// missing sidecar materialization is repair debt, never an implicit ACK.
    pub fn request_private_bash_notification(
        &mut self,
        request_id: &str,
        recipient: &FreshRecipientIdentity,
    ) -> Result<i64, String> {
        let event = self.selected_private_bash_event(request_id)?;
        let child = self.require_complete_bash_child(request_id)?;
        let (root, original_actor) = self.released_handoff_for_root(&event.root_id)?;
        if *recipient != original_actor
            || event.source_id != child.handle
            || event.attempt_id != child.invocation_uuid
            || event.root_id != root.old_release.prepared.root_id
        {
            return Err("fresh notification requester is not the exact original listener".into());
        }
        let session = self
            .read_session(&root.d_key)?
            .ok_or("fresh root listener D absent")?;
        self.require_released_invocation(&root, recipient, &session)?;
        let identity = Self::recipient_identity_json(recipient)?;
        let mut state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        state
            .execute_batch("PRAGMA synchronous=FULL")
            .map_err(|e| e.to_string())?;
        let tx = state
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT OR IGNORE INTO fresh_bash_notify_request
             (request_id,source_id,attempt_id,listener_session_id,listener_invocation_uuid,
              recipient_identity,root_id,owner_generation,requested_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                request_id,
                event.source_id,
                event.attempt_id,
                session.session_id,
                root.invocation_uuid,
                identity,
                event.root_id,
                event.owner_generation,
                Utc::now().to_rfc3339()
            ],
        )
        .map_err(|e| e.to_string())?;
        let stored: (String, String, String, String, String, String, String) = tx
            .query_row(
                "SELECT source_id,attempt_id,listener_session_id,listener_invocation_uuid,
             recipient_identity,root_id,owner_generation FROM fresh_bash_notify_request
             WHERE request_id=?1",
                [request_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .map_err(|e| e.to_string())?;
        if stored
            != (
                event.source_id.clone(),
                event.attempt_id.clone(),
                session.session_id.clone(),
                root.invocation_uuid.clone(),
                identity.clone(),
                event.root_id.clone(),
                event.owner_generation.clone(),
            )
        {
            return Err("fresh Bash notification request replay conflict".into());
        }
        tx.execute(
            "INSERT OR IGNORE INTO fresh_lane_recipient_attachment
             (session_id,lane_id,source_generation,root_id,owner_generation,recipient_identity,attached_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![session.session_id,event.lane_id,event.source_generation,event.root_id,
                event.owner_generation,identity,Utc::now().to_rfc3339()],
        ).map_err(|e| e.to_string())?;
        let attachment: (String, String, String, String, String) = tx
            .query_row(
                "SELECT lane_id,source_generation,root_id,owner_generation,recipient_identity
             FROM fresh_lane_recipient_attachment WHERE session_id=?1",
                [&session.session_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .map_err(|e| e.to_string())?;
        if attachment
            != (
                event.lane_id.clone(),
                event.source_generation.clone(),
                event.root_id.clone(),
                event.owner_generation.clone(),
                identity,
            )
        {
            return Err("fresh Bash recipient attachment replay conflict".into());
        }
        tx.commit().map_err(|e| e.to_string())?;
        #[cfg(feature = "age319-private-broker-fixture")]
        if std::env::var_os("AGE319_PRIVATE_NOTIFY_STATE_ONLY_V1").is_some() {
            return Err("fresh notification State request retained before sidecar repair".into());
        }
        self.reconcile_private_bash_notification(request_id)
    }

    /// Repairs only a selected W with an explicit request. Output is read and
    /// verified again from broker-owned physical files. Each cross-file step
    /// has exact readback; an error leaves the State request visible as debt.
    pub fn reconcile_private_bash_notification(&mut self, request_id: &str) -> Result<i64, String> {
        let event = self.selected_private_bash_event(request_id)?;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let request: (String, String, String, String, String, String, String) = state
            .query_row(
                "SELECT source_id,attempt_id,listener_session_id,listener_invocation_uuid,
             recipient_identity,root_id,owner_generation FROM fresh_bash_notify_request
             WHERE request_id=?1",
                [request_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .map_err(|_| {
                "fresh Bash notification request absent; original sync is response-only".to_string()
            })?;
        let (root, root_actor) = self.released_handoff_for_root(&event.root_id)?;
        let root_session = self
            .read_session(&root.d_key)?
            .ok_or("fresh root D absent")?;
        if request
            != (
                event.source_id.clone(),
                event.attempt_id.clone(),
                root_session.session_id.clone(),
                root.invocation_uuid.clone(),
                Self::recipient_identity_json(&root_actor)?,
                event.root_id.clone(),
                event.owner_generation.clone(),
            )
        {
            return Err("fresh Bash request/listener/recipient binding changed".into());
        }
        self.require_state_recipient_attachment(
            &root_session.session_id,
            &request.4,
            &event.root_id,
            &event.owner_generation,
        )?;
        let source = (
            event.attempt_id.clone(),
            event.state_admission_id.clone(),
            event.registration_digest.clone(),
            event.source_generation.clone(),
            event.lane_id.clone(),
            event.root_id.clone(),
            event.owner_generation.clone(),
        );
        {
            let side = &self.sidecar.mailbox().conn;
            side.execute(
                "INSERT OR IGNORE INTO fresh_recipient_source VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    event.source_id,
                    source.0,
                    source.1,
                    source.2,
                    source.3,
                    source.4,
                    source.5,
                    source.6
                ],
            )
            .map_err(|e| e.to_string())?;
            let stored_source: (String, String, String, String, String, String, String) = side
                .query_row(
                    "SELECT attempt_id,state_admission_id,registration_digest,source_generation,
             lane_id,root_id,owner_generation FROM fresh_recipient_source WHERE source_id=?1",
                    [&event.source_id],
                    |r| {
                        Ok((
                            r.get(0)?,
                            r.get(1)?,
                            r.get(2)?,
                            r.get(3)?,
                            r.get(4)?,
                            r.get(5)?,
                            r.get(6)?,
                        ))
                    },
                )
                .map_err(|e| e.to_string())?;
            if stored_source != source {
                return Err("fresh recipient source sidecar conflict".into());
            }
            side.execute(
                "INSERT OR IGNORE INTO fresh_recipient_binding VALUES(?1,?2,?3,?4,?5)",
                params![
                    root_session.session_id,
                    request.4,
                    event.root_id,
                    event.owner_generation,
                    event.source_generation
                ],
            )
            .map_err(|e| e.to_string())?;
            let binding: (String, String, String, String) = side
                .query_row(
                    "SELECT recipient_identity,root_id,owner_generation,source_generation
             FROM fresh_recipient_binding WHERE session_id=?1",
                    [&root_session.session_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .map_err(|e| e.to_string())?;
            if binding
                != (
                    request.4.clone(),
                    event.root_id.clone(),
                    event.owner_generation.clone(),
                    event.source_generation.clone(),
                )
            {
                return Err("fresh recipient binding sidecar conflict".into());
            }
        }
        let directory = self
            .state_path
            .parent()
            .ok_or("fresh State parent absent")?
            .join(FRESH_PROVIDER_DIRECTORY);
        let stdout = fs::read(directory.join(format!("{}.stdout", event.physical_grant_id)))
            .map_err(|e| e.to_string())?;
        let stderr = fs::read(directory.join(format!("{}.stderr", event.physical_grant_id)))
            .map_err(|e| e.to_string())?;
        if stdout.len() as u64 != event.stdout_len
            || stderr.len() as u64 != event.stderr_len
            || format!("{:x}", Sha256::digest(&stdout)) != event.stdout_sha256
            || format!("{:x}", Sha256::digest(&stderr)) != event.stderr_sha256
        {
            return Err("fresh original raw payload changed".into());
        }
        let payload = serde_json::to_string(&serde_json::json!({
            "protocol":"fresh-bash-complete-v30", "source":event,
            "stdout_bytes":stdout, "stderr_bytes":stderr,
        }))
        .map_err(|e| e.to_string())?;
        let result =
            self.sidecar
                .mailbox_mut()
                .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
                    session_id: &root_session.session_id,
                    handle: &event.source_id,
                    payload_json: &payload,
                    owner_invocation_uuid: Some(&root.invocation_uuid),
                    matched_os_pid: None,
                    matched_os_boot_id: None,
                    matched_os_pid_starttime_ticks: None,
                    matched_chain_index: None,
                    state_dir: "fresh-v30",
                    meta_path: "fresh-v30",
                    log_path: "fresh-v30",
                    rc_path: "fresh-v30",
                    rc: event.wait_status,
                })?;
        let row = match result {
            EnqueueResult::Inserted(row) | EnqueueResult::AlreadyEnqueued(row) => row,
            EnqueueResult::Conflict { .. } => return Err("fresh Bash mailbox row conflict".into()),
        };
        if row.session_id != root_session.session_id
            || row.handle != event.source_id
            || row.owner_invocation_uuid.as_deref() != Some(root.invocation_uuid.as_str())
            || row.kind != AGENT_BASH_COMPLETE_KIND
            || row.rc != event.wait_status
            || row.target_kind.is_some()
            || row.target_id.is_some()
            || row.payload_retention_policy.as_deref() != Some("until_terminal_disposition")
        {
            return Err("fresh Bash mailbox row identity changed".into());
        }
        let sha = row
            .payload_sha256
            .as_ref()
            .ok_or("fresh Bash row payload digest absent")?;
        let len = row
            .payload_byte_len
            .ok_or("fresh Bash row payload length absent")?;
        self.sidecar
            .mailbox()
            .payloads()
            .verify_mailbox_row_payload(&row)?;
        #[cfg(feature = "age319-private-broker-fixture")]
        if std::env::var_os("AGE319_PRIVATE_NOTIFY_ROW_ONLY_V1").is_some() {
            return Err("fresh notification row retained before source mapping repair".into());
        }
        let side = &self.sidecar.mailbox().conn;
        side.execute(
            "INSERT OR IGNORE INTO fresh_recipient_row_source VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                root_session.session_id,
                row.seq,
                event.source_id,
                event.attempt_id,
                sha,
                len
            ],
        )
        .map_err(|e| e.to_string())?;
        let mapped: (String, String, String, i64) = side
            .query_row(
                "SELECT source_id,attempt_id,payload_sha256,payload_byte_len
             FROM fresh_recipient_row_source WHERE session_id=?1 AND seq=?2",
                params![root_session.session_id, row.seq],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .map_err(|e| e.to_string())?;
        if mapped
            != (
                event.source_id.clone(),
                event.attempt_id.clone(),
                sha.clone(),
                len,
            )
        {
            return Err("fresh Bash row source sidecar conflict".into());
        }
        if row.delivered_at.is_some() {
            let acknowledged: bool = side.query_row(
                "SELECT EXISTS(SELECT 1 FROM fresh_recipient_grant WHERE session_id=?1 AND seq=?2
                 AND source_id=?3 AND attempt_id=?4 AND payload_sha256=?5 AND payload_byte_len=?6
                 AND phase='acked' AND acknowledged_at IS NOT NULL)",
                params![root_session.session_id,row.seq,event.source_id,event.attempt_id,sha,len],
                |r| r.get(0),
            ).map_err(|e| e.to_string())?;
            if !acknowledged {
                return Err("fresh Bash row has no exact ACK provenance".into());
            }
        }
        Ok(row.seq)
    }

    pub fn repair_private_bash_notifications(&mut self) -> Result<(), String> {
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut statement = state
            .prepare("SELECT request_id FROM fresh_bash_notify_request ORDER BY request_id")
            .map_err(|e| e.to_string())?;
        let ids = statement
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        drop(statement);
        drop(state);
        for id in ids {
            self.reconcile_private_bash_notification(&id)?;
        }
        Ok(())
    }
}
