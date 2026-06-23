PRAGMA foreign_keys = ON;

BEGIN IMMEDIATE;

DROP TABLE IF EXISTS s11_wu4_last_rollback_counts;
CREATE TABLE s11_wu4_last_rollback_counts (
    key TEXT PRIMARY KEY,
    value INTEGER NOT NULL
);

INSERT INTO s11_wu4_last_rollback_counts(key, value)
SELECT 'chain_rows_to_restore', COUNT(*)
FROM s11_wu4_restore_session_ownership_preimage
WHERE entity_kind = 'chain';

INSERT INTO s11_wu4_last_rollback_counts(key, value)
SELECT 'segment_rows_to_restore', COUNT(*)
FROM s11_wu4_restore_session_ownership_preimage
WHERE entity_kind = 'segment'
  AND old_provider_name <> new_provider_name;

INSERT INTO s11_wu4_last_rollback_counts(key, value)
SELECT 'turn_rows_to_restore', COUNT(*)
FROM s11_wu4_restore_session_ownership_preimage
WHERE entity_kind = 'turn';

INSERT INTO s11_wu4_last_rollback_counts(key, value)
SELECT 'segment_rows_reinserted', COUNT(*)
FROM s11_wu4_restore_session_ownership_preimage
WHERE entity_kind = 'segment_delete';

INSERT INTO s11_wu4_last_rollback_counts(key, value)
SELECT 'turn_rows_reinserted', COUNT(*)
FROM s11_wu4_restore_session_ownership_preimage
WHERE entity_kind = 'turn_delete';

INSERT INTO s11_wu4_last_rollback_counts(key, value)
SELECT 'segment_merge_survivors_restored', COUNT(*)
FROM s11_wu4_restore_session_ownership_preimage
WHERE entity_kind = 'segment_merge_survivor';

UPDATE session_chain_segments
SET chain_id = (
        SELECT preimage.chain_id
        FROM s11_wu4_restore_session_ownership_preimage preimage
        WHERE preimage.entity_kind = 'segment_merge_survivor'
          AND preimage.segment_id = session_chain_segments.id
    ),
    provider_name = (
        SELECT preimage.old_provider_name
        FROM s11_wu4_restore_session_ownership_preimage preimage
        WHERE preimage.entity_kind = 'segment_merge_survivor'
          AND preimage.segment_id = session_chain_segments.id
    ),
    session_id = (
        SELECT preimage.session_id
        FROM s11_wu4_restore_session_ownership_preimage preimage
        WHERE preimage.entity_kind = 'segment_merge_survivor'
          AND preimage.segment_id = session_chain_segments.id
    ),
    started_at = (
        SELECT preimage.segment_started_at
        FROM s11_wu4_restore_session_ownership_preimage preimage
        WHERE preimage.entity_kind = 'segment_merge_survivor'
          AND preimage.segment_id = session_chain_segments.id
    ),
    ended_at = (
        SELECT preimage.segment_ended_at
        FROM s11_wu4_restore_session_ownership_preimage preimage
        WHERE preimage.entity_kind = 'segment_merge_survivor'
          AND preimage.segment_id = session_chain_segments.id
    ),
    last_turn_id = (
        SELECT preimage.segment_last_turn_id
        FROM s11_wu4_restore_session_ownership_preimage preimage
        WHERE preimage.entity_kind = 'segment_merge_survivor'
          AND preimage.segment_id = session_chain_segments.id
    ),
    transition_reason = (
        SELECT preimage.segment_transition_reason
        FROM s11_wu4_restore_session_ownership_preimage preimage
        WHERE preimage.entity_kind = 'segment_merge_survivor'
          AND preimage.segment_id = session_chain_segments.id
    )
WHERE id IN (
    SELECT segment_id
    FROM s11_wu4_restore_session_ownership_preimage
    WHERE entity_kind = 'segment_merge_survivor'
);

UPDATE session_turns
SET provider_name = (
    SELECT preimage.old_provider_name
    FROM s11_wu4_restore_session_ownership_preimage preimage
    WHERE preimage.entity_kind = 'turn'
      AND preimage.turn_row_id = session_turns.id
)
WHERE id IN (
    SELECT turn_row_id FROM s11_wu4_restore_session_ownership_preimage WHERE entity_kind = 'turn'
)
  AND provider_name = (
    SELECT preimage.new_provider_name
    FROM s11_wu4_restore_session_ownership_preimage preimage
    WHERE preimage.entity_kind = 'turn'
      AND preimage.turn_row_id = session_turns.id
  );

UPDATE session_chain_segments
SET provider_name = (
    SELECT preimage.old_provider_name
    FROM s11_wu4_restore_session_ownership_preimage preimage
    WHERE preimage.entity_kind = 'segment'
      AND preimage.segment_id = session_chain_segments.id
)
WHERE id IN (
    SELECT segment_id FROM s11_wu4_restore_session_ownership_preimage WHERE entity_kind = 'segment'
)
  AND id NOT IN (
    SELECT segment_id
    FROM s11_wu4_restore_session_ownership_preimage
    WHERE entity_kind = 'segment_merge_survivor'
  )
  AND provider_name = (
    SELECT preimage.new_provider_name
    FROM s11_wu4_restore_session_ownership_preimage preimage
    WHERE preimage.entity_kind = 'segment'
      AND preimage.segment_id = session_chain_segments.id
  );

INSERT INTO session_chain_segments (
    id,
    chain_id,
    provider_name,
    session_id,
    started_at,
    ended_at,
    last_turn_id,
    transition_reason
)
SELECT
    segment_id,
    chain_id,
    old_provider_name,
    session_id,
    segment_started_at,
    segment_ended_at,
    segment_last_turn_id,
    segment_transition_reason
FROM s11_wu4_restore_session_ownership_preimage
WHERE entity_kind = 'segment_delete';

INSERT INTO session_turns (
    id,
    provider_name,
    session_id,
    turn_id,
    timestamp,
    role,
    parent_turn_id,
    is_sidechain,
    is_compaction_boundary,
    source_file,
    ingested_at,
    body
)
SELECT
    turn_row_id,
    old_provider_name,
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
FROM s11_wu4_restore_session_ownership_preimage
WHERE entity_kind = 'turn_delete';

UPDATE session_chains
SET model_name = (
    SELECT preimage.old_model_name
    FROM s11_wu4_restore_session_ownership_preimage preimage
    WHERE preimage.entity_kind = 'chain'
      AND preimage.chain_id = session_chains.chain_id
)
WHERE chain_id IN (
    SELECT chain_id FROM s11_wu4_restore_session_ownership_preimage WHERE entity_kind = 'chain'
)
  AND model_name = (
    SELECT preimage.new_model_name
    FROM s11_wu4_restore_session_ownership_preimage preimage
    WHERE preimage.entity_kind = 'chain'
      AND preimage.chain_id = session_chains.chain_id
  );

COMMIT;
