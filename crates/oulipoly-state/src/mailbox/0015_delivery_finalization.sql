-- Live finalizers retain exact attempt membership independently of terminal history.
-- No FK: the reference is acquired before registering/publishing the attempt.
CREATE TABLE IF NOT EXISTS mailbox_delivery_finalizers (
    token TEXT PRIMARY KEY,
    attempt_id TEXT NOT NULL,
    os_pid INTEGER NOT NULL,
    os_boot_id TEXT NOT NULL,
    os_pid_starttime_ticks INTEGER NOT NULL,
    checked_order INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_mailbox_delivery_finalizers_attempt
    ON mailbox_delivery_finalizers(attempt_id);
CREATE INDEX IF NOT EXISTS idx_mailbox_delivery_finalizers_check
    ON mailbox_delivery_finalizers(checked_order, token);
