//! Role: mapper.

use super::errors::ExternalProviderDispatchError;
use super::request_builder::LaunchCandidate;
use oulipoly_config::PromptMode;
use oulipoly_provider::generated::PolicyEvaluateResult;
use std::collections::BTreeMap;

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
    // Prompt replacement changes launch text, not the mailbox batch this attempt delivers.
    let argv_transformed = apply_optional_argv(&mut candidate, result.argv);
    apply_optional_env(&mut candidate, result.env);
    apply_optional_stdin(&mut candidate, result.stdin);
    apply_optional_prompt(&mut candidate, result.prompt, argv_transformed);
    candidate
}

fn apply_optional_argv(candidate: &mut LaunchCandidate, argv: Option<Vec<String>>) -> bool {
    let transformed = argv.is_some();
    if let Some(argv) = argv {
        candidate.argv = argv;
    }
    transformed
}

fn apply_optional_env(candidate: &mut LaunchCandidate, env: Option<BTreeMap<String, String>>) {
    if let Some(env) = env {
        candidate.env.extend(env);
    }
}

fn apply_optional_stdin(candidate: &mut LaunchCandidate, stdin: Option<String>) {
    if let Some(stdin) = stdin {
        candidate.stdin = Some(stdin);
    }
}

fn apply_optional_prompt(
    candidate: &mut LaunchCandidate,
    prompt: Option<String>,
    argv_transformed: bool,
) {
    if let Some(prompt) = prompt {
        rewrite_arg_prompt_if_needed(candidate, &prompt, argv_transformed);
        candidate.prompt = prompt;
    }
}

fn rewrite_arg_prompt_if_needed(
    candidate: &mut LaunchCandidate,
    prompt: &str,
    argv_transformed: bool,
) {
    if should_rewrite_arg_prompt(argv_transformed, candidate.prompt_mode) {
        replace_arg_prompt(&mut candidate.argv, &candidate.prompt, prompt);
    }
}

fn should_rewrite_arg_prompt(argv_transformed: bool, prompt_mode: PromptMode) -> bool {
    !argv_transformed && matches!(prompt_mode, PromptMode::Arg)
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

#[cfg(test)]
mod tests {
    use super::apply_policy_transform;
    use crate::executor::external_provider::request_builder::LaunchCandidate;
    use crate::services::MailboxDeliveryCorrelation;
    use oulipoly_config::PromptMode;
    use oulipoly_provider::generated::PolicyEvaluateResult;
    use std::collections::BTreeMap;

    #[test]
    fn prompt_replacement_preserves_mailbox_delivery_correlation() {
        let candidate = LaunchCandidate {
            argv: vec!["provider".to_string(), "original".to_string()],
            env: BTreeMap::new(),
            stdin: None,
            prompt: "original".to_string(),
            prompt_mode: PromptMode::Arg,
            working_directory: ".".to_string(),
            mailbox_delivery_correlation: Some(MailboxDeliveryCorrelation {
                delivery_nonce: "delivery-123".to_string(),
            }),
            completion_registration_authority: None,
        };
        let transformed = apply_policy_transform(
            candidate,
            PolicyEvaluateResult {
                accepted: true,
                argv: None,
                env: None,
                stdin: None,
                prompt: Some("replacement".to_string()),
                diagnostics: Vec::new(),
                markers: Vec::new(),
            },
        )
        .unwrap();

        assert_eq!(transformed.prompt, "replacement");
        assert_eq!(
            transformed
                .mailbox_delivery_correlation
                .as_ref()
                .map(|correlation| correlation.delivery_nonce.as_str()),
            Some("delivery-123")
        );
    }
}
