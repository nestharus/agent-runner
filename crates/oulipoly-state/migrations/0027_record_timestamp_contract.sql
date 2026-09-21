-- AGE-371: authoritative UTC lifecycle timestamps and explicit retention
-- eligibility for State-owned historical roots.  A NULL eligibility time is
-- intentional: terminal state alone is not evidence that a legacy row is old
-- enough (or safe enough) to retire.

CREATE TABLE record_timestamp_repairs (
    repair_id TEXT PRIMARY KEY,
    record_family TEXT NOT NULL CHECK (record_family IN (
        'invocation',
        'provider_logical_launch',
        'provider_launch_attempt',
        'completed_turn'
    )),
    record_key TEXT NOT NULL,
    field_name TEXT NOT NULL CHECK (field_name IN ('finished_at', 'closed_at')),
    old_value TEXT,
    new_value TEXT NOT NULL,
    actor TEXT NOT NULL CHECK (trim(actor) <> ''),
    reason TEXT NOT NULL CHECK (trim(reason) <> ''),
    repaired_at TEXT NOT NULL
);

CREATE INDEX idx_record_timestamp_repairs_record
    ON record_timestamp_repairs(record_family, record_key, repaired_at);

CREATE TRIGGER record_timestamp_repairs_append_only_update
BEFORE UPDATE ON record_timestamp_repairs
BEGIN
    SELECT RAISE(ABORT, 'record timestamp repair audit is append-only');
END;

CREATE TRIGGER record_timestamp_repairs_append_only_delete
BEFORE DELETE ON record_timestamp_repairs
BEGIN
    SELECT RAISE(ABORT, 'record timestamp repair audit is append-only');
END;

ALTER TABLE invocations ADD COLUMN lifecycle_updated_at TEXT;
ALTER TABLE invocations ADD COLUMN retention_eligible_at TEXT;
ALTER TABLE invocations ADD COLUMN retention_status TEXT NOT NULL DEFAULT 'legacy_unknown'
    CHECK (retention_status IN ('pending', 'eligible', 'legacy_unknown', 'clock_anomaly'));

UPDATE invocations
SET lifecycle_updated_at = created_at,
    retention_status = 'pending'
WHERE status = 'running';

-- The versionless rebuild historically copied created_at into finished_at.
-- Equality therefore is not strong closure evidence.  Preserve the bytes, but
-- keep those rows (and all explicit legacy rows) ineligible.
UPDATE invocations
SET lifecycle_updated_at = finished_at,
    retention_eligible_at = CASE
        WHEN status IN ('succeeded', 'failed')
         AND finished_at IS NOT NULL
         AND finished_at <> created_at
         AND julianday(finished_at) >= julianday(created_at)
        THEN finished_at
        ELSE NULL
    END,
    retention_status = CASE
        WHEN status IN ('succeeded', 'failed')
         AND finished_at IS NOT NULL
         AND finished_at <> created_at
         AND julianday(finished_at) < julianday(created_at)
        THEN 'clock_anomaly'
        WHEN status IN ('succeeded', 'failed')
         AND finished_at IS NOT NULL
         AND finished_at <> created_at
         AND julianday(finished_at) >= julianday(created_at)
        THEN 'eligible'
        ELSE 'legacy_unknown'
    END
WHERE status <> 'running';

CREATE INDEX idx_invocations_retention_eligible
    ON invocations(retention_eligible_at, id)
    WHERE retention_status = 'eligible' AND retention_eligible_at IS NOT NULL;

