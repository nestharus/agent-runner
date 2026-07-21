//! ## Declared roles
//!
//! `accessor`
//!
//! Stable resume CLI helper surface over focused diagnostics and target modules.

mod diagnostics;
mod target;

pub(super) use diagnostics::{
    format_resume_error, format_resume_service_rejection, render_resume_model_pool_mismatch,
    renderable_resume_execution_target, resume_model_pool_mismatch_message,
    resume_result_error_category,
};
pub(super) use target::{
    ResumeExecutionTarget, interactive_resume_execution_target, resume_migration_pool,
};
