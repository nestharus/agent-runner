ALTER TABLE session_turns ADD COLUMN ingest_digest TEXT NULL
    CHECK (ingest_digest IS NULL OR length(ingest_digest) = 64);
ALTER TABLE session_turns ADD COLUMN body_state TEXT NULL
    CHECK (body_state IS NULL OR body_state IN ('inline', 'absent', 'omitted_oversize'));
ALTER TABLE session_turns ADD COLUMN body_sha256 TEXT NULL
    CHECK (body_sha256 IS NULL OR length(body_sha256) = 64);
ALTER TABLE session_turns ADD COLUMN body_bytes INTEGER NULL
    CHECK (body_bytes IS NULL OR body_bytes >= 0);
ALTER TABLE session_turns ADD COLUMN canonical_text_sha256 TEXT NULL
    CHECK (canonical_text_sha256 IS NULL OR length(canonical_text_sha256) = 64);
ALTER TABLE session_turns ADD COLUMN canonical_text_digest_verified INTEGER NOT NULL DEFAULT 0
    CHECK (canonical_text_digest_verified IN (0, 1));

CREATE INDEX idx_session_turns_canonical_text_sha256
    ON session_turns (provider_name, session_id, canonical_text_sha256)
    WHERE canonical_text_sha256 IS NOT NULL;

CREATE TABLE session_turn_ingest_streams (
    provider_name TEXT NOT NULL,
    provider_instance_id TEXT NOT NULL,
    settings_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    projection TEXT NOT NULL CHECK (projection IN ('canonical_ingest', 'user_observation')),
    paging_protocol TEXT NOT NULL CHECK (paging_protocol = 'oulipoly.session_turn_pages/v1'),
    checkpoint_generation INTEGER NOT NULL DEFAULT 0 CHECK (checkpoint_generation >= 0),
    after_token TEXT NULL,
    after_token_sha256 TEXT NULL CHECK (after_token_sha256 IS NULL OR length(after_token_sha256) = 64),
    snapshot_id TEXT NULL,
    next_page_token TEXT NULL,
    expected_page_index INTEGER NOT NULL DEFAULT 0 CHECK (expected_page_index >= 0),
    expected_turn_sequence INTEGER NOT NULL DEFAULT 0 CHECK (expected_turn_sequence >= 0),
    last_committed_snapshot_id TEXT NULL,
    last_committed_page_index INTEGER NULL CHECK (last_committed_page_index IS NULL OR last_committed_page_index >= 0),
    last_request_token_sha256 TEXT NULL CHECK (last_request_token_sha256 IS NULL OR length(last_request_token_sha256) = 64),
    last_page_digest TEXT NULL CHECK (last_page_digest IS NULL OR length(last_page_digest) = 64),
    committed_prefix_digest TEXT NULL CHECK (committed_prefix_digest IS NULL OR length(committed_prefix_digest) = 64),
    committed_page_count INTEGER NOT NULL DEFAULT 0 CHECK (committed_page_count >= 0),
    committed_turn_count INTEGER NOT NULL DEFAULT 0 CHECK (committed_turn_count >= 0),
    status TEXT NOT NULL DEFAULT 'ready'
        CHECK (status IN ('ready', 'active', 'caught_up', 'retry_wait', 'unsupported', 'quarantined')),
    priority INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TEXT NULL,
    retry_count INTEGER NOT NULL DEFAULT 0 CHECK (retry_count >= 0),
    last_success_at TEXT NULL,
    last_error TEXT NULL,
    lease_owner TEXT NULL,
    lease_expires_at TEXT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (provider_name, provider_instance_id, settings_id, session_id, projection),
    CHECK ((snapshot_id IS NULL) = (next_page_token IS NULL))
);

CREATE INDEX idx_session_turn_ingest_streams_ready
    ON session_turn_ingest_streams (status, next_attempt_at, priority DESC, updated_at);
