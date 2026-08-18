-- State-side append authority for sidecar completion continuity.
-- Version 14 installations may already contain this table when created by an
-- earlier S2 binary, so every schema object is deliberately idempotent.
-- ## Declared roles
-- `mapper`, `validator`

CREATE TABLE IF NOT EXISTS invocation_completion_continuity (
    authority_ordinal INTEGER NOT NULL PRIMARY KEY
        CONSTRAINT completion_continuity_positive_ordinal CHECK (authority_ordinal > 0),
    admission_id ANY NOT NULL UNIQUE REFERENCES invocation_completion_obligations(admission_id)
        CONSTRAINT completion_continuity_admission_id_text
        CHECK (typeof(admission_id) = 'text' AND admission_id = trim(admission_id) AND length(admission_id) > 0),
    expected_sidecar_generation ANY NOT NULL
        CONSTRAINT completion_continuity_generation_text
        CHECK (
            typeof(expected_sidecar_generation) = 'text'
            AND expected_sidecar_generation = trim(expected_sidecar_generation)
            AND length(expected_sidecar_generation) > 0
        ),
    invocation_uuid ANY NOT NULL
        CONSTRAINT completion_continuity_invocation_uuid_text
        CHECK (typeof(invocation_uuid) = 'text' AND invocation_uuid = trim(invocation_uuid) AND length(invocation_uuid) > 0),
    event_id ANY NOT NULL
        CONSTRAINT completion_continuity_event_id_text
        CHECK (typeof(event_id) = 'text' AND event_id = trim(event_id) AND length(event_id) > 0),
    owner_invocation_uuid ANY NOT NULL
        CONSTRAINT completion_continuity_owner_invocation_uuid_text
        CHECK (
            typeof(owner_invocation_uuid) = 'text'
            AND owner_invocation_uuid = trim(owner_invocation_uuid)
            AND length(owner_invocation_uuid) > 0
        ),
    owner_session_id ANY NOT NULL
        CONSTRAINT completion_continuity_owner_session_id_text
        CHECK (typeof(owner_session_id) = 'text' AND owner_session_id = trim(owner_session_id) AND length(owner_session_id) > 0),
    previous_continuity_digest ANY NOT NULL
        CONSTRAINT completion_continuity_previous_digest_text
        CHECK (
            typeof(previous_continuity_digest) = 'text'
            AND length(previous_continuity_digest) = 64
            AND previous_continuity_digest NOT GLOB '*[^0-9a-f]*'
        ),
    continuity_digest ANY NOT NULL UNIQUE
        CONSTRAINT completion_continuity_digest_text
        CHECK (
            typeof(continuity_digest) = 'text'
            AND length(continuity_digest) = 64
            AND continuity_digest NOT GLOB '*[^0-9a-f]*'
        )
) STRICT;

CREATE INDEX IF NOT EXISTS idx_invocation_completion_continuity_head
    ON invocation_completion_continuity (authority_ordinal DESC);

