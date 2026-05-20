-- AGE-163 WU-A.1: working-set columns on provider_quotas + round-robin cursor.
-- Idempotency: column-existence guarded in the post-sql hook
-- (`apply_v9_working_set_columns`) so an ALTER on a previously-applied DB
-- becomes a no-op. The CREATE TABLE statement uses IF NOT EXISTS.

CREATE TABLE IF NOT EXISTS model_round_robin_cursor (
    model_name TEXT PRIMARY KEY,
    last_index INTEGER NOT NULL,
    updated_at TEXT NOT NULL
);
