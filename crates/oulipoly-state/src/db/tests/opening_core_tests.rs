//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/tests/opening_core_tests.rs
//!     role: intrinsic-surface
//!     Domain: opening-core-tests-test-fixture
//!     Owns:
//!       - the db test fixture surface this module owns: StateDb-owned temp databases,
//!       -   schema/rows, and concern DTOs it seeds and inspects via `use super::*`
//!       - all StateDb/rusqlite carriers referenced via `use super::*`, subordinate to
//!       -   this fixture domain: StateDb, sqlite, params, Connection, Transaction, Row,
//!       -   Statement, Uuid, and the concern-owned DTOs each test exercises
//! ```

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
