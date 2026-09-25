-- The physical write, native turn, and automatic settlement are three distinct facts.
CREATE TABLE fresh_native_f_transport (
 preparation_request_id TEXT PRIMARY KEY REFERENCES fresh_native_f_submission(preparation_request_id),
 grant_id TEXT NOT NULL UNIQUE, recipient_identity TEXT NOT NULL,
 input_sha256 TEXT NOT NULL, input_byte_len INTEGER NOT NULL,
 record_json TEXT NOT NULL, accepted_at TEXT NOT NULL
);
CREATE TRIGGER fresh_native_f_transport_no_update BEFORE UPDATE ON fresh_native_f_transport
BEGIN SELECT RAISE(ABORT,'fresh native F transport immutable'); END;
CREATE TRIGGER fresh_native_f_transport_no_delete BEFORE DELETE ON fresh_native_f_transport
BEGIN SELECT RAISE(ABORT,'fresh native F transport retained'); END;
CREATE TABLE fresh_native_f_receipt (
 preparation_request_id TEXT PRIMARY KEY REFERENCES fresh_native_f_submission(preparation_request_id),
 grant_id TEXT NOT NULL UNIQUE, recipient_identity TEXT NOT NULL,
 provider_session_id TEXT NOT NULL, turn_id TEXT NOT NULL,
 envelope_sha256 TEXT NOT NULL, payload_sha256 TEXT NOT NULL,
 record_json TEXT NOT NULL, observed_at TEXT NOT NULL,
 UNIQUE(provider_session_id,turn_id)
);
CREATE TRIGGER fresh_native_f_receipt_no_update BEFORE UPDATE ON fresh_native_f_receipt
BEGIN SELECT RAISE(ABORT,'fresh native F receipt immutable'); END;
CREATE TRIGGER fresh_native_f_receipt_no_delete BEFORE DELETE ON fresh_native_f_receipt
BEGIN SELECT RAISE(ABORT,'fresh native F receipt retained'); END;
CREATE TABLE fresh_native_f_auto_ack (
 grant_id TEXT PRIMARY KEY REFERENCES fresh_recipient_grant(grant_id),
 preparation_request_id TEXT NOT NULL UNIQUE REFERENCES fresh_native_f_receipt(preparation_request_id),
 delivery_request_id TEXT NOT NULL UNIQUE, delivery_token_sha256 TEXT NOT NULL,
 session_id TEXT NOT NULL, seq INTEGER NOT NULL,
 source_id TEXT NOT NULL, attempt_id TEXT NOT NULL,
 recipient_identity TEXT NOT NULL, payload_sha256 TEXT NOT NULL,
 payload_byte_len INTEGER NOT NULL, turn_id TEXT NOT NULL,
 basis TEXT NOT NULL CHECK(basis='native_f_receipt'), acknowledged_at TEXT NOT NULL,
 UNIQUE(session_id,seq)
);
CREATE TRIGGER fresh_native_f_auto_ack_no_update BEFORE UPDATE ON fresh_native_f_auto_ack
BEGIN SELECT RAISE(ABORT,'fresh native F auto ACK immutable'); END;
CREATE TRIGGER fresh_native_f_auto_ack_no_delete BEFORE DELETE ON fresh_native_f_auto_ack
BEGIN SELECT RAISE(ABORT,'fresh native F auto ACK retained'); END;
