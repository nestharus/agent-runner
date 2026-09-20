CREATE TABLE IF NOT EXISTS mailbox_completed_turn_pins (
 invocation_uuid TEXT PRIMARY KEY,
 attempt_id TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS mailbox_completed_turn_pins_attempt ON mailbox_completed_turn_pins(attempt_id);
CREATE VIEW IF NOT EXISTS mailbox_retained_delivery_finalizers AS
 SELECT attempt_id FROM mailbox_delivery_finalizers
 UNION ALL SELECT attempt_id FROM mailbox_completed_turn_pins;

CREATE TABLE mailbox_completed_turn_tails (
    invocation_uuid TEXT PRIMARY KEY,
    settlement_id TEXT NOT NULL UNIQUE,
    session_id TEXT NOT NULL,
    claim_token TEXT
);
