-- A committed submission attempt is spent before the original PTY master is
-- written. A lost reply, restart, or partial write cannot authorize replay.
CREATE TABLE fresh_native_f_submission (
 preparation_request_id TEXT PRIMARY KEY REFERENCES fresh_native_f_preparation(preparation_request_id),
 grant_id TEXT NOT NULL UNIQUE REFERENCES fresh_recipient_grant(grant_id),
 recipient_identity TEXT NOT NULL,
 envelope_sha256 TEXT NOT NULL,
 input_sha256 TEXT NOT NULL,
 input_byte_len INTEGER NOT NULL CHECK(input_byte_len > 0),
 runtime_generation_id TEXT NOT NULL,
 record_json TEXT NOT NULL,
 entered_at TEXT NOT NULL
);
CREATE TRIGGER fresh_native_f_submission_no_update
BEFORE UPDATE ON fresh_native_f_submission
BEGIN SELECT RAISE(ABORT,'fresh native F submission immutable'); END;
CREATE TRIGGER fresh_native_f_submission_no_delete
BEFORE DELETE ON fresh_native_f_submission
BEGIN SELECT RAISE(ABORT,'fresh native F submission retained'); END;
