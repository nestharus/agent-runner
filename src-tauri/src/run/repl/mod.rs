//! Interactive REPL module wiring.

mod disposition;
mod finalization;
mod formatter;
mod mapper;
mod orchestration;
mod resolution;
#[cfg(test)]
mod source_guard;
mod validator;

pub(crate) use orchestration::run_repl;
