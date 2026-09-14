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

impl MailboxDb {
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
                "DROP TRIGGER completion_continuation_notification_ack;
            DROP TABLE completion_continuation_notification; PRAGMA user_version=18;",
            )
            .unwrap();
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
