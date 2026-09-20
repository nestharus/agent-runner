-- Custody is not acknowledgement or terminal success. One immutable duty per invocation.
CREATE TABLE completed_turn_selections (
 invocation_id INTEGER PRIMARY KEY REFERENCES invocations(id),
 references_json TEXT NOT NULL
);
CREATE TABLE completed_turns (
 invocation_id INTEGER PRIMARY KEY REFERENCES invocations(id),
 invocation_uuid TEXT NOT NULL UNIQUE,
 settlement_id TEXT NOT NULL UNIQUE,
 owner_json TEXT,
 effects_json TEXT NOT NULL,
 context_json TEXT NOT NULL,
 content_sha256 TEXT NOT NULL,
 committed_at TEXT,
 tails_json TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX completed_turns_pending ON completed_turns(committed_at, invocation_id);
