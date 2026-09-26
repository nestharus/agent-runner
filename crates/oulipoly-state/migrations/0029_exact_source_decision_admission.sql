-- Original-child Broker decisions are consumed only by the explicitly called
-- State registration API. Historical obligations remain without attribution.
-- Projection is deliberately unavailable: the user sidecar and State database
-- cannot share an atomic WAL commit.
CREATE TABLE invocation_completion_exact_source_decisions (
    admission_id TEXT NOT NULL PRIMARY KEY
        REFERENCES invocation_completion_obligations(admission_id),
    request_id TEXT NOT NULL UNIQUE,
    decision_id TEXT NOT NULL UNIQUE,
    registration_id TEXT NOT NULL UNIQUE,
    registration_sha256 TEXT NOT NULL CHECK(length(registration_sha256)=64),
    root_id TEXT NOT NULL,
    source_generation TEXT NOT NULL,
    owner_generation TEXT NOT NULL,
    supervisor_id TEXT NOT NULL,
    issuer_stamp_json TEXT NOT NULL,
    original_state_device INTEGER NOT NULL,
    original_state_inode INTEGER NOT NULL,
    broker_readback_json BLOB NOT NULL,
    projection_state TEXT NOT NULL DEFAULT 'unavailable'
        CHECK(projection_state='unavailable')
) STRICT;

CREATE TRIGGER invocation_completion_exact_source_decisions_no_update
BEFORE UPDATE ON invocation_completion_exact_source_decisions
BEGIN SELECT RAISE(ABORT, 'exact source decision attribution immutable'); END;

CREATE TRIGGER invocation_completion_exact_source_decisions_no_delete
BEFORE DELETE ON invocation_completion_exact_source_decisions
BEGIN SELECT RAISE(ABORT, 'exact source decision attribution immutable'); END;
