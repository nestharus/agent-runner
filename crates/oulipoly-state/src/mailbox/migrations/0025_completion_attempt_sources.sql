-- Existing attempts keep unknown association completeness. A deleted claim
-- cannot establish which other accepted sources shared its activation.
-- No historical row or mailbox payload is read by this schema step.
-- Extend the existing source guards instead of adding two schema objects to
-- every ordinary-open fingerprint. The old supervisor checks stay intact.
DROP TRIGGER completion_source_supervisor_authority_insert;
CREATE TRIGGER completion_source_supervisor_authority_insert BEFORE INSERT
ON completion_continuation_source
WHEN NEW.attempt_association_history NOT IN ('known','unknown')
  OR NOT EXISTS(
    SELECT 1 FROM completion_supervisor_authority a
    WHERE a.authority_id=NEW.supervisor_authority_id
      AND a.domain_id=NEW.domain_id)
BEGIN
  SELECT CASE WHEN NEW.attempt_association_history NOT IN ('known','unknown')
    THEN RAISE(ABORT,'invalid completion source attempt history') END;
  SELECT CASE WHEN NOT EXISTS(
    SELECT 1 FROM completion_supervisor_authority a
    WHERE a.authority_id=NEW.supervisor_authority_id
      AND a.domain_id=NEW.domain_id)
    THEN RAISE(ABORT,'completion source supervisor authority absent') END;
END;
DROP TRIGGER completion_source_supervisor_authority_immutable;
CREATE TRIGGER completion_source_supervisor_authority_immutable BEFORE UPDATE
ON completion_continuation_source
WHEN NEW.supervisor_authority_id IS NOT OLD.supervisor_authority_id
  OR NEW.attempt_association_history IS NOT OLD.attempt_association_history
BEGIN
  SELECT CASE WHEN NEW.supervisor_authority_id IS NOT OLD.supervisor_authority_id
    THEN RAISE(ABORT,'completion source supervisor authority is immutable') END;
  SELECT CASE WHEN NEW.attempt_association_history IS NOT OLD.attempt_association_history
    THEN RAISE(ABORT,'completion source attempt history is immutable') END;
END;
CREATE TRIGGER IF NOT EXISTS completion_continuation_attempt_association_insert_valid
BEFORE INSERT ON completion_continuation_attempt
WHEN NEW.association_completeness NOT IN ('known','unknown')
BEGIN SELECT RAISE(ABORT,'invalid completion attempt association completeness'); END;
CREATE TRIGGER IF NOT EXISTS completion_continuation_attempt_association_immutable
BEFORE UPDATE ON completion_continuation_attempt
WHEN NEW.association_completeness IS NOT OLD.association_completeness
BEGIN SELECT RAISE(ABORT,'completion attempt association completeness is immutable'); END;
CREATE TABLE IF NOT EXISTS completion_continuation_attempt_source (
    attempt_id TEXT NOT NULL REFERENCES completion_continuation_attempt(attempt_id),
    registration_id TEXT NOT NULL REFERENCES completion_continuation_source(registration_id),
    listener_revision INTEGER NOT NULL,
    PRIMARY KEY (attempt_id, registration_id)
);
CREATE INDEX IF NOT EXISTS completion_continuation_attempt_source_registration
    ON completion_continuation_attempt_source(registration_id, attempt_id);
CREATE TRIGGER IF NOT EXISTS completion_continuation_attempt_source_retain
BEFORE DELETE ON completion_continuation_attempt_source
BEGIN SELECT RAISE(ABORT,'completion attempt source association must be retained'); END;
CREATE TRIGGER IF NOT EXISTS completion_continuation_attempt_source_immutable
BEFORE UPDATE ON completion_continuation_attempt_source
BEGIN SELECT RAISE(ABORT,'completion attempt source association is immutable'); END;
