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
  AND provider_name = (
    SELECT preimage.new_provider_name
    FROM s11_wu4_restore_session_ownership_preimage preimage
    WHERE preimage.entity_kind = 'segment'
      AND preimage.segment_id = session_chain_segments.id
  );

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
