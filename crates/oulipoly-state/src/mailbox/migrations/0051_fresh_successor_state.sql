-- Separate from the immutable original attachment. State commits first; the
-- Broker sidecar is reconciled by an exact root retry after a partial write.
CREATE TABLE fresh_lane_successor_admission (
 generation TEXT PRIMARY KEY,
 offer_request_id TEXT NOT NULL UNIQUE,
 session_id TEXT NOT NULL,
 seq INTEGER NOT NULL,
 source_id TEXT NOT NULL,
 attempt_id TEXT NOT NULL,
 lane_id TEXT NOT NULL,
 source_generation TEXT NOT NULL,
 root_id TEXT NOT NULL,
 owner_generation TEXT NOT NULL,
 original_identity TEXT NOT NULL,
 successor_identity TEXT NOT NULL,
 payload_sha256 TEXT NOT NULL,
 payload_byte_len INTEGER NOT NULL CHECK(payload_byte_len>=0),
 admitted_at TEXT NOT NULL,
 UNIQUE(session_id,seq)
);
CREATE TRIGGER fresh_lane_successor_admission_no_update BEFORE UPDATE ON fresh_lane_successor_admission
BEGIN SELECT RAISE(ABORT,'fresh State successor admission immutable'); END;
CREATE TRIGGER fresh_lane_successor_admission_no_delete BEFORE DELETE ON fresh_lane_successor_admission
BEGIN SELECT RAISE(ABORT,'fresh State successor admission retained'); END;
