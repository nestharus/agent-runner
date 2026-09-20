-- Recovery material belongs to the existing append-only admission, never an
-- intent/outbox that could independently confer registration authority.
-- NULL preserves old admissions as old evidence; no v2 authority is backfilled.
ALTER TABLE invocation_completion_obligations ADD COLUMN completion_v2_binding BLOB
    CHECK (completion_v2_binding IS NULL OR
           (typeof(completion_v2_binding) = 'blob' AND length(completion_v2_binding) BETWEEN 1 AND 2097152));
