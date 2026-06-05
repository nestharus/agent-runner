//! Headless resume module wiring.

mod disposition;
mod filter;
mod finalization;
mod formatter;
mod mapper;
mod orchestration;
mod predicate;
#[cfg(test)]
mod source_guard;
mod validator;

pub(crate) use orchestration::run_resume;
pub(crate) use validator::validate_resume_input;
