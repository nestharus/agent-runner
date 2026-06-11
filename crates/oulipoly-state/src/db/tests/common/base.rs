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
