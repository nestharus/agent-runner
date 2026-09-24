CREATE TABLE fresh_lane_accepted_source (
 source_id TEXT PRIMARY KEY, attempt_id TEXT NOT NULL UNIQUE,
 state_admission_id TEXT NOT NULL, registration_digest TEXT NOT NULL,
 source_generation TEXT NOT NULL, lane_id TEXT NOT NULL,
 root_id TEXT NOT NULL, owner_generation TEXT NOT NULL,
 accepted_at TEXT NOT NULL
);
CREATE TRIGGER fresh_lane_accepted_source_no_update BEFORE UPDATE ON fresh_lane_accepted_source
BEGIN SELECT RAISE(ABORT,'accepted fresh source immutable'); END;
CREATE TRIGGER fresh_lane_accepted_source_no_delete BEFORE DELETE ON fresh_lane_accepted_source
BEGIN SELECT RAISE(ABORT,'accepted fresh source retained'); END;
CREATE TABLE fresh_lane_recipient_attachment (
 session_id TEXT PRIMARY KEY, lane_id TEXT NOT NULL,
 source_generation TEXT NOT NULL, root_id TEXT NOT NULL,
 owner_generation TEXT NOT NULL, recipient_identity TEXT NOT NULL,
 attached_at TEXT NOT NULL
);
CREATE TRIGGER fresh_lane_recipient_attachment_no_update
BEFORE UPDATE ON fresh_lane_recipient_attachment
BEGIN SELECT RAISE(ABORT,'fresh recipient attachment immutable'); END;
CREATE TRIGGER fresh_lane_recipient_attachment_no_delete
BEFORE DELETE ON fresh_lane_recipient_attachment
BEGIN SELECT RAISE(ABORT,'fresh recipient attachment retained'); END;
