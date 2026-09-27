CREATE TABLE fresh_lane_state_identity (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1),
 protocol TEXT NOT NULL CHECK(protocol='fresh-v30-lane-v1'),
 lane_id TEXT NOT NULL,
 domain_id TEXT NOT NULL,
 source_generation TEXT NOT NULL
);
CREATE TRIGGER fresh_lane_state_identity_no_update BEFORE UPDATE ON fresh_lane_state_identity
BEGIN SELECT RAISE(ABORT,'fresh State lane identity immutable'); END;
CREATE TRIGGER fresh_lane_state_identity_no_delete BEFORE DELETE ON fresh_lane_state_identity
BEGIN SELECT RAISE(ABORT,'fresh State lane identity immutable'); END;
CREATE TABLE fresh_lane_session_admission (
 request_id TEXT PRIMARY KEY,
 session_id TEXT NOT NULL UNIQUE,
 allocation_id TEXT NOT NULL UNIQUE,
 lane_id TEXT NOT NULL,
 source_generation TEXT NOT NULL,
 admitted_at TEXT NOT NULL
);
CREATE TRIGGER fresh_lane_session_admission_no_update BEFORE UPDATE ON fresh_lane_session_admission
BEGIN SELECT RAISE(ABORT,'fresh State session admission immutable'); END;
CREATE TRIGGER fresh_lane_session_admission_no_delete BEFORE DELETE ON fresh_lane_session_admission
BEGIN SELECT RAISE(ABORT,'fresh State session admission immutable'); END;
