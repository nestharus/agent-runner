-- One immutable no-effect admission for the exact retained executable plan.
-- This row is not a physical K or permission to execute a provider.
CREATE TABLE fresh_normal_provider_admission (
 handoff_id TEXT PRIMARY KEY REFERENCES fresh_normal_executable_plan(handoff_id),
 admission_json TEXT NOT NULL,
 admitted_at TEXT NOT NULL
);
CREATE TRIGGER fresh_normal_provider_admission_no_update BEFORE UPDATE ON fresh_normal_provider_admission
BEGIN SELECT RAISE(ABORT,'normal provider admission immutable'); END;
CREATE TRIGGER fresh_normal_provider_admission_no_delete BEFORE DELETE ON fresh_normal_provider_admission
BEGIN SELECT RAISE(ABORT,'normal provider admission immutable'); END;
