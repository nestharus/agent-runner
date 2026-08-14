//! ## Declared roles
//! orchestration, accessor, mapper, parser, filter, predicate, validator, formatter
//!
mod fixtures;

use fixtures::user_version;
use oulipoly_state::StateDb;
use oulipoly_state::deployment::row_version::registry;
use oulipoly_state::deployment::row_version::triggers_sql::{
    generate_all_triggers, generate_triggers_for_table,
};
use oulipoly_state::migrations;
use rusqlite::Connection;
use std::path::Path;

#[test]
fn ti_17_old_writer_inserts_and_updates_advance_row_version() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    let conn = migrated_schema6_db(&db_path);

    // Exercise representative trigger shapes; the generator-name test verifies
    // every registry table is included in generated trigger SQL.
    assert_row_version_lifecycle(
        &conn,
        "invocations",
        "INSERT INTO invocations
            (id, invocation_uuid, model_name, provider_name, provider_index, status, created_at)
         VALUES
            (6101, '61016101-6101-4101-8101-610161016101', 'rv-model', 'rv-provider', 0,
             'running', '2026-05-01T00:00:00Z')",
        "UPDATE invocations SET status = 'succeeded', finished_at = '2026-05-01T00:00:01Z'
         WHERE id = 6101",
        "UPDATE invocations SET row_version = ?1, terminal_reason = 'explicit'
         WHERE id = 6101",
        "SELECT row_version FROM invocations WHERE id = 6101",
    );
    assert_row_version_lifecycle(
        &conn,
        "providers",
        "INSERT INTO providers
            (model_name, provider_name, invocation_count, error_count, last_invoked_at)
         VALUES ('rv-model', 'rv-provider', 1, 0, '2026-05-01T00:00:00Z')",
        "UPDATE providers SET invocation_count = invocation_count + 1
         WHERE model_name = 'rv-model' AND provider_name = 'rv-provider'",
        "UPDATE providers SET row_version = ?1, last_error = 'explicit'
         WHERE model_name = 'rv-model' AND provider_name = 'rv-provider'",
        "SELECT row_version FROM providers
         WHERE model_name = 'rv-model' AND provider_name = 'rv-provider'",
    );
    assert_row_version_lifecycle(
        &conn,
        "memory_nodes",
        "INSERT INTO memory_nodes
            (id, node_type, label, data, created_at, updated_at)
         VALUES
            ('rv-memory-node', 'provider', 'Provider', '{}',
             '2026-05-01T00:00:00Z', '2026-05-01T00:00:00Z')",
        "UPDATE memory_nodes SET label = 'Provider updated',
                 updated_at = '2026-05-01T00:00:01Z'
         WHERE id = 'rv-memory-node'",
        "UPDATE memory_nodes SET row_version = ?1, data = '{\"explicit\":true}'
         WHERE id = 'rv-memory-node'",
        "SELECT row_version FROM memory_nodes WHERE id = 'rv-memory-node'",
    );
    assert_row_version_lifecycle(
        &conn,
        "cli_providers",
        "INSERT INTO cli_providers
            (cli_name, display_name, installed, version, config_dir, last_synced)
         VALUES
            ('rv-cli', 'Row Version CLI', 1, '1.0.0', '/tmp/rv-cli',
             '2026-05-01T00:00:00Z')",
        "UPDATE cli_providers SET installed = 0 WHERE cli_name = 'rv-cli'",
        "UPDATE cli_providers SET row_version = ?1, version = '1.0.1'
         WHERE cli_name = 'rv-cli'",
        "SELECT row_version FROM cli_providers WHERE cli_name = 'rv-cli'",
    );
    assert_row_version_lifecycle(
        &conn,
        "accounts",
        "INSERT INTO accounts
            (id, provider, profile_name, auth_method, auth_status, created_at)
         VALUES
            ('rv-account', 'rv-cli', 'default', 'oauth', 'unknown',
             '2026-05-01T00:00:00Z')",
        "UPDATE accounts SET auth_status = 'authenticated'
         WHERE id = 'rv-account' AND provider = 'rv-cli'",
        "UPDATE accounts SET row_version = ?1, profile_name = 'explicit'
         WHERE id = 'rv-account' AND provider = 'rv-cli'",
        "SELECT row_version FROM accounts WHERE id = 'rv-account' AND provider = 'rv-cli'",
    );
    assert_row_version_lifecycle(
        &conn,
        "session_chains",
        "INSERT INTO session_chains
            (chain_id, created_at, last_used_at, model_name)
         VALUES
            ('rv-chain', '2026-05-01T00:00:00Z', '2026-05-01T00:00:00Z', 'rv-model')",
        "UPDATE session_chains SET last_used_at = '2026-05-01T00:00:01Z'
         WHERE chain_id = 'rv-chain'",
        "UPDATE session_chains SET row_version = ?1, model_name = 'rv-model-explicit'
         WHERE chain_id = 'rv-chain'",
        "SELECT row_version FROM session_chains WHERE chain_id = 'rv-chain'",
    );
    assert_row_version_lifecycle(
        &conn,
        "invocation_returned_artifacts",
        "INSERT INTO invocation_returned_artifacts
            (invocation_id, ordinal, version_id, name, workflow_run_id, artifact_name, version,
             sha256, content_len, format_hint, verdict_line, source_kind, source_json, returned_at)
         VALUES
            (6101, 0, 'rv-artifact-version', 'rv.txt',
             'return:61016101-6101-4101-8101-610161016101', 'rv.txt', 1,
             'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
             12, 'text/plain', 'ok', 'scratchpad', '{}', '2026-05-01T00:00:02Z')",
        "UPDATE invocation_returned_artifacts SET verdict_line = 'updated'
         WHERE version_id = 'rv-artifact-version'",
        "UPDATE invocation_returned_artifacts SET row_version = ?1, format_hint = 'text/explicit'
         WHERE version_id = 'rv-artifact-version'",
        "SELECT row_version FROM invocation_returned_artifacts
         WHERE version_id = 'rv-artifact-version'",
    );
}

