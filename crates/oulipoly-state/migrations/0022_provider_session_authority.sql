-- Persist the authenticated endpoint identity that owns provider sessions.
-- Existing rows remain unclaimed because their originating endpoint identity
-- was not durably recorded and cannot be reconstructed safely.

CREATE TABLE IF NOT EXISTS session_chain_segment_provider_authority (
    segment_id INTEGER PRIMARY KEY
        REFERENCES session_chain_segments(id) ON DELETE CASCADE,
    provider_instance_id TEXT NOT NULL CHECK (trim(provider_instance_id) <> ''),
    settings_id TEXT NOT NULL CHECK (trim(settings_id) <> '')
);

CREATE TABLE IF NOT EXISTS invocation_provider_session_authority (
    invocation_id INTEGER PRIMARY KEY
        REFERENCES invocations(id) ON DELETE CASCADE,
    provider_instance_id TEXT NOT NULL CHECK (trim(provider_instance_id) <> ''),
    settings_id TEXT NOT NULL CHECK (trim(settings_id) <> '')
);
