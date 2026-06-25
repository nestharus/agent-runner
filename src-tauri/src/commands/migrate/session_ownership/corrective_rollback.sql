PRAGMA foreign_keys = ON;

BEGIN IMMEDIATE;

DROP TABLE IF EXISTS s11_m2c_model_corrective_last_rollback_plan;
CREATE TABLE s11_m2c_model_corrective_last_rollback_plan AS
SELECT
    preimage.chain_id,
    preimage.old_model_name,
    preimage.new_model_name
FROM s11_m2c_model_corrective_preimage preimage
JOIN session_chains chain ON chain.chain_id = preimage.chain_id
WHERE chain.model_name = preimage.new_model_name;

UPDATE session_chains
SET model_name = (
    SELECT plan.old_model_name
    FROM s11_m2c_model_corrective_last_rollback_plan plan
    WHERE plan.chain_id = session_chains.chain_id
)
WHERE chain_id IN (SELECT chain_id FROM s11_m2c_model_corrective_last_rollback_plan)
  AND model_name = (
    SELECT plan.new_model_name
    FROM s11_m2c_model_corrective_last_rollback_plan plan
    WHERE plan.chain_id = session_chains.chain_id
  );

DROP TABLE IF EXISTS s11_m2c_model_corrective_last_rollback_counts;
CREATE TABLE s11_m2c_model_corrective_last_rollback_counts (
    key TEXT PRIMARY KEY,
    value INTEGER NOT NULL
);

INSERT INTO s11_m2c_model_corrective_last_rollback_counts(key, value)
SELECT 'corrective_chain_model_updates_restored', COUNT(*)
FROM s11_m2c_model_corrective_last_rollback_plan plan
JOIN session_chains chain ON chain.chain_id = plan.chain_id
WHERE chain.model_name = plan.old_model_name;

INSERT INTO s11_m2c_model_corrective_last_rollback_counts(key, value)
SELECT 'corrective_chain_model_rollback_mismatches', COUNT(*)
FROM s11_m2c_model_corrective_preimage preimage
LEFT JOIN session_chains chain ON chain.chain_id = preimage.chain_id
WHERE chain.chain_id IS NULL
   OR chain.model_name <> preimage.old_model_name;

COMMIT;
