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
    resume_acceptance_status TEXT,
    resume_acceptance_evidence TEXT,
    created_at TEXT NOT NULL,
    finished_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_invocations_uuid ON invocations (invocation_uuid);
CREATE INDEX IF NOT EXISTS idx_invocations_parent ON invocations (parent_invocation_id, created_at);
CREATE INDEX IF NOT EXISTS idx_invocations_provider_created ON invocations (provider_name, created_at);
CREATE INDEX IF NOT EXISTS idx_invocations_provider_session ON invocations (provider_name, session_id)
    WHERE session_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS providers (
    model_name TEXT NOT NULL,
    provider_name TEXT NOT NULL,
    invocation_count INTEGER NOT NULL DEFAULT 0,
    error_count INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    last_error_at TEXT,
    last_invoked_at TEXT,
    PRIMARY KEY (model_name, provider_name)
);

CREATE TABLE IF NOT EXISTS provider_quotas (
    provider_name TEXT PRIMARY KEY,
    used_percent REAL NOT NULL DEFAULT 0,
    resets_at TEXT,
    calls_since_refresh INTEGER NOT NULL DEFAULT 0,
    refreshed_at TEXT,
    last_empty_refresh_at TEXT,
    exhausted_at TEXT NULL,
    topology_peak_live_window_count INTEGER NOT NULL DEFAULT 0,
    last_topology_probe_at TEXT
);

CREATE TABLE IF NOT EXISTS provider_quota_windows (
    provider_name TEXT NOT NULL,
    window_id INTEGER NOT NULL,
    used_percent REAL NOT NULL DEFAULT 0,
    resets_at TEXT NOT NULL,
    last_delta_percent REAL,
    last_delta_calls INTEGER,
    PRIMARY KEY (provider_name, window_id)
);

INSERT OR IGNORE INTO provider_quota_windows (provider_name, window_id, used_percent, resets_at)
SELECT provider_name, 0, used_percent, resets_at
FROM provider_quotas
WHERE resets_at IS NOT NULL;

