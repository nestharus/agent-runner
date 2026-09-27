-- Broker-created v30 invocation ancestry must retain its exact registration
-- route even if provider session capture metadata later changes. Historical
-- v29 and already admitted v30 bindings remain readable and repairable.
CREATE TABLE invocation_completion_exact_required (
    invocation_uuid TEXT NOT NULL PRIMARY KEY REFERENCES invocations(invocation_uuid),
    route TEXT NOT NULL CHECK(route='broker-v30'),
    capability_digest TEXT NOT NULL CHECK(length(capability_digest)=64)
) STRICT;

CREATE TRIGGER invocation_completion_exact_required_no_update
BEFORE UPDATE ON invocation_completion_exact_required
BEGIN SELECT RAISE(ABORT, 'exact registration route immutable'); END;

CREATE TRIGGER invocation_completion_exact_required_no_delete
BEFORE DELETE ON invocation_completion_exact_required
BEGIN SELECT RAISE(ABORT, 'exact registration route immutable'); END;
