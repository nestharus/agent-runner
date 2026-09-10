CREATE TABLE IF NOT EXISTS mailbox_observation_stops (
    stop_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    reason TEXT NOT NULL,
    error TEXT NOT NULL,
    stopped_at TEXT NOT NULL,
    rearmed_at TEXT,
    resolution TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_mailbox_observation_active_stop
    ON mailbox_observation_stops(session_id) WHERE rearmed_at IS NULL;
