-- Offers are made by the pinned successor socket peer. Only the original
-- released root can turn one into an admission. Neither row grants F by itself.
CREATE TABLE fresh_successor_offer (
 offer_request_id TEXT PRIMARY KEY,
 generation TEXT NOT NULL UNIQUE,
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
 offered_at TEXT NOT NULL,
 UNIQUE(session_id,seq,successor_identity)
);
CREATE TRIGGER fresh_successor_offer_no_update BEFORE UPDATE ON fresh_successor_offer
BEGIN SELECT RAISE(ABORT,'fresh successor offer immutable'); END;
CREATE TRIGGER fresh_successor_offer_no_delete BEFORE DELETE ON fresh_successor_offer
BEGIN SELECT RAISE(ABORT,'fresh successor offer retained'); END;
CREATE TABLE fresh_successor_admission (
 generation TEXT PRIMARY KEY REFERENCES fresh_successor_offer(generation),
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
CREATE TRIGGER fresh_successor_admission_no_update BEFORE UPDATE ON fresh_successor_admission
BEGIN SELECT RAISE(ABORT,'fresh successor admission immutable'); END;
CREATE TRIGGER fresh_successor_admission_no_delete BEFORE DELETE ON fresh_successor_admission
BEGIN SELECT RAISE(ABORT,'fresh successor admission retained'); END;
