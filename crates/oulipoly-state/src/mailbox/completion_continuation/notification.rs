//! Listener notification policy is neither host consumption nor physical drain.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationPolicy {
    Unknown,
    ResponseOnly,
    Notify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationDisposition {
    Unknown,
    Pending,
    NoNotificationRequired,
    Handled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationDeliveryEvidence {
    HistoricalAck,
    ManualAssertion,
    TransportHandoff,
    NativeInputReceipt,
    Unknown,
}

fn delivery_evidence(basis: Option<&str>) -> NotificationDeliveryEvidence {
    match basis {
        Some("manual_ack") => NotificationDeliveryEvidence::ManualAssertion,
        Some("injected") => NotificationDeliveryEvidence::TransportHandoff,
        Some("native_receipt") => NotificationDeliveryEvidence::NativeInputReceipt,
        Some(b) if b.starts_with("historical:") => NotificationDeliveryEvidence::HistoricalAck,
        _ => NotificationDeliveryEvidence::Unknown,
    }
}

/// A request is explicit authority to notify this listener, not delivery evidence.
/// Repetition is idempotent by the durable event/listener identity.
#[derive(Debug, Clone, Copy)]
pub struct CompletionNotificationRequest<'a> {
    pub event_id: &'a str,
    pub listener_id: &'a str,
}

/// Admit initial listener delivery eligibility; exact bound-source policy is
/// classified by this owner once registration has established its binding.
pub(in crate::mailbox) fn register_listener_on(
    tx: &Transaction<'_>,
    input: &CompletionEventRegistrationInput<'_>,
    listeners_frozen: bool,
    now: &str,
) -> Result<(), String> {
    let (Some(session_id), Some(invocation_uuid)) =
        (input.owner_session_id, input.owner_invocation_uuid)
    else {
        return Err("Completion event owner session and invocation are both required".to_string());
    };
    if let Some(listener) = completion_event_listener_on(tx, input.event_id, invocation_uuid)? {
        return validate_completion_event_listener_replay(&listener, session_id, invocation_uuid);
    }
    if listeners_frozen {
        return Err(format!(
            "Completion event {} cannot register a listener after it was triggered",
            input.event_id
        ));
    }
    tx.execute(
        "INSERT OR IGNORE INTO completion_event_listener (
            event_id, listener_id, session_id, owner_invocation_uuid, active, created_at
         ) VALUES (?1, ?2, ?3, ?2, ?4, ?5)",
        params![
            input.event_id,
            invocation_uuid,
            session_id,
            input.delivery_mode == "async",
            now,
        ],
    )
    .map_err(|err| format!("Failed to register completion event listener: {err}"))?;
    let listener = completion_event_listener_on(tx, input.event_id, invocation_uuid)?
        .ok_or_else(|| "Registered completion listener disappeared".to_string())?;
    validate_completion_event_listener_replay(&listener, session_id, invocation_uuid)
}

