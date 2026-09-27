-- No production writer for accepted fresh source and recipient attachment
-- exists yet. This schema cannot make copied v29 debt eligible for delivery.
CREATE TABLE fresh_recipient_source (
 source_id TEXT PRIMARY KEY, attempt_id TEXT NOT NULL UNIQUE,
 state_admission_id TEXT NOT NULL, registration_digest TEXT NOT NULL,
 source_generation TEXT NOT NULL, lane_id TEXT NOT NULL,
 root_id TEXT NOT NULL, owner_generation TEXT NOT NULL
);
CREATE TRIGGER fresh_recipient_source_no_update BEFORE UPDATE ON fresh_recipient_source
BEGIN SELECT RAISE(ABORT,'fresh recipient source immutable'); END;
CREATE TRIGGER fresh_recipient_source_no_delete BEFORE DELETE ON fresh_recipient_source
BEGIN SELECT RAISE(ABORT,'fresh recipient source retained'); END;
CREATE TABLE fresh_recipient_binding (
 session_id TEXT PRIMARY KEY REFERENCES fresh_lane_session(session_id),
 recipient_identity TEXT NOT NULL, root_id TEXT NOT NULL,
 owner_generation TEXT NOT NULL, source_generation TEXT NOT NULL
);
CREATE TRIGGER fresh_recipient_binding_no_update BEFORE UPDATE ON fresh_recipient_binding
BEGIN SELECT RAISE(ABORT,'fresh recipient binding immutable'); END;
CREATE TRIGGER fresh_recipient_binding_no_delete BEFORE DELETE ON fresh_recipient_binding
BEGIN SELECT RAISE(ABORT,'fresh recipient binding retained'); END;
CREATE TABLE fresh_recipient_row_source (
 session_id TEXT NOT NULL, seq INTEGER NOT NULL,
 source_id TEXT NOT NULL REFERENCES fresh_recipient_source(source_id),
 attempt_id TEXT NOT NULL, payload_sha256 TEXT NOT NULL,
 payload_byte_len INTEGER NOT NULL CHECK(payload_byte_len>=0),
 PRIMARY KEY(session_id,seq)
);
CREATE TRIGGER fresh_recipient_row_source_no_update BEFORE UPDATE ON fresh_recipient_row_source
BEGIN SELECT RAISE(ABORT,'fresh recipient row source immutable'); END;
CREATE TRIGGER fresh_recipient_row_source_no_delete BEFORE DELETE ON fresh_recipient_row_source
BEGIN SELECT RAISE(ABORT,'fresh recipient row source retained'); END;
CREATE TABLE fresh_recipient_grant (
 grant_id TEXT PRIMARY KEY, delivery_request_id TEXT NOT NULL UNIQUE,
 delivery_token TEXT NOT NULL UNIQUE,
 session_id TEXT NOT NULL, seq INTEGER NOT NULL,
 source_id TEXT NOT NULL, attempt_id TEXT NOT NULL,
 lane_id TEXT NOT NULL, source_generation TEXT NOT NULL,
 root_id TEXT NOT NULL, owner_generation TEXT NOT NULL,
 recipient_identity TEXT NOT NULL, payload_sha256 TEXT NOT NULL,
 payload_byte_len INTEGER NOT NULL, phase TEXT NOT NULL
 CHECK(phase IN ('unknown','submitted','acked')),
 created_at TEXT NOT NULL, submitted_at TEXT, acknowledged_at TEXT,
 UNIQUE(session_id,seq)
);
CREATE TRIGGER fresh_recipient_grant_no_delete BEFORE DELETE ON fresh_recipient_grant
BEGIN SELECT RAISE(ABORT,'fresh recipient grant retained'); END;
CREATE TRIGGER fresh_recipient_grant_update_guard BEFORE UPDATE ON fresh_recipient_grant
WHEN NEW.grant_id!=OLD.grant_id OR NEW.delivery_request_id!=OLD.delivery_request_id
 OR NEW.delivery_token!=OLD.delivery_token
 OR NEW.session_id!=OLD.session_id OR NEW.seq!=OLD.seq
 OR NEW.source_id!=OLD.source_id OR NEW.attempt_id!=OLD.attempt_id
 OR NEW.lane_id!=OLD.lane_id OR NEW.source_generation!=OLD.source_generation
 OR NEW.root_id!=OLD.root_id OR NEW.owner_generation!=OLD.owner_generation
 OR NEW.recipient_identity!=OLD.recipient_identity
 OR NEW.payload_sha256!=OLD.payload_sha256 OR NEW.payload_byte_len!=OLD.payload_byte_len
 OR NEW.created_at!=OLD.created_at
 OR NOT ((OLD.phase='unknown' AND NEW.phase IN ('submitted','acked'))
      OR (OLD.phase='submitted' AND NEW.phase='acked'))
BEGIN SELECT RAISE(ABORT,'fresh recipient grant transition invalid'); END;
CREATE TABLE fresh_recipient_ack_delegation (
 delegation_id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
 owner_identity TEXT NOT NULL, delegate_identity TEXT NOT NULL,
 created_at TEXT NOT NULL, consumed_at TEXT
);
CREATE TABLE fresh_recipient_ack_delegation_item (
 delegation_id TEXT NOT NULL REFERENCES fresh_recipient_ack_delegation(delegation_id),
 grant_id TEXT NOT NULL REFERENCES fresh_recipient_grant(grant_id),
 PRIMARY KEY(delegation_id,grant_id), UNIQUE(grant_id)
);
CREATE TRIGGER fresh_recipient_ack_delegation_item_no_update
BEFORE UPDATE ON fresh_recipient_ack_delegation_item
BEGIN SELECT RAISE(ABORT,'delegated batch immutable'); END;
CREATE TRIGGER fresh_recipient_ack_delegation_item_no_delete
BEFORE DELETE ON fresh_recipient_ack_delegation_item
BEGIN SELECT RAISE(ABORT,'delegated batch retained'); END;
