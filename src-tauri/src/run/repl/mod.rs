//! Interactive REPL module wiring.

mod disposition;
mod execution;
mod finalization;
mod formatter;
mod mapper;
mod migration;
mod orchestration;
mod resolution;
#[cfg(test)]
mod source_guard;
mod terminal;
mod validator;

pub(crate) use orchestration::run_repl;
