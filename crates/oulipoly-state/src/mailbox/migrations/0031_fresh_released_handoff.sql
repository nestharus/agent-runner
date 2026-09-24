-- A broker-authenticated copy of the old gate's immutable handoff receipt.
-- This records a root invocation. A Bash descendant has no handle here.
CREATE TABLE fresh_released_handoff (
 handoff_id TEXT PRIMARY KEY,
 d_key TEXT NOT NULL UNIQUE,
 invocation_uuid TEXT NOT NULL UNIQUE,
 root_intent_kind TEXT NOT NULL CHECK(root_intent_kind IN ('cli_help','cli_diagnostics','private_probe')),
 root_id TEXT NOT NULL UNIQUE,
 actor_identity TEXT NOT NULL UNIQUE,
 receipt_json TEXT NOT NULL,
 bound_at TEXT NOT NULL
);
CREATE TRIGGER fresh_released_handoff_no_update BEFORE UPDATE ON fresh_released_handoff
BEGIN SELECT RAISE(ABORT,'released handoff immutable'); END;
CREATE TRIGGER fresh_released_handoff_no_delete BEFORE DELETE ON fresh_released_handoff
BEGIN SELECT RAISE(ABORT,'released handoff immutable'); END;
CREATE TABLE fresh_released_owner (
 handoff_id TEXT PRIMARY KEY,
 invocation_uuid TEXT NOT NULL UNIQUE,
 session_id TEXT NOT NULL UNIQUE,
 actor_identity TEXT NOT NULL UNIQUE,
 bound_at TEXT NOT NULL,
 FOREIGN KEY(handoff_id) REFERENCES fresh_released_handoff(handoff_id)
);
CREATE TRIGGER fresh_released_owner_no_update BEFORE UPDATE ON fresh_released_owner
BEGIN SELECT RAISE(ABORT,'released owner immutable'); END;
CREATE TRIGGER fresh_released_owner_no_delete BEFORE DELETE ON fresh_released_owner
BEGIN SELECT RAISE(ABORT,'released owner immutable'); END;
