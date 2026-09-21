-- AGE-371: timestamp/retention contract for PID-sidecar historical roots.
-- Legacy rows without an authoritative transition instant stay explicitly
-- ineligible.  `blocked` means the row has a terminal observation but a
-- retained dependency still owns its physical lifetime.

CREATE TABLE sidecar_timestamp_repairs (
    repair_id TEXT PRIMARY KEY,
    record_family TEXT NOT NULL CHECK(record_family IN (
        'mailbox', 'mailbox_delivery_attempt', 'completion_event',
        'completion_event_listener', 'runtime_generation'
    )),
    record_key TEXT NOT NULL,
    field_name TEXT NOT NULL,
    old_value TEXT,
    new_value TEXT NOT NULL,
    actor TEXT NOT NULL CHECK(trim(actor) <> ''),
    reason TEXT NOT NULL CHECK(trim(reason) <> ''),
    repaired_at TEXT NOT NULL
);
CREATE INDEX idx_sidecar_timestamp_repairs_record
    ON sidecar_timestamp_repairs(record_family, record_key, repaired_at);
CREATE TRIGGER sidecar_timestamp_repairs_append_only_update
BEFORE UPDATE ON sidecar_timestamp_repairs
BEGIN SELECT RAISE(ABORT, 'sidecar timestamp repair audit is append-only'); END;
CREATE TRIGGER sidecar_timestamp_repairs_append_only_delete
BEFORE DELETE ON sidecar_timestamp_repairs
BEGIN SELECT RAISE(ABORT, 'sidecar timestamp repair audit is append-only'); END;

ALTER TABLE mailbox ADD COLUMN closed_at TEXT;
ALTER TABLE mailbox ADD COLUMN retention_eligible_at TEXT;
ALTER TABLE mailbox ADD COLUMN retention_status TEXT NOT NULL DEFAULT 'legacy_unknown'
    CHECK(retention_status IN ('pending','eligible','legacy_unknown','clock_anomaly'));
UPDATE mailbox
SET closed_at = delivered_at,
    retention_eligible_at = CASE
        WHEN delivered_at IS NOT NULL AND julianday(delivered_at) >= julianday(enqueued_at)
        THEN delivered_at ELSE NULL END,
    retention_status = CASE
        WHEN delivered_at IS NOT NULL
         AND (julianday(delivered_at) IS NULL OR julianday(enqueued_at) IS NULL)
        THEN 'legacy_unknown'
        WHEN delivered_at IS NOT NULL AND julianday(delivered_at) < julianday(enqueued_at)
        THEN 'clock_anomaly'
        WHEN delivered_at IS NOT NULL THEN 'eligible'
        WHEN delivery_error IN ('wake_sweep_abandoned','mailbox_payload_verification_failed','mailbox_ingress_expired')
        THEN 'legacy_unknown'
        ELSE 'pending' END;
CREATE INDEX idx_mailbox_retention_eligible_v23 ON mailbox(retention_eligible_at, seq)
    WHERE retention_status='eligible' AND retention_eligible_at IS NOT NULL;
CREATE TRIGGER mailbox_timestamp_after_insert
AFTER INSERT ON mailbox
BEGIN
    UPDATE mailbox
    SET closed_at=NEW.delivered_at,
        retention_eligible_at=CASE
          WHEN NEW.delivered_at IS NOT NULL
           AND julianday(NEW.delivered_at)>=julianday(NEW.enqueued_at)
          THEN NEW.delivered_at ELSE NULL END,
        retention_status=CASE
          WHEN NEW.delivered_at IS NULL THEN 'pending'
          WHEN julianday(NEW.delivered_at) IS NULL OR julianday(NEW.enqueued_at) IS NULL THEN 'legacy_unknown'
          WHEN julianday(NEW.delivered_at)<julianday(NEW.enqueued_at) THEN 'clock_anomaly'
          ELSE 'eligible' END
    WHERE seq=NEW.seq;
