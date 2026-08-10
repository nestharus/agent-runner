//! Balancing module wiring.
//!
//! ## Declared roles
//!
//! `orchestration`, `mapper`, `accessor`
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/src/run/balancing/mod.rs
//!     role: intrinsic-surface
//!     Domain: balancing_module_facade
//!     Owns:
//!       - balancing child module declarations
//!       - balancing orchestration and diagnostic re-exports
//!       - balancing reservation policy mappers
//! ```

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
pub(in crate::run) use orchestration::run_reserved_with_balancing;
pub(crate) use orchestration::run_with_balancing;

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
