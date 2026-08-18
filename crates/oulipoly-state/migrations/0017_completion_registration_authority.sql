-- Caller-bound authority for immutable completion admission. Only the digest is
-- durable; the bearer value is transported in the launched invocation's
-- private process environment.
-- ## Declared roles
-- `validator`

-- Preserve the established missing-invocations repair path when this migration
-- is the first step to encounter a deliberately removed table.
CREATE TABLE IF NOT EXISTS invocations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    invocation_uuid TEXT NOT NULL UNIQUE,
    model_name TEXT NOT NULL,
    provider_name TEXT,
    provider_index INTEGER NOT NULL,
    parent_invocation_id INTEGER REFERENCES invocations(id),
    status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'legacy')),
    success INTEGER,
    exit_code INTEGER,
    error_category TEXT,
    terminal_reason TEXT,
    session_id TEXT,
    session_capture_method TEXT,
    provider_session_id TEXT,
    resume_input_id TEXT,
    provider_session_capture_method TEXT,
    provider_session_resolved_account TEXT,
    resume_acceptance_status TEXT,
    resume_acceptance_evidence TEXT,
    created_at TEXT NOT NULL,
    finished_at TEXT,
    row_version INTEGER NOT NULL DEFAULT 0
);

ALTER TABLE invocations
ADD COLUMN completion_registration_capability_digest TEXT
    CONSTRAINT invocation_completion_registration_capability_digest_shape
    CHECK (
        completion_registration_capability_digest IS NULL
        OR (
            length(completion_registration_capability_digest) = 64
            AND completion_registration_capability_digest NOT GLOB '*[^0-9a-f]*'
        )
    );

CREATE TRIGGER trg_invocation_completion_registration_capability_immutable
BEFORE UPDATE OF completion_registration_capability_digest ON invocations
WHEN OLD.completion_registration_capability_digest
     IS NOT NEW.completion_registration_capability_digest
BEGIN
    SELECT RAISE(ABORT, 'completion registration capability is immutable');
END;
