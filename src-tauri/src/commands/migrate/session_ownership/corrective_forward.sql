PRAGMA foreign_keys = ON;

BEGIN IMMEDIATE;

CREATE TABLE IF NOT EXISTS s11_m2c_model_corrective_preimage (
    chain_id TEXT PRIMARY KEY,
    old_model_name TEXT NOT NULL,
    new_model_name TEXT NOT NULL,
    evidence_source TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

DROP TABLE IF EXISTS s11_m2c_model_corrective_last_run_plan;
CREATE TABLE s11_m2c_model_corrective_last_run_plan AS
SELECT
    plan.chain_id,
    plan.old_model_name,
    plan.new_model_name,
    plan.evidence_source
FROM s11_m2c_model_corrective_plan plan
JOIN session_chains chain ON chain.chain_id = plan.chain_id
WHERE chain.model_name = plan.old_model_name
  AND plan.old_model_name <> plan.new_model_name;

INSERT OR IGNORE INTO s11_m2c_model_corrective_preimage (
    chain_id,
    old_model_name,
    new_model_name,
    evidence_source
)
SELECT
    chain_id,
    old_model_name,
    new_model_name,
    evidence_source
FROM s11_m2c_model_corrective_last_run_plan;

UPDATE session_chains
SET model_name = (
    SELECT plan.new_model_name
    FROM s11_m2c_model_corrective_last_run_plan plan
    WHERE plan.chain_id = session_chains.chain_id
)
WHERE chain_id IN (SELECT chain_id FROM s11_m2c_model_corrective_last_run_plan)
  AND model_name = (
    SELECT plan.old_model_name
    FROM s11_m2c_model_corrective_last_run_plan plan
    WHERE plan.chain_id = session_chains.chain_id
  );

DROP TABLE IF EXISTS s11_m2c_model_corrective_last_run_counts;
CREATE TABLE s11_m2c_model_corrective_last_run_counts (
    key TEXT PRIMARY KEY,
    value INTEGER NOT NULL
);

INSERT INTO s11_m2c_model_corrective_last_run_counts(key, value)
SELECT 'corrective_chain_model_updates_to_apply', COUNT(*)
FROM s11_m2c_model_corrective_last_run_plan;

INSERT INTO s11_m2c_model_corrective_last_run_counts(key, value)
SELECT 'corrective_chain_model_updates_applied', COUNT(*)
FROM s11_m2c_model_corrective_last_run_plan plan
JOIN session_chains chain ON chain.chain_id = plan.chain_id
WHERE chain.model_name = plan.new_model_name;

INSERT INTO s11_m2c_model_corrective_last_run_counts(key, value)
SELECT 'corrective_preimage_rows', COUNT(*)
FROM s11_m2c_model_corrective_preimage;

INSERT OR REPLACE INTO s11_m2c_model_corrective_last_run_counts(key, value)
SELECT 'new_model_name ' || new_model_name, COUNT(*)
FROM s11_m2c_model_corrective_preimage
GROUP BY new_model_name;

INSERT OR REPLACE INTO s11_m2c_model_corrective_last_run_counts(key, value)
SELECT 'evidence_source ' || evidence_source, COUNT(*)
FROM s11_m2c_model_corrective_preimage
GROUP BY evidence_source;

COMMIT;
