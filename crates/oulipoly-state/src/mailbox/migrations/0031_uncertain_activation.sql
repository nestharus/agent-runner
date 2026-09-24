-- The original activation custodian owns this disposition. A drained attempt
-- may release its wake claim only after its still-pending selected rows have
-- been fenced in the same transaction. Keep exact row/attempt history for
-- readback and a later evidence-based resolution.
CREATE TABLE IF NOT EXISTS completion_uncertain_input (
    mailbox_seq INTEGER PRIMARY KEY,
    attempt_id TEXT NOT NULL REFERENCES completion_continuation_attempt(attempt_id),
    recorded_at TEXT NOT NULL,
    disposition TEXT NOT NULL CHECK(disposition = 'effect_uncertain')
);
DROP INDEX IF EXISTS completion_uncertain_input_attempt;
CREATE INDEX completion_uncertain_input_attempt
    ON completion_uncertain_input(attempt_id, mailbox_seq);

-- A generic failure recorder cannot make a fenced row deliverable again.
-- Exact receipt settlement and explicit manual ACK can still set delivered_at;
-- they already have their own authority checks.
DROP TRIGGER IF EXISTS completion_uncertain_input_preserve;
CREATE TRIGGER completion_uncertain_input_preserve
BEFORE UPDATE OF delivery_error, delivered_at ON mailbox
WHEN OLD.delivery_error='completion_effect_uncertain'
 AND NEW.delivered_at IS NULL
 AND NEW.delivery_error IS NOT 'completion_effect_uncertain'
BEGIN SELECT RAISE(ABORT, 'uncertain activation requires exact settlement'); END;

DROP INDEX idx_mailbox_deliverable_session_live;
DROP INDEX idx_mailbox_deliverable_target_live;
DROP INDEX idx_mailbox_deliverable_global;
CREATE INDEX idx_mailbox_deliverable_session_live
    ON mailbox(session_id, seq)
    WHERE delivered_at IS NULL
      AND (delivery_error IS NULL OR delivery_error NOT IN (
          'wake_sweep_abandoned', 'mailbox_payload_verification_failed',
          'mailbox_ingress_expired', 'completion_effect_uncertain'));
CREATE INDEX idx_mailbox_deliverable_target_live
    ON mailbox(target_kind, target_id, seq)
    WHERE delivered_at IS NULL
      AND (delivery_error IS NULL OR delivery_error NOT IN (
          'wake_sweep_abandoned', 'mailbox_payload_verification_failed',
          'mailbox_ingress_expired', 'completion_effect_uncertain'));
CREATE INDEX idx_mailbox_deliverable_global
    ON mailbox(seq, session_id)
    WHERE delivered_at IS NULL
      AND (delivery_error IS NULL OR delivery_error NOT IN (
          'wake_sweep_abandoned', 'mailbox_payload_verification_failed',
          'mailbox_ingress_expired', 'completion_effect_uncertain'));
