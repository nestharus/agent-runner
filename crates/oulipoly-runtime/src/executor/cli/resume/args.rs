//! ## Declared roles
//!
//! Roles: orchestration, validator, formatter.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/resume/args.rs
//!     role: adapter
//!     Translates:
//!       - oulipoly-config-resume-strategy-contract
//!       - provider-resume-argv-contract
//! ```

use super::messages::{resume_flag_required_message, resume_subcommand_required_message};
use oulipoly_config::{ResumeKind, ResumeStrategy};

pub struct ResumePayload<'a> {
    pub session_id: &'a str,
    pub strategy: &'a ResumeStrategy,
}

pub fn compose_resume_args(
    strategy: &ResumeStrategy,
    session_id: &str,
) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    append_resume_args(&mut args, strategy, session_id)?;
    Ok(args)
}

pub(in crate::executor::cli) fn compose_resume_provider_args(
    mut provider_args: Vec<String>,
    resume: ResumePayload<'_>,
) -> Result<Vec<String>, String> {
    append_resume_args(&mut provider_args, resume.strategy, resume.session_id)?;
    Ok(provider_args)
}

fn append_resume_args(
    provider_args: &mut Vec<String>,
    strategy: &ResumeStrategy,
    session_id: &str,
) -> Result<(), String> {
    let args = validate_resume_strategy(strategy)?;
    append_validated_resume_args(provider_args, args, session_id);
    Ok(())
}

enum ValidatedResumeArgs<'a> {
    Flag(&'a str),
    Subcommand(&'a [String]),
}

fn validate_resume_strategy(strategy: &ResumeStrategy) -> Result<ValidatedResumeArgs<'_>, String> {
    match strategy.kind {
        ResumeKind::Flag => validate_resume_flag_strategy(strategy),
        ResumeKind::Subcommand => validate_resume_subcommand_strategy(strategy),
    }
}

fn validate_resume_flag_strategy(
    strategy: &ResumeStrategy,
) -> Result<ValidatedResumeArgs<'_>, String> {
    let flag = strategy
        .flag
        .as_ref()
        .ok_or_else(resume_flag_required_message)?;
    Ok(ValidatedResumeArgs::Flag(flag))
}

fn validate_resume_subcommand_strategy(
    strategy: &ResumeStrategy,
) -> Result<ValidatedResumeArgs<'_>, String> {
    let subcommand = strategy
        .subcommand
        .as_ref()
        .ok_or_else(resume_subcommand_required_message)?;
    validate_resume_subcommand_non_empty(subcommand)?;
    Ok(ValidatedResumeArgs::Subcommand(subcommand))
}

fn validate_resume_subcommand_non_empty(subcommand: &[String]) -> Result<(), String> {
    if subcommand.is_empty() {
        return Err(resume_subcommand_required_message());
    }
    Ok(())
}

fn append_validated_resume_args(
    provider_args: &mut Vec<String>,
    args: ValidatedResumeArgs<'_>,
    session_id: &str,
) {
    match args {
        ValidatedResumeArgs::Flag(flag) => {
            provider_args.push(flag.to_string());
            provider_args.push(session_id.to_string());
        }
        ValidatedResumeArgs::Subcommand(subcommand) => {
            provider_args.extend(subcommand.iter().cloned());
            provider_args.push(session_id.to_string());
        }
    }
}