CREATE TABLE IF NOT EXISTS memory_nodes (
    id TEXT PRIMARY KEY,
    node_type TEXT NOT NULL,
    label TEXT NOT NULL,
    data TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS memory_edges (
    source_id TEXT NOT NULL REFERENCES memory_nodes(id),
    target_id TEXT NOT NULL REFERENCES memory_nodes(id),
    edge_type TEXT NOT NULL,
    data TEXT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (source_id, target_id, edge_type)
);

DROP TABLE IF EXISTS memory_edges_age32_new;
CREATE TABLE memory_edges_age32_new (
    source_id TEXT NOT NULL REFERENCES memory_nodes(id),
    target_id TEXT NOT NULL REFERENCES memory_nodes(id),
    edge_type TEXT NOT NULL,
    data TEXT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (source_id, target_id, edge_type)
);
INSERT OR IGNORE INTO memory_edges_age32_new (source_id, target_id, edge_type, data, created_at)
SELECT source_id, target_id, edge_type, data, created_at FROM memory_edges;
DROP TABLE memory_edges;
ALTER TABLE memory_edges_age32_new RENAME TO memory_edges;

CREATE TABLE IF NOT EXISTS setup_sessions (
    id TEXT PRIMARY KEY,
    started_at TEXT NOT NULL,
    ended_at TEXT,
    outcome TEXT,
    turn_count INTEGER DEFAULT 0
);

CREATE TABLE IF NOT EXISTS setup_turns (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    turn_number INTEGER NOT NULL,
    agent_prompt TEXT NOT NULL,
    agent_response TEXT NOT NULL,
    events_emitted TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS cli_providers (
    cli_name TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    installed INTEGER NOT NULL DEFAULT 0,
    version TEXT,
    config_dir TEXT,
    last_synced TEXT
);

CREATE TABLE IF NOT EXISTS accounts (
    id TEXT NOT NULL,
    provider TEXT NOT NULL REFERENCES cli_providers(cli_name),
    profile_name TEXT NOT NULL,
    auth_method TEXT NOT NULL,
    auth_status TEXT NOT NULL DEFAULT 'unknown',
    created_at TEXT NOT NULL,
    PRIMARY KEY (id, provider)
);
CREATE INDEX IF NOT EXISTS idx_accounts_provider ON accounts (provider);

CREATE TABLE IF NOT EXISTS discovered_models (
    canonical_name TEXT NOT NULL,
    provider TEXT NOT NULL,
    discovered_at TEXT NOT NULL,
    cli_version TEXT NOT NULL,
    PRIMARY KEY (canonical_name, provider)
);

CREATE TABLE IF NOT EXISTS model_parameters (
    model_name TEXT NOT NULL,
    provider TEXT NOT NULL,
    name TEXT NOT NULL,
    display_name TEXT NOT NULL,
    param_type TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    cli_mapping TEXT NOT NULL,
    PRIMARY KEY (model_name, provider, name)
);

CREATE TABLE IF NOT EXISTS session_turns (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    provider_name TEXT NOT NULL,
    session_id TEXT NOT NULL,
    turn_id TEXT NOT NULL,
    timestamp TEXT NOT NULL,
    role TEXT NOT NULL,
    parent_turn_id TEXT,
    is_sidechain INTEGER NOT NULL DEFAULT 0,
    is_compaction_boundary INTEGER NOT NULL DEFAULT 0,
    source_file TEXT NOT NULL,
    ingested_at TEXT NOT NULL,
    body TEXT,
    UNIQUE (provider_name, session_id, turn_id)
);
CREATE INDEX IF NOT EXISTS idx_session_turns_provider_ts ON session_turns (provider_name, role, timestamp);
CREATE INDEX IF NOT EXISTS idx_session_turns_session_ts ON session_turns (provider_name, session_id, timestamp);
CREATE INDEX IF NOT EXISTS idx_session_turns_session_lookup ON session_turns (session_id, timestamp);
CREATE INDEX IF NOT EXISTS idx_session_turns_parent ON session_turns (provider_name, session_id, parent_turn_id, timestamp);

CREATE TABLE IF NOT EXISTS session_chains (
    chain_id TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    last_used_at TEXT NOT NULL,
    model_name TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS session_chain_segments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    chain_id TEXT NOT NULL REFERENCES session_chains(chain_id),
    provider_name TEXT NOT NULL,
    session_id TEXT NOT NULL,
    started_at TEXT NOT NULL,
    ended_at TEXT,
    last_turn_id TEXT,
    transition_reason TEXT NOT NULL CHECK (transition_reason IN
        ('initial', 'manual', 'quota_threshold', 'exhausted', 'imported')),
    UNIQUE(chain_id, provider_name, session_id)
);
CREATE INDEX IF NOT EXISTS idx_segments_session ON session_chain_segments(session_id);
CREATE INDEX IF NOT EXISTS idx_segments_chain_active ON session_chain_segments(chain_id, ended_at);

-- Legacy repair representation for TI-40:
-- ALTER TABLE invocations ADD COLUMN session_id TEXT
-- ALTER TABLE invocations ADD COLUMN session_capture_method TEXT
-- ALTER TABLE invocations ADD COLUMN resume_acceptance_status TEXT
-- ALTER TABLE invocations ADD COLUMN resume_acceptance_evidence TEXT
-- ALTER TABLE invocations ADD COLUMN terminal_reason TEXT
-- ALTER TABLE invocations DROP COLUMN quota_tight_routing
-- ALTER TABLE providers RENAME TO providers_legacy_index_keyed
-- DROP TABLE providers_legacy_index_keyed
-- ALTER TABLE session_turns ADD COLUMN parent_turn_id TEXT
-- ALTER TABLE session_turns ADD COLUMN is_sidechain INTEGER NOT NULL DEFAULT 0
-- ALTER TABLE session_turns ADD COLUMN is_compaction_boundary INTEGER NOT NULL DEFAULT 0
-- ALTER TABLE session_turns ADD COLUMN body TEXT
-- ALTER TABLE provider_quotas ADD COLUMN last_empty_refresh_at TEXT
-- ALTER TABLE provider_quotas ADD COLUMN exhausted_at TEXT NULL
-- ALTER TABLE provider_quotas DROP COLUMN last_delta_percent
-- ALTER TABLE provider_quotas DROP COLUMN last_delta_calls
-- ALTER TABLE provider_quotas ADD COLUMN topology_peak_live_window_count INTEGER NOT NULL DEFAULT 0
-- ALTER TABLE provider_quotas ADD COLUMN last_topology_probe_at TEXT
-- ALTER TABLE provider_quota_windows ADD COLUMN last_delta_percent REAL NULL
-- ALTER TABLE provider_quota_windows ADD COLUMN last_delta_calls INTEGER NULL
