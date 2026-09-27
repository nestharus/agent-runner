-- A normal root may reach this broker-owned, no-fork preparation boundary.
-- No row in this table is a native K, provider result, or physical drain.
CREATE TABLE fresh_normal_work_preparation (
 handoff_id TEXT PRIMARY KEY REFERENCES fresh_released_handoff(handoff_id),
 invocation_uuid TEXT NOT NULL UNIQUE,
 session_id TEXT NOT NULL UNIQUE,
 actor_identity TEXT NOT NULL,
 intent_json TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state='held'),
 prepared_at TEXT NOT NULL
);
CREATE TRIGGER fresh_normal_work_preparation_no_update BEFORE UPDATE ON fresh_normal_work_preparation
BEGIN SELECT RAISE(ABORT,'normal work preparation immutable'); END;
CREATE TRIGGER fresh_normal_work_preparation_no_delete BEFORE DELETE ON fresh_normal_work_preparation
BEGIN SELECT RAISE(ABORT,'normal work preparation immutable'); END;
