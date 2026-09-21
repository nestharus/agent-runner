CREATE INDEX IF NOT EXISTS completion_continuation_attempt_native_runtime
    ON completion_continuation_attempt(
        runtime_generation_uuid, spawn_invocation_uuid, domain_id
    )
    WHERE operation = 'activation';
