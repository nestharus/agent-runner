PRAGMA foreign_keys = ON;

BEGIN IMMEDIATE;

CREATE TABLE IF NOT EXISTS s11_wu4_restore_session_ownership_preimage (
    migration_id TEXT NOT NULL,
    entity_kind TEXT NOT NULL CHECK(entity_kind IN ('chain', 'segment', 'turn', 'segment_delete', 'turn_delete', 'segment_merge_survivor')),
    row_pk TEXT NOT NULL,
    chain_id TEXT,
    segment_id INTEGER,
    turn_row_id INTEGER,
    old_model_name TEXT,
    new_model_name TEXT,
    old_provider_name TEXT,
    new_provider_name TEXT,
    session_id TEXT,
    segment_started_at TEXT,
    segment_ended_at TEXT,
    segment_last_turn_id TEXT,
    segment_transition_reason TEXT,
    turn_id TEXT,
    turn_timestamp TEXT,
    turn_role TEXT,
    turn_parent_turn_id TEXT,
    turn_is_sidechain INTEGER,
    turn_is_compaction_boundary INTEGER,
    turn_source_file TEXT,
    turn_ingested_at TEXT,
    turn_body TEXT,
    new_started_at TEXT,
    new_ended_at TEXT,
    new_last_turn_id TEXT,
    new_transition_reason TEXT,
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
FROM s11_wu4_candidate_segments candidate
LEFT JOIN s11_wu4_segment_merge_deletes deleted ON deleted.segment_id = candidate.segment_id
WHERE deleted.segment_id IS NULL;

INSERT OR IGNORE INTO s11_wu4_restore_session_ownership_preimage (
    migration_id,
    entity_kind,
    row_pk,
    chain_id,
    segment_id,
    old_provider_name,
    new_provider_name,
    session_id,
    segment_started_at,
    segment_ended_at,
    segment_last_turn_id,
    segment_transition_reason
)
SELECT
    (SELECT value FROM s11_wu4_migration_params WHERE key = 'migration_id'),
    'segment_delete',
    CAST(segment.id AS TEXT),
    segment.chain_id,
    segment.id,
    segment.provider_name,
    candidate.new_provider_name,
    segment.session_id,
    segment.started_at,
    segment.ended_at,
    segment.last_turn_id,
    segment.transition_reason
FROM s11_wu4_segment_merge_deletes planned
JOIN session_chain_segments segment ON segment.id = planned.segment_id
JOIN s11_wu4_candidate_segments candidate ON candidate.segment_id = segment.id;

INSERT OR IGNORE INTO s11_wu4_restore_session_ownership_preimage (
    migration_id,
    entity_kind,
    row_pk,
    chain_id,
    segment_id,
    old_provider_name,
    new_provider_name,
    session_id,
    segment_started_at,
    segment_ended_at,
    segment_last_turn_id,
    segment_transition_reason,
    new_started_at,
    new_ended_at,
    new_last_turn_id,
    new_transition_reason
)
SELECT
    (SELECT value FROM s11_wu4_migration_params WHERE key = 'migration_id'),
    'segment_merge_survivor',
    CAST(segment.id AS TEXT),
    segment.chain_id,
    segment.id,
    segment.provider_name,
    candidate.new_provider_name,
    segment.session_id,
    segment.started_at,
    segment.ended_at,
    segment.last_turn_id,
    segment.transition_reason,
    planned.merged_started_at,
    planned.merged_ended_at,
    planned.merged_last_turn_id,
    planned.merged_transition_reason
FROM s11_wu4_segment_merge_groups planned
JOIN session_chain_segments segment ON segment.id = planned.survivor_segment_id
JOIN s11_wu4_candidate_segments candidate ON candidate.segment_id = segment.id;

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
LEFT JOIN s11_wu4_turn_dedup_deletes deleted ON deleted.loser_turn_row_id = turn.id
WHERE candidate.old_provider_name <> candidate.new_provider_name
  AND deleted.loser_turn_row_id IS NULL;

INSERT OR IGNORE INTO s11_wu4_restore_session_ownership_preimage (
    migration_id,
    entity_kind,
    row_pk,
    turn_row_id,
    old_provider_name,
    new_provider_name,
    session_id,
    turn_id,
    turn_timestamp,
    turn_role,
    turn_parent_turn_id,
    turn_is_sidechain,
    turn_is_compaction_boundary,
    turn_source_file,
    turn_ingested_at,
    turn_body
)
SELECT
    (SELECT value FROM s11_wu4_migration_params WHERE key = 'migration_id'),
    'turn_delete',
    CAST(turn.id AS TEXT),
    turn.id,
    turn.provider_name,
    planned.new_provider_name,
    turn.session_id,
    turn.turn_id,
    turn.timestamp,
    turn.role,
    turn.parent_turn_id,
    turn.is_sidechain,
    turn.is_compaction_boundary,
    turn.source_file,
    turn.ingested_at,
    turn.body
FROM s11_wu4_turn_dedup_deletes planned
JOIN session_turns turn ON turn.id = planned.loser_turn_row_id;

DROP TABLE IF EXISTS s11_wu4_forward_guard;
CREATE TEMP TABLE s11_wu4_forward_guard (id INTEGER PRIMARY KEY ON CONFLICT ROLLBACK);
INSERT INTO s11_wu4_forward_guard(id) VALUES (1);
INSERT OR ROLLBACK INTO s11_wu4_forward_guard(id)
SELECT 1
WHERE EXISTS (SELECT 1 FROM s11_wu4_candidate_segments)
  AND EXISTS (
    SELECT 1
    FROM s11_wu4_turn_dedup_deletes planned
    LEFT JOIN session_turns loser ON loser.id = planned.loser_turn_row_id
    LEFT JOIN session_turns winner ON winner.id = planned.winner_turn_row_id
    WHERE loser.id IS NULL
       OR winner.id IS NULL
       OR loser.session_id <> planned.session_id
       OR loser.turn_id <> planned.turn_id
       OR winner.session_id <> planned.session_id
       OR winner.turn_id <> planned.turn_id
       OR NOT (loser.timestamp IS winner.timestamp)
       OR NOT (loser.role IS winner.role)
       OR NOT (loser.parent_turn_id IS winner.parent_turn_id)
       OR NOT (loser.is_sidechain IS winner.is_sidechain)
       OR NOT (loser.is_compaction_boundary IS winner.is_compaction_boundary)
       OR NOT (loser.source_file IS winner.source_file)
       OR NOT (loser.ingested_at IS winner.ingested_at)
       OR NOT (loser.body IS winner.body)
);

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
LEFT JOIN s11_wu4_segment_merge_deletes deleted ON deleted.segment_id = candidate.segment_id
WHERE deleted.segment_id IS NULL
  AND segment.provider_name = candidate.old_provider_name
  AND candidate.old_provider_name <> candidate.new_provider_name;

INSERT INTO s11_wu4_last_run_counts(key, value)
SELECT 'turn_provider_updates_to_apply', COUNT(DISTINCT turn.id)
FROM s11_wu4_candidate_segments candidate
JOIN session_turns turn
  ON turn.provider_name = candidate.old_provider_name
 AND turn.session_id = candidate.session_id
LEFT JOIN s11_wu4_turn_dedup_deletes deleted ON deleted.loser_turn_row_id = turn.id
WHERE deleted.loser_turn_row_id IS NULL
  AND candidate.old_provider_name <> candidate.new_provider_name;

INSERT INTO s11_wu4_last_run_counts(key, value)
SELECT 'segment_rows_merged_away', COUNT(*)
FROM s11_wu4_segment_merge_deletes planned
JOIN session_chain_segments segment ON segment.id = planned.segment_id;

INSERT INTO s11_wu4_last_run_counts(key, value)
SELECT 'turn_rows_deduped_away', COUNT(*)
FROM s11_wu4_turn_dedup_deletes planned
JOIN session_turns turn ON turn.id = planned.loser_turn_row_id;

INSERT INTO s11_wu4_last_run_counts(key, value)
SELECT 'segment_merge_survivors_updated', COUNT(*)
FROM s11_wu4_segment_merge_groups planned
JOIN session_chain_segments segment ON segment.id = planned.survivor_segment_id;

DELETE FROM session_turns
WHERE id IN (SELECT loser_turn_row_id FROM s11_wu4_turn_dedup_deletes);

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

DELETE FROM session_chain_segments
WHERE id IN (SELECT segment_id FROM s11_wu4_segment_merge_deletes);

UPDATE session_chain_segments
SET started_at = (
        SELECT planned.merged_started_at
        FROM s11_wu4_segment_merge_groups planned
        WHERE planned.survivor_segment_id = session_chain_segments.id
    ),
    ended_at = (
        SELECT planned.merged_ended_at
        FROM s11_wu4_segment_merge_groups planned
        WHERE planned.survivor_segment_id = session_chain_segments.id
    ),
    last_turn_id = (
        SELECT planned.merged_last_turn_id
        FROM s11_wu4_segment_merge_groups planned
        WHERE planned.survivor_segment_id = session_chain_segments.id
    ),
    transition_reason = (
        SELECT planned.merged_transition_reason
        FROM s11_wu4_segment_merge_groups planned
        WHERE planned.survivor_segment_id = session_chain_segments.id
    )
WHERE id IN (SELECT survivor_segment_id FROM s11_wu4_segment_merge_groups);

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

INSERT INTO s11_wu4_last_run_counts(key, value)
SELECT 'post_apply_segment_collision_count', COUNT(*)
FROM (
    SELECT chain_id, provider_name, session_id
    FROM session_chain_segments
    GROUP BY chain_id, provider_name, session_id
    HAVING COUNT(*) > 1
);

INSERT INTO s11_wu4_last_run_counts(key, value)
SELECT 'post_apply_turn_collision_count', COUNT(*)
FROM (
    SELECT provider_name, session_id, turn_id
    FROM session_turns
    GROUP BY provider_name, session_id, turn_id
    HAVING COUNT(*) > 1
);

COMMIT;
