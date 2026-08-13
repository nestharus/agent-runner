-- Immutable state-side identity for sidecar-owned completion obligations.
-- DML requires every writing connection to register `oulipoly_utf8_text`.
-- Plain sqlite3 restores and future table-rebuild migrations must register that
-- function or explicitly account for the insert trigger before copying rows.
-- ## Declared roles
-- `mapper`, `validator`

CREATE TABLE invocation_completion_obligations (
    admission_id ANY NOT NULL PRIMARY KEY
        CONSTRAINT completion_obligation_admission_id_text
        CHECK (typeof(admission_id) = 'text' AND length(trim(admission_id)) > 0),
    invocation_uuid ANY NOT NULL REFERENCES invocations(invocation_uuid)
        CONSTRAINT completion_obligation_invocation_uuid_text
        CHECK (typeof(invocation_uuid) = 'text' AND length(trim(invocation_uuid)) > 0),
    event_id ANY NOT NULL
        CONSTRAINT completion_obligation_event_id_text
        CHECK (typeof(event_id) = 'text' AND length(trim(event_id)) > 0),
    owner_invocation_uuid ANY NOT NULL REFERENCES invocations(invocation_uuid)
        CONSTRAINT completion_obligation_owner_invocation_uuid_text
        CHECK (typeof(owner_invocation_uuid) = 'text' AND length(trim(owner_invocation_uuid)) > 0),
    owner_session_id ANY NOT NULL
        CONSTRAINT completion_obligation_owner_session_id_text
        CHECK (typeof(owner_session_id) = 'text' AND length(trim(owner_session_id)) > 0),
    expected_sidecar_generation ANY NOT NULL
        CONSTRAINT completion_obligation_expected_sidecar_generation_text
        CHECK (
            typeof(expected_sidecar_generation) = 'text'
            AND length(trim(expected_sidecar_generation)) > 0
        ),
    admitted_at ANY NOT NULL
        CONSTRAINT completion_obligation_admitted_at_text
        CHECK (typeof(admitted_at) = 'text' AND length(trim(admitted_at)) > 0),
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
