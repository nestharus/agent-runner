#[test]
fn ti_39_state_db_public_api_has_no_raw_mutable_connection_escape() {
    let db_source = include_str!("../src/db.rs");
    let state_db_impl = db_source
        .split_once("impl StateDb {")
        .map(|(_, body)| body)
        .expect("db.rs should contain the StateDb impl");

    for forbidden in [
        "pub fn connection_mut",
        "pub fn into_connection",
        "pub fn raw_connection",
        "pub fn raw_connection_after_migration",
        "pub fn connection(&mut self)",
        "-> &mut Connection",
        "-> rusqlite::Connection",
        "-> Connection",
    ] {
        assert!(
            !db_source.contains(forbidden),
            "StateDb must expose writes through with_write_txn only; found forbidden signature fragment {forbidden}"
        );
    }

    for forbidden_method in ["fn into_connection", "fn connection_mut"] {
        assert!(
            !state_db_impl.contains(forbidden_method),
            "StateDb public surface must not define forbidden raw connection escape method {forbidden_method}"
        );
    }

    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("state.db");
    let state = oulipoly_state::StateDb::open(&db_path).unwrap();
    let read_only_connection: &rusqlite::Connection = state.connection();
    read_only_connection
        .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
        .unwrap();

    assert!(
        db_source.contains("pub fn with_write_txn"),
        "StateDb must expose the closure-scoped write transaction API"
    );
    assert!(
        db_source.contains("FnOnce(&mut rusqlite::Transaction<'_>)")
            || db_source.contains("FnOnce(&mut Transaction<'_>)"),
        "with_write_txn must scope writes to a non-escaping rusqlite transaction"
    );
}

#[test]
fn ti_39_session_replace_uses_state_db_write_transaction_not_raw_connection_writes() {
    let source = include_str!("../../../crates/oulipoly-runtime/src/session_replace/mod.rs");

    assert!(
        source.contains("with_write_txn"),
        "session_replace replacement writes must route through StateDb::with_write_txn"
    );
    assert!(
        !source.contains("Connection::open(data_root.join(\"state.db\"))")
            && !source.contains("Connection::open(&db_path)"),
        "session_replace must not reopen state.db as a raw writable rusqlite::Connection"
    );
    assert!(
        !source.contains("replace_db_turns(&mut conn"),
        "replace_db_turns must receive the transaction from with_write_txn, not a raw Connection"
    );
}
