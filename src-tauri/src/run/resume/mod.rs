//! Headless resume module wiring.

use oulipoly_state::CompositeInvocationId;

use super::reservation::ReservedRun;

mod disposition;
mod execution;
mod filter;
mod finalization;
mod formatter;
mod lifecycle;
mod mapper;
mod migration;
mod orchestration;
mod predicate;
#[cfg(test)]
mod source_guard;
mod terminal;
mod validator;
mod wake;

pub(crate) use orchestration::run_resume;
pub(crate) use validator::validate_resume_input;

pub(in crate::run) fn composite_invocation_id(
    provider_name: &str,
    reservation: Option<&ReservedRun>,
) -> CompositeInvocationId {
    mapper::composite_invocation_id(provider_name, reservation)
}

pub(in crate::run) fn max_attempts(
    ordinary_max_attempts: usize,
    reservation: Option<&ReservedRun>,
) -> usize {
    reservation.map_or(ordinary_max_attempts, ReservedRun::max_attempts)
}

pub(in crate::run) fn parent_invocation_row_id(
    ordinary_parent: Option<i64>,
    reservation: Option<&ReservedRun>,
) -> Option<i64> {
    reservation
        .map(ReservedRun::parent_invocation_row_id)
        .or(ordinary_parent)
}

pub(in crate::run) fn migration_allowed(reservation: Option<&ReservedRun>) -> bool {
    reservation.is_none()
}
