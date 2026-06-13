//! ## Declared roles
//!
//! - accessor
//! - orchestration
//!
//! Role set: { accessor, orchestration }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/tests/common/base.rs
//!     role: intrinsic-surface
//!     Domain: base-test-fixture
//!     Owns:
//!       - the db test fixture surface this module owns: StateDb-owned temp databases,
//!       -   schema/rows, and concern DTOs it seeds and inspects via `use super::*`
//!       - all StateDb/rusqlite carriers referenced via `use super::*`, subordinate to
//!       -   this fixture domain: StateDb, sqlite, params, Connection, Transaction, Row,
//!       -   Statement, Uuid, and the concern-owned DTOs each test exercises
//! ```

use super::super::*;
pub(in crate::db::tests) fn test_db() -> StateDb {
    StateDb::open(Path::new(":memory:")).unwrap()
}

pub(in crate::db::tests) fn busy_timeout_ms(db: &StateDb) -> i64 {
    db.connection()
        .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
        .unwrap()
}
