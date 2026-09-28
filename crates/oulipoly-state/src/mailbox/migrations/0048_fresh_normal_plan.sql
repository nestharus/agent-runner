-- One immutable, no-effect executable recipe for the exact selected account.
CREATE TABLE fresh_normal_executable_plan (
 handoff_id TEXT PRIMARY KEY REFERENCES fresh_normal_model_selection(handoff_id),
 plan_json TEXT NOT NULL,
 planned_at TEXT NOT NULL
);
CREATE TRIGGER fresh_normal_executable_plan_no_update BEFORE UPDATE ON fresh_normal_executable_plan
BEGIN SELECT RAISE(ABORT,'normal executable plan immutable'); END;
CREATE TRIGGER fresh_normal_executable_plan_no_delete BEFORE DELETE ON fresh_normal_executable_plan
BEGIN SELECT RAISE(ABORT,'normal executable plan immutable'); END;
