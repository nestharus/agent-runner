-- Existing attempts keep unknown association completeness. A deleted claim
-- cannot establish which other accepted sources shared its activation.
-- No historical row or mailbox payload is read by this schema step.
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
