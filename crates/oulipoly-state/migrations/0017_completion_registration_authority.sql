-- Caller-bound authority for immutable completion admission. Only the digest is
-- durable; the bearer value is transported in the launched invocation's
-- private process environment.
-- ## Declared roles
-- `validator`

ALTER TABLE invocations
ADD COLUMN completion_registration_capability_digest TEXT
    CONSTRAINT invocation_completion_registration_capability_digest_shape
    CHECK (
        completion_registration_capability_digest IS NULL
        OR (
            length(completion_registration_capability_digest) = 64
            AND completion_registration_capability_digest NOT GLOB '*[^0-9a-f]*'
        )
    );

CREATE TRIGGER trg_invocation_completion_registration_capability_immutable
BEFORE UPDATE OF completion_registration_capability_digest ON invocations
WHEN OLD.completion_registration_capability_digest
     IS NOT NEW.completion_registration_capability_digest
BEGIN
    SELECT RAISE(ABORT, 'completion registration capability is immutable');
END;
