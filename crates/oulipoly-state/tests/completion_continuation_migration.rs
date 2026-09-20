use oulipoly_state::{StateDb, migrations};
use rusqlite::Connection;

fn schema_23() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    drop(StateDb::open(&path).unwrap());
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("ALTER TABLE invocation_completion_obligations DROP COLUMN completion_v2_binding; PRAGMA user_version=23;").unwrap();
    (dir, path)
}

#[test]
fn completion_continuation_migration_extends_existing_ledger_once() {
    let (_dir, path) = schema_23();
    let original: String = Connection::open(&path).unwrap().query_row(
        "SELECT sql FROM sqlite_master WHERE name='trg_invocation_completion_obligations_append_only_update'", [], |r| r.get(0),
    ).unwrap();
    drop(StateDb::open(&path).unwrap());
    drop(StateDb::open(&path).unwrap());
    let conn = Connection::open(path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 24);
    let columns: i64 = conn.query_row("SELECT COUNT(*) FROM pragma_table_info('invocation_completion_obligations') WHERE name='completion_v2_binding' AND type='BLOB'", [], |r| r.get(0)).unwrap();
    assert_eq!(columns, 1);
    let trigger: String = conn.query_row("SELECT sql FROM sqlite_master WHERE name='trg_invocation_completion_obligations_append_only_update'", [], |r| r.get(0)).unwrap();
    assert_eq!(trigger, original);
    let competing: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name LIKE '%completion_v2%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        competing, 0,
        "binding must extend authority, not duplicate it"
    );
}

#[test]
fn completion_continuation_interrupted_ddl_rolls_back_then_migrates() {
    let (_dir, path) = schema_23();
    let mut conn = Connection::open(&path).unwrap();
    {
        let tx = conn.transaction().unwrap();
        tx.execute_batch(migrations::plan(23, 24).unwrap()[0].sql)
            .unwrap();
        // Discard before migration version/commit publication.
    }
    let columns: i64 = conn.query_row("SELECT COUNT(*) FROM pragma_table_info('invocation_completion_obligations') WHERE name='completion_v2_binding'", [], |r| r.get(0)).unwrap();
    assert_eq!(columns, 0);
    drop(conn);
    drop(StateDb::open(&path).unwrap());
}

#[test]
fn completion_continuation_read_only_schema_23_probe_does_not_migrate() {
    let (_dir, path) = schema_23();
    let _ = StateDb::open_read_only(&path);
    let conn = Connection::open(path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 23);
    assert!(
        migrations::plan(24, 23).is_err(),
        "old schema plans cannot write newer data"
    );
}

#[test]
fn completion_continuation_current_schema_missing_binding_is_not_repaired() {
    let (_dir, path) = schema_23();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA user_version=24").unwrap();
    drop(conn);
    assert!(StateDb::open_read_only(&path).is_err());
    assert!(StateDb::open(&path).is_err());
    let conn = Connection::open(path).unwrap();
    let count:i64=conn.query_row("SELECT COUNT(*) FROM pragma_table_info('invocation_completion_obligations') WHERE name='completion_v2_binding'",[],|r|r.get(0)).unwrap();
    assert_eq!(count, 0);
}
