//! Role: mapper.

use super::errors::ExternalProviderDispatchError;
use super::request_builder::LaunchCandidate;
use oulipoly_provider::generated::PolicyEvaluateResult;

pub(crate) fn apply_policy_transform(
    mut candidate: LaunchCandidate,
    result: PolicyEvaluateResult,
) -> Result<LaunchCandidate, ExternalProviderDispatchError> {
    if !result.accepted {
        return Err(ExternalProviderDispatchError::policy_rejected());
    }
    let argv_transformed = result.argv.is_some();
    if let Some(argv) = result.argv {
        candidate.argv = argv;
    }
    if let Some(env) = result.env {
        candidate.env = env;
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
    Ok(candidate)
}

fn replace_arg_prompt(argv: &mut [String], previous: &str, next: &str) {
    if let Some(last) = argv.last_mut()
        && last == previous
    {
        *last = next.to_string();
    }
}