/// Only the exact immutable admitted binding can classify original sync.
/// Existing active/materialized obligations are never retracted or called detach.
pub(in crate::mailbox) fn classify_on(
    tx: &Transaction<'_>,
    binding: &AdmittedSourceBinding,
) -> Result<(), String> {
    let source = binding.registration()?;
    tx.execute(
        "INSERT OR IGNORE INTO completion_continuation_notification(event_id,listener_id)
        SELECT event_id,listener_id FROM completion_event_listener WHERE event_id=?1",
        [&source.handle],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE completion_continuation_notification SET
        policy=CASE WHEN ?2='sync' AND EXISTS(SELECT 1 FROM completion_event_listener l
            WHERE l.event_id=completion_continuation_notification.event_id
            AND l.listener_id=completion_continuation_notification.listener_id
            AND l.session_id=?3 AND l.owner_invocation_uuid=?4 AND l.active=0
            AND l.mailbox_seq IS NULL AND l.acknowledged_at IS NULL)
            THEN 'response_only' ELSE 'notify' END,
        policy_origin=?5, policy_recorded_at=?6
        WHERE event_id=?1 AND policy='unknown'",
        params![
            source.handle,
            source.delivery_mode,
            source.owner_session_id,
            source.owner_invocation_uuid,
            format!("exact_admitted_binding:{}", source.registration_id),
            now_rfc3339()
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// One completion-delivery owner applies policy and projects obligations for
/// both verified acceptance and admitted-source repair, in their transaction.
/// Producers provide source truth, not a second eligibility decision.
pub(in crate::mailbox) fn reconcile_on(
    tx: &Transaction<'_>,
    event: &CompletionEventRow,
    binding: Option<&AdmittedSourceBinding>,
    now: &str,
) -> Result<(), String> {
    if let Some(binding) = binding {
        classify_on(tx, binding)?;
        if event.state == "triggered" {
            tx.execute(
                "UPDATE completion_event_listener SET active=1,retirement_pending=1
                 WHERE event_id=?1 AND acknowledged_at IS NULL AND EXISTS (
                   SELECT 1 FROM completion_continuation_notification n
                   WHERE n.event_id=completion_event_listener.event_id
                     AND n.listener_id=completion_event_listener.listener_id
                     AND n.policy='notify')",
                [&event.event_id],
            )
            .map_err(|e| format!("Failed to apply completion presentation policy: {e}"))?;
            tx.execute(
                "UPDATE completion_event_listener AS listener
                 SET retirement_pending=0
                 WHERE listener.event_id=?1
                   AND listener.acknowledged_at IS NULL
                   AND listener.active=0
                   AND listener.mailbox_seq IS NULL
                   AND EXISTS (
                       SELECT 1 FROM completion_continuation_notification AS notification
                       WHERE notification.event_id=listener.event_id
                         AND notification.listener_id=listener.listener_id
                         AND notification.policy='response_only'
                         AND notification.requested_at IS NULL
                   )
                   AND EXISTS (
                       SELECT 1 FROM completion_continuation_source AS source
                       WHERE source.event_id=listener.event_id AND source.phase='accepted'
                   )",
                [&event.event_id],
            )
            .map_err(|e| format!("Failed to settle response-only listener retirement: {e}"))?;
        }
    }
    materialize_on(tx, event, binding.is_some(), now)
}

fn materialize_on(
    tx: &Transaction<'_>,
    event: &CompletionEventRow,
    accepted_v2_binding: bool,
    now: &str,
) -> Result<(), String> {
    if event.state != "triggered" {
        return Ok(());
    }
    let listeners = completion_event_listeners_on(tx, &event.event_id)?;
    let listener_count = listeners.len();
    let pending = listeners
        .into_iter()
        .filter(|listener| {
            listener.active && listener.acknowledged_at.is_none() && listener.mailbox_seq.is_none()
        })
        .collect::<Vec<_>>();
    if pending.is_empty() {
        return Ok(());
    }
    let provenance = if accepted_v2_binding {
        "v2"
    } else if source::bound_event(tx, &event.event_id)? {
        "v2"
    } else {
        super::super::schema::classify_existing_completion_payload(
            tx,
            event.payload_json.as_deref().unwrap_or(""),
            event.payload_file_path.as_deref(),
            event.payload_sha256.as_deref(),
            event.payload_byte_len,
            event.payload_retention_policy.as_deref(),
            event.triggered_at.as_deref(),
        )
        .unwrap_or("unclassified")
    };
    for listener in pending {
        let handle = completion_listener_mailbox_handle(event, &listener, listener_count);
        let changed =
            insert_completion_listener_mailbox_row(tx, event, &listener, &handle, now, provenance)?;
        let row = query_mailbox_by_kind_handle_tx(tx, AGENT_BASH_COMPLETE_KIND, &handle)?
            .ok_or_else(|| "Completion listener mailbox row disappeared".to_string())?;
        if changed == 0
            && (row.session_id != listener.session_id
                || row.owner_invocation_uuid.as_deref()
                    != Some(listener.owner_invocation_uuid.as_str())
                || row.payload_sha256 != event.payload_sha256)
        {
            return Err(format!(
                "Completion event {} mailbox identity conflicts with an existing row",
                event.event_id
            ));
        }
        let stored_provenance: String = tx
            .query_row(
                "SELECT completion_provenance FROM mailbox WHERE seq=?1",
                [row.seq],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if stored_provenance != provenance {
            return Err(
                "completion listener mailbox provenance conflicts with accepted source".into(),
            );
        }
        tx.execute(
            "UPDATE completion_event_listener
             SET mailbox_seq = ?3
             WHERE event_id = ?1 AND listener_id = ?2 AND mailbox_seq IS NULL",
            params![event.event_id, listener.listener_id, row.seq],
        )
        .map_err(|err| format!("Failed to bind completion listener mailbox row: {err}"))?;
    }
    Ok(())
}

impl MailboxDb {
    pub fn activate_completion_event_listeners(
        &mut self,
        event_id: &str,
    ) -> Result<CompletionEventTriggerResult, String> {
        self.request_completion_notification_on_scope(event_id, None)
    }

    /// Explicit request scoped to one listener; event-wide administrative
    /// activation remains a separate API and is not an inferred detach.
    pub fn request_completion_notification(
        &mut self,
        request: CompletionNotificationRequest<'_>,
    ) -> Result<CompletionEventTriggerResult, String> {
        self.request_completion_notification_on_scope(request.event_id, Some(request.listener_id))
    }

    fn request_completion_notification_on_scope(
        &mut self,
        event_id: &str,
        listener_id: Option<&str>,
    ) -> Result<CompletionEventTriggerResult, String> {
        let now = now_rfc3339();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|err| {
                format!("Failed to start completion listener activation transaction: {err}")
            })?;
        if let Some(listener_id) = listener_id
            && completion_event_listener_on(&tx, event_id, listener_id)?.is_none()
        {
            return Err("notification request listener is not registered".into());
        }
        let event = completion_event_by_id_on(&tx, event_id)?
            .ok_or_else(|| format!("Completion event {event_id} is not registered"))?;
        if completion_continuation::bound_event(&tx, event_id)? {
            let running: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM completion_continuation_owner WHERE phase='running')", [], |r| r.get(0)).map_err(|e| e.to_string())?;
            if !running {
                return Err("completion_owner_closed_retryable: notification request requires a running owner".into());
            }
        }
        // Record first effective scoped request facts, not incoming retry packets.
        // Neither scope grants an inferred detach actor or a receipt.
        tx.execute("INSERT INTO completion_continuation_notification(event_id,listener_id,requested_at,request_basis)
            SELECT event_id,listener_id,?2,CASE WHEN ?3 IS NULL THEN 'explicit_event_activation_unattributed' ELSE 'explicit_listener_activation_unattributed' END
            FROM completion_event_listener WHERE event_id=?1 AND acknowledged_at IS NULL AND (?3 IS NULL OR listener_id=?3)
            ON CONFLICT(event_id,listener_id) DO UPDATE SET requested_at=COALESCE(requested_at,excluded.requested_at),
            request_basis=COALESCE(request_basis,excluded.request_basis)", params![event_id,&now,listener_id])
            .map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE completion_event_listener
             SET active = 1, retirement_pending = 1
             WHERE event_id = ?1 AND acknowledged_at IS NULL AND (?2 IS NULL OR listener_id=?2)",
            params![event_id, listener_id],
        )
        .map_err(|err| format!("Failed to activate completion event listeners: {err}"))?;
        if event.state == "triggered" {
            materialize_on(&tx, &event, false, &now)?;
        }
        tx.commit()
            .map_err(|err| format!("Failed to commit completion listener activation: {err}"))?;
        self.completion_event_trigger_result(event_id, false)
    }

    /// Existing detach CLI shape targets the immutable original caller for v2;
    /// legacy event activation retains its event-wide administrative meaning.
    pub fn request_original_completion_notification(
        &mut self,
        event: &str,
    ) -> Result<CompletionEventTriggerResult, String> {
        let encoded: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT binding FROM completion_continuation_source WHERE event_id=?1",
                [event],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let listener = encoded
            .map(|b| {
                AdmittedSourceBinding::decode(&b)?
                    .registration()
                    .map(|s| s.owner_invocation_uuid)
            })
            .transpose()?;
        match listener {
            Some(listener_id) => {
                self.request_completion_notification(CompletionNotificationRequest {
                    event_id: event,
                    listener_id: &listener_id,
                })
            }
            None => self.activate_completion_event_listeners(event),
        }
    }

    /// Read-only diagnostics. Actor labels are assertions, not authentication;
    /// injected means transport handoff, never recipient consumption.
    pub fn completion_notification_diagnostics(
        &self,
        event: &str,
    ) -> Result<Vec<serde_json::Value>, String> {
        let mut stmt = self.conn.prepare("SELECT l.listener_id,l.active,l.mailbox_seq,l.acknowledged_at,
            COALESCE(n.policy,'unknown'),n.policy_origin,n.policy_recorded_at,n.requested_at,n.request_basis,
            n.ack_at,n.ack_basis,n.ack_actor_label,n.ack_mailbox_seq,e.state,
            EXISTS(SELECT 1 FROM completion_continuation_source s WHERE s.event_id=l.event_id AND s.phase='accepted'),n.ack_native_evidence
            FROM completion_event_listener l JOIN completion_event e ON e.event_id=l.event_id
            LEFT JOIN completion_continuation_notification n ON n.event_id=l.event_id AND n.listener_id=l.listener_id
            WHERE l.event_id=?1 ORDER BY l.listener_id").map_err(|e| e.to_string())?;
        let rows = stmt.query_map([event], |r| {
            let active: bool = r.get(1)?;
            let seq: Option<i64> = r.get(2)?;
            let ack: Option<String> = r.get(3)?;
            let policy = match r.get::<_,String>(4)?.as_str() {
                "response_only" => NotificationPolicy::ResponseOnly,
                "notify" => NotificationPolicy::Notify,
                _ => NotificationPolicy::Unknown,
            };
            let request: Option<String> = r.get(7)?;
            let accepted: bool = r.get(14)?;
            let disposition = if ack.is_some() { NotificationDisposition::Handled }
                else if policy == NotificationPolicy::ResponseOnly && request.is_none() && !active && seq.is_none()
                    && accepted && r.get::<_,String>(13)? == "triggered" { NotificationDisposition::NoNotificationRequired }
                else if policy == NotificationPolicy::Unknown { NotificationDisposition::Unknown }
                else { NotificationDisposition::Pending };
            Ok(serde_json::json!({
                "listener_id":r.get::<_,String>(0)?, "active":active,"mailbox_seq":seq,
                "policy":policy,"policy_origin":r.get::<_,Option<String>>(5)?,
                "policy_recorded_at":r.get::<_,Option<String>>(6)?,"requested_at":request,
                "request_basis":r.get::<_,Option<String>>(8)?,"disposition":disposition,
                "acknowledged_at":ack,"ack_recorded_at":r.get::<_,Option<String>>(9)?,
                "delivery_evidence":delivery_evidence(r.get::<_,Option<String>>(10)?.as_deref()),
                "ack_basis":r.get::<_,Option<String>>(10)?,"ack_actor_label":r.get::<_,Option<String>>(11)?,
                "ack_original_mailbox_seq":r.get::<_,Option<i64>>(12)?,
                "native_receipt_evidence":r.get::<_,Option<String>>(15)?,
                "actor_attribution":if r.get::<_,Option<String>>(15)?.is_some() { "exact_native_attempt_binding" } else { "asserted_label_not_authenticated" },
                "recipient_consumption":"unconfirmed", "source_accepted":accepted,
                "physical_custody":"separate_obligation"
            }))
        }).map_err(|e| e.to_string())?;
        rows.map(|r| r.map_err(|e| e.to_string())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualified_native_receipt_survives_projection_failure_and_history_pruning() {
        let root = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&root.path().join("pid-identity.db")).unwrap();
        db.register_completion_event(CompletionEventRegistrationInput {
            event_id: "receipt-event",
            delivery_mode: "async",
            owner_session_id: Some("session"),
            owner_invocation_uuid: Some("owner"),
            state_dir: "/offline",
            meta_path: "/offline/meta",
            log_path: "/offline/log",
            rc_path: "/offline/rc",
        })
        .unwrap();
        let result = db
            .trigger_completion_event(CompletionEventTriggerInput {
                event_id: "receipt-event",
                payload_json: "{}",
                rc: 0,
                state_dir: "/offline",
                meta_path: "/offline/meta",
                log_path: "/offline/log",
                rc_path: "/offline/rc",
            })
            .unwrap();
        let seq = result.mailbox_rows[0].seq;
        db.register_delivery_attempt("attempt", "session", "native", &[seq], 0)
            .unwrap();
        let anchor = MailboxDeliveryObservationAnchor {
            provider_name: "provider".into(),
            provider_instance_id: "instance".into(),
            settings_id: "settings".into(),
            provider_session_id: "session".into(),
            resume_token: Some("anchor".into()),
            expected_sha256: "a".repeat(64),
        };
        db.record_delivery_observation_anchor("attempt", "session", &anchor)
            .unwrap();
        db.advance_delivery_observation_progress("attempt", None, "checkpoint")
            .unwrap();
        let mut wrong = anchor.clone();
        wrong.resume_token = Some("wrong".into());
        assert!(
            !db.confirm_native_delivery_receipt("attempt", "native", &wrong, "checkpoint", "turn")
                .unwrap()
        );
        db.conn.execute_batch("CREATE TRIGGER reject_projection BEFORE UPDATE OF delivered_at ON mailbox BEGIN SELECT RAISE(ABORT,'projection fault'); END;").unwrap();
        assert!(
            db.confirm_native_delivery_receipt("attempt", "native", &anchor, "checkpoint", "turn")
                .unwrap_err()
                .contains("projection fault")
        );
        assert_eq!(
            db.delivery_observation_confirmation("attempt")
                .unwrap()
                .as_deref(),
            Some("turn")
        );
        assert!(
            db.completion_event_listeners("receipt-event").unwrap()[0]
                .acknowledged_at
                .is_none()
        );
        db.conn
            .execute_batch("DROP TRIGGER reject_projection;")
            .unwrap();
        db.project_confirmed_native_delivery_receipt("attempt")
            .unwrap();
        let before = db
            .completion_notification_diagnostics("receipt-event")
            .unwrap();
        assert_eq!(before[0]["delivery_evidence"], "native_input_receipt");
        let native: serde_json::Value =
            serde_json::from_str(before[0]["native_receipt_evidence"].as_str().unwrap()).unwrap();
        assert_eq!(native["attempt_id"], "attempt");
        assert_eq!(native["anchor_token"], "anchor");
        assert_eq!(native["turn_id"], "turn");
        // A repeated manual ACK must not replace the first effective native evidence.
        db.acknowledge_range("session", seq, seq, "manual-label")
            .unwrap();
        assert_eq!(
            before,
            db.completion_notification_diagnostics("receipt-event")
                .unwrap()
        );
        db.allow_historical_access_for_test();
        let pruned = db.prune_terminal_history_with_keep(256, 0).unwrap();
        assert_eq!(pruned.delivery_attempts_deleted, 1);
        let after = db
            .completion_notification_diagnostics("receipt-event")
            .unwrap();
        for key in [
            "ack_original_mailbox_seq",
            "ack_recorded_at",
            "ack_basis",
            "ack_actor_label",
            "native_receipt_evidence",
            "delivery_evidence",
        ] {
            assert_eq!(before[0][key], after[0][key]);
        }
        assert!(after[0]["mailbox_seq"].is_null());
        // Old ACKs retain their actual meaning even when pruning has already
        // removed sequence/actor attribution. Migration must not invent either.
        db.conn
            .execute_batch(
                "DROP VIEW mailbox_retained_delivery_finalizers;
            DROP INDEX mailbox_completed_turn_pins_attempt;
            DROP TABLE mailbox_completed_turn_pins;
            DROP TABLE mailbox_completed_turn_tails;
            DROP TRIGGER completion_continuation_notification_ack;
            DROP TABLE completion_continuation_notification; PRAGMA user_version=18;",
            )
            .unwrap();
        super::super::schema::remove_completion_recovery_working_set_for_legacy_fixture(&db.conn);
        db.conn.pragma_update(None, "user_version", 18).unwrap();
        db = MailboxDb::open(&root.path().join("pid-identity.db")).unwrap();
        let historical = db
            .completion_notification_diagnostics("receipt-event")
            .unwrap();
        assert_eq!(historical[0]["delivery_evidence"], "historical_ack");
        assert_eq!(
            historical[0]["acknowledged_at"],
            before[0]["acknowledged_at"]
        );
        assert_eq!(historical[0]["disposition"], "handled");
        assert!(historical[0]["ack_actor_label"].is_null());
        assert!(historical[0]["ack_original_mailbox_seq"].is_null());
        assert!(historical[0]["native_receipt_evidence"].is_null());
    }
}
