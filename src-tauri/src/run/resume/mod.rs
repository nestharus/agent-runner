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
