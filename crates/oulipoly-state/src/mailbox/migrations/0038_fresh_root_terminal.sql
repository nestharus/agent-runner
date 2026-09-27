-- Private v30 result. Only independently checked execution evidence is frozen.
-- Recipient and caller publication are separate, monotonic readback facts.
CREATE TABLE fresh_root_terminal (
 handoff_id TEXT PRIMARY KEY REFERENCES fresh_released_handoff(handoff_id),
 invocation_uuid TEXT NOT NULL UNIQUE,
 session_id TEXT NOT NULL UNIQUE,
 root_id TEXT NOT NULL UNIQUE,
 owner_generation TEXT NOT NULL,
 actor_identity TEXT NOT NULL,
 execution_json TEXT NOT NULL,
 recorded_at TEXT NOT NULL
);
CREATE TRIGGER fresh_root_terminal_no_update BEFORE UPDATE ON fresh_root_terminal
BEGIN SELECT RAISE(ABORT,'fresh root terminal immutable'); END;
CREATE TRIGGER fresh_root_terminal_no_delete BEFORE DELETE ON fresh_root_terminal
BEGIN SELECT RAISE(ABORT,'fresh root terminal retained'); END;
-- Unknown is recorded before a caller write. A failed or lost write can never
-- be interpreted as delivered, and this private API has no success assertion.
CREATE TABLE fresh_root_publication (
 handoff_id TEXT PRIMARY KEY REFERENCES fresh_root_terminal(handoff_id),
 artifact_sha256 TEXT NOT NULL,
 artifact_byte_len INTEGER NOT NULL CHECK(artifact_byte_len>=0),
 phase TEXT NOT NULL CHECK(phase='unknown'),
 started_at TEXT NOT NULL
);
CREATE TRIGGER fresh_root_publication_no_update BEFORE UPDATE ON fresh_root_publication
BEGIN SELECT RAISE(ABORT,'fresh root publication immutable'); END;
CREATE TRIGGER fresh_root_publication_no_delete BEFORE DELETE ON fresh_root_publication
BEGIN SELECT RAISE(ABORT,'fresh root publication retained'); END;