CREATE TRIGGER invocations_timestamp_after_insert
AFTER INSERT ON invocations
BEGIN
    UPDATE invocations
    SET lifecycle_updated_at = CASE
            WHEN NEW.status = 'running' THEN NEW.created_at ELSE NEW.finished_at END,
        retention_eligible_at = CASE
            WHEN NEW.status IN ('succeeded', 'failed')
             AND NEW.finished_at IS NOT NULL
             AND julianday(NEW.finished_at) >= julianday(NEW.created_at)
            THEN NEW.finished_at ELSE NULL END,
        retention_status = CASE
            WHEN NEW.status = 'running' THEN 'pending'
            WHEN NEW.status IN ('succeeded', 'failed')
             AND NEW.finished_at IS NOT NULL
             AND julianday(NEW.finished_at) < julianday(NEW.created_at)
            THEN 'clock_anomaly'
            WHEN NEW.status IN ('succeeded', 'failed')
             AND NEW.finished_at IS NOT NULL
             AND julianday(NEW.finished_at) >= julianday(NEW.created_at)
            THEN 'eligible'
            ELSE 'legacy_unknown' END
    WHERE id = NEW.id;
END;

CREATE TRIGGER invocations_timestamp_after_terminal
AFTER UPDATE OF status, finished_at ON invocations
WHEN NEW.status IN ('succeeded', 'failed', 'legacy')
 AND (OLD.status IS NOT NEW.status OR OLD.finished_at IS NOT NEW.finished_at)
BEGIN
    UPDATE invocations
    SET lifecycle_updated_at = NEW.finished_at,
        retention_eligible_at = CASE
            WHEN NEW.status IN ('succeeded', 'failed')
             AND NEW.finished_at IS NOT NULL
             AND julianday(NEW.finished_at) >= julianday(NEW.created_at)
            THEN NEW.finished_at ELSE NULL END,
        retention_status = CASE
            WHEN NEW.status IN ('succeeded', 'failed')
             AND NEW.finished_at IS NOT NULL
             AND julianday(NEW.finished_at) < julianday(NEW.created_at)
            THEN 'clock_anomaly'
            WHEN NEW.status IN ('succeeded', 'failed')
             AND NEW.finished_at IS NOT NULL
             AND julianday(NEW.finished_at) >= julianday(NEW.created_at)
            THEN 'eligible'
            ELSE 'legacy_unknown' END
    WHERE id = NEW.id;
END;

CREATE TRIGGER invocations_created_at_immutable
BEFORE UPDATE OF created_at ON invocations
WHEN NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'invocation creation timestamp is immutable');
END;

CREATE TRIGGER invocations_finished_at_immutable
BEFORE UPDATE OF finished_at ON invocations
WHEN OLD.finished_at IS NOT NULL
 AND NEW.finished_at IS NOT OLD.finished_at
 AND NOT EXISTS (
     SELECT 1 FROM record_timestamp_repairs repair
     WHERE repair.record_family = 'invocation'
       AND repair.record_key = OLD.invocation_uuid
       AND repair.field_name = 'finished_at'
       AND repair.old_value IS OLD.finished_at
       AND repair.new_value = NEW.finished_at
       AND NOT EXISTS (
           SELECT 1 FROM record_timestamp_repairs newer
           WHERE newer.record_family = repair.record_family
             AND newer.record_key = repair.record_key
             AND newer.field_name = repair.field_name
             AND newer.rowid > repair.rowid
       )
 )
BEGIN
    SELECT RAISE(ABORT, 'invocation terminal timestamp is immutable');
END;

CREATE TRIGGER invocations_terminal_reopen_forbidden
BEFORE UPDATE OF status ON invocations
WHEN OLD.status IN ('succeeded', 'failed', 'legacy')
 AND NEW.status IS NOT OLD.status
BEGIN
    SELECT RAISE(ABORT, 'invocation terminal state cannot reopen');
END;

ALTER TABLE provider_logical_launches ADD COLUMN retention_eligible_at TEXT;
ALTER TABLE provider_logical_launches ADD COLUMN retention_status TEXT NOT NULL DEFAULT 'pending'
    CHECK (retention_status IN ('pending', 'blocked', 'eligible', 'legacy_unknown', 'clock_anomaly'));

