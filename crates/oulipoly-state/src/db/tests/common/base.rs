//! ## Declared roles
//!
//! - accessor
//! - orchestration
//!
//! Role set: { accessor, orchestration }

use super::super::*;
pub(in crate::db::tests) fn test_db() -> StateDb {
    StateDb::open(Path::new(":memory:")).unwrap()
}

pub(in crate::db::tests) fn busy_timeout_ms(db: &StateDb) -> i64 {
    db.connection()
        .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
        .unwrap()
}
