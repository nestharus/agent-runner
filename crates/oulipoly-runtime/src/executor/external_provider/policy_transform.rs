//! Role: mapper.

use super::errors::ExternalProviderDispatchError;
use super::request_builder::LaunchCandidate;
use oulipoly_provider::generated::PolicyEvaluateResult;

pub(crate) fn apply_policy_transform(
    candidate: LaunchCandidate,
    result: PolicyEvaluateResult,
) -> Result<LaunchCandidate, ExternalProviderDispatchError> {
    Ok(apply_accepted_policy_transform(
        candidate,
        accepted_policy_transform(result)?,
    ))
}

fn accepted_policy_transform(
    result: PolicyEvaluateResult,
) -> Result<PolicyEvaluateResult, ExternalProviderDispatchError> {
    if result.accepted {
        Ok(result)
    } else {
        Err(ExternalProviderDispatchError::policy_rejected(
            result.diagnostics,
        ))
    }
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
    if let Some(target) = matching_prompt_arg(argv, previous) {
        replace_prompt_arg(target, next);
    }
}

fn matching_prompt_arg<'a>(argv: &'a mut [String], expected: &str) -> Option<&'a mut String> {
    let candidate = final_prompt_arg(argv)?;
    prompt_arg_matches(candidate, expected).then_some(candidate)
}

fn final_prompt_arg(argv: &mut [String]) -> Option<&mut String> {
    argv.last_mut()
}

fn prompt_arg_matches(candidate: &str, expected: &str) -> bool {
    candidate == expected
}

fn replace_prompt_arg(target: &mut String, next: &str) {
    *target = next.to_string();
}
