pub mod checksum;
pub mod compare;
pub mod migrate_v6;
pub mod registry;
pub mod triggers_sql;

pub use compare::{ApplyDecision, RowSnapshot, RowVersion, decide_apply, same_or_higher};
pub use registry::{REGISTRY, RowKind, TableRegistration, iter, lookup};
