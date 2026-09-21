use rusqlite::Connection;

/// Test-only reverse of schema 27 for fixtures that deliberately reconstruct
/// an older installed schema from a freshly opened current database.
pub fn remove_v27_timestamp_contract(conn: &Connection) {
    let present: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master
                           WHERE type='table' AND name='record_timestamp_repairs')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    if !present {
        return;
    }

    conn.execute_batch(
        "DROP TRIGGER record_timestamp_repairs_append_only_update;
         DROP TRIGGER record_timestamp_repairs_append_only_delete;
         DROP TRIGGER invocations_timestamp_after_insert;
         DROP TRIGGER invocations_timestamp_after_terminal;
         DROP TRIGGER invocations_created_at_immutable;
         DROP TRIGGER invocations_finished_at_immutable;
         DROP TRIGGER invocations_terminal_reopen_forbidden;
         DROP INDEX idx_invocations_retention_eligible;
         DROP TRIGGER provider_logical_launch_finished_timestamp;
         DROP TRIGGER provider_logical_launch_recovery_blocked_timestamp;
         DROP TRIGGER provider_logical_launch_created_at_immutable;
         DROP TRIGGER provider_logical_launch_finished_at_immutable;
         DROP TRIGGER provider_logical_launch_terminal_reopen_forbidden;
         DROP INDEX provider_logical_launch_retention_eligible;
         DROP TRIGGER provider_launch_attempt_finished_timestamp;
         DROP TRIGGER provider_launch_attempt_recovery_blocked_timestamp;
         DROP TRIGGER provider_launch_attempt_created_at_immutable;
         DROP TRIGGER provider_launch_attempt_finished_at_immutable;
         DROP TRIGGER provider_launch_attempt_terminal_reopen_forbidden;
         DROP INDEX provider_launch_attempt_retention_eligible;
         DROP TRIGGER provider_launch_transition_replay_recorded_at_immutable;
         DROP TRIGGER completed_turn_created_at_immutable;
         DROP TRIGGER completed_turn_closed_at_immutable;
         DROP TRIGGER completed_turn_terminal_reopen_forbidden;
         DROP INDEX completed_turns_retention_eligible;
         ALTER TABLE invocations DROP COLUMN retention_status;
         ALTER TABLE invocations DROP COLUMN retention_eligible_at;
         ALTER TABLE invocations DROP COLUMN lifecycle_updated_at;
         ALTER TABLE provider_logical_launches DROP COLUMN retention_status;
         ALTER TABLE provider_logical_launches DROP COLUMN retention_eligible_at;
         ALTER TABLE provider_launch_attempts DROP COLUMN retention_status;
         ALTER TABLE provider_launch_attempts DROP COLUMN retention_eligible_at;
         ALTER TABLE provider_launch_transition_replays DROP COLUMN retention_status;
         ALTER TABLE provider_launch_transition_replays DROP COLUMN recorded_at;
         ALTER TABLE completed_turns DROP COLUMN retention_status;
         ALTER TABLE completed_turns DROP COLUMN retention_eligible_at;
         ALTER TABLE completed_turns DROP COLUMN closed_at;
         ALTER TABLE completed_turns DROP COLUMN updated_at;
         ALTER TABLE completed_turns DROP COLUMN created_at;
         DROP INDEX idx_record_timestamp_repairs_record;
         DROP TABLE record_timestamp_repairs;",
    )
    .unwrap();
}
