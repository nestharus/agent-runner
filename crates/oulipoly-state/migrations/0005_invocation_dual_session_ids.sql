ALTER TABLE invocations ADD COLUMN provider_session_id TEXT;
ALTER TABLE invocations ADD COLUMN resume_input_id TEXT;
ALTER TABLE invocations ADD COLUMN provider_session_capture_method TEXT;

UPDATE invocations
SET provider_session_id = session_id,
    provider_session_capture_method = session_capture_method
WHERE session_id IS NOT NULL
  AND (session_capture_method IS NULL OR session_capture_method <> 'resumed');

UPDATE invocations
SET resume_input_id = session_id
WHERE session_id IS NOT NULL
  AND session_capture_method = 'resumed';

CREATE INDEX IF NOT EXISTS idx_invocations_provider_provider_session
  ON invocations(provider_name, provider_index, provider_session_id)
  WHERE provider_session_id IS NOT NULL;
