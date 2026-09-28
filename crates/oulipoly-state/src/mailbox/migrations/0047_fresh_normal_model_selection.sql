-- One immutable, no-effect model choice for an already held normal root.
-- The broker must recheck source bytes and the live actor on every readback.
CREATE TABLE fresh_normal_model_selection (
 handoff_id TEXT PRIMARY KEY REFERENCES fresh_normal_work_preparation(handoff_id),
 selection_json TEXT NOT NULL,
 selected_at TEXT NOT NULL
);
CREATE TRIGGER fresh_normal_model_selection_no_update BEFORE UPDATE ON fresh_normal_model_selection
BEGIN SELECT RAISE(ABORT,'normal model selection immutable'); END;
CREATE TRIGGER fresh_normal_model_selection_no_delete BEFORE DELETE ON fresh_normal_model_selection
BEGIN SELECT RAISE(ABORT,'normal model selection immutable'); END;
