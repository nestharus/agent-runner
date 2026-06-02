//! ## Declared roles
//!
//! Roles: none.
//!
//! Functionless module inventory; no A1 function-role claim.
//!
//! Neutral resume argument composition, output parsing, acceptance mapping, and
//! message formatting live in the child leaves re-exported by this facade.

mod acceptance;
mod args;
mod messages;
mod output;
mod patterns;

pub(super) use acceptance::classify_resume_acceptance;
pub(super) use args::compose_resume_provider_args;
pub use args::{ResumePayload, compose_resume_args};
