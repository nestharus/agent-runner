-- The selected K's headless second turn has its own one-use reservation and
-- settlement. The resident PTY preparation/transport/receipt tables do not
-- authorize this path.
CREATE TABLE fresh_headless_native_f_attempt (
 grant_id TEXT PRIMARY KEY REFERENCES fresh_recipient_grant(grant_id),
 delivery_request_id TEXT NOT NULL UNIQUE,
 recipient_identity TEXT NOT NULL,
 native_session_id TEXT NOT NULL,
 first_turn_id TEXT NOT NULL,
 nonce TEXT NOT NULL UNIQUE,
 envelope_sha256 TEXT NOT NULL UNIQUE,
 record_json TEXT NOT NULL,
 reserved_at TEXT NOT NULL
);
CREATE TRIGGER fresh_headless_native_f_attempt_no_update BEFORE UPDATE ON fresh_headless_native_f_attempt
BEGIN SELECT RAISE(ABORT,'fresh headless native F attempt immutable'); END;
CREATE TRIGGER fresh_headless_native_f_attempt_no_delete BEFORE DELETE ON fresh_headless_native_f_attempt
BEGIN SELECT RAISE(ABORT,'fresh headless native F attempt retained'); END;

CREATE TABLE fresh_headless_native_f_ack (
 grant_id TEXT PRIMARY KEY REFERENCES fresh_headless_native_f_attempt(grant_id),
 delivery_request_id TEXT NOT NULL UNIQUE,
 recipient_identity TEXT NOT NULL,
 native_session_id TEXT NOT NULL,
 turn_id TEXT NOT NULL,
 user_item_id TEXT NOT NULL,
 assistant_response_sha256 TEXT NOT NULL,
 receipt_sha256 TEXT NOT NULL,
 basis TEXT NOT NULL CHECK(basis='native_codex_f_assistant_ack'),
 record_json TEXT NOT NULL,
 acknowledged_at TEXT NOT NULL,
 UNIQUE(native_session_id,turn_id)
);
CREATE TRIGGER fresh_headless_native_f_ack_no_update BEFORE UPDATE ON fresh_headless_native_f_ack
BEGIN SELECT RAISE(ABORT,'fresh headless native F ACK immutable'); END;
CREATE TRIGGER fresh_headless_native_f_ack_no_delete BEFORE DELETE ON fresh_headless_native_f_ack
BEGIN SELECT RAISE(ABORT,'fresh headless native F ACK retained'); END;