END;
CREATE TRIGGER mailbox_timestamp_after_delivery
AFTER UPDATE OF delivered_at ON mailbox
WHEN OLD.delivered_at IS NULL AND NEW.delivered_at IS NOT NULL
BEGIN
    UPDATE mailbox
    SET closed_at=NEW.delivered_at,
        retention_eligible_at=CASE WHEN julianday(NEW.delivered_at)>=julianday(NEW.enqueued_at) THEN NEW.delivered_at ELSE NULL END,
        retention_status=CASE
          WHEN julianday(NEW.delivered_at) IS NULL OR julianday(NEW.enqueued_at) IS NULL THEN 'legacy_unknown'
          WHEN julianday(NEW.delivered_at)<julianday(NEW.enqueued_at) THEN 'clock_anomaly' ELSE 'eligible' END
    WHERE seq=NEW.seq;
END;
CREATE TRIGGER mailbox_timestamp_after_terminal_error
AFTER UPDATE OF delivery_error ON mailbox
WHEN NEW.delivered_at IS NULL
 AND NEW.delivery_error IN ('wake_sweep_abandoned','mailbox_payload_verification_failed','mailbox_ingress_expired')
 AND (OLD.delivery_error IS NULL OR OLD.delivery_error NOT IN ('wake_sweep_abandoned','mailbox_payload_verification_failed','mailbox_ingress_expired'))
BEGIN
    UPDATE mailbox
    SET closed_at=strftime('%Y-%m-%dT%H:%M:%fZ','now'),
        retention_eligible_at=CASE
            WHEN julianday('now')>=julianday(NEW.enqueued_at) THEN strftime('%Y-%m-%dT%H:%M:%fZ','now') ELSE NULL END,
        retention_status=CASE WHEN julianday(NEW.enqueued_at) IS NULL THEN 'legacy_unknown'
          WHEN julianday('now')<julianday(NEW.enqueued_at) THEN 'clock_anomaly' ELSE 'eligible' END
    WHERE seq=NEW.seq;
END;
CREATE TRIGGER mailbox_enqueued_at_immutable BEFORE UPDATE OF enqueued_at ON mailbox
WHEN NEW.enqueued_at IS NOT OLD.enqueued_at
BEGIN SELECT RAISE(ABORT,'mailbox creation timestamp is immutable'); END;
CREATE TRIGGER mailbox_closed_at_immutable BEFORE UPDATE OF closed_at ON mailbox
WHEN OLD.closed_at IS NOT NULL AND NEW.closed_at IS NOT OLD.closed_at
 AND NOT EXISTS(SELECT 1 FROM sidecar_timestamp_repairs r
   WHERE r.record_family='mailbox' AND r.record_key=CAST(OLD.seq AS TEXT)
     AND r.field_name='closed_at' AND r.old_value IS OLD.closed_at AND r.new_value=NEW.closed_at
     AND NOT EXISTS(SELECT 1 FROM sidecar_timestamp_repairs newer
       WHERE newer.record_family=r.record_family AND newer.record_key=r.record_key
         AND newer.field_name=r.field_name AND newer.rowid>r.rowid))
BEGIN SELECT RAISE(ABORT,'mailbox terminal timestamp is immutable'); END;
CREATE TRIGGER mailbox_delivered_at_immutable BEFORE UPDATE OF delivered_at ON mailbox
WHEN OLD.delivered_at IS NOT NULL AND NEW.delivered_at IS NOT OLD.delivered_at
BEGIN SELECT RAISE(ABORT,'mailbox delivery timestamp is immutable'); END;

ALTER TABLE mailbox_delivery_attempts ADD COLUMN updated_at TEXT;
ALTER TABLE mailbox_delivery_attempts ADD COLUMN retention_eligible_at TEXT;
ALTER TABLE mailbox_delivery_attempts ADD COLUMN retention_status TEXT NOT NULL DEFAULT 'legacy_unknown'
    CHECK(retention_status IN ('pending','eligible','legacy_unknown','clock_anomaly'));
