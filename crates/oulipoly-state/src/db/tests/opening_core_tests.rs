//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::common::*;
use super::*;
#[test]
fn state_db_open_sets_busy_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");

    let db = StateDb::open(&path).unwrap();
    let busy_timeout = busy_timeout_ms(&db);

    assert!(
        busy_timeout >= 5000,
        "StateDb::open should configure busy_timeout >= 5000ms, got {busy_timeout}ms"
    );
}
