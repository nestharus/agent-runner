//! Role: mapper.

use super::errors::ExternalProviderDispatchError;
use super::request_builder::LaunchCandidate;
use oulipoly_provider::generated::PolicyEvaluateResult;

pub(crate) fn apply_policy_transform(
    candidate: LaunchCandidate,
    result: PolicyEvaluateResult,
) -> Result<LaunchCandidate, ExternalProviderDispatchError> {
    ensure_policy_accepted(&result)?;
    Ok(apply_accepted_policy_transform(candidate, result))
}

fn ensure_policy_accepted(
    result: &PolicyEvaluateResult,
) -> Result<(), ExternalProviderDispatchError> {
    result
        .accepted
        .then_some(())
        .ok_or_else(ExternalProviderDispatchError::policy_rejected)
}

fn apply_accepted_policy_transform(
    mut candidate: LaunchCandidate,
    result: PolicyEvaluateResult,
) -> LaunchCandidate {
    let argv_transformed = result.argv.is_some();
    if let Some(argv) = result.argv {
        candidate.argv = argv;
    }
    if let Some(env) = result.env {
        candidate.env.extend(env);
    }
    if let Some(stdin) = result.stdin {
        candidate.stdin = Some(stdin);
    }
    if let Some(prompt) = result.prompt {
        if !argv_transformed && matches!(candidate.prompt_mode, oulipoly_config::PromptMode::Arg) {
            replace_arg_prompt(&mut candidate.argv, &candidate.prompt, &prompt);
        }
        candidate.prompt = prompt;
    }
    candidate
}

fn replace_arg_prompt(argv: &mut [String], previous: &str, next: &str) {
    if let Some(last) = argv.last_mut()
        && prompt_arg_matches(last, previous)
    {
        replace_prompt_arg(last, next);
    }
}

fn prompt_arg_matches(candidate: &str, expected: &str) -> bool {
    candidate == expected
}

fn replace_prompt_arg(target: &mut String, next: &str) {
    *target = next.to_string();
}
