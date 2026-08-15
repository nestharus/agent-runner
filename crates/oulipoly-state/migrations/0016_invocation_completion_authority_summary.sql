-- Constant-work proof that every retained obligation has an exact State
-- continuity admission. The counters are maintained in the same transactions
-- as their append-only source rows.
-- ## Declared roles
-- `mapper`, `validator`

CREATE TABLE invocation_completion_authority_summary (
    invocation_uuid ANY NOT NULL PRIMARY KEY
        CONSTRAINT completion_authority_summary_invocation_uuid_text
        CHECK (
            typeof(invocation_uuid) = 'text'
            AND invocation_uuid = trim(invocation_uuid)
            AND length(invocation_uuid) > 0
        ),
    obligation_count INTEGER NOT NULL
        CONSTRAINT completion_authority_summary_obligation_count_nonnegative
        CHECK (obligation_count >= 0),
    continuity_count INTEGER NOT NULL
        CONSTRAINT completion_authority_summary_continuity_count_nonnegative
        CHECK (continuity_count >= 0 AND continuity_count <= obligation_count)
) STRICT;

INSERT INTO invocation_completion_authority_summary (
    invocation_uuid,
    obligation_count,
    continuity_count
)
SELECT
    invocation_uuid,
    SUM(obligation_count),
    SUM(continuity_count)
FROM (
    SELECT invocation_uuid, COUNT(*) AS obligation_count, 0 AS continuity_count
    FROM invocation_completion_obligations
    GROUP BY invocation_uuid
    UNION ALL
    SELECT invocation_uuid, 0 AS obligation_count, COUNT(*) AS continuity_count
    FROM invocation_completion_continuity
    GROUP BY invocation_uuid
)
GROUP BY invocation_uuid;

CREATE TRIGGER trg_invocation_completion_authority_summary_obligation_insert
AFTER INSERT ON invocation_completion_obligations
BEGIN
    INSERT INTO invocation_completion_authority_summary (
        invocation_uuid,
        obligation_count,
        continuity_count
    ) VALUES (NEW.invocation_uuid, 1, 0)
    ON CONFLICT(invocation_uuid) DO UPDATE SET
        obligation_count = obligation_count + 1;
END;

CREATE TRIGGER trg_invocation_completion_authority_summary_continuity_insert
AFTER INSERT ON invocation_completion_continuity
BEGIN
    UPDATE invocation_completion_authority_summary
    SET continuity_count = continuity_count + 1
    WHERE invocation_uuid = NEW.invocation_uuid;
    SELECT CASE
        WHEN changes() <> 1
        THEN RAISE(ABORT, 'completion authority summary has no obligation owner')
    END;
END;
