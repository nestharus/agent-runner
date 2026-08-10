//! ## Declared roles
//!
//! `orchestration`, `parser`, `validator`, `accessor`, `formatter`, `mapper`

use oulipoly_runtime::fresh_continuation::ReservedInvocation;
use oulipoly_state::{InvocationRecord, StateDb};
use uuid::Uuid;

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct ReservedRun {
    invocation_id: String,
    parent_invocation_row_id: i64,
}

#[cfg_attr(not(test), allow(dead_code))]
impl ReservedRun {
    pub(crate) fn resolve(
        state: &StateDb,
        reservation: &ReservedInvocation,
    ) -> Result<Self, String> {
        parse_reserved_invocation_id(&reservation.invocation_id)
            .map_err(format_invalid_reserved_invocation_id)?;
        let parent = reserved_parent(state, &reservation.parent_invocation_id)?;
        let parent = require_reserved_parent(parent, &reservation.parent_invocation_id)?;
        Ok(map_reserved_run(reservation, &parent))
    }

    pub(crate) fn invocation_id(&self) -> &str {
        &self.invocation_id
    }

    pub(crate) fn parent_invocation_row_id(&self) -> i64 {
        self.parent_invocation_row_id
    }

    pub(crate) fn max_attempts(&self) -> usize {
        1
    }
}

fn parse_reserved_invocation_id(invocation_id: &str) -> Result<Uuid, uuid::Error> {
    Uuid::parse_str(invocation_id)
}

fn reserved_parent(
    state: &StateDb,
    parent_invocation_id: &str,
) -> Result<Option<InvocationRecord>, String> {
    state.get_invocation_by_uuid(parent_invocation_id)
}

fn require_reserved_parent(
    parent: Option<InvocationRecord>,
    parent_invocation_id: &str,
) -> Result<InvocationRecord, String> {
    parent.ok_or_else(|| format_missing_reserved_parent(parent_invocation_id))
}

fn map_reserved_run(reservation: &ReservedInvocation, parent: &InvocationRecord) -> ReservedRun {
    ReservedRun {
        invocation_id: reservation.invocation_id.clone(),
        parent_invocation_row_id: parent.id,
    }
}

fn format_invalid_reserved_invocation_id(error: uuid::Error) -> String {
    format!("Invalid reserved invocation ID: {error}")
}

fn format_missing_reserved_parent(parent_invocation_id: &str) -> String {
    format!("Reserved invocation parent not found: {parent_invocation_id}")
}
