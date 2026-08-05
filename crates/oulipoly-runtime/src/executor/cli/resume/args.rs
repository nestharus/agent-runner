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

use super::super::provider_identity::provider_executable_name;
use super::messages::{resume_flag_required_message, resume_subcommand_required_message};
use oulipoly_config::{ProviderConfig, ResumeKind, ResumeStrategy};

const CLAUDE_RESUME_FLAG: &str = "--resume";
const CLAUDE_PAIR_FLAGS_TO_DROP_ON_NATIVE_RESUME: &[&str] = &[
    "--model",
    "--append-system-prompt",
    "--append-system-prompt-file",
    "--system-prompt",
    "--system-prompt-file",
    "--allowed-tools",
    "--allowedTools",
    "--disallowed-tools",
    "--disallowedTools",
    "--tools",
];
const CLAUDE_BOOL_FLAGS_TO_DROP_ON_NATIVE_RESUME: &[&str] = &["--disable-slash-commands"];

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
    provider: &ProviderConfig,
    mut provider_args: Vec<String>,
    resume: ResumePayload<'_>,
) -> Result<Vec<String>, String> {
    sanitize_native_resume_provider_args(provider, resume.strategy, &mut provider_args);
    append_resume_args(&mut provider_args, resume.strategy, resume.session_id)?;
    Ok(provider_args)
}

fn sanitize_native_resume_provider_args(
    provider: &ProviderConfig,
    strategy: &ResumeStrategy,
    provider_args: &mut Vec<String>,
) {
    if !is_claude_native_resume(provider, strategy) {
        return;
    }
    *provider_args = claude_native_resume_passthrough_args(provider_args);
}

fn is_claude_native_resume(provider: &ProviderConfig, strategy: &ResumeStrategy) -> bool {
    provider_is_claude(provider)
        && matches!(strategy.kind, ResumeKind::Flag)
        && strategy.flag.as_deref() == Some(CLAUDE_RESUME_FLAG)
}

fn provider_is_claude(provider: &ProviderConfig) -> bool {
    provider.name.starts_with("claude")
        || provider_executable_name(provider).is_some_and(|name| name.starts_with("claude"))
}

fn claude_native_resume_passthrough_args(args: &[String]) -> Vec<String> {
    let mut passthrough = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if is_drop_pair_flag(arg) {
            index += pair_flag_width(arg, args.get(index + 1));
        } else if is_drop_bool_flag(arg) {
            index += 1;
        } else {
            passthrough.push(arg.clone());
            index += 1;
        }
    }
    passthrough
}

fn is_drop_pair_flag(arg: &str) -> bool {
    CLAUDE_PAIR_FLAGS_TO_DROP_ON_NATIVE_RESUME
        .iter()
        .any(|flag| {
            arg == *flag
                || arg
                    .strip_prefix(flag)
                    .is_some_and(|rest| rest.starts_with('='))
        })
}

fn pair_flag_width(arg: &str, next: Option<&String>) -> usize {
    if arg.contains('=') {
        1
    } else if next.is_some() {
        2
    } else {
        1
    }
}

fn is_drop_bool_flag(arg: &str) -> bool {
    CLAUDE_BOOL_FLAGS_TO_DROP_ON_NATIVE_RESUME
        .iter()
        .any(|flag| arg == *flag)
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

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_config::InvocationMode;

    fn provider(name: &str, command: &str) -> ProviderConfig {
        ProviderConfig {
            name: name.to_string(),
            command: command.to_string(),
            args: Vec::new(),
            environment: Default::default(),
            unset_environment: Default::default(),
            interactive_args: None,
            resume: Some(claude_resume_strategy()),
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: InvocationMode::Headless,
        }
    }

    fn claude_resume_strategy() -> ResumeStrategy {
        ResumeStrategy {
            kind: ResumeKind::Flag,
            flag: Some("--resume".to_string()),
            subcommand: None,
        }
    }

    fn other_resume_strategy() -> ResumeStrategy {
        ResumeStrategy {
            kind: ResumeKind::Flag,
            flag: Some("--session".to_string()),
            subcommand: None,
        }
    }

    fn resume_payload(strategy: &ResumeStrategy) -> ResumePayload<'_> {
        ResumePayload {
            session_id: "ses_fixture",
            strategy,
        }
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_string()).collect()
    }

    #[test]
    fn claude_native_resume_drops_model_and_policy_context_args() {
        let provider = provider("claude", "env -u CLAUDECODE claude");
        let strategy = provider.resume.as_ref().unwrap();
        let provider_args = strings(&[
            "--dangerously-skip-permissions",
            "--model",
            "haiku",
            "--append-system-prompt",
            "runner policy",
            "--disallowed-tools",
            "Task",
            "--disable-slash-commands",
        ]);

        let args = compose_resume_provider_args(&provider, provider_args, resume_payload(strategy))
            .unwrap();

        assert_eq!(
            args,
            ["--dangerously-skip-permissions", "--resume", "ses_fixture"]
        );
    }

    #[test]
    fn claude_native_resume_drops_inline_context_args() {
        let provider = provider("claude", "claude");
        let strategy = provider.resume.as_ref().unwrap();
        let provider_args = strings(&[
            "--model=haiku",
            "--append-system-prompt=runner policy",
            "--allowedTools=Read",
            "--dangerously-skip-permissions",
        ]);

        let args = compose_resume_provider_args(&provider, provider_args, resume_payload(strategy))
            .unwrap();

        assert_eq!(
            args,
            ["--dangerously-skip-permissions", "--resume", "ses_fixture"]
        );
    }

    #[test]
    fn non_claude_resume_preserves_provider_args() {
        let mut provider = provider("fixture", "fixture");
        provider.resume = Some(other_resume_strategy());
        let strategy = provider.resume.as_ref().unwrap();
        let provider_args = strings(&["--model", "haiku"]);

        let args = compose_resume_provider_args(&provider, provider_args, resume_payload(strategy))
            .unwrap();

        assert_eq!(args, ["--model", "haiku", "--session", "ses_fixture"]);
    }
}
