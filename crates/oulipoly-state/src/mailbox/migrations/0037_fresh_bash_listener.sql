-- Original Bash C chooses the listener policy before any source W is accepted.
-- The row is immutable. A later response-only inference cannot erase an F row.
CREATE TABLE fresh_bash_listener_registration (
 request_id TEXT PRIMARY KEY REFERENCES fresh_bash_source_registration(request_id),
 source_id TEXT NOT NULL, attempt_id TEXT NOT NULL,
 listener_policy TEXT NOT NULL CHECK(listener_policy IN ('response_only','notify')),
 recipient_identity TEXT NOT NULL, root_id TEXT NOT NULL,
 owner_generation TEXT NOT NULL, registered_at TEXT NOT NULL
);
CREATE TRIGGER fresh_bash_listener_registration_no_update BEFORE UPDATE ON fresh_bash_listener_registration
BEGIN SELECT RAISE(ABORT,'fresh Bash listener registration immutable'); END;
CREATE TRIGGER fresh_bash_listener_registration_no_delete BEFORE DELETE ON fresh_bash_listener_registration
BEGIN SELECT RAISE(ABORT,'fresh Bash listener registration retained'); END;
