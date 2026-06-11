//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::*;
#[test]
fn state_db_open_sets_busy_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");

    let db = StateDb::open(&path).unwrap();
    let busy_timeout = db
        .connection()
        .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
        .unwrap();

    assert!(
        busy_timeout >= 5000,
        "StateDb::open should configure busy_timeout >= 5000ms, got {busy_timeout}ms"
    );
}
