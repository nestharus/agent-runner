-- The original receiver's UID is fixed at F, before the path is issued.
CREATE TABLE fresh_original_grant_uid (
 grant_id TEXT PRIMARY KEY REFERENCES fresh_recipient_grant(grant_id),
 recipient_identity TEXT NOT NULL,
 recipient_uid INTEGER NOT NULL CHECK(recipient_uid>=0)
);
CREATE TRIGGER fresh_original_grant_uid_no_update BEFORE UPDATE ON fresh_original_grant_uid
BEGIN SELECT RAISE(ABORT,'original grant UID immutable'); END;
CREATE TRIGGER fresh_original_grant_uid_no_delete BEFORE DELETE ON fresh_original_grant_uid
BEGIN SELECT RAISE(ABORT,'original grant UID retained'); END;
CREATE TABLE fresh_original_receipt (
 grant_id TEXT PRIMARY KEY REFERENCES fresh_recipient_grant(grant_id),
 delivery_request_id TEXT NOT NULL UNIQUE,
 session_id TEXT NOT NULL, seq INTEGER NOT NULL,
 source_id TEXT NOT NULL, attempt_id TEXT NOT NULL,
 lane_id TEXT NOT NULL, source_generation TEXT NOT NULL,
 root_id TEXT NOT NULL, owner_generation TEXT NOT NULL,
 recipient_identity TEXT NOT NULL, recipient_uid INTEGER NOT NULL,
 payload_sha256 TEXT NOT NULL, payload_byte_len INTEGER NOT NULL,
 delivery_token_sha256 TEXT NOT NULL,
 receipt_sha256 TEXT NOT NULL, receipt_device INTEGER NOT NULL,
 receipt_inode INTEGER NOT NULL, certified_at TEXT NOT NULL,
 UNIQUE(session_id,seq)
);
CREATE TRIGGER fresh_original_receipt_no_update BEFORE UPDATE ON fresh_original_receipt
BEGIN SELECT RAISE(ABORT,'original receipt immutable'); END;
CREATE TRIGGER fresh_original_receipt_no_delete BEFORE DELETE ON fresh_original_receipt
BEGIN SELECT RAISE(ABORT,'original receipt retained'); END;
-- One-use manual ACK captures the exact certificate identity in the same
-- transaction as mailbox delivery and fresh_recipient_ack_evidence.
CREATE TABLE fresh_original_ack_receipt (
 grant_id TEXT PRIMARY KEY REFERENCES fresh_recipient_grant(grant_id),
 delivery_request_id TEXT NOT NULL UNIQUE,
 recipient_uid INTEGER NOT NULL,
 receipt_sha256 TEXT NOT NULL,
 receipt_device INTEGER NOT NULL,
 receipt_inode INTEGER NOT NULL,
 acknowledged_at TEXT NOT NULL
);
CREATE TRIGGER fresh_original_ack_receipt_no_update BEFORE UPDATE ON fresh_original_ack_receipt
BEGIN SELECT RAISE(ABORT,'original ACK receipt immutable'); END;
CREATE TRIGGER fresh_original_ack_receipt_no_delete BEFORE DELETE ON fresh_original_ack_receipt
BEGIN SELECT RAISE(ABORT,'original ACK receipt retained'); END;
