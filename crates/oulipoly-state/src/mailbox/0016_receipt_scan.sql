-- Independent fair keyset cursor; never modifies the semantic submission anchor.
CREATE TABLE IF NOT EXISTS mailbox_receipt_scan (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    after_attempt_id TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_mailbox_receipt_scan_candidates
ON mailbox_delivery_attempts(attempt_id)
WHERE resolved_at IS NULL AND observation_confirmed_at IS NULL
    AND headless_submission_state = 'possible' AND submission_started_at IS NOT NULL
    AND observation_anchor_token IS NOT NULL AND observation_expected_sha256 IS NOT NULL;
