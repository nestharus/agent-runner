-- Immutable state-side identity for sidecar-owned completion obligations.
-- DML requires every writing connection to register `oulipoly_utf8_text`.
-- Plain sqlite3 restores and future table-rebuild migrations must register that
-- function or explicitly account for the insert trigger before copying rows.
-- ## Declared roles
-- `mapper`, `validator`

CREATE TABLE invocation_completion_obligations (
    admission_id ANY NOT NULL PRIMARY KEY
        CONSTRAINT completion_obligation_admission_id_text
        CHECK (typeof(admission_id) = 'text' AND admission_id = trim(admission_id) AND length(admission_id) > 0),
    invocation_uuid ANY NOT NULL REFERENCES invocations(invocation_uuid)
        CONSTRAINT completion_obligation_invocation_uuid_text
        CHECK (typeof(invocation_uuid) = 'text' AND invocation_uuid = trim(invocation_uuid) AND length(invocation_uuid) > 0),
    event_id ANY NOT NULL
        CONSTRAINT completion_obligation_event_id_text
        CHECK (typeof(event_id) = 'text' AND event_id = trim(event_id) AND length(event_id) > 0),
    owner_invocation_uuid ANY NOT NULL REFERENCES invocations(invocation_uuid)
        CONSTRAINT completion_obligation_owner_invocation_uuid_text
        CHECK (typeof(owner_invocation_uuid) = 'text' AND owner_invocation_uuid = trim(owner_invocation_uuid) AND length(owner_invocation_uuid) > 0),
    owner_session_id ANY NOT NULL
        CONSTRAINT completion_obligation_owner_session_id_text
        CHECK (typeof(owner_session_id) = 'text' AND owner_session_id = trim(owner_session_id) AND length(owner_session_id) > 0),
    expected_sidecar_generation ANY NOT NULL
        CONSTRAINT completion_obligation_expected_sidecar_generation_text
        CHECK (
            typeof(expected_sidecar_generation) = 'text'
            AND expected_sidecar_generation = trim(expected_sidecar_generation)
            AND length(expected_sidecar_generation) > 0
        ),
    admitted_at ANY NOT NULL
        CONSTRAINT completion_obligation_admitted_at_text
        CHECK (typeof(admitted_at) = 'text' AND admitted_at = trim(admitted_at) AND length(admitted_at) > 0),
    UNIQUE (event_id, owner_invocation_uuid)
) STRICT;

CREATE INDEX idx_invocation_completion_obligations_invocation
    ON invocation_completion_obligations (
        invocation_uuid,
        admitted_at,
        admission_id
    );

CREATE TRIGGER trg_invocation_completion_obligations_generation_insert
BEFORE INSERT ON invocation_completion_obligations
BEGIN
    SELECT CASE
        WHEN (
            typeof(NEW.admission_id) = 'text'
            AND NOT oulipoly_utf8_text(NEW.admission_id)
        ) OR (
            typeof(NEW.invocation_uuid) = 'text'
            AND NOT oulipoly_utf8_text(NEW.invocation_uuid)
        ) OR (
            typeof(NEW.event_id) = 'text'
            AND NOT oulipoly_utf8_text(NEW.event_id)
        ) OR (
            typeof(NEW.owner_invocation_uuid) = 'text'
            AND NOT oulipoly_utf8_text(NEW.owner_invocation_uuid)
        ) OR (
            typeof(NEW.owner_session_id) = 'text'
            AND NOT oulipoly_utf8_text(NEW.owner_session_id)
        ) OR (
            typeof(NEW.expected_sidecar_generation) = 'text'
            AND NOT oulipoly_utf8_text(NEW.expected_sidecar_generation)
        ) OR (
            typeof(NEW.admitted_at) = 'text'
            AND NOT oulipoly_utf8_text(NEW.admitted_at)
        ) THEN RAISE(ABORT, 'completion obligation invalid UTF-8 TEXT')
        WHEN EXISTS (
            SELECT 1
            FROM invocation_completion_obligations
            WHERE admission_id = NEW.admission_id
               OR (
                   event_id = NEW.event_id
                   AND owner_invocation_uuid = NEW.owner_invocation_uuid
               )
        ) THEN RAISE(ABORT, 'completion obligation immutable identity conflict')
        WHEN EXISTS (
            SELECT 1
            FROM invocation_completion_obligations
            WHERE event_id = NEW.event_id
              AND expected_sidecar_generation <> NEW.expected_sidecar_generation
        ) THEN RAISE(ABORT, 'completion event sidecar generation conflict')
    END;
END;

CREATE TRIGGER trg_invocation_completion_obligations_append_only_update
BEFORE UPDATE ON invocation_completion_obligations
BEGIN
    SELECT RAISE(ABORT, 'completion obligation is append-only: update forbidden');
END;

CREATE TRIGGER trg_invocation_completion_obligations_append_only_delete
BEFORE DELETE ON invocation_completion_obligations
BEGIN
    SELECT RAISE(ABORT, 'completion obligation is append-only: delete forbidden');
END;

CREATE TABLE invocation_completion_continuity (
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

CREATE INDEX idx_invocation_completion_continuity_head
    ON invocation_completion_continuity (authority_ordinal DESC);

CREATE TRIGGER trg_invocation_completion_continuity_insert
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

CREATE TRIGGER trg_invocation_completion_continuity_append_only_update
BEFORE UPDATE ON invocation_completion_continuity
BEGIN
    SELECT RAISE(ABORT, 'completion continuity is append-only: update forbidden');
END;

CREATE TRIGGER trg_invocation_completion_continuity_append_only_delete
BEFORE DELETE ON invocation_completion_continuity
BEGIN
    SELECT RAISE(ABORT, 'completion continuity is append-only: delete forbidden');
END;
