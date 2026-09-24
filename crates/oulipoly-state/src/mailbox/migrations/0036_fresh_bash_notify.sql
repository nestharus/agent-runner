-- The private C registration is response-only until its exact root listener
-- explicitly requests notification. This State fence is retained even if the
-- sidecar row cannot yet be materialized; restart repairs the same W/request.
CREATE TABLE fresh_bash_notify_request (
 request_id TEXT PRIMARY KEY REFERENCES fresh_bash_source_registration(request_id),
 source_id TEXT NOT NULL, attempt_id TEXT NOT NULL,
 listener_session_id TEXT NOT NULL, listener_invocation_uuid TEXT NOT NULL,
 recipient_identity TEXT NOT NULL, root_id TEXT NOT NULL,
 owner_generation TEXT NOT NULL, requested_at TEXT NOT NULL
);
CREATE TRIGGER fresh_bash_notify_request_no_update BEFORE UPDATE ON fresh_bash_notify_request
BEGIN SELECT RAISE(ABORT,'fresh Bash notify request immutable'); END;
CREATE TRIGGER fresh_bash_notify_request_no_delete BEFORE DELETE ON fresh_bash_notify_request
BEGIN SELECT RAISE(ABORT,'fresh Bash notify request retained'); END;
