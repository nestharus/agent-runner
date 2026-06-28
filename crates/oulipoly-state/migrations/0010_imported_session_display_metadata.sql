-- Imported provider-native session display metadata.
-- Additive only: this table is keyed independently from session chains so
-- re-import can refresh display fields without touching chain ownership rows.

CREATE TABLE IF NOT EXISTS imported_session_display_metadata (
    provider_name TEXT NOT NULL,
    provider_session_id TEXT NOT NULL,
    title TEXT,
    cwd TEXT,
    turn_count INTEGER CHECK (turn_count IS NULL OR turn_count >= 0),
    provider_updated_at TEXT,
    first_seen_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    PRIMARY KEY (provider_name, provider_session_id)
);

CREATE INDEX IF NOT EXISTS idx_imported_session_display_metadata_provider_seen
    ON imported_session_display_metadata (provider_name, last_seen_at);