UPDATE mailbox_delivery_attempts
SET updated_at=COALESCE(resolved_at, acknowledged_at, submission_started_at, created_at),
    retention_eligible_at=CASE WHEN resolved_at IS NOT NULL AND julianday(resolved_at)>=julianday(created_at) THEN resolved_at ELSE NULL END,
    retention_status=CASE
      WHEN resolved_at IS NULL THEN 'pending'
      WHEN julianday(resolved_at) IS NULL OR julianday(created_at) IS NULL THEN 'legacy_unknown'
      WHEN julianday(resolved_at)<julianday(created_at) THEN 'clock_anomaly'
      ELSE 'eligible' END;
CREATE INDEX idx_mailbox_delivery_attempt_retention_v23
    ON mailbox_delivery_attempts(retention_eligible_at, attempt_id)
    WHERE retention_status='eligible' AND retention_eligible_at IS NOT NULL;
CREATE TRIGGER mailbox_delivery_attempt_timestamp_after_insert
AFTER INSERT ON mailbox_delivery_attempts
BEGIN
  UPDATE mailbox_delivery_attempts
  SET updated_at=COALESCE(NEW.resolved_at,NEW.observation_confirmed_at,NEW.evidence_reconciled_at,
                          NEW.acknowledged_at,NEW.submission_started_at,NEW.created_at),
      retention_eligible_at=CASE
        WHEN NEW.resolved_at IS NOT NULL AND julianday(NEW.resolved_at)>=julianday(NEW.created_at)
        THEN NEW.resolved_at ELSE NULL END,
      retention_status=CASE
        WHEN NEW.resolved_at IS NULL THEN 'pending'
        WHEN julianday(NEW.resolved_at) IS NULL OR julianday(NEW.created_at) IS NULL THEN 'legacy_unknown'
        WHEN julianday(NEW.resolved_at)<julianday(NEW.created_at) THEN 'clock_anomaly'
        ELSE 'eligible' END
  WHERE attempt_id=NEW.attempt_id;
END;
CREATE TRIGGER mailbox_delivery_attempt_timestamp_after_update
AFTER UPDATE OF submission_started_at, acknowledged_at, evidence_reconciled_at,
    observation_confirmed_at, resolved_at ON mailbox_delivery_attempts
BEGIN
  UPDATE mailbox_delivery_attempts
  SET updated_at=COALESCE(NEW.resolved_at,NEW.observation_confirmed_at,NEW.evidence_reconciled_at,NEW.acknowledged_at,NEW.submission_started_at,OLD.updated_at,NEW.created_at),
      retention_eligible_at=CASE WHEN NEW.resolved_at IS NOT NULL AND julianday(NEW.resolved_at)>=julianday(NEW.created_at) THEN NEW.resolved_at ELSE NULL END,
      retention_status=CASE WHEN NEW.resolved_at IS NULL THEN 'pending'
        WHEN julianday(NEW.resolved_at) IS NULL OR julianday(NEW.created_at) IS NULL THEN 'legacy_unknown'
        WHEN julianday(NEW.resolved_at)<julianday(NEW.created_at) THEN 'clock_anomaly' ELSE 'eligible' END
  WHERE attempt_id=NEW.attempt_id;
