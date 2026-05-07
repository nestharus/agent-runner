mod common;

use oulipoly_agent_store::Store;

// proposal-test-intent-row 14: bootstrap idempotency
// assumption-register: A4
// named risk: bootstrap could create duplicate metadata rows or mutate schema version on repeated initialization
// selected level: particular-integration
#[test]
fn init_is_idempotent_and_keeps_single_schema_version_row() {
    let db = common::TempDb::new();

    Store::init(db.path()).expect("first init");
    Store::init(db.path()).expect("second init");
    Store::open(db.path()).expect("open after init");

    let conn = rusqlite::Connection::open(db.path()).expect("sqlite open");
    let rows: Vec<String> = conn
        .prepare("SELECT value FROM schema_meta WHERE key = 'schema_version'")
        .expect("prepare")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("collect");

    assert_eq!(rows, vec!["1".to_string()]);
}
