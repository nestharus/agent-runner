CREATE TABLE fresh_lane_identity (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1),
 protocol TEXT NOT NULL CHECK(protocol='fresh-v30-lane-v1'),
 lane_id TEXT NOT NULL,
 domain_id TEXT NOT NULL,
 source_generation TEXT NOT NULL,
 state_device INTEGER NOT NULL,
 state_inode INTEGER NOT NULL
);
CREATE TRIGGER fresh_lane_identity_no_update BEFORE UPDATE ON fresh_lane_identity
BEGIN SELECT RAISE(ABORT,'fresh lane identity immutable'); END;
CREATE TRIGGER fresh_lane_identity_no_delete BEFORE DELETE ON fresh_lane_identity
BEGIN SELECT RAISE(ABORT,'fresh lane identity immutable'); END;
CREATE TABLE fresh_lane_session (
 session_id TEXT PRIMARY KEY,
 allocation_id TEXT NOT NULL UNIQUE,
 lane_id TEXT NOT NULL,
 source_generation TEXT NOT NULL,
 allocated_at TEXT NOT NULL
);
CREATE TRIGGER fresh_lane_session_no_update BEFORE UPDATE ON fresh_lane_session
BEGIN SELECT RAISE(ABORT,'fresh lane session immutable'); END;
CREATE TRIGGER fresh_lane_session_no_delete BEFORE DELETE ON fresh_lane_session
BEGIN SELECT RAISE(ABORT,'fresh lane session immutable'); END;
