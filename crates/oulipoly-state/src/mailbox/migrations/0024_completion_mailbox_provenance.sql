-- Keep the v23 -> v24 schema writer independent of mailbox row count.
-- SQLite scans preexisting rows when ADD COLUMN includes a CHECK constraint.
ALTER TABLE mailbox ADD COLUMN completion_provenance TEXT NOT NULL DEFAULT 'unclassified';
