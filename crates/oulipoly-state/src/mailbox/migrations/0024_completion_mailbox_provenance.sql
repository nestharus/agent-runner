ALTER TABLE mailbox ADD COLUMN completion_provenance TEXT NOT NULL DEFAULT 'unclassified'
    CHECK(completion_provenance IN ('unclassified','legacy','v2'));
