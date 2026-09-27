-- One immutable response reservation for the original response-only C.
-- A broker or Bash write cannot turn unknown into a consumer acknowledgement.
CREATE TABLE fresh_bash_sync_publication (
 request_id TEXT PRIMARY KEY REFERENCES fresh_bash_selected_event(request_id),
 receipt_json TEXT NOT NULL,
 reserved_at TEXT NOT NULL
);
CREATE TRIGGER fresh_bash_sync_publication_no_update BEFORE UPDATE ON fresh_bash_sync_publication
BEGIN SELECT RAISE(ABORT,'fresh Bash sync publication immutable'); END;
CREATE TRIGGER fresh_bash_sync_publication_no_delete BEFORE DELETE ON fresh_bash_sync_publication
BEGIN SELECT RAISE(ABORT,'fresh Bash sync publication retained'); END;