UPDATE provider_logical_launches
SET retention_eligible_at = CASE
        WHEN status IN ('succeeded', 'failed', 'cancelled')
         AND finished_at IS NOT NULL
         AND julianday(finished_at) >= julianday(created_at)
        THEN finished_at ELSE NULL END,
    retention_status = CASE
        WHEN status = 'recovery_blocked' THEN 'blocked'
        WHEN status IN ('succeeded', 'failed', 'cancelled') AND finished_at IS NULL
        THEN 'legacy_unknown'
        WHEN status IN ('succeeded', 'failed', 'cancelled')
         AND (julianday(finished_at) IS NULL OR julianday(created_at) IS NULL)
        THEN 'legacy_unknown'
        WHEN status IN ('succeeded', 'failed', 'cancelled')
         AND julianday(finished_at) < julianday(created_at)
        THEN 'clock_anomaly'
        WHEN status IN ('succeeded', 'failed', 'cancelled')
        THEN 'eligible'
        ELSE 'pending' END;

CREATE INDEX provider_logical_launch_retention_eligible
    ON provider_logical_launches(retention_eligible_at, logical_launch_id)
    WHERE retention_status = 'eligible' AND retention_eligible_at IS NOT NULL;

CREATE TRIGGER provider_logical_launch_finished_timestamp
AFTER UPDATE OF finished_at ON provider_logical_launches
WHEN OLD.finished_at IS NULL AND NEW.finished_at IS NOT NULL
BEGIN
    UPDATE provider_logical_launches
    SET retention_eligible_at = CASE
            WHEN julianday(NEW.finished_at) >= julianday(NEW.created_at)
            THEN NEW.finished_at ELSE NULL END,
        retention_status = CASE
            WHEN julianday(NEW.finished_at) IS NULL OR julianday(NEW.created_at) IS NULL
            THEN 'legacy_unknown'
            WHEN julianday(NEW.finished_at) < julianday(NEW.created_at)
            THEN 'clock_anomaly' ELSE 'eligible' END
    WHERE logical_launch_id = NEW.logical_launch_id;
END;

CREATE TRIGGER provider_logical_launch_recovery_blocked_timestamp
AFTER UPDATE OF status ON provider_logical_launches
WHEN NEW.status = 'recovery_blocked' AND OLD.status IS NOT NEW.status
BEGIN
    UPDATE provider_logical_launches
    SET retention_eligible_at = NULL, retention_status = 'blocked'
    WHERE logical_launch_id = NEW.logical_launch_id;
END;

CREATE TRIGGER provider_logical_launch_created_at_immutable
BEFORE UPDATE OF created_at ON provider_logical_launches
WHEN NEW.created_at IS NOT OLD.created_at
BEGIN SELECT RAISE(ABORT, 'provider logical launch creation timestamp is immutable'); END;

CREATE TRIGGER provider_logical_launch_finished_at_immutable
BEFORE UPDATE OF finished_at ON provider_logical_launches
WHEN OLD.finished_at IS NOT NULL
 AND NEW.finished_at IS NOT OLD.finished_at
 AND NOT EXISTS (
     SELECT 1 FROM record_timestamp_repairs repair
     WHERE repair.record_family = 'provider_logical_launch'
       AND repair.record_key = OLD.logical_launch_id
       AND repair.field_name = 'finished_at'
       AND repair.old_value IS OLD.finished_at
       AND repair.new_value = NEW.finished_at
       AND NOT EXISTS (
           SELECT 1 FROM record_timestamp_repairs newer
           WHERE newer.record_family = repair.record_family
             AND newer.record_key = repair.record_key
             AND newer.field_name = repair.field_name
             AND newer.rowid > repair.rowid
       )
 )
BEGIN SELECT RAISE(ABORT, 'provider logical launch terminal timestamp is immutable'); END;

CREATE TRIGGER provider_logical_launch_terminal_reopen_forbidden
BEFORE UPDATE OF status ON provider_logical_launches
WHEN OLD.status IN ('succeeded', 'failed', 'cancelled')
 AND NEW.status IS NOT OLD.status
