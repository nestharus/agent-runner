//! ## Declared roles
//!
//! - accessor
//!
//! Role set: { accessor }

use super::super::*;
pub(in crate::db::tests) fn test_db() -> StateDb {
    StateDb::open(Path::new(":memory:")).unwrap()
}
