-- Explicit fresh-domain initialization only. Ordinary opens do not migrate
-- existing v17 domains into this lane; old v17 writers reject user_version=18.
CREATE TABLE completion_continuation_domain (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    domain_id TEXT NOT NULL UNIQUE,
    lineage TEXT NOT NULL CHECK(lineage='main-native-completion-v2')
);
CREATE TABLE completion_continuation_owner (
    generation TEXT PRIMARY KEY,
    domain_id TEXT NOT NULL REFERENCES completion_continuation_domain(domain_id),
    phase TEXT NOT NULL CHECK(phase IN ('running','closing','lost')),
    guardian_identity TEXT NOT NULL,
    driver_identity TEXT NOT NULL,
    endpoint TEXT NOT NULL
);
CREATE UNIQUE INDEX completion_continuation_owner_running ON completion_continuation_owner(domain_id) WHERE phase='running';
-- This is a projection of State admission, not a second admission authority.
CREATE TABLE completion_continuation_source (
    registration_id TEXT PRIMARY KEY,
    domain_id TEXT NOT NULL REFERENCES completion_continuation_domain(domain_id),
    source_id TEXT NOT NULL,
    event_id TEXT NOT NULL UNIQUE REFERENCES completion_event(event_id),
    registration_digest TEXT NOT NULL,
    binding BLOB NOT NULL,
    phase TEXT NOT NULL DEFAULT 'registered' CHECK(phase IN ('registered','accepted','conflict')),
    snapshot_sha256 TEXT,
    outcome_sha256 TEXT,
    payload_sha256 TEXT,
    payload_byte_len INTEGER,
    UNIQUE(domain_id,source_id),
    CHECK((snapshot_sha256 IS NULL AND outcome_sha256 IS NULL AND payload_sha256 IS NULL AND payload_byte_len IS NULL)
        OR (phase='accepted' AND snapshot_sha256 IS NOT NULL AND outcome_sha256 IS NOT NULL AND payload_sha256 IS NOT NULL AND payload_byte_len IS NOT NULL))
);
CREATE TRIGGER completion_continuation_source_immutable BEFORE UPDATE ON completion_continuation_source
WHEN NEW.registration_id IS NOT OLD.registration_id OR NEW.domain_id IS NOT OLD.domain_id
 OR NEW.source_id IS NOT OLD.source_id OR NEW.event_id IS NOT OLD.event_id
 OR NEW.registration_digest IS NOT OLD.registration_digest OR NEW.binding IS NOT OLD.binding
 OR (OLD.snapshot_sha256 IS NOT NULL AND (NEW.snapshot_sha256 IS NOT OLD.snapshot_sha256
 OR NEW.outcome_sha256 IS NOT OLD.outcome_sha256 OR NEW.payload_sha256 IS NOT OLD.payload_sha256
 OR NEW.payload_byte_len IS NOT OLD.payload_byte_len OR NEW.phase IS NOT OLD.phase))
BEGIN SELECT RAISE(ABORT,'completion immutable source conflict'); END;
CREATE TABLE completion_continuation_attempt (
    attempt_id TEXT PRIMARY KEY,
    domain_id TEXT NOT NULL REFERENCES completion_continuation_domain(domain_id),
    owner_generation TEXT NOT NULL REFERENCES completion_continuation_owner(generation),
    operation TEXT NOT NULL CHECK(operation IN ('source_recovery','activation','transport')),
    request_sha256 TEXT NOT NULL,
    source_registration_id TEXT,
    source_listener_revision INTEGER,
    session_id TEXT,
    claim_token TEXT,
    phase TEXT NOT NULL CHECK(phase IN ('reserved','accepted','starting','running','unknown_custody','drained','never_started')),
    revision INTEGER NOT NULL DEFAULT 1 CHECK(revision>0),
    custodian_identity TEXT,
    launcher_identity TEXT,
    spawn_invocation_uuid TEXT,
    runtime_generation_uuid TEXT,
    result_path TEXT NOT NULL,
    integrated INTEGER NOT NULL DEFAULT 0 CHECK(integrated IN (0,1)),
    drain_receipt TEXT,
    CHECK(phase NOT IN ('drained','never_started') OR (drain_receipt IS NOT NULL AND integrated=1)),
    CHECK(operation!='activation' OR (session_id IS NOT NULL AND claim_token IS NOT NULL))
);
CREATE UNIQUE INDEX completion_continuation_activation_live ON completion_continuation_attempt(domain_id,session_id)
 WHERE operation='activation' AND phase NOT IN ('drained','never_started');
CREATE TRIGGER completion_continuation_attempt_immutable BEFORE UPDATE ON completion_continuation_attempt
WHEN NEW.attempt_id IS NOT OLD.attempt_id OR NEW.domain_id IS NOT OLD.domain_id
 OR NEW.owner_generation IS NOT OLD.owner_generation OR NEW.operation IS NOT OLD.operation
 OR NEW.request_sha256 IS NOT OLD.request_sha256 OR NEW.source_registration_id IS NOT OLD.source_registration_id
 OR NEW.source_listener_revision IS NOT OLD.source_listener_revision OR NEW.session_id IS NOT OLD.session_id
 OR NEW.claim_token IS NOT OLD.claim_token OR NEW.result_path IS NOT OLD.result_path
 OR (OLD.custodian_identity IS NOT NULL AND NEW.custodian_identity IS NOT OLD.custodian_identity)
 OR (OLD.launcher_identity IS NOT NULL AND NEW.launcher_identity IS NOT OLD.launcher_identity)
 OR (OLD.runtime_generation_uuid IS NOT NULL AND NEW.runtime_generation_uuid IS NOT OLD.runtime_generation_uuid)
 OR (OLD.spawn_invocation_uuid IS NOT NULL AND NEW.spawn_invocation_uuid IS NOT OLD.spawn_invocation_uuid)
 OR NEW.revision != OLD.revision+1
 OR (OLD.phase IN ('drained','never_started') AND NEW.phase IS NOT OLD.phase)
BEGIN SELECT RAISE(ABORT,'immutable completion attempt identity/revision conflict'); END;
CREATE TRIGGER completion_continuation_attempt_retain BEFORE DELETE ON completion_continuation_attempt
BEGIN SELECT RAISE(ABORT,'completion attempt physical history must be retained'); END;
CREATE TRIGGER completion_continuation_claim_delete BEFORE DELETE ON session_wake_claim
WHEN EXISTS(SELECT 1 FROM completion_continuation_attempt a WHERE a.session_id=OLD.session_id AND a.claim_token=OLD.claim_token
 AND a.operation='activation' AND a.phase NOT IN ('drained','never_started'))
BEGIN SELECT RAISE(ABORT,'completion activation retains physical custody'); END;
CREATE TRIGGER completion_continuation_claim_replace BEFORE UPDATE ON session_wake_claim
WHEN NEW.claim_token IS NOT OLD.claim_token AND EXISTS(SELECT 1 FROM completion_continuation_attempt a WHERE a.session_id=OLD.session_id
 AND a.operation='activation' AND a.phase NOT IN ('drained','never_started'))
BEGIN SELECT RAISE(ABORT,'completion activation retains session reservation'); END;
