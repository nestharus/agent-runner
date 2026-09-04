//! Run command wiring.
//!
//! ## Declared roles
//!
//! `orchestration`, `accessor`
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/src/run/mod.rs
//!     role: intrinsic-surface
//!     Domain: run_module_facade
//!     Owns:
//!       - run child module declarations
//!       - run_repl and run_resume public re-exports
//! ```

pub(crate) mod balancing;
pub(crate) mod continuation_artifact;
pub(crate) mod continuation_command;
pub(crate) mod continuation_fresh;
pub(crate) mod continuation_handoff;
pub(crate) mod continuation_outcome;
pub(crate) mod continuation_request;
pub(crate) mod continuation_resume;
pub(crate) mod repl;
pub(crate) mod reservation;
pub(crate) mod resume;
mod spooled_success_delivery;

#[cfg(test)]
mod continuation_command_tests;
#[cfg(test)]
mod continuation_fresh_tests;
#[cfg(test)]
mod continuation_handoff_tests;
#[cfg(test)]
mod continuation_outcome_tests;
#[cfg(test)]
mod continuation_request_tests;
#[cfg(test)]
mod continuation_resume_tests;
#[cfg(test)]
mod continuation_test_support;
#[cfg(test)]
mod reservation_tests;

pub(crate) use repl::run_repl;
pub(crate) use resume::run_resume;