END;
CREATE TRIGGER mailbox_delivery_attempt_created_at_immutable BEFORE UPDATE OF created_at ON mailbox_delivery_attempts
WHEN NEW.created_at IS NOT OLD.created_at
BEGIN SELECT RAISE(ABORT,'delivery attempt creation timestamp is immutable'); END;
CREATE TRIGGER mailbox_delivery_attempt_resolved_at_immutable BEFORE UPDATE OF resolved_at ON mailbox_delivery_attempts
WHEN OLD.resolved_at IS NOT NULL AND NEW.resolved_at IS NOT OLD.resolved_at
 AND NOT EXISTS(SELECT 1 FROM sidecar_timestamp_repairs r
   WHERE r.record_family='mailbox_delivery_attempt' AND r.record_key=OLD.attempt_id
     AND r.field_name='resolved_at' AND r.old_value IS OLD.resolved_at AND r.new_value=NEW.resolved_at
     AND NOT EXISTS(SELECT 1 FROM sidecar_timestamp_repairs newer
       WHERE newer.record_family=r.record_family AND newer.record_key=r.record_key
         AND newer.field_name=r.field_name AND newer.rowid>r.rowid))
BEGIN SELECT RAISE(ABORT,'delivery attempt terminal timestamp is immutable'); END;

ALTER TABLE completion_event ADD COLUMN updated_at TEXT;
ALTER TABLE completion_event ADD COLUMN closed_at TEXT;
ALTER TABLE completion_event ADD COLUMN retention_eligible_at TEXT;
ALTER TABLE completion_event ADD COLUMN retention_status TEXT NOT NULL DEFAULT 'legacy_unknown'
    CHECK(retention_status IN ('pending','blocked','eligible','legacy_unknown','clock_anomaly'));
UPDATE completion_event
SET updated_at=COALESCE(triggered_at,created_at),
    closed_at=triggered_at,
    retention_status=CASE WHEN state='pending' THEN 'pending' ELSE 'legacy_unknown' END;
CREATE INDEX idx_completion_event_retention_v23 ON completion_event(retention_eligible_at,event_id)
    WHERE retention_status='eligible' AND retention_eligible_at IS NOT NULL;
CREATE TRIGGER completion_event_timestamp_after_trigger
AFTER UPDATE OF state,triggered_at ON completion_event
WHEN OLD.state='pending' AND NEW.state='triggered'
BEGIN
  UPDATE completion_event
  SET updated_at=NEW.triggered_at,closed_at=NEW.triggered_at,
      retention_eligible_at=CASE
        WHEN julianday(NEW.triggered_at)>=julianday(NEW.created_at)
         AND NOT EXISTS(SELECT 1 FROM completion_event_listener l WHERE l.event_id=NEW.event_id AND l.retirement_pending=1)
        THEN NEW.triggered_at ELSE NULL END,
      retention_status=CASE
        WHEN julianday(NEW.triggered_at) IS NULL OR julianday(NEW.created_at) IS NULL THEN 'legacy_unknown'
        WHEN julianday(NEW.triggered_at)<julianday(NEW.created_at) THEN 'clock_anomaly'
        WHEN EXISTS(SELECT 1 FROM completion_event_listener l WHERE l.event_id=NEW.event_id AND l.retirement_pending=1) THEN 'blocked'
        ELSE 'eligible' END
  WHERE event_id=NEW.event_id;
END;
CREATE TRIGGER completion_event_triggered_at_immutable BEFORE UPDATE OF triggered_at ON completion_event
WHEN OLD.triggered_at IS NOT NULL AND NEW.triggered_at IS NOT OLD.triggered_at
 AND NOT EXISTS(SELECT 1 FROM sidecar_timestamp_repairs r
  WHERE r.record_family='completion_event' AND r.record_key=OLD.event_id
    AND r.field_name='triggered_at' AND r.old_value IS OLD.triggered_at AND r.new_value=NEW.triggered_at
    AND NOT EXISTS(SELECT 1 FROM sidecar_timestamp_repairs newer
      WHERE newer.record_family=r.record_family AND newer.record_key=r.record_key
        AND newer.field_name=r.field_name AND newer.rowid>r.rowid))
BEGIN SELECT RAISE(ABORT,'completion event terminal timestamp is immutable'); END;

