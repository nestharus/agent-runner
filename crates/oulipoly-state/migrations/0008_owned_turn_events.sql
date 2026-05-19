CREATE TABLE IF NOT EXISTS owned_turn_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    turn_uuid TEXT NOT NULL,
    is_compaction_boundary INTEGER NOT NULL DEFAULT 0,
    summary_metadata_json TEXT,
    ingested_at TEXT NOT NULL,
    UNIQUE (session_id, turn_uuid)
);

CREATE INDEX IF NOT EXISTS idx_owned_turn_events_session
    ON owned_turn_events (session_id, id);

CREATE INDEX IF NOT EXISTS idx_owned_turn_events_compaction
    ON owned_turn_events (session_id, is_compaction_boundary, id);
