-- One original root H authority can be delegated to one admitted Bash child.
-- The selected provider K and its process identity are frozen before the
-- child may consume the authority. A lost consume reply remains spent.
CREATE TABLE fresh_root_h_delegation (
 handoff_id TEXT PRIMARY KEY REFERENCES fresh_released_handoff(handoff_id),
 child_request_id TEXT NOT NULL UNIQUE REFERENCES fresh_bash_child(request_id),
 receipt_json TEXT NOT NULL,
 issued_at TEXT NOT NULL
);
CREATE TRIGGER fresh_root_h_delegation_no_update BEFORE UPDATE ON fresh_root_h_delegation
BEGIN SELECT RAISE(ABORT,'root H delegation immutable'); END;
CREATE TRIGGER fresh_root_h_delegation_no_delete BEFORE DELETE ON fresh_root_h_delegation
BEGIN SELECT RAISE(ABORT,'root H delegation retained'); END;
CREATE TABLE fresh_root_h_consumption (
 handoff_id TEXT PRIMARY KEY REFERENCES fresh_root_h_delegation(handoff_id),
 child_request_id TEXT NOT NULL UNIQUE,
 consumed_at TEXT NOT NULL
);
CREATE TRIGGER fresh_root_h_consumption_no_update BEFORE UPDATE ON fresh_root_h_consumption
BEGIN SELECT RAISE(ABORT,'root H consumption immutable'); END;
CREATE TRIGGER fresh_root_h_consumption_no_delete BEFORE DELETE ON fresh_root_h_consumption
BEGIN SELECT RAISE(ABORT,'root H consumption retained'); END;
