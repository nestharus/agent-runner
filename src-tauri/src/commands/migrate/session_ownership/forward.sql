PRAGMA foreign_keys = ON;

BEGIN IMMEDIATE;

CREATE TABLE IF NOT EXISTS s11_wu4_restore_session_ownership_preimage (
    migration_id TEXT NOT NULL,
    entity_kind TEXT NOT NULL CHECK(entity_kind IN ('chain', 'segment', 'turn')),
    row_pk TEXT NOT NULL,
    chain_id TEXT,
    segment_id INTEGER,
    turn_row_id INTEGER,
    old_model_name TEXT,
    new_model_name TEXT,
    old_provider_name TEXT,
    new_provider_name TEXT,
    session_id TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (migration_id, entity_kind, row_pk)
);

DROP TABLE IF EXISTS s11_wu4_candidate_segments;
CREATE TEMP TABLE s11_wu4_candidate_segments (
    chain_id TEXT NOT NULL,
    old_model_name TEXT NOT NULL,
    target_model_name TEXT NOT NULL,
    segment_id INTEGER NOT NULL,
    old_provider_name TEXT NOT NULL,
    session_id TEXT NOT NULL,
    new_provider_name TEXT NOT NULL,
    remap_reason TEXT NOT NULL,
    issue52_unregistered INTEGER NOT NULL CHECK(issue52_unregistered IN (0, 1)),
    PRIMARY KEY (chain_id, segment_id)
);

INSERT INTO s11_wu4_candidate_segments (
    chain_id,
    old_model_name,
    target_model_name,
    segment_id,
    old_provider_name,
    session_id,
    new_provider_name,
    remap_reason,
    issue52_unregistered
)
WITH params AS (
    SELECT
        (SELECT value FROM s11_wu4_migration_params WHERE key = 'target_model_name') AS target_model_name
)
SELECT
    chain.chain_id,
    chain.model_name,
    params.target_model_name,
    segment.id,
    segment.provider_name,
    segment.session_id,
    COALESCE(alias.new_provider_name, segment.provider_name),
    COALESCE(alias.reason, inventory.source),
    CASE WHEN original.provider_name IS NULL THEN 1 ELSE 0 END
FROM s11_wu4_source_chain_candidates source
JOIN session_chains chain ON chain.chain_id = source.chain_id
JOIN session_chain_segments segment ON segment.chain_id = chain.chain_id
CROSS JOIN params
LEFT JOIN s11_wu4_original_target_provider_inventory original
  ON original.provider_name = segment.provider_name
LEFT JOIN s11_wu4_target_provider_inventory inventory
  ON inventory.provider_name = segment.provider_name
LEFT JOIN s11_wu4_provider_aliases alias
  ON alias.old_provider_name = segment.provider_name
WHERE chain.model_name <> params.target_model_name;

INSERT OR IGNORE INTO s11_wu4_restore_session_ownership_preimage (
    migration_id,
    entity_kind,
    row_pk,
    chain_id,
    old_model_name,
    new_model_name
)
SELECT DISTINCT
    (SELECT value FROM s11_wu4_migration_params WHERE key = 'migration_id'),
    'chain',
    candidate.chain_id,
    candidate.chain_id,
    candidate.old_model_name,
    candidate.target_model_name
FROM s11_wu4_candidate_segments candidate
WHERE candidate.old_model_name <> candidate.target_model_name;

INSERT OR IGNORE INTO s11_wu4_restore_session_ownership_preimage (
    migration_id,
    entity_kind,
    row_pk,
    chain_id,
    segment_id,
    old_provider_name,
    new_provider_name,
    session_id
)
SELECT
    (SELECT value FROM s11_wu4_migration_params WHERE key = 'migration_id'),
    'segment',
    CAST(candidate.segment_id AS TEXT),
    candidate.chain_id,
    candidate.segment_id,
    candidate.old_provider_name,
    candidate.new_provider_name,
    candidate.session_id
FROM s11_wu4_candidate_segments candidate;

INSERT OR IGNORE INTO s11_wu4_restore_session_ownership_preimage (
    migration_id,
    entity_kind,
    row_pk,
    turn_row_id,
    old_provider_name,
    new_provider_name,
    session_id
)
SELECT DISTINCT
    (SELECT value FROM s11_wu4_migration_params WHERE key = 'migration_id'),
    'turn',
    CAST(turn.id AS TEXT),
    turn.id,
    turn.provider_name,
    candidate.new_provider_name,
    turn.session_id
