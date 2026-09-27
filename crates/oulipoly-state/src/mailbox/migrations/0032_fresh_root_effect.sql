-- A start is durable before the released root may perform an entry effect.
-- A missing return remains unknown; it is never inferred from process drain.
CREATE TABLE fresh_root_effect (
 handoff_id TEXT PRIMARY KEY REFERENCES fresh_released_handoff(handoff_id),
 invocation_uuid TEXT NOT NULL UNIQUE,
 session_id TEXT NOT NULL UNIQUE,
 actor_identity TEXT NOT NULL,
 intent_json TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('started','returned_success','returned_failure')),
 started_at TEXT NOT NULL,
 returned_at TEXT
);
CREATE TRIGGER fresh_root_effect_no_delete BEFORE DELETE ON fresh_root_effect
BEGIN SELECT RAISE(ABORT,'root effect immutable'); END;
CREATE TRIGGER fresh_root_effect_return_once BEFORE UPDATE ON fresh_root_effect
WHEN OLD.state!='started' OR NEW.state='started' OR NEW.handoff_id!=OLD.handoff_id
 OR NEW.invocation_uuid!=OLD.invocation_uuid OR NEW.session_id!=OLD.session_id
 OR NEW.actor_identity!=OLD.actor_identity OR NEW.intent_json!=OLD.intent_json
 OR NEW.started_at!=OLD.started_at OR NEW.returned_at IS NULL
BEGIN SELECT RAISE(ABORT,'root effect return transition invalid'); END;