BEGIN SELECT RAISE(ABORT, 'provider logical launch terminal state cannot reopen'); END;

ALTER TABLE provider_launch_attempts ADD COLUMN retention_eligible_at TEXT;
ALTER TABLE provider_launch_attempts ADD COLUMN retention_status TEXT NOT NULL DEFAULT 'pending'
    CHECK (retention_status IN ('pending', 'blocked', 'eligible', 'legacy_unknown', 'clock_anomaly'));

UPDATE provider_launch_attempts
SET retention_eligible_at = CASE
        WHEN status IN ('superseded', 'succeeded', 'failed', 'cancelled')
         AND finished_at IS NOT NULL
         AND julianday(finished_at) >= julianday(created_at)
        THEN finished_at ELSE NULL END,
    retention_status = CASE
        WHEN status = 'recovery_blocked' THEN 'blocked'
        WHEN status IN ('superseded', 'succeeded', 'failed', 'cancelled') AND finished_at IS NULL
        THEN 'legacy_unknown'
        WHEN status IN ('superseded', 'succeeded', 'failed', 'cancelled')
         AND (julianday(finished_at) IS NULL OR julianday(created_at) IS NULL)
        THEN 'legacy_unknown'
        WHEN status IN ('superseded', 'succeeded', 'failed', 'cancelled')
         AND julianday(finished_at) < julianday(created_at)
        THEN 'clock_anomaly'
        WHEN status IN ('superseded', 'succeeded', 'failed', 'cancelled')
        THEN 'eligible'
        ELSE 'pending' END;

CREATE INDEX provider_launch_attempt_retention_eligible
    ON provider_launch_attempts(retention_eligible_at, attempt_id)
    WHERE retention_status = 'eligible' AND retention_eligible_at IS NOT NULL;

CREATE TRIGGER provider_launch_attempt_finished_timestamp
AFTER UPDATE OF finished_at ON provider_launch_attempts
WHEN OLD.finished_at IS NULL AND NEW.finished_at IS NOT NULL
BEGIN
    UPDATE provider_launch_attempts
    SET retention_eligible_at = CASE
            WHEN julianday(NEW.finished_at) >= julianday(NEW.created_at)
            THEN NEW.finished_at ELSE NULL END,
        retention_status = CASE
            WHEN julianday(NEW.finished_at) IS NULL OR julianday(NEW.created_at) IS NULL
            THEN 'legacy_unknown'
            WHEN julianday(NEW.finished_at) < julianday(NEW.created_at)
            THEN 'clock_anomaly' ELSE 'eligible' END
    WHERE attempt_id = NEW.attempt_id;
END;

CREATE TRIGGER provider_launch_attempt_recovery_blocked_timestamp
AFTER UPDATE OF status ON provider_launch_attempts
WHEN NEW.status = 'recovery_blocked' AND OLD.status IS NOT NEW.status
BEGIN
    UPDATE provider_launch_attempts
    SET retention_eligible_at = NULL, retention_status = 'blocked'
    WHERE attempt_id = NEW.attempt_id;
END;

CREATE TRIGGER provider_launch_attempt_created_at_immutable
BEFORE UPDATE OF created_at ON provider_launch_attempts
WHEN NEW.created_at IS NOT OLD.created_at
BEGIN SELECT RAISE(ABORT, 'provider launch attempt creation timestamp is immutable'); END;

CREATE TRIGGER provider_launch_attempt_finished_at_immutable
BEFORE UPDATE OF finished_at ON provider_launch_attempts
WHEN OLD.finished_at IS NOT NULL
 AND NEW.finished_at IS NOT OLD.finished_at
 AND NOT EXISTS (
     SELECT 1 FROM record_timestamp_repairs repair
     WHERE repair.record_family = 'provider_launch_attempt'
       AND repair.record_key = OLD.attempt_id
       AND repair.field_name = 'finished_at'
       AND repair.old_value IS OLD.finished_at
       AND repair.new_value = NEW.finished_at
       AND NOT EXISTS (
           SELECT 1 FROM record_timestamp_repairs newer
           WHERE newer.record_family = repair.record_family
             AND newer.record_key = repair.record_key
             AND newer.field_name = repair.field_name
             AND newer.rowid > repair.rowid
       )
 )