FROM s11_wu4_candidate_segments candidate
JOIN session_turns turn
  ON turn.provider_name = candidate.old_provider_name
 AND turn.session_id = candidate.session_id
WHERE candidate.old_provider_name <> candidate.new_provider_name;

DROP TABLE IF EXISTS s11_wu4_last_run_counts;
CREATE TABLE s11_wu4_last_run_counts (
    key TEXT PRIMARY KEY,
    value INTEGER NOT NULL
);

INSERT INTO s11_wu4_last_run_counts(key, value)
SELECT 'candidate_chains', COUNT(DISTINCT chain_id) FROM s11_wu4_candidate_segments;

INSERT INTO s11_wu4_last_run_counts(key, value)
SELECT 'candidate_segments', COUNT(*) FROM s11_wu4_candidate_segments;

INSERT INTO s11_wu4_last_run_counts(key, value)
SELECT 'issue52_unregistered_segments', COALESCE(SUM(issue52_unregistered), 0)
FROM s11_wu4_candidate_segments;

INSERT INTO s11_wu4_last_run_counts(key, value)
SELECT 'blocked_segments', 0;

INSERT INTO s11_wu4_last_run_counts(key, value)
SELECT 'chain_model_updates_to_apply', COUNT(DISTINCT candidate.chain_id)
FROM s11_wu4_candidate_segments candidate
JOIN session_chains chain ON chain.chain_id = candidate.chain_id
WHERE chain.model_name = candidate.old_model_name
  AND candidate.old_model_name <> candidate.target_model_name;

INSERT INTO s11_wu4_last_run_counts(key, value)
SELECT 'segment_provider_updates_to_apply', COUNT(*)
FROM s11_wu4_candidate_segments candidate
JOIN session_chain_segments segment ON segment.id = candidate.segment_id
WHERE segment.provider_name = candidate.old_provider_name
  AND candidate.old_provider_name <> candidate.new_provider_name;

INSERT INTO s11_wu4_last_run_counts(key, value)
SELECT 'turn_provider_updates_to_apply', COUNT(DISTINCT turn.id)
FROM s11_wu4_candidate_segments candidate
JOIN session_turns turn
  ON turn.provider_name = candidate.old_provider_name
 AND turn.session_id = candidate.session_id
WHERE candidate.old_provider_name <> candidate.new_provider_name;

UPDATE session_turns
SET provider_name = (
    SELECT preimage.new_provider_name
    FROM s11_wu4_restore_session_ownership_preimage preimage
    WHERE preimage.entity_kind = 'turn'
      AND preimage.turn_row_id = session_turns.id
)
WHERE id IN (
    SELECT turn_row_id FROM s11_wu4_restore_session_ownership_preimage WHERE entity_kind = 'turn'
)
  AND provider_name = (
    SELECT preimage.old_provider_name
    FROM s11_wu4_restore_session_ownership_preimage preimage
    WHERE preimage.entity_kind = 'turn'
      AND preimage.turn_row_id = session_turns.id
  );

UPDATE session_chain_segments
SET provider_name = (
    SELECT preimage.new_provider_name
    FROM s11_wu4_restore_session_ownership_preimage preimage
    WHERE preimage.entity_kind = 'segment'
      AND preimage.segment_id = session_chain_segments.id
)
WHERE id IN (
    SELECT segment_id FROM s11_wu4_restore_session_ownership_preimage WHERE entity_kind = 'segment'
)
  AND provider_name = (
    SELECT preimage.old_provider_name
    FROM s11_wu4_restore_session_ownership_preimage preimage
    WHERE preimage.entity_kind = 'segment'
      AND preimage.segment_id = session_chain_segments.id
  );

UPDATE session_chains
SET model_name = (
    SELECT preimage.new_model_name
    FROM s11_wu4_restore_session_ownership_preimage preimage
    WHERE preimage.entity_kind = 'chain'
      AND preimage.chain_id = session_chains.chain_id
)
WHERE chain_id IN (
    SELECT chain_id FROM s11_wu4_restore_session_ownership_preimage WHERE entity_kind = 'chain'
)
  AND model_name = (
    SELECT preimage.old_model_name
    FROM s11_wu4_restore_session_ownership_preimage preimage
    WHERE preimage.entity_kind = 'chain'
      AND preimage.chain_id = session_chains.chain_id
  );

COMMIT;
