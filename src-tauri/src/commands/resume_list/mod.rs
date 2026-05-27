//! Declared roles: orchestration

mod formatter;
mod orchestration;
mod parser;
mod validator;

#[cfg(test)]
mod tests;

#[cfg(test)]
use formatter::format_resume_list_line;
pub(crate) use orchestration::run_resume_list;
pub(crate) use parser::normalize_resume_list_args;
