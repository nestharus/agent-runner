-- Keep the live recovery target on the pending row. A target lookup must not
-- walk unrelated pending duties while State owns a writer reservation.
ALTER TABLE completed_turns ADD COLUMN recovery_fallback_session TEXT;
ALTER TABLE completed_turns ADD COLUMN recovery_provider_name TEXT;
ALTER TABLE completed_turns ADD COLUMN recovery_provider_session TEXT;

UPDATE completed_turns
SET recovery_fallback_session = COALESCE(
    json_extract(context_json, '$.provider_session'),
    json_extract(effects_json, '$.session_id')
)
WHERE recovery_pending = 1;
UPDATE completed_turns
SET recovery_provider_name = (
        SELECT provider_name FROM invocations WHERE id = completed_turns.invocation_id
    ),
    recovery_provider_session = COALESCE(
        (SELECT provider_session_id FROM invocations WHERE id = completed_turns.invocation_id),
        (SELECT session_id FROM invocations WHERE id = completed_turns.invocation_id),
        recovery_fallback_session
    )
WHERE recovery_pending = 1;

CREATE INDEX completed_turns_recovery_target
    ON completed_turns(recovery_provider_name, recovery_provider_session, invocation_id)
    WHERE recovery_pending = 1;
CREATE INDEX completed_turns_recovery_session
    ON completed_turns(recovery_provider_session, invocation_id)
    WHERE recovery_pending = 1;

-- A newly admitted older invocation can land behind a running cursor. New
-- higher IDs and terminal mutations must not starve the current pass.
CREATE TABLE completed_turn_recovery_epoch (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    epoch INTEGER NOT NULL CHECK (epoch >= 0),
    max_admitted_id INTEGER NOT NULL CHECK (max_admitted_id >= 0)
);
INSERT INTO completed_turn_recovery_epoch
SELECT 1, 0, COALESCE(MAX(invocation_id), 0) FROM completed_turns;

CREATE TRIGGER completed_turn_recovery_epoch_insert
AFTER INSERT ON completed_turns
BEGIN
    UPDATE completed_turn_recovery_epoch
    SET epoch = epoch + CASE
            WHEN NEW.recovery_pending = 1 AND NEW.invocation_id <= max_admitted_id THEN 1
            ELSE 0
        END,
        max_admitted_id = MAX(max_admitted_id, NEW.invocation_id)
    WHERE singleton = 1;
END;

CREATE TRIGGER completed_turn_recovery_epoch_pending_update
AFTER UPDATE OF recovery_pending ON completed_turns
WHEN OLD.recovery_pending = 0 AND NEW.recovery_pending = 1
BEGIN
    UPDATE completed_turn_recovery_epoch SET epoch = epoch + 1 WHERE singleton = 1;
END;

CREATE TRIGGER completed_turn_recovery_target_insert
AFTER INSERT ON completed_turns
BEGIN
    UPDATE completed_turns
    SET recovery_fallback_session = COALESCE(
            json_extract(NEW.context_json, '$.provider_session'),
            json_extract(NEW.effects_json, '$.session_id')
        ),
        recovery_provider_name = (
            SELECT provider_name FROM invocations WHERE id = NEW.invocation_id
        ),
        recovery_provider_session = COALESCE(
            (SELECT provider_session_id FROM invocations WHERE id = NEW.invocation_id),
            (SELECT session_id FROM invocations WHERE id = NEW.invocation_id),
            json_extract(NEW.context_json, '$.provider_session'),
            json_extract(NEW.effects_json, '$.session_id')
        )
    WHERE invocation_id = NEW.invocation_id AND recovery_pending = 1;
    SELECT CASE WHEN NEW.recovery_pending = 1 AND EXISTS (
        SELECT 1 FROM completed_turns
        WHERE invocation_id = NEW.invocation_id
          AND (recovery_provider_name IS NULL OR recovery_provider_session IS NULL)
    ) THEN RAISE(ABORT, 'completed turn recovery target unavailable') END;
END;

CREATE TRIGGER completed_turn_recovery_target_invocation_update
AFTER UPDATE OF provider_name, provider_session_id, session_id ON invocations
BEGIN
    UPDATE completed_turns
    SET recovery_provider_name = NEW.provider_name,
        recovery_provider_session = COALESCE(
            NEW.provider_session_id, NEW.session_id, recovery_fallback_session
        )
    WHERE invocation_id = NEW.id AND recovery_pending = 1;
    SELECT CASE WHEN EXISTS (
        SELECT 1 FROM completed_turns
        WHERE invocation_id = NEW.id AND recovery_pending = 1
          AND (recovery_provider_name IS NULL OR recovery_provider_session IS NULL)
    ) THEN RAISE(ABORT, 'completed turn recovery target unavailable') END;
END;

CREATE TRIGGER completed_turn_recovery_target_payload_update
AFTER UPDATE OF context_json, effects_json ON completed_turns
WHEN NEW.recovery_pending = 1
BEGIN
    UPDATE completed_turns
    SET recovery_fallback_session = COALESCE(
            json_extract(NEW.context_json, '$.provider_session'),
            json_extract(NEW.effects_json, '$.session_id')
        ),
        recovery_provider_session = COALESCE(
            (SELECT provider_session_id FROM invocations WHERE id = NEW.invocation_id),
            (SELECT session_id FROM invocations WHERE id = NEW.invocation_id),
            json_extract(NEW.context_json, '$.provider_session'),
            json_extract(NEW.effects_json, '$.session_id')
        )
    WHERE invocation_id = NEW.invocation_id;
    SELECT CASE WHEN EXISTS (
        SELECT 1 FROM completed_turns
        WHERE invocation_id = NEW.invocation_id
          AND (recovery_provider_name IS NULL OR recovery_provider_session IS NULL)
    ) THEN RAISE(ABORT, 'completed turn recovery target unavailable') END;
END;
