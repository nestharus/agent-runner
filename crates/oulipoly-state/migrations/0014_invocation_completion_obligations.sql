-- Immutable state-side identity for sidecar-owned completion obligations.
-- ## Declared roles
-- `mapper`, `validator`

CREATE TABLE invocation_completion_obligations (
    admission_id TEXT PRIMARY KEY CHECK (length(trim(admission_id)) > 0),
    invocation_uuid TEXT NOT NULL REFERENCES invocations(invocation_uuid)
        CHECK (length(trim(invocation_uuid)) > 0),
    event_id TEXT NOT NULL UNIQUE CHECK (length(trim(event_id)) > 0),
    owner_invocation_uuid TEXT NOT NULL REFERENCES invocations(invocation_uuid)
        CHECK (length(trim(owner_invocation_uuid)) > 0),
    expected_sidecar_generation TEXT NOT NULL
        CHECK (length(trim(expected_sidecar_generation)) > 0),
    admitted_at TEXT NOT NULL CHECK (length(trim(admitted_at)) > 0)
);

CREATE INDEX idx_invocation_completion_obligations_invocation
    ON invocation_completion_obligations (
        invocation_uuid,
        admitted_at,
        admission_id
    );
