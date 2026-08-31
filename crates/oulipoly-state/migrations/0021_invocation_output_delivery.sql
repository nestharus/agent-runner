CREATE TABLE IF NOT EXISTS invocation_output_deliveries (
    invocation_id INTEGER PRIMARY KEY REFERENCES invocations(id) ON DELETE CASCADE,
    invocation_uuid TEXT NOT NULL,
    provider_outcome_state TEXT NOT NULL DEFAULT 'pending'
        CHECK (provider_outcome_state IN ('pending', 'settled')),
    delivery_state TEXT NOT NULL DEFAULT 'pending'
        CHECK (delivery_state IN ('pending', 'delivered', 'failed')),
    stdout_path TEXT NOT NULL,
    stdout_bytes INTEGER NOT NULL CHECK (stdout_bytes >= 0),
    stdout_sha256 TEXT NOT NULL CHECK (length(stdout_sha256) = 64),
    stderr_path TEXT NOT NULL,
    stderr_bytes INTEGER NOT NULL CHECK (stderr_bytes >= 0),
    stderr_sha256 TEXT NOT NULL CHECK (length(stderr_sha256) = 64),
    data_event_count INTEGER NOT NULL CHECK (data_event_count >= 0),
    delivery_failure_stage TEXT NULL,
    delivery_failure_kind TEXT NULL,
    delivery_failure_bytes INTEGER NULL CHECK (delivery_failure_bytes IS NULL OR delivery_failure_bytes >= 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    delivered_at TEXT NULL
);

CREATE INDEX IF NOT EXISTS idx_invocation_output_deliveries_state
    ON invocation_output_deliveries (delivery_state, updated_at);
