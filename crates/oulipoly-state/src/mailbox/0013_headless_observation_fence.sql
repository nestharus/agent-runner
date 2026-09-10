-- NULL is legacy uncertainty, never evidence of non-submission.
ALTER TABLE mailbox_delivery_attempts ADD COLUMN headless_submission_state TEXT
    CHECK(headless_submission_state IN ('prepared', 'possible'));
ALTER TABLE mailbox_delivery_attempts ADD COLUMN observation_progress TEXT;