ALTER TABLE completion_event_listener ADD COLUMN updated_at TEXT;
ALTER TABLE completion_event_listener ADD COLUMN closed_at TEXT;
ALTER TABLE completion_event_listener ADD COLUMN retention_eligible_at TEXT;
ALTER TABLE completion_event_listener ADD COLUMN retention_status TEXT NOT NULL DEFAULT 'pending'
    CHECK(retention_status IN ('pending','eligible','legacy_unknown','clock_anomaly'));
UPDATE completion_event_listener
SET updated_at=COALESCE(acknowledged_at,created_at),
    closed_at=acknowledged_at,
    retention_eligible_at=CASE WHEN acknowledged_at IS NOT NULL AND julianday(acknowledged_at)>=julianday(created_at) THEN acknowledged_at ELSE NULL END,
    retention_status=CASE
      WHEN retirement_pending=1 THEN 'pending'
      WHEN acknowledged_at IS NULL THEN 'legacy_unknown'
      WHEN julianday(acknowledged_at) IS NULL OR julianday(created_at) IS NULL THEN 'legacy_unknown'
      WHEN julianday(acknowledged_at)<julianday(created_at) THEN 'clock_anomaly'
      ELSE 'eligible' END;
CREATE INDEX idx_completion_event_listener_retention_v23
    ON completion_event_listener(retention_eligible_at,event_id,listener_id)
    WHERE retention_status='eligible' AND retention_eligible_at IS NOT NULL;
CREATE TRIGGER completion_listener_timestamp_after_reactivation
AFTER UPDATE OF retirement_pending ON completion_event_listener
WHEN OLD.retirement_pending=0 AND NEW.retirement_pending=1
BEGIN
  UPDATE completion_event_listener
  SET updated_at=strftime('%Y-%m-%dT%H:%M:%SZ','now'),
      retention_eligible_at=NULL,
      retention_status=CASE
        WHEN OLD.retention_status='clock_anomaly' THEN 'clock_anomaly'
        WHEN OLD.retention_status='legacy_unknown' THEN 'legacy_unknown'
        WHEN julianday(NEW.created_at) IS NULL OR julianday(OLD.updated_at) IS NULL
          OR julianday('now') IS NULL THEN 'legacy_unknown'
        WHEN julianday('now')<julianday(NEW.created_at)
          OR julianday('now')<julianday(OLD.updated_at) THEN 'clock_anomaly'
        ELSE 'pending' END
  WHERE event_id=NEW.event_id AND listener_id=NEW.listener_id;
  UPDATE completion_event
  SET updated_at=(SELECT changed.updated_at
                  FROM completion_event_listener AS changed
                  WHERE changed.event_id=NEW.event_id
                    AND changed.listener_id=NEW.listener_id),
      retention_eligible_at=NULL,
      retention_status=CASE
        WHEN retention_status='clock_anomaly' THEN 'clock_anomaly'
        WHEN retention_status='legacy_unknown' THEN 'legacy_unknown'
        WHEN (SELECT changed.retention_status
              FROM completion_event_listener AS changed
              WHERE changed.event_id=NEW.event_id
                AND changed.listener_id=NEW.listener_id)='clock_anomaly'
        THEN 'clock_anomaly'
        WHEN (SELECT changed.retention_status
              FROM completion_event_listener AS changed
              WHERE changed.event_id=NEW.event_id
                AND changed.listener_id=NEW.listener_id)='legacy_unknown'
        THEN 'legacy_unknown'
        WHEN julianday(created_at) IS NULL OR julianday(updated_at) IS NULL
          OR julianday((SELECT changed.updated_at
                        FROM completion_event_listener AS changed
                        WHERE changed.event_id=NEW.event_id
                          AND changed.listener_id=NEW.listener_id)) IS NULL
        THEN 'legacy_unknown'
        WHEN julianday((SELECT changed.updated_at
                        FROM completion_event_listener AS changed
                        WHERE changed.event_id=NEW.event_id
                          AND changed.listener_id=NEW.listener_id))<julianday(created_at)
          OR julianday((SELECT changed.updated_at
                        FROM completion_event_listener AS changed
                        WHERE changed.event_id=NEW.event_id
                          AND changed.listener_id=NEW.listener_id))<julianday(updated_at)
        THEN 'clock_anomaly'
        ELSE 'blocked' END
  WHERE event_id=NEW.event_id AND state='triggered';
