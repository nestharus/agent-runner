-- A selected async W creates one immutable delivery debt. This record grants
-- neither successor admission nor F/ACK authority.
CREATE TABLE fresh_bash_wake_obligation (
 request_id TEXT PRIMARY KEY,
 session_id TEXT NOT NULL,
 seq INTEGER NOT NULL,
 source_id TEXT NOT NULL,
 attempt_id TEXT NOT NULL,
 lane_id TEXT NOT NULL,
 source_generation TEXT NOT NULL,
 root_id TEXT NOT NULL,
 owner_generation TEXT NOT NULL,
 original_identity TEXT NOT NULL,
 payload_sha256 TEXT NOT NULL,
 payload_byte_len INTEGER NOT NULL CHECK(payload_byte_len>=0),
 recorded_at TEXT NOT NULL,
 UNIQUE(session_id,seq),
 UNIQUE(source_id,attempt_id)
);
CREATE TRIGGER fresh_bash_wake_obligation_no_update
BEFORE UPDATE ON fresh_bash_wake_obligation
BEGIN SELECT RAISE(ABORT,'fresh Bash wake obligation immutable'); END;
CREATE TRIGGER fresh_bash_wake_obligation_no_delete
BEFORE DELETE ON fresh_bash_wake_obligation
BEGIN SELECT RAISE(ABORT,'fresh Bash wake obligation retained'); END;
