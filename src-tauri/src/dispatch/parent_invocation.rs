//! Declared roles: orchestration, accessor

use oulipoly_state::{CompositeInvocationId, InvocationRecord, StateDb};

pub(crate) fn resolve_parent_invocation_id(state: &StateDb) -> Option<i64> {
    let raw_parent = read_parent_invocation_env()?;
    let composite = super::parser::parse_parent_invocation_env(&raw_parent)?;
    lookup_parent_invocation_record(state, &composite).map(|record| record.id)
}

fn read_parent_invocation_env() -> Option<String> {
    std::env::var("OULIPOLY_PARENT_INVOCATION").ok()
}

fn lookup_parent_invocation_record(
    state: &StateDb,
    composite: &CompositeInvocationId,
) -> Option<InvocationRecord> {
    state.get_invocation_by_uuid(&composite.id).ok().flatten()
}
