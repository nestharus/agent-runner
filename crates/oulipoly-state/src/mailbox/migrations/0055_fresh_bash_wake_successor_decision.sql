-- A selected W can have only one proposed successor offer. This is a
-- scheduling decision, not an offer, admission, F grant or ACK.
CREATE TABLE fresh_bash_wake_successor_decision (
 wake_request_id TEXT PRIMARY KEY REFERENCES fresh_bash_wake_obligation(request_id),
 offer_request_id TEXT NOT NULL UNIQUE,
 obligation_json TEXT NOT NULL,
 decided_at TEXT NOT NULL
);
CREATE TRIGGER fresh_bash_wake_successor_decision_no_update
BEFORE UPDATE ON fresh_bash_wake_successor_decision
BEGIN SELECT RAISE(ABORT,'fresh Bash wake successor decision immutable'); END;
CREATE TRIGGER fresh_bash_wake_successor_decision_no_delete
BEFORE DELETE ON fresh_bash_wake_successor_decision
BEGIN SELECT RAISE(ABORT,'fresh Bash wake successor decision retained'); END;
