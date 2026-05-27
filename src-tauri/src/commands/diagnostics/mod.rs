//! Declared roles: orchestration

mod accessor;
mod formatter;
mod mapper;
mod orchestration;
mod service;
mod validator;

pub(crate) use orchestration::run_diagnostics;
