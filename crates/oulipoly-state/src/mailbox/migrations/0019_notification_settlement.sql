-- Candidate-only evolution. Deployment requires exclusion of all old writers.
CREATE TABLE completion_continuation_notification (
    event_id TEXT NOT NULL,
    listener_id TEXT NOT NULL,
    policy TEXT NOT NULL DEFAULT 'unknown' CHECK(policy IN ('unknown','response_only','notify')),
    policy_origin TEXT,
    policy_recorded_at TEXT,
    requested_at TEXT,
    request_basis TEXT,
    ack_at TEXT,
    ack_basis TEXT,
    ack_actor_label TEXT,
    ack_mailbox_seq INTEGER,
    ack_native_evidence TEXT,
    CHECK ((policy='unknown' AND policy_origin IS NULL AND policy_recorded_at IS NULL)
        OR (policy!='unknown' AND policy_origin IS NOT NULL AND policy_recorded_at IS NOT NULL)),
    CHECK ((requested_at IS NULL) = (request_basis IS NULL)),
    CHECK ((ack_at IS NULL) = (ack_basis IS NULL)),
    CHECK (ack_native_evidence IS NULL OR (ack_at IS NOT NULL AND json_valid(ack_native_evidence))),
    PRIMARY KEY(event_id,listener_id),
    FOREIGN KEY(event_id,listener_id) REFERENCES completion_event_listener(event_id,listener_id) ON DELETE CASCADE
);
-- Historical ACK remains genuine historical evidence; do not invent an actor.
INSERT INTO completion_continuation_notification(event_id,listener_id,ack_at,ack_basis,ack_mailbox_seq)
SELECT event_id,listener_id,acknowledged_at,
       CASE WHEN acknowledged_at IS NOT NULL THEN 'historical:' || acknowledgement_reason END,
       CASE WHEN acknowledged_at IS NOT NULL THEN mailbox_seq END
FROM completion_event_listener;
CREATE TRIGGER completion_continuation_notification_ack AFTER UPDATE OF acknowledged_at ON completion_event_listener
WHEN OLD.acknowledged_at IS NULL AND NEW.acknowledged_at IS NOT NULL
BEGIN
    INSERT INTO completion_continuation_notification(event_id,listener_id,ack_at,ack_basis,ack_actor_label,ack_mailbox_seq)
    VALUES(NEW.event_id,NEW.listener_id,NEW.acknowledged_at,NEW.acknowledgement_reason,
        (SELECT delivered_by_invocation_uuid FROM mailbox WHERE seq=NEW.mailbox_seq),NEW.mailbox_seq)
    ON CONFLICT(event_id,listener_id) DO UPDATE SET
        ack_at=excluded.ack_at,ack_basis=excluded.ack_basis,
        ack_actor_label=excluded.ack_actor_label,ack_mailbox_seq=excluded.ack_mailbox_seq
    WHERE completion_continuation_notification.ack_at IS NULL;
END;

-- Facts are first-effective facts, not a mutable administrative history.
CREATE TRIGGER completion_continuation_notification_immutable BEFORE UPDATE ON completion_continuation_notification
WHEN NEW.event_id IS NOT OLD.event_id OR NEW.listener_id IS NOT OLD.listener_id
 OR (OLD.policy!='unknown' AND (NEW.policy IS NOT OLD.policy OR NEW.policy_origin IS NOT OLD.policy_origin OR NEW.policy_recorded_at IS NOT OLD.policy_recorded_at))
 OR (OLD.requested_at IS NOT NULL AND (NEW.requested_at IS NOT OLD.requested_at OR NEW.request_basis IS NOT OLD.request_basis))
 OR (OLD.ack_at IS NOT NULL AND (NEW.ack_at IS NOT OLD.ack_at OR NEW.ack_basis IS NOT OLD.ack_basis OR NEW.ack_actor_label IS NOT OLD.ack_actor_label OR NEW.ack_mailbox_seq IS NOT OLD.ack_mailbox_seq))
 OR (OLD.ack_native_evidence IS NOT NULL AND NEW.ack_native_evidence IS NOT OLD.ack_native_evidence)
BEGIN SELECT RAISE(ABORT,'notification first-effective facts are immutable'); END;
