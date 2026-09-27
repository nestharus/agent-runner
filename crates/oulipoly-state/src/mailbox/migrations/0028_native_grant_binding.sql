-- One native accepted attempt may acquire one broker grant. Historical Bash
-- work and unbound continuation attempts need no synthetic grant values.
CREATE TABLE completion_native_grant_binding (
    attempt_id TEXT PRIMARY KEY REFERENCES completion_continuation_attempt(attempt_id),
    grant_id TEXT NOT NULL UNIQUE,
    protocol TEXT NOT NULL CHECK(protocol='native-continuation-v1'),
    accepted_revision INTEGER NOT NULL CHECK(accepted_revision=2),
    domain_id TEXT NOT NULL,
    kernel_root_id TEXT NOT NULL,
    supervisor_authority_id TEXT NOT NULL,
    owner_generation TEXT NOT NULL,
    guardian_identity TEXT NOT NULL,
    accepted_snapshot_sha256 TEXT NOT NULL CHECK(length(accepted_snapshot_sha256)=64),
    custodian_request_sha256 TEXT NOT NULL CHECK(length(custodian_request_sha256)=64)
);
CREATE TRIGGER completion_native_grant_binding_immutable
BEFORE UPDATE ON completion_native_grant_binding
BEGIN SELECT RAISE(ABORT,'native grant binding is immutable'); END;
CREATE TRIGGER completion_native_grant_binding_retain
BEFORE DELETE ON completion_native_grant_binding
BEGIN SELECT RAISE(ABORT,'native grant binding must be retained'); END;
