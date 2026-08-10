use oulipoly_runtime::fresh_continuation::ReservedInvocation;
use oulipoly_state::StateDb;
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
        Uuid::parse_str(&reservation.invocation_id)
            .map_err(|error| format!("Invalid reserved invocation ID: {error}"))?;
        let parent = state
            .get_invocation_by_uuid(&reservation.parent_invocation_id)?
            .ok_or_else(|| {
                format!(
                    "Reserved invocation parent not found: {}",
                    reservation.parent_invocation_id
                )
            })?;

        Ok(Self {
            invocation_id: reservation.invocation_id.clone(),
            parent_invocation_row_id: parent.id,
        })
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