#[test]
fn repaired_missing_invocations_table_installs_row_version_triggers() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    {
        let conn = migrated_schema6_db(&db_path);
        conn.execute_batch("DROP TABLE invocations").unwrap();
    }

    let db = StateDb::open(&db_path).unwrap();
    let connection = Connection::open(db.path()).unwrap();

    assert_row_version_lifecycle(
        &connection,
        "invocations",
        "INSERT INTO invocations
            (id, invocation_uuid, model_name, provider_name, provider_index, status, created_at)
         VALUES
            (6201, '62016201-6201-4201-8201-620162016201', 'repair-model',
             'repair-provider', 0, 'running', '2026-05-01T00:00:00Z')",
        "UPDATE invocations SET status = 'failed', finished_at = '2026-05-01T00:00:01Z'
         WHERE id = 6201",
        "UPDATE invocations SET row_version = ?1, terminal_reason = 'explicit'
         WHERE id = 6201",
        "SELECT row_version FROM invocations WHERE id = 6201",
    );
}

#[test]
fn repaired_uuid_invocations_table_installs_missing_row_version_triggers() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    {
        let conn = migrated_schema6_db(&db_path);
        conn.execute_batch(
            "DROP TRIGGER IF EXISTS trg_invocations_row_version_insert;
             DROP TRIGGER IF EXISTS trg_invocations_row_version_update;
             ALTER TABLE invocations DROP COLUMN row_version;",
        )
        .unwrap();
    }

    let db = StateDb::open(&db_path).unwrap();
    let connection = Connection::open(db.path()).unwrap();

    assert_row_version_lifecycle(
        &connection,
        "invocations",
        "INSERT INTO invocations
            (id, invocation_uuid, model_name, provider_name, provider_index, status, created_at)
         VALUES
            (6202, '62026202-6202-4202-8202-620262026202', 'repair-model',
             'repair-provider', 0, 'running', '2026-05-01T00:00:00Z')",
        "UPDATE invocations SET status = 'succeeded', finished_at = '2026-05-01T00:00:01Z'
         WHERE id = 6202",
        "UPDATE invocations SET row_version = ?1, terminal_reason = 'explicit'
         WHERE id = 6202",
        "SELECT row_version FROM invocations WHERE id = 6202",
    );
}

#[test]
fn trigger_generator_names_match_the_migration_trigger_shape() {
    let all_triggers = generate_all_triggers();

    for registration in registry::iter() {
        let per_table = generate_triggers_for_table(registration);
        let insert_name = format!("trg_{}_row_version_insert", registration.table);
        let update_name = format!("trg_{}_row_version_update", registration.table);

        assert!(
            per_table.contains(&format!("CREATE TRIGGER IF NOT EXISTS {insert_name}")),
            "missing insert trigger shape for {}: {per_table}",
            registration.table
        );
        assert!(
            per_table.contains(&format!("CREATE TRIGGER IF NOT EXISTS {update_name}")),
            "missing update trigger shape for {}: {per_table}",
            registration.table
        );
        assert!(
            all_triggers.contains(&insert_name) && all_triggers.contains(&update_name),
            "generate_all_triggers omitted trigger names for {}",
            registration.table
        );
    }
}

// Declared role: orchestration
fn migrated_schema6_db(path: &Path) -> Connection {
    let mut conn = Connection::open(path).unwrap();
    conn.pragma_update(None, "user_version", 0).unwrap();
    let schema5_plan = migrations::plan(0, 5).unwrap();
    migrations::run_with_db_path(&mut conn, &schema5_plan, path.to_path_buf()).unwrap();
    let mut conn = assert_schema5_user_version(conn);

    let schema6_plan = migrations::plan(5, 6).unwrap();
    migrations::run_with_db_path(&mut conn, &schema6_plan, path.to_path_buf()).unwrap();
    assert_schema6_user_version(conn)
}

// Declared role: validator
fn assert_schema5_user_version(conn: Connection) -> Connection {
    assert_eq!(user_version(&conn), 5);
    conn
}

// Declared role: validator
fn assert_schema6_user_version(conn: Connection) -> Connection {
    assert_eq!(user_version(&conn), 6);
    conn
}

// Declared role: validator
fn assert_row_version_lifecycle(
    conn: &Connection,
    table: &str,
    insert_sql: &str,
    legacy_update_sql: &str,
    explicit_update_sql: &str,
    select_row_version_sql: &str,
) {
    conn.execute(insert_sql, []).unwrap();
    let inserted = row_version(conn, select_row_version_sql);
    assert!(
        inserted >= 1,
        "{table} old-writer INSERT must initialize row_version, got {inserted}"
    );

    conn.execute(legacy_update_sql, []).unwrap();
    let advanced = row_version(conn, select_row_version_sql);
    assert!(
        advanced > inserted,
        "{table} old-writer UPDATE must advance row_version, got {inserted} -> {advanced}"
    );

    let explicit = advanced + 1;
    conn.execute(explicit_update_sql, [explicit]).unwrap();
    let after_explicit = row_version(conn, select_row_version_sql);
    assert_eq!(
        after_explicit, explicit,
        "{table} new-binary explicit row_version update must not be bumped again"
    );
}

// Declared role: accessor
fn row_version(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |row| row.get(0)).unwrap()
}
