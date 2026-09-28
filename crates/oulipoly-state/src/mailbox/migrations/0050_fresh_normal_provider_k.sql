-- An admission can fund exactly one physical attempt. Commit this row before
-- a provider process can exist. A lost reply or failed launch is spent debt.
CREATE TABLE fresh_normal_provider_k (
 handoff_id TEXT PRIMARY KEY REFERENCES fresh_normal_provider_admission(handoff_id),
 k_json TEXT NOT NULL,
 consumed_at TEXT NOT NULL
);
CREATE TRIGGER fresh_normal_provider_k_no_update BEFORE UPDATE ON fresh_normal_provider_k
BEGIN SELECT RAISE(ABORT,'normal provider K immutable'); END;
CREATE TRIGGER fresh_normal_provider_k_no_delete BEFORE DELETE ON fresh_normal_provider_k
BEGIN SELECT RAISE(ABORT,'normal provider K immutable'); END;
