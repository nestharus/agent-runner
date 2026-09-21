-- Partial live projections keep terminal history out of launch, delivery,
-- recovery, and supervisor zero-work plans.
CREATE INDEX IF NOT EXISTS idx_mailbox_pending_session_live
    ON mailbox(session_id, seq)
    WHERE delivered_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_mailbox_pending_target_live
    ON mailbox(target_kind, target_id, seq)
    WHERE delivered_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_mailbox_deliverable_session_live
    ON mailbox(session_id, seq)
    WHERE delivered_at IS NULL
      AND (delivery_error IS NULL OR delivery_error NOT IN (
          'wake_sweep_abandoned',
          'mailbox_payload_verification_failed',
          'mailbox_ingress_expired'
      ));

CREATE INDEX IF NOT EXISTS idx_mailbox_deliverable_target_live
    ON mailbox(target_kind, target_id, seq)
    WHERE delivered_at IS NULL
      AND (delivery_error IS NULL OR delivery_error NOT IN (
          'wake_sweep_abandoned',
          'mailbox_payload_verification_failed',
          'mailbox_ingress_expired'
      ));

CREATE INDEX IF NOT EXISTS idx_mailbox_deliverable_global
    ON mailbox(seq, session_id)
    WHERE delivered_at IS NULL
      AND (delivery_error IS NULL OR delivery_error NOT IN (
          'wake_sweep_abandoned',
          'mailbox_payload_verification_failed',
          'mailbox_ingress_expired'
      ));

CREATE INDEX IF NOT EXISTS idx_mailbox_delivery_attempt_unresolved
    ON mailbox_delivery_attempts(session_id, attempt_id)
    WHERE resolved_at IS NULL;

DROP INDEX IF EXISTS idx_completion_event_listener_pending;
DROP INDEX IF EXISTS idx_completion_event_listener_unacknowledged;
CREATE INDEX IF NOT EXISTS idx_completion_event_listener_session_live
    ON completion_event_listener(session_id, active, event_id)
    WHERE retirement_pending = 1;

-- A response-only listener with an accepted source and no explicit request is
-- explicitly terminal even though its historical acknowledgement stays NULL.
-- Materialize the retirement obligation so idle-close never anti-joins that
-- retained history while holding the authority writers.
UPDATE completion_event_listener AS listener
SET active = 0
WHERE EXISTS (
    SELECT 1
    FROM mailbox
    WHERE mailbox.seq = listener.mailbox_seq
      AND mailbox.delivered_at IS NULL
      AND mailbox.delivery_error IN (
          'wake_sweep_abandoned',
          'mailbox_payload_verification_failed',
          'mailbox_ingress_expired'
      )
);

UPDATE completion_event_listener AS listener
SET retirement_pending = 0
WHERE listener.acknowledged_at IS NOT NULL
   OR EXISTS (
        SELECT 1
        FROM mailbox
        WHERE mailbox.seq = listener.mailbox_seq
          AND mailbox.delivered_at IS NULL
          AND mailbox.delivery_error IN (
              'wake_sweep_abandoned',
              'mailbox_payload_verification_failed',
              'mailbox_ingress_expired'
          )
   )
   OR (
        listener.active = 0
        AND listener.mailbox_seq IS NULL
        AND EXISTS (
            SELECT 1
            FROM completion_continuation_notification AS notification
            WHERE notification.event_id = listener.event_id
              AND notification.listener_id = listener.listener_id
              AND notification.policy = 'response_only'
              AND notification.requested_at IS NULL
        )
        AND EXISTS (
            SELECT 1 FROM completion_event AS event
            WHERE event.event_id = listener.event_id
              AND event.state = 'triggered'
        )
        AND EXISTS (
            SELECT 1 FROM completion_continuation_source AS source
            WHERE source.event_id = listener.event_id
              AND source.phase = 'accepted'
        )
   );

CREATE INDEX IF NOT EXISTS idx_completion_event_listener_retirement_pending
    ON completion_event_listener(event_id, listener_id)
    WHERE retirement_pending = 1;

CREATE INDEX IF NOT EXISTS idx_runtime_generation_live_session
    ON runtime_generation(session_id, generation_uuid)
    WHERE lifecycle_state != 'exited';
