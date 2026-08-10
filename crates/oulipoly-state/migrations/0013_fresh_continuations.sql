-- Durable exact-identity state machine for fresh continuation handoffs.
-- ## Declared roles
-- `mapper`, `validator`

CREATE TABLE fresh_continuations (
    logical_request_key TEXT PRIMARY KEY CHECK (logical_request_key <> ''),
    continuation_id TEXT NOT NULL UNIQUE CHECK (continuation_id <> ''),
    validated_fingerprint TEXT NOT NULL CHECK (validated_fingerprint <> ''),
    resume_invocation_id TEXT NOT NULL UNIQUE CHECK (resume_invocation_id <> ''),
    resume_parent_invocation_id TEXT NOT NULL CHECK (resume_parent_invocation_id <> ''),
    resume_stage TEXT NOT NULL DEFAULT 'reserved' CHECK (
        resume_stage IN ('reserved', 'running', 'recorded')
    ),
    resume_outcome_json TEXT,
    fresh_invocation_id TEXT NOT NULL UNIQUE CHECK (fresh_invocation_id <> ''),
    fresh_parent_invocation_id TEXT NOT NULL CHECK (fresh_parent_invocation_id <> ''),
    fresh_stage TEXT NOT NULL DEFAULT 'reserved' CHECK (
        fresh_stage IN ('reserved', 'running', 'recorded')
    ),
    fresh_outcome_json TEXT,
    handoff_json TEXT,
    terminal_outcome_json TEXT,
    CHECK ((resume_stage = 'recorded') = (resume_outcome_json IS NOT NULL)),
    CHECK ((fresh_stage = 'recorded') = (fresh_outcome_json IS NOT NULL)),
    CHECK (resume_outcome_json IS NULL OR json_valid(resume_outcome_json)),
    CHECK (fresh_outcome_json IS NULL OR json_valid(fresh_outcome_json)),
    CHECK (handoff_json IS NULL OR json_valid(handoff_json)),
    CHECK (terminal_outcome_json IS NULL OR json_valid(terminal_outcome_json)),
    CHECK (fresh_stage = 'reserved' OR resume_stage = 'recorded'),
    CHECK ((handoff_json IS NULL) = (terminal_outcome_json IS NULL)),
    CHECK (
        terminal_outcome_json IS NULL
        OR (resume_stage = 'recorded' AND fresh_stage = 'recorded')
    ),
    CHECK (fresh_parent_invocation_id = resume_invocation_id)
);