END;
CREATE TRIGGER completion_listener_timestamp_after_retirement
AFTER UPDATE OF acknowledged_at,retirement_pending ON completion_event_listener
WHEN OLD.retirement_pending=1 AND NEW.retirement_pending=0
BEGIN
  UPDATE completion_event_listener
  SET updated_at=COALESCE(NEW.acknowledged_at,strftime('%Y-%m-%dT%H:%M:%SZ','now')),
      closed_at=COALESCE(OLD.closed_at,NEW.acknowledged_at,strftime('%Y-%m-%dT%H:%M:%SZ','now')),
      retention_eligible_at=CASE
        WHEN OLD.retention_status NOT IN ('legacy_unknown','clock_anomaly')
         AND julianday(COALESCE(NEW.acknowledged_at,'now'))>=julianday(NEW.created_at)
         AND julianday(COALESCE(NEW.acknowledged_at,'now'))>=julianday(OLD.updated_at)
        THEN COALESCE(NEW.acknowledged_at,strftime('%Y-%m-%dT%H:%M:%SZ','now')) ELSE NULL END,
      retention_status=CASE
        WHEN OLD.retention_status='clock_anomaly' THEN 'clock_anomaly'
        WHEN OLD.retention_status='legacy_unknown' THEN 'legacy_unknown'
        WHEN julianday(NEW.created_at) IS NULL OR julianday(OLD.updated_at) IS NULL
          OR julianday(COALESCE(NEW.acknowledged_at,'now')) IS NULL
        THEN 'legacy_unknown'
        WHEN julianday(COALESCE(NEW.acknowledged_at,'now'))<julianday(NEW.created_at)
          OR julianday(COALESCE(NEW.acknowledged_at,'now'))<julianday(OLD.updated_at)
        THEN 'clock_anomaly' ELSE 'eligible' END
  WHERE event_id=NEW.event_id AND listener_id=NEW.listener_id;
  UPDATE completion_event
  SET updated_at=(SELECT changed.updated_at
                  FROM completion_event_listener AS changed
                  WHERE changed.event_id=NEW.event_id
                    AND changed.listener_id=NEW.listener_id),
      retention_eligible_at=CASE
        WHEN retention_status NOT IN ('legacy_unknown','clock_anomaly')
         AND (SELECT changed.retention_status
              FROM completion_event_listener AS changed
              WHERE changed.event_id=NEW.event_id
                AND changed.listener_id=NEW.listener_id)='eligible'
         AND julianday((SELECT changed.updated_at
                        FROM completion_event_listener AS changed
                        WHERE changed.event_id=NEW.event_id
                          AND changed.listener_id=NEW.listener_id))>=julianday(created_at)
         AND julianday((SELECT changed.updated_at
                        FROM completion_event_listener AS changed
                        WHERE changed.event_id=NEW.event_id
                          AND changed.listener_id=NEW.listener_id))>=julianday(updated_at)
         AND NOT EXISTS(
             SELECT 1 FROM completion_event_listener AS pending
             INDEXED BY idx_completion_event_listener_retirement_pending
             WHERE pending.event_id=NEW.event_id AND pending.retirement_pending=1)
        THEN (SELECT changed.updated_at
              FROM completion_event_listener AS changed
              WHERE changed.event_id=NEW.event_id
                AND changed.listener_id=NEW.listener_id)
        ELSE NULL END,
      retention_status=CASE
        WHEN retention_status='clock_anomaly' THEN 'clock_anomaly'
        WHEN retention_status='legacy_unknown' THEN 'legacy_unknown'
        WHEN (SELECT changed.retention_status
              FROM completion_event_listener AS changed
              WHERE changed.event_id=NEW.event_id
                AND changed.listener_id=NEW.listener_id)='clock_anomaly'
        THEN 'clock_anomaly'
        WHEN (SELECT changed.retention_status
              FROM completion_event_listener AS changed
              WHERE changed.event_id=NEW.event_id
                AND changed.listener_id=NEW.listener_id)='legacy_unknown'
        THEN 'legacy_unknown'
        WHEN julianday(created_at) IS NULL OR julianday(updated_at) IS NULL
          OR julianday((SELECT changed.updated_at
                        FROM completion_event_listener AS changed
                        WHERE changed.event_id=NEW.event_id
                          AND changed.listener_id=NEW.listener_id)) IS NULL
        THEN 'legacy_unknown'
        WHEN julianday((SELECT changed.updated_at
                        FROM completion_event_listener AS changed
                        WHERE changed.event_id=NEW.event_id
                          AND changed.listener_id=NEW.listener_id))<julianday(created_at)
          OR julianday((SELECT changed.updated_at
                        FROM completion_event_listener AS changed
                        WHERE changed.event_id=NEW.event_id
                          AND changed.listener_id=NEW.listener_id))<julianday(updated_at)
        THEN 'clock_anomaly'
        WHEN EXISTS(
             SELECT 1 FROM completion_event_listener AS pending
             INDEXED BY idx_completion_event_listener_retirement_pending
             WHERE pending.event_id=NEW.event_id AND pending.retirement_pending=1)
        THEN 'blocked'
        ELSE 'eligible' END
  WHERE event_id=NEW.event_id AND state='triggered';
