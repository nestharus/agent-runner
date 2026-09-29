-- The admitted successor has independent F and receipt/ACK evidence. The
-- original recipient grant and immutable attachment are never repurposed.
CREATE TABLE fresh_successor_grant (
 grant_id TEXT PRIMARY KEY,
 delivery_request_id TEXT NOT NULL UNIQUE,
 delivery_token TEXT NOT NULL UNIQUE,
 generation TEXT NOT NULL UNIQUE REFERENCES fresh_successor_admission(generation),
 offer_request_id TEXT NOT NULL,
 session_id TEXT NOT NULL,
 seq INTEGER NOT NULL,
 source_id TEXT NOT NULL,
 attempt_id TEXT NOT NULL,
 lane_id TEXT NOT NULL,
 source_generation TEXT NOT NULL,
 root_id TEXT NOT NULL,
 owner_generation TEXT NOT NULL,
 successor_identity TEXT NOT NULL,
 recipient_uid INTEGER NOT NULL CHECK(recipient_uid>=0),
 payload_sha256 TEXT NOT NULL,
 payload_byte_len INTEGER NOT NULL CHECK(payload_byte_len>=0),
 phase TEXT NOT NULL CHECK(phase IN ('unknown','submitted','acked')),
 created_at TEXT NOT NULL,
 submitted_at TEXT,
 acknowledged_at TEXT,
 UNIQUE(session_id,seq)
);
CREATE TRIGGER fresh_successor_grant_no_delete BEFORE DELETE ON fresh_successor_grant
BEGIN SELECT RAISE(ABORT,'successor F grant retained'); END;
CREATE TRIGGER fresh_successor_grant_update_guard BEFORE UPDATE ON fresh_successor_grant
WHEN NEW.grant_id!=OLD.grant_id OR NEW.delivery_request_id!=OLD.delivery_request_id
 OR NEW.delivery_token!=OLD.delivery_token OR NEW.generation!=OLD.generation
 OR NEW.offer_request_id!=OLD.offer_request_id OR NEW.session_id!=OLD.session_id
 OR NEW.seq!=OLD.seq OR NEW.source_id!=OLD.source_id OR NEW.attempt_id!=OLD.attempt_id
 OR NEW.lane_id!=OLD.lane_id OR NEW.source_generation!=OLD.source_generation
 OR NEW.root_id!=OLD.root_id OR NEW.owner_generation!=OLD.owner_generation
 OR NEW.successor_identity!=OLD.successor_identity
 OR NEW.recipient_uid!=OLD.recipient_uid
 OR NEW.payload_sha256!=OLD.payload_sha256 OR NEW.payload_byte_len!=OLD.payload_byte_len
 OR NEW.created_at!=OLD.created_at
 OR NOT ((OLD.phase='unknown' AND NEW.phase IN ('submitted','acked'))
      OR (OLD.phase='submitted' AND NEW.phase='acked'))
BEGIN SELECT RAISE(ABORT,'successor F transition invalid'); END;
CREATE TABLE fresh_successor_receipt (
 grant_id TEXT PRIMARY KEY REFERENCES fresh_successor_grant(grant_id),
 delivery_request_id TEXT NOT NULL UNIQUE,
 generation TEXT NOT NULL,
 session_id TEXT NOT NULL,
 seq INTEGER NOT NULL,
 source_id TEXT NOT NULL,
 attempt_id TEXT NOT NULL,
 successor_identity TEXT NOT NULL,
 payload_sha256 TEXT NOT NULL,
 payload_byte_len INTEGER NOT NULL,
 delivery_token_sha256 TEXT NOT NULL,
 receipt_sha256 TEXT NOT NULL,
 receipt_device INTEGER NOT NULL,
 receipt_inode INTEGER NOT NULL,
 certified_at TEXT NOT NULL,
 UNIQUE(session_id,seq)
);
CREATE TRIGGER fresh_successor_receipt_no_update BEFORE UPDATE ON fresh_successor_receipt
BEGIN SELECT RAISE(ABORT,'successor receipt immutable'); END;
CREATE TRIGGER fresh_successor_receipt_no_delete BEFORE DELETE ON fresh_successor_receipt
BEGIN SELECT RAISE(ABORT,'successor receipt retained'); END;
CREATE TABLE fresh_successor_ack_evidence (
 grant_id TEXT PRIMARY KEY REFERENCES fresh_successor_grant(grant_id),
 delivery_request_id TEXT NOT NULL UNIQUE,
 generation TEXT NOT NULL,
 session_id TEXT NOT NULL,
 seq INTEGER NOT NULL,
 source_id TEXT NOT NULL,
 attempt_id TEXT NOT NULL,
 successor_identity TEXT NOT NULL,
 payload_sha256 TEXT NOT NULL,
 payload_byte_len INTEGER NOT NULL,
 delivery_token_sha256 TEXT NOT NULL,
 receipt_sha256 TEXT NOT NULL,
 acknowledged_at TEXT NOT NULL,
 UNIQUE(session_id,seq)
);
CREATE TRIGGER fresh_successor_ack_no_update BEFORE UPDATE ON fresh_successor_ack_evidence
BEGIN SELECT RAISE(ABORT,'successor ACK immutable'); END;
CREATE TRIGGER fresh_successor_ack_no_delete BEFORE DELETE ON fresh_successor_ack_evidence
BEGIN SELECT RAISE(ABORT,'successor ACK retained'); END;
