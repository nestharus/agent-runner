-- Private fresh source registration is minted from the broker-observed C row.
-- It does not import a v29 registration or authorize recipient delivery.
CREATE TABLE fresh_bash_source_registration (
 request_id TEXT PRIMARY KEY REFERENCES fresh_bash_child(request_id),
 source_id TEXT NOT NULL UNIQUE, attempt_id TEXT NOT NULL UNIQUE,
 state_admission_id TEXT NOT NULL, registration_digest TEXT NOT NULL,
 completion_policy TEXT NOT NULL CHECK(completion_policy='tree'),
 registered_at TEXT NOT NULL
);
CREATE TRIGGER fresh_bash_source_registration_no_update BEFORE UPDATE ON fresh_bash_source_registration
BEGIN SELECT RAISE(ABORT,'fresh Bash source registration immutable'); END;
CREATE TRIGGER fresh_bash_source_registration_no_delete BEFORE DELETE ON fresh_bash_source_registration
BEGIN SELECT RAISE(ABORT,'fresh Bash source registration retained'); END;
CREATE TABLE fresh_bash_selected_event (
 request_id TEXT PRIMARY KEY REFERENCES fresh_bash_source_registration(request_id),
 source_id TEXT NOT NULL UNIQUE, attempt_id TEXT NOT NULL UNIQUE,
 physical_grant_id TEXT NOT NULL UNIQUE, receipt_json TEXT NOT NULL,
 selected_at TEXT NOT NULL
);
CREATE TRIGGER fresh_bash_selected_event_no_update BEFORE UPDATE ON fresh_bash_selected_event
BEGIN SELECT RAISE(ABORT,'fresh Bash selected event immutable'); END;
CREATE TRIGGER fresh_bash_selected_event_no_delete BEFORE DELETE ON fresh_bash_selected_event
BEGIN SELECT RAISE(ABORT,'fresh Bash selected event retained'); END;