END;
CREATE TRIGGER completion_listener_created_at_immutable BEFORE UPDATE OF created_at ON completion_event_listener
WHEN NEW.created_at IS NOT OLD.created_at
BEGIN SELECT RAISE(ABORT,'completion listener creation timestamp is immutable'); END;
CREATE TRIGGER completion_listener_closed_at_immutable BEFORE UPDATE OF closed_at ON completion_event_listener
WHEN OLD.closed_at IS NOT NULL AND NEW.closed_at IS NOT OLD.closed_at
 AND NOT EXISTS(SELECT 1 FROM sidecar_timestamp_repairs r
  WHERE r.record_family='completion_event_listener'
    AND r.record_key=OLD.event_id||'/'||OLD.listener_id
    AND r.field_name='closed_at' AND r.old_value IS OLD.closed_at AND r.new_value=NEW.closed_at
    AND NOT EXISTS(SELECT 1 FROM sidecar_timestamp_repairs newer
      WHERE newer.record_family=r.record_family AND newer.record_key=r.record_key
        AND newer.field_name=r.field_name AND newer.rowid>r.rowid))
BEGIN SELECT RAISE(ABORT,'completion listener terminal timestamp is immutable'); END;

ALTER TABLE runtime_generation ADD COLUMN updated_at TEXT;
ALTER TABLE runtime_generation ADD COLUMN retention_eligible_at TEXT;
ALTER TABLE runtime_generation ADD COLUMN retention_status TEXT NOT NULL DEFAULT 'legacy_unknown'
    CHECK(retention_status IN ('pending','eligible','legacy_unknown','clock_anomaly'));
UPDATE runtime_generation
SET updated_at=COALESCE(exited_at,draining_at,running_at,created_at),
    retention_eligible_at=CASE
      WHEN lifecycle_state='exited' AND created_at!='1970-01-01T00:00:00Z'
       AND exited_at IS NOT NULL AND julianday(exited_at)>=julianday(created_at) THEN exited_at ELSE NULL END,
    retention_status=CASE
      WHEN lifecycle_state!='exited' THEN 'pending'
      WHEN created_at='1970-01-01T00:00:00Z' OR exited_at IS NULL THEN 'legacy_unknown'
      WHEN julianday(exited_at) IS NULL OR julianday(created_at) IS NULL THEN 'legacy_unknown'
      WHEN julianday(exited_at)<julianday(created_at) THEN 'clock_anomaly'
      ELSE 'eligible' END;
