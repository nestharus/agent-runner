-- Broker-owned pre-D request intent for one pinned Runner child process.
-- It does not authorize source registration, W, acceptance, or delivery.
CREATE TABLE fresh_lane_child_request (
 request_id TEXT PRIMARY KEY,
 invocation_uuid TEXT NOT NULL UNIQUE,
 actor_identity TEXT NOT NULL UNIQUE,
 lane_id TEXT NOT NULL,
 source_generation TEXT NOT NULL,
 reserved_at TEXT NOT NULL
);
CREATE TRIGGER fresh_lane_child_request_no_update BEFORE UPDATE ON fresh_lane_child_request
BEGIN SELECT RAISE(ABORT,'fresh child request immutable'); END;
CREATE TRIGGER fresh_lane_child_request_no_delete BEFORE DELETE ON fresh_lane_child_request
BEGIN SELECT RAISE(ABORT,'fresh child request immutable'); END;
