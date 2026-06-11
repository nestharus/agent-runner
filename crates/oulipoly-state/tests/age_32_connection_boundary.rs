//! ## Declared roles
//! orchestration, accessor, mapper, parser, filter, predicate, validator, formatter
//!
#[test]
fn ti_39_state_db_public_api_has_no_raw_mutable_connection_escape() {
    let public_boundary_source = public_boundary_source();
    let opening_source = opening_source();
    let state_db_impl = state_db_impl(opening_source);

    assert_no_raw_mutable_connection_escape(&public_boundary_source);
    assert_no_forbidden_state_db_impl_escape(state_db_impl);
    assert_read_only_connection_smoke();
    assert_with_write_txn_surface(opening_source);
}

fn assert_no_raw_mutable_connection_escape(public_boundary_source: &str) {
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
            !public_boundary_source.contains(forbidden),
            "StateDb must expose writes through with_write_txn only; found forbidden signature fragment {forbidden}"
        );
    }
}

fn assert_no_forbidden_state_db_impl_escape(state_db_impl: &str) {
    for forbidden_method in ["fn into_connection", "fn connection_mut"] {
        assert!(
            !state_db_impl.contains(forbidden_method),
            "StateDb public surface must not define forbidden raw connection escape method {forbidden_method}"
        );
    }
}

fn assert_read_only_connection_smoke() {
    assert_eq!(read_only_connection_smoke_value(), 1);
}

fn read_only_connection_smoke_value() -> i64 {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("state.db");
    let state = oulipoly_state::StateDb::open(&db_path).unwrap();
    let read_only_connection: &rusqlite::Connection = state.connection();
    read_only_connection
        .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
        .unwrap()
}

fn assert_with_write_txn_surface(opening_source: &str) {
    assert!(
        opening_source.contains("pub fn with_write_txn"),
        "StateDb must expose the closure-scoped write transaction API"
    );
    assert!(
        opening_source.contains("FnOnce(&mut rusqlite::Transaction<'_>)")
            || opening_source.contains("FnOnce(&mut Transaction<'_>)"),
        "with_write_txn must scope writes to a non-escaping rusqlite transaction"
    );
}

fn public_boundary_source() -> String {
    format!("{}\n{}", db_source(), opening_source())
}

fn db_source() -> &'static str {
    include_str!("../src/db.rs")
}

fn opening_source() -> &'static str {
    concat!(
        include_str!("../src/db/opening_read_only.rs"),
        include_str!("../src/db/opening_write.rs"),
        include_str!("../src/db/opening_migrations.rs"),
    )
}

fn state_db_impl(opening_source: &str) -> &str {
    require_state_db_impl(parse_state_db_impl(opening_source))
}

fn parse_state_db_impl(opening_source: &str) -> Option<&str> {
    opening_source
        .split_once("impl StateDb {")
        .map(|(_, body)| body)
}

fn require_state_db_impl(body: Option<&str>) -> &str {
    body.expect("split opening modules should contain the StateDb impl")
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