CREATE INDEX idx_runtime_generation_retention_v23 ON runtime_generation(retention_eligible_at,generation_uuid)
    WHERE retention_status='eligible' AND retention_eligible_at IS NOT NULL;
CREATE TRIGGER runtime_generation_timestamp_after_transition
AFTER UPDATE OF lifecycle_state,running_at,draining_at,exited_at ON runtime_generation
BEGIN
 UPDATE runtime_generation
 SET updated_at=COALESCE(NEW.exited_at,NEW.draining_at,NEW.running_at,OLD.updated_at,NEW.created_at),
     retention_eligible_at=CASE WHEN NEW.lifecycle_state='exited' AND NEW.created_at!='1970-01-01T00:00:00Z'
       AND NEW.exited_at IS NOT NULL AND julianday(NEW.exited_at)>=julianday(NEW.created_at) THEN NEW.exited_at ELSE NULL END,
     retention_status=CASE WHEN NEW.lifecycle_state!='exited' THEN 'pending'
       WHEN NEW.created_at='1970-01-01T00:00:00Z' OR NEW.exited_at IS NULL THEN 'legacy_unknown'
       WHEN julianday(NEW.exited_at) IS NULL OR julianday(NEW.created_at) IS NULL THEN 'legacy_unknown'
       WHEN julianday(NEW.exited_at)<julianday(NEW.created_at) THEN 'clock_anomaly' ELSE 'eligible' END
 WHERE generation_uuid=NEW.generation_uuid;
END;
CREATE TRIGGER runtime_generation_created_at_immutable BEFORE UPDATE OF created_at ON runtime_generation
WHEN NEW.created_at IS NOT OLD.created_at
BEGIN SELECT RAISE(ABORT,'runtime generation creation timestamp is immutable'); END;
CREATE TRIGGER runtime_generation_exited_at_immutable BEFORE UPDATE OF exited_at ON runtime_generation
WHEN OLD.exited_at IS NOT NULL AND NEW.exited_at IS NOT OLD.exited_at
 AND NOT EXISTS(SELECT 1 FROM sidecar_timestamp_repairs r
  WHERE r.record_family='runtime_generation' AND r.record_key=OLD.generation_uuid
    AND r.field_name='exited_at' AND r.old_value IS OLD.exited_at AND r.new_value=NEW.exited_at
    AND NOT EXISTS(SELECT 1 FROM sidecar_timestamp_repairs newer
      WHERE newer.record_family=r.record_family AND newer.record_key=r.record_key
        AND newer.field_name=r.field_name AND newer.rowid>r.rowid))
BEGIN SELECT RAISE(ABORT,'runtime generation terminal timestamp is immutable'); END;

CREATE TRIGGER completion_event_created_at_immutable BEFORE UPDATE OF created_at ON completion_event
WHEN NEW.created_at IS NOT OLD.created_at
BEGIN SELECT RAISE(ABORT,'completion event creation timestamp is immutable'); END;
CREATE TRIGGER completion_event_terminal_reopen_forbidden BEFORE UPDATE OF state ON completion_event
WHEN OLD.state='triggered' AND NEW.state IS NOT OLD.state
BEGIN SELECT RAISE(ABORT,'completion event terminal state cannot reopen'); END;
CREATE TRIGGER runtime_generation_terminal_reopen_forbidden BEFORE UPDATE OF lifecycle_state ON runtime_generation
WHEN OLD.lifecycle_state='exited' AND NEW.lifecycle_state IS NOT OLD.lifecycle_state
BEGIN SELECT RAISE(ABORT,'runtime generation terminal state cannot reopen'); END;
