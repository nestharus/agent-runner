-- Durable live projections used by launch and recovery.  The projections are
-- derived only from explicit obligation state; age and absence are never
-- interpreted as terminality.
ALTER TABLE completed_turns ADD COLUMN recovery_pending INTEGER NOT NULL DEFAULT 1
    CHECK (recovery_pending IN (0, 1));

UPDATE completed_turns
SET recovery_pending = 0
WHERE committed_at IS NOT NULL
  AND json_valid(tails_json)
  AND json_extract(tails_json, '$.native') = 'complete_or_standalone'
  AND json_extract(tails_json, '$.delivery') IN ('complete', 'not_applicable')
  AND json_extract(tails_json, '$.idle') IN ('complete', 'no_runtime')
  AND json_extract(tails_json, '$.wake') IN (
      'no_pending_at_recheck', 'distinct_next_turn_pending', 'no_mailbox'
  );

DROP INDEX completed_turns_pending;
CREATE INDEX completed_turns_recovery_pending
    ON completed_turns(invocation_id)
    WHERE recovery_pending = 1;

CREATE INDEX idx_invocation_completion_obligations_legacy
    ON invocation_completion_obligations(admission_id)
    WHERE completion_v2_binding IS NULL;
CREATE INDEX idx_invocation_completion_obligations_event
    ON invocation_completion_obligations(event_id, admission_id)
    WHERE completion_v2_binding IS NOT NULL;

-- Keep immutable v2 source identities queryable without decoding the complete
-- admission ledger. Multiple listeners may legitimately share one source, so
-- the digest is evidence rather than a uniqueness constraint.
CREATE TABLE invocation_completion_v2_identity (
    admission_id TEXT PRIMARY KEY
        REFERENCES invocation_completion_obligations(admission_id),
    domain_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    registration_id TEXT NOT NULL,
    handle TEXT NOT NULL,
    registration_digest TEXT NOT NULL
) STRICT;
CREATE INDEX idx_invocation_completion_v2_registration
    ON invocation_completion_v2_identity(registration_id, registration_digest);
CREATE INDEX idx_invocation_completion_v2_source
    ON invocation_completion_v2_identity(domain_id, source_id, registration_digest);
CREATE INDEX idx_invocation_completion_v2_handle
    ON invocation_completion_v2_identity(domain_id, handle, registration_digest);

INSERT INTO invocation_completion_v2_identity (
    admission_id, domain_id, source_id, registration_id, handle,
    registration_digest
)
SELECT admission_id,
       json_extract(
           json_extract(CAST(completion_v2_binding AS TEXT), '$.registration_bytes_utf8'),
           '$.domain_id'
       ),
       json_extract(
           json_extract(CAST(completion_v2_binding AS TEXT), '$.registration_bytes_utf8'),
           '$.source_id'
       ),
       json_extract(
           json_extract(CAST(completion_v2_binding AS TEXT), '$.registration_bytes_utf8'),
           '$.registration_id'
       ),
       json_extract(
           json_extract(CAST(completion_v2_binding AS TEXT), '$.registration_bytes_utf8'),
           '$.handle'
       ),
       json_extract(CAST(completion_v2_binding AS TEXT), '$.registration_digest')
FROM invocation_completion_obligations
WHERE completion_v2_binding IS NOT NULL;

CREATE TRIGGER trg_invocation_completion_v2_identity_append_only_update
BEFORE UPDATE ON invocation_completion_v2_identity
BEGIN
    SELECT RAISE(ABORT, 'completion v2 identity is append-only: update forbidden');
END;

CREATE TRIGGER trg_invocation_completion_v2_identity_append_only_delete
BEFORE DELETE ON invocation_completion_v2_identity
BEGIN
    SELECT RAISE(ABORT, 'completion v2 identity is append-only: delete forbidden');
END;

-- Continuing native-channel custody is current authority, not replay history.
-- Keep the append-only replay as evidence and project only the exact live key.
CREATE TABLE provider_launch_native_channel_duties (
    logical_launch_id TEXT NOT NULL REFERENCES provider_logical_launches(logical_launch_id),
    attempt_id TEXT NOT NULL UNIQUE REFERENCES provider_launch_attempts(attempt_id),
    domain_id TEXT NOT NULL,
    settlement_json TEXT NOT NULL,
    PRIMARY KEY (logical_launch_id, attempt_id)
);
CREATE INDEX provider_launch_native_channel_duty_domain
    ON provider_launch_native_channel_duties(domain_id, attempt_id);

INSERT INTO provider_launch_native_channel_duties (
    logical_launch_id, attempt_id, domain_id, settlement_json
)
SELECT logical_launch_id,
       substr(operation_key, 1, length(operation_key) - length('/native-channel-duty')),
       json_extract(result_json, '$.continuing_custody.domain_id'),
       result_json
FROM provider_launch_transition_replays
WHERE operation_key LIKE '%/native-channel-duty'
  AND json_type(result_json, '$.continuing_custody.domain_id') = 'text';

CREATE INDEX provider_launch_cancelling
    ON provider_logical_launches(logical_launch_id)
    WHERE status = 'cancelling';
