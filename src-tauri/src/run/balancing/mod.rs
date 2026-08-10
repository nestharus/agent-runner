//! Balancing module wiring.

use oulipoly_state::CompositeInvocationId;

use super::reservation::ReservedRun;

mod accessor;
mod diagnostics;
#[cfg(test)]
mod diagnostics_tests;
mod disposition;
mod filter;
mod finalization;
mod formatter;
mod mapper;
mod orchestration;
mod parser;
mod predicate;
#[cfg(test)]
mod source_guard;
mod state_update;
mod validator;

pub(crate) use diagnostics::balanced_result_error_category;
pub(crate) use orchestration::run_with_balancing;

pub(in crate::run) fn composite_invocation_id(
    provider_name: &str,
    reservation: Option<&ReservedRun>,
) -> CompositeInvocationId {
    mapper::composite_invocation_id(provider_name, reservation)
}
