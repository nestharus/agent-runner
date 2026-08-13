-- Immutable state-side identity for sidecar-owned completion obligations.
-- ## Declared roles
-- `mapper`, `validator`

CREATE TABLE invocation_completion_obligations (
    admission_id TEXT PRIMARY KEY CHECK (length(trim(admission_id)) > 0),
    invocation_uuid TEXT NOT NULL REFERENCES invocations(invocation_uuid)
        CHECK (length(trim(invocation_uuid)) > 0),
    event_id TEXT NOT NULL CHECK (length(trim(event_id)) > 0),
    owner_invocation_uuid TEXT NOT NULL REFERENCES invocations(invocation_uuid)
        CHECK (length(trim(owner_invocation_uuid)) > 0),
    owner_session_id TEXT NOT NULL CHECK (length(trim(owner_session_id)) > 0),
    expected_sidecar_generation TEXT NOT NULL
        CHECK (length(trim(expected_sidecar_generation)) > 0),
    admitted_at TEXT NOT NULL CHECK (length(trim(admitted_at)) > 0),
    UNIQUE (event_id, owner_invocation_uuid)
);

CREATE INDEX idx_invocation_completion_obligations_invocation
    ON invocation_completion_obligations (
        invocation_uuid,
        admitted_at,
        admission_id
    );

CREATE TRIGGER trg_invocation_completion_obligations_generation_insert
BEFORE INSERT ON invocation_completion_obligations
WHEN EXISTS (
    SELECT 1
    FROM invocation_completion_obligations
    WHERE event_id = NEW.event_id
      AND expected_sidecar_generation <> NEW.expected_sidecar_generation
)
BEGIN
    SELECT RAISE(ABORT, 'completion event sidecar generation conflict');
END;

CREATE TRIGGER trg_invocation_completion_obligations_generation_update
BEFORE UPDATE OF event_id, expected_sidecar_generation
ON invocation_completion_obligations
WHEN EXISTS (
    SELECT 1
    FROM invocation_completion_obligations
    WHERE event_id = NEW.event_id
      AND admission_id <> OLD.admission_id
      AND expected_sidecar_generation <> NEW.expected_sidecar_generation
)
BEGIN
    SELECT RAISE(ABORT, 'completion event sidecar generation conflict');
END;