BEGIN SELECT RAISE(ABORT, 'provider launch attempt terminal timestamp is immutable'); END;

CREATE TRIGGER provider_launch_attempt_terminal_reopen_forbidden
BEFORE UPDATE OF status ON provider_launch_attempts
WHEN OLD.status IN ('superseded', 'succeeded', 'failed', 'cancelled')
 AND NEW.status IS NOT OLD.status
BEGIN SELECT RAISE(ABORT, 'provider launch attempt terminal state cannot reopen'); END;

ALTER TABLE provider_launch_transition_replays ADD COLUMN recorded_at TEXT;
ALTER TABLE provider_launch_transition_replays ADD COLUMN retention_status TEXT NOT NULL DEFAULT 'legacy_unknown'
    CHECK (retention_status IN ('inherits_parent', 'legacy_unknown'));

CREATE TRIGGER provider_launch_transition_replay_recorded_at_immutable
BEFORE UPDATE OF recorded_at ON provider_launch_transition_replays
WHEN NEW.recorded_at IS NOT OLD.recorded_at
BEGIN SELECT RAISE(ABORT, 'provider launch replay occurrence timestamp is immutable'); END;

ALTER TABLE completed_turns ADD COLUMN created_at TEXT;
ALTER TABLE completed_turns ADD COLUMN updated_at TEXT;
ALTER TABLE completed_turns ADD COLUMN closed_at TEXT;
ALTER TABLE completed_turns ADD COLUMN retention_eligible_at TEXT;
ALTER TABLE completed_turns ADD COLUMN retention_status TEXT NOT NULL DEFAULT 'legacy_unknown'
    CHECK (retention_status IN ('pending', 'eligible', 'legacy_unknown', 'clock_anomaly'));

UPDATE completed_turns
SET retention_status = CASE WHEN recovery_pending = 1 THEN 'pending' ELSE 'legacy_unknown' END;

CREATE INDEX completed_turns_retention_eligible
    ON completed_turns(retention_eligible_at, invocation_id)
    WHERE retention_status = 'eligible' AND retention_eligible_at IS NOT NULL;

CREATE TRIGGER completed_turn_created_at_immutable
BEFORE UPDATE OF created_at ON completed_turns
WHEN NEW.created_at IS NOT OLD.created_at
BEGIN SELECT RAISE(ABORT, 'completed turn creation timestamp is immutable'); END;

CREATE TRIGGER completed_turn_closed_at_immutable
BEFORE UPDATE OF closed_at ON completed_turns
WHEN OLD.closed_at IS NOT NULL
 AND NEW.closed_at IS NOT OLD.closed_at
 AND NOT EXISTS (
     SELECT 1 FROM record_timestamp_repairs repair
     WHERE repair.record_family = 'completed_turn'
       AND repair.record_key = OLD.invocation_uuid
       AND repair.field_name = 'closed_at'
       AND repair.old_value IS OLD.closed_at
       AND repair.new_value = NEW.closed_at
       AND NOT EXISTS (
           SELECT 1 FROM record_timestamp_repairs newer
           WHERE newer.record_family = repair.record_family
             AND newer.record_key = repair.record_key
             AND newer.field_name = repair.field_name
             AND newer.rowid > repair.rowid
       )
 )
BEGIN SELECT RAISE(ABORT, 'completed turn terminal timestamp is immutable'); END;

CREATE TRIGGER completed_turn_terminal_reopen_forbidden
BEFORE UPDATE OF recovery_pending ON completed_turns
WHEN OLD.recovery_pending = 0 AND NEW.recovery_pending = 1
BEGIN SELECT RAISE(ABORT, 'completed turn terminal state cannot reopen'); END;
