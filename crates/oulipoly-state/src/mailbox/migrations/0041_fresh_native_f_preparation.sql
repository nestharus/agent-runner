-- Private, authority-neutral pre-send record. It never asserts PTY submission,
-- provider receipt, or ACK. One F grant and one nonce can name only one record.
CREATE TABLE fresh_native_f_preparation (
 preparation_request_id TEXT PRIMARY KEY,
 grant_id TEXT NOT NULL UNIQUE REFERENCES fresh_recipient_grant(grant_id),
 delivery_request_id TEXT NOT NULL UNIQUE,
 delivery_token_sha256 TEXT NOT NULL,
 recipient_identity TEXT NOT NULL,
 root_id TEXT NOT NULL,
 owner_generation TEXT NOT NULL,
 session_id TEXT NOT NULL,
 seq INTEGER NOT NULL,
 source_id TEXT NOT NULL,
 attempt_id TEXT NOT NULL,
 lane_id TEXT NOT NULL,
 source_generation TEXT NOT NULL,
 payload_sha256 TEXT NOT NULL,
 payload_byte_len INTEGER NOT NULL CHECK(payload_byte_len>=0),
 runtime_generation_id TEXT NOT NULL,
 runtime_spawn_invocation_uuid TEXT NOT NULL,
 pty_control_path TEXT NOT NULL,
 pty_control_device INTEGER NOT NULL,
 pty_control_inode INTEGER NOT NULL,
 provider_account TEXT NOT NULL,
 provider_instance_id TEXT NOT NULL,
 settings_id TEXT NOT NULL,
 provider_session_id TEXT NOT NULL,
 envelope_nonce TEXT NOT NULL UNIQUE,
 envelope_text TEXT NOT NULL,
 envelope_sha256 TEXT NOT NULL,
 tail_resume_token TEXT NOT NULL,
 record_json TEXT NOT NULL,
 prepared_at TEXT NOT NULL,
 UNIQUE(session_id,seq)
);
CREATE TRIGGER fresh_native_f_preparation_no_update
BEFORE UPDATE ON fresh_native_f_preparation
BEGIN SELECT RAISE(ABORT,'fresh native F preparation immutable'); END;
CREATE TRIGGER fresh_native_f_preparation_no_delete
BEFORE DELETE ON fresh_native_f_preparation
BEGIN SELECT RAISE(ABORT,'fresh native F preparation retained'); END;
