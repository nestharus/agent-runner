-- A durable supervisor authority is a process-tree epoch, not the installation
-- domain and not a live PID. Existing rows use one explicit legacy epoch. This
-- is deliberate: upgrading must not rewrite the historical attempt table just
-- to classify old terminal history. A new root explicitly inherits that epoch
-- only when it still contains unresolved work.
CREATE TABLE completion_supervisor_authority (
    authority_id TEXT PRIMARY KEY,
    domain_id TEXT NOT NULL REFERENCES completion_continuation_domain(domain_id)
        ON UPDATE CASCADE,
    phase TEXT NOT NULL CHECK(phase IN ('active','retired')),
    created_by_generation TEXT NOT NULL,
    -- NULL exists only for an ownerless legacy epoch. Every root created by
    -- v21 binds its durable authority to one immutable guardian incarnation.
    guardian_identity TEXT
);

ALTER TABLE completion_continuation_owner
    ADD COLUMN supervisor_authority_id TEXT NOT NULL
    DEFAULT '00000000-0000-4000-8000-000000000021';
ALTER TABLE completion_continuation_source
    ADD COLUMN supervisor_authority_id TEXT NOT NULL
    DEFAULT '00000000-0000-4000-8000-000000000021';
ALTER TABLE completion_continuation_attempt
    ADD COLUMN supervisor_authority_id TEXT NOT NULL
    DEFAULT '00000000-0000-4000-8000-000000000021';

INSERT OR IGNORE INTO completion_supervisor_authority(
    authority_id,domain_id,phase,created_by_generation,guardian_identity)
SELECT '00000000-0000-4000-8000-000000000021',domain_id,
       CASE WHEN EXISTS(
           SELECT 1 FROM completion_continuation_owner WHERE phase='running')
           THEN 'active' ELSE 'retired' END,
       'legacy-completion-source-history',
       (SELECT guardian_identity FROM completion_continuation_owner
        WHERE phase='running' LIMIT 1)
FROM completion_continuation_domain;

CREATE TABLE completion_supervisor_inheritance (
    authority_id TEXT NOT NULL REFERENCES completion_supervisor_authority(authority_id),
    predecessor_authority_id TEXT NOT NULL REFERENCES completion_supervisor_authority(authority_id),
    inherited_by_generation TEXT NOT NULL,
    PRIMARY KEY(authority_id,predecessor_authority_id),
    CHECK(authority_id!=predecessor_authority_id)
);

CREATE TRIGGER completion_owner_supervisor_authority_insert BEFORE INSERT
ON completion_continuation_owner
WHEN NOT EXISTS(
    SELECT 1 FROM completion_supervisor_authority a
    WHERE a.authority_id=NEW.supervisor_authority_id
      AND a.domain_id=NEW.domain_id
      AND a.guardian_identity=NEW.guardian_identity)
BEGIN SELECT RAISE(ABORT,'completion owner supervisor authority absent'); END;

CREATE TRIGGER completion_owner_supervisor_authority_immutable BEFORE UPDATE
ON completion_continuation_owner
WHEN NEW.supervisor_authority_id IS NOT OLD.supervisor_authority_id
BEGIN SELECT RAISE(ABORT,'completion owner supervisor authority is immutable'); END;

CREATE TRIGGER completion_source_supervisor_authority_insert BEFORE INSERT
ON completion_continuation_source
WHEN NOT EXISTS(
    SELECT 1 FROM completion_supervisor_authority a
    WHERE a.authority_id=NEW.supervisor_authority_id
      AND a.domain_id=NEW.domain_id)
BEGIN SELECT RAISE(ABORT,'completion source supervisor authority absent'); END;

CREATE TRIGGER completion_source_supervisor_authority_immutable BEFORE UPDATE
ON completion_continuation_source
WHEN NEW.supervisor_authority_id IS NOT OLD.supervisor_authority_id
BEGIN SELECT RAISE(ABORT,'completion source supervisor authority is immutable'); END;

CREATE TRIGGER completion_attempt_supervisor_authority_insert BEFORE INSERT
ON completion_continuation_attempt
WHEN NOT EXISTS(
    SELECT 1 FROM completion_supervisor_authority a
    WHERE a.authority_id=NEW.supervisor_authority_id
      AND a.domain_id=NEW.domain_id)
BEGIN SELECT RAISE(ABORT,'completion attempt supervisor authority absent'); END;

CREATE TRIGGER completion_attempt_supervisor_authority_immutable BEFORE UPDATE
ON completion_continuation_attempt
WHEN NEW.supervisor_authority_id IS NOT OLD.supervisor_authority_id
BEGIN SELECT RAISE(ABORT,'completion attempt supervisor authority is immutable'); END;

-- Keep terminal completion history auditable without making it part of root
-- startup or steady-state recovery. The predicates intentionally match the
-- recovery SQL verbatim so SQLite can use these partial indexes directly.
CREATE INDEX completion_continuation_attempt_unresolved
    ON completion_continuation_attempt(
        supervisor_authority_id,owner_generation,attempt_id)
    WHERE phase NOT IN ('drained','never_started');

CREATE INDEX completion_continuation_source_unaccepted
    ON completion_continuation_source(supervisor_authority_id,registration_id)
    WHERE phase='registered';

CREATE INDEX completion_supervisor_inheritance_predecessor
    ON completion_supervisor_inheritance(predecessor_authority_id,authority_id);
