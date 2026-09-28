-- The original pinned caller records completion only after both Q-bound
-- streams have been written and flushed. The prior unknown intent is retained.
CREATE TABLE fresh_root_caller_settlement (
 handoff_id TEXT PRIMARY KEY REFERENCES fresh_root_publication(handoff_id),
 artifact_sha256 TEXT NOT NULL,
 artifact_byte_len INTEGER NOT NULL CHECK(artifact_byte_len>=0),
 settled_at TEXT NOT NULL
);
CREATE TRIGGER fresh_root_caller_settlement_no_update BEFORE UPDATE ON fresh_root_caller_settlement
BEGIN SELECT RAISE(ABORT,'fresh root caller settlement immutable'); END;
CREATE TRIGGER fresh_root_caller_settlement_no_delete BEFORE DELETE ON fresh_root_caller_settlement
BEGIN SELECT RAISE(ABORT,'fresh root caller settlement retained'); END;
