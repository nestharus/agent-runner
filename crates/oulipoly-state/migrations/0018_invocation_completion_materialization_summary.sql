-- Per-invocation expected sidecar materialization authority. This row is
-- advanced in the same State transaction as each append-only continuity row.
-- ## Declared roles
-- `mapper`, `validator`

CREATE TABLE invocation_completion_materialization_summary (
    invocation_uuid ANY NOT NULL PRIMARY KEY
        CONSTRAINT completion_materialization_summary_invocation_uuid_text
        CHECK (
            typeof(invocation_uuid) = 'text'
            AND invocation_uuid = trim(invocation_uuid)
            AND length(invocation_uuid) > 0
        ),
    materialized_count INTEGER NOT NULL
        CONSTRAINT completion_materialization_summary_positive_count
        CHECK (materialized_count > 0),
    authority_ordinal INTEGER NOT NULL
        CONSTRAINT completion_materialization_summary_positive_ordinal
        CHECK (authority_ordinal > 0),
    sidecar_generation ANY NOT NULL
        CONSTRAINT completion_materialization_summary_generation_text
        CHECK (
            typeof(sidecar_generation) = 'text'
            AND sidecar_generation = trim(sidecar_generation)
            AND length(sidecar_generation) > 0
        ),
    continuity_digest ANY NOT NULL
        CONSTRAINT completion_materialization_summary_digest_text
        CHECK (
            typeof(continuity_digest) = 'text'
            AND length(continuity_digest) = 64
            AND continuity_digest NOT GLOB '*[^0-9a-f]*'
        )
) STRICT;

-- Backfill only invocations for which every obligation already has an exact
-- proven continuity row. Schema-14 obligations intentionally remain unproven.
INSERT INTO invocation_completion_materialization_summary (
    invocation_uuid,
    materialized_count,
    authority_ordinal,
    sidecar_generation,
    continuity_digest
)
SELECT
    head.invocation_uuid,
    (
        SELECT COUNT(*)
        FROM invocation_completion_continuity AS counted
        WHERE counted.invocation_uuid = head.invocation_uuid
    ),
    head.authority_ordinal,
    head.expected_sidecar_generation,
    head.continuity_digest
FROM invocation_completion_continuity AS head
WHERE head.authority_ordinal = (
    SELECT MAX(candidate.authority_ordinal)
    FROM invocation_completion_continuity AS candidate
    WHERE candidate.invocation_uuid = head.invocation_uuid
)
AND (
    SELECT COUNT(*)
    FROM invocation_completion_continuity AS counted
    WHERE counted.invocation_uuid = head.invocation_uuid
) = (
    SELECT COUNT(*)
    FROM invocation_completion_obligations AS obligation
    WHERE obligation.invocation_uuid = head.invocation_uuid
)
AND NOT EXISTS (
    SELECT 1
    FROM invocation_completion_continuity AS continuity
    LEFT JOIN invocation_completion_obligations AS obligation
      ON obligation.admission_id = continuity.admission_id
     AND obligation.expected_sidecar_generation = continuity.expected_sidecar_generation
     AND obligation.invocation_uuid = continuity.invocation_uuid
     AND obligation.event_id = continuity.event_id
     AND obligation.owner_invocation_uuid = continuity.owner_invocation_uuid
     AND obligation.owner_session_id = continuity.owner_session_id
    WHERE continuity.invocation_uuid = head.invocation_uuid
      AND obligation.admission_id IS NULL
);

CREATE TRIGGER trg_invocation_completion_materialization_summary_continuity_insert
AFTER INSERT ON invocation_completion_continuity
BEGIN
    INSERT INTO invocation_completion_materialization_summary (
        invocation_uuid,
        materialized_count,
        authority_ordinal,
        sidecar_generation,
        continuity_digest
    ) VALUES (
        NEW.invocation_uuid,
        1,
        NEW.authority_ordinal,
        NEW.expected_sidecar_generation,
        NEW.continuity_digest
    )
    ON CONFLICT(invocation_uuid) DO UPDATE SET
        materialized_count = materialized_count + 1,
        authority_ordinal = NEW.authority_ordinal,
        sidecar_generation = NEW.expected_sidecar_generation,
        continuity_digest = NEW.continuity_digest;
END;