CREATE TRIGGER IF NOT EXISTS trg_invocation_completion_continuity_insert
BEFORE INSERT ON invocation_completion_continuity
BEGIN
    SELECT CASE
        WHEN NOT EXISTS (
            SELECT 1
            FROM invocation_completion_obligations
            WHERE admission_id = NEW.admission_id
              AND expected_sidecar_generation = NEW.expected_sidecar_generation
              AND invocation_uuid = NEW.invocation_uuid
              AND event_id = NEW.event_id
              AND owner_invocation_uuid = NEW.owner_invocation_uuid
              AND owner_session_id = NEW.owner_session_id
        ) THEN RAISE(ABORT, 'completion continuity obligation identity mismatch')
        WHEN NEW.authority_ordinal <> COALESCE(
            (
                SELECT authority_ordinal + 1
                FROM invocation_completion_continuity
                ORDER BY authority_ordinal DESC
                LIMIT 1
            ),
            1
        ) THEN RAISE(ABORT, 'completion continuity ordinal is not append-only')
        WHEN NEW.previous_continuity_digest <> COALESCE(
            (
                SELECT continuity_digest
                FROM invocation_completion_continuity
                ORDER BY authority_ordinal DESC
                LIMIT 1
            ),
            '0000000000000000000000000000000000000000000000000000000000000000'
        ) THEN RAISE(ABORT, 'completion continuity previous digest mismatch')
        WHEN EXISTS (
            SELECT 1
            FROM invocation_completion_continuity
            ORDER BY authority_ordinal DESC
            LIMIT 1
        ) AND NEW.expected_sidecar_generation <> (
            SELECT expected_sidecar_generation
            FROM invocation_completion_continuity
            ORDER BY authority_ordinal DESC
            LIMIT 1
        ) THEN RAISE(ABORT, 'completion continuity sidecar generation changed')
        WHEN NOT oulipoly_utf8_text(NEW.admission_id)
          OR NOT oulipoly_utf8_text(NEW.expected_sidecar_generation)
          OR NOT oulipoly_utf8_text(NEW.invocation_uuid)
          OR NOT oulipoly_utf8_text(NEW.event_id)
          OR NOT oulipoly_utf8_text(NEW.owner_invocation_uuid)
          OR NOT oulipoly_utf8_text(NEW.owner_session_id)
          OR NOT oulipoly_utf8_text(NEW.previous_continuity_digest)
          OR NOT oulipoly_utf8_text(NEW.continuity_digest)
        THEN RAISE(ABORT, 'completion continuity invalid UTF-8 TEXT')
    END;
END;

CREATE TRIGGER IF NOT EXISTS trg_invocation_completion_continuity_append_only_update
BEFORE UPDATE ON invocation_completion_continuity
BEGIN
    SELECT RAISE(ABORT, 'completion continuity is append-only: update forbidden');
END;

CREATE TRIGGER IF NOT EXISTS trg_invocation_completion_continuity_append_only_delete
BEFORE DELETE ON invocation_completion_continuity
BEGIN
    SELECT RAISE(ABORT, 'completion continuity is append-only: delete forbidden');
END;

-- A pre-S2 schema-14 obligation has no state continuity proof and cannot be
-- joined to a sidecar listener safely during a State-only migration. Preserve
-- it and make the operator-owned destructive recovery path explicit instead.
CREATE TABLE IF NOT EXISTS invocation_completion_continuity_recovery (
    singleton INTEGER NOT NULL PRIMARY KEY
        CONSTRAINT completion_continuity_recovery_singleton CHECK (singleton = 1),
    recovery_state ANY NOT NULL
        CONSTRAINT completion_continuity_recovery_state
        CHECK (
            typeof(recovery_state) = 'text'
            AND recovery_state = 'operator_recovery_required'
        ),
    unproven_obligation_count INTEGER NOT NULL
        CONSTRAINT completion_continuity_recovery_positive_count
        CHECK (unproven_obligation_count > 0)
) STRICT;

INSERT OR IGNORE INTO invocation_completion_continuity_recovery (
    singleton,
    recovery_state,
    unproven_obligation_count
)
SELECT
    1,
    'operator_recovery_required',
    COUNT(*)
FROM invocation_completion_obligations AS obligation
WHERE NOT EXISTS (
    SELECT 1
    FROM invocation_completion_continuity AS continuity
    WHERE continuity.admission_id = obligation.admission_id
      AND continuity.expected_sidecar_generation = obligation.expected_sidecar_generation
      AND continuity.invocation_uuid = obligation.invocation_uuid
      AND continuity.event_id = obligation.event_id
      AND continuity.owner_invocation_uuid = obligation.owner_invocation_uuid
      AND continuity.owner_session_id = obligation.owner_session_id
)
HAVING COUNT(*) > 0;

CREATE TRIGGER IF NOT EXISTS trg_invocation_completion_continuity_recovery_update
BEFORE UPDATE ON invocation_completion_continuity_recovery
BEGIN
    SELECT RAISE(ABORT, 'completion continuity recovery state is operator-owned: update forbidden');
END;

CREATE TRIGGER IF NOT EXISTS trg_invocation_completion_continuity_recovery_delete
BEFORE DELETE ON invocation_completion_continuity_recovery
BEGIN
    SELECT RAISE(ABORT, 'completion continuity recovery state is operator-owned: delete forbidden');
END;
