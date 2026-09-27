-- A Bash descendant is a separate invocation. The request key is only an
-- idempotency key; broker-observed process and released root are the authority.
CREATE TABLE fresh_bash_child (
 request_id TEXT PRIMARY KEY,
 d_key TEXT NOT NULL UNIQUE,
 invocation_uuid TEXT NOT NULL UNIQUE,
 handle TEXT NOT NULL UNIQUE,
 root_handoff_id TEXT NOT NULL,
 root_id TEXT NOT NULL,
 parent_invocation_uuid TEXT NOT NULL,
 actor_identity TEXT NOT NULL UNIQUE,
 receipt_json TEXT NOT NULL,
 admitted_at TEXT NOT NULL,
 FOREIGN KEY(root_handoff_id) REFERENCES fresh_released_handoff(handoff_id)
);
CREATE TRIGGER fresh_bash_child_no_update BEFORE UPDATE ON fresh_bash_child
BEGIN SELECT RAISE(ABORT,'fresh Bash child immutable'); END;
CREATE TRIGGER fresh_bash_child_no_delete BEFORE DELETE ON fresh_bash_child
BEGIN SELECT RAISE(ABORT,'fresh Bash child immutable'); END;
-- Private source fixture: a single one-use effect reservation and terminal
-- observation. This is not a production work/drain/source admission.
CREATE TABLE fresh_bash_private_work (
 request_id TEXT PRIMARY KEY REFERENCES fresh_bash_child(request_id),
 grant_id TEXT NOT NULL UNIQUE,
 admitted_at TEXT NOT NULL
);
CREATE TRIGGER fresh_bash_private_work_no_update BEFORE UPDATE ON fresh_bash_private_work
BEGIN SELECT RAISE(ABORT,'fresh Bash private work immutable'); END;
CREATE TRIGGER fresh_bash_private_work_no_delete BEFORE DELETE ON fresh_bash_private_work
BEGIN SELECT RAISE(ABORT,'fresh Bash private work immutable'); END;
CREATE TABLE fresh_bash_private_result (
 request_id TEXT PRIMARY KEY REFERENCES fresh_bash_private_work(request_id),
 grant_id TEXT NOT NULL UNIQUE,
 exit_code INTEGER NOT NULL,
 stdout_sha256 TEXT NOT NULL,
 stdout_len INTEGER NOT NULL,
 stderr_sha256 TEXT NOT NULL,
 stderr_len INTEGER NOT NULL,
 observed_at TEXT NOT NULL
);
CREATE TRIGGER fresh_bash_private_result_no_update BEFORE UPDATE ON fresh_bash_private_result
BEGIN SELECT RAISE(ABORT,'fresh Bash private result immutable'); END;
CREATE TRIGGER fresh_bash_private_result_no_delete BEFORE DELETE ON fresh_bash_private_result
BEGIN SELECT RAISE(ABORT,'fresh Bash private result immutable'); END;
