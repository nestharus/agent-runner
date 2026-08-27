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
    let original_argv = candidate.argv.clone();
    let original_stdin = candidate.stdin.clone();
    let original_prompt = candidate.prompt.clone();
    let argv_transformed = apply_optional_argv(&mut candidate, result.argv);
    apply_optional_env(&mut candidate, result.env);
    apply_optional_stdin(&mut candidate, result.stdin);
    apply_optional_prompt(&mut candidate, result.prompt, argv_transformed);
    if candidate.argv != original_argv
        || candidate.stdin != original_stdin
        || candidate.prompt != original_prompt
    {
        candidate.prompt_acceptance = None;
    }
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
        if !argv_transformed
            && matches!(candidate.prompt_mode, PromptMode::Arg)
            && let Some(argument) = candidate.argv.last_mut()
            && argument == &candidate.prompt
        {
            *argument = prompt.clone();
        }
        candidate.prompt = prompt;
    }
}

#[cfg(test)]
mod tests {
    use super::apply_policy_transform;
    use crate::executor::external_provider::request_builder::{
        LaunchCandidate, PromptAcceptanceCandidate,
    };
    use crate::services::MailboxDeliveryCorrelation;
    use oulipoly_config::PromptMode;
    use oulipoly_provider::generated::PolicyEvaluateResult;
    use std::collections::BTreeMap;

    fn launch_candidate(
        argv: &[&str],
        stdin: Option<&str>,
        prompt_mode: PromptMode,
    ) -> LaunchCandidate {
        LaunchCandidate {
            argv: argv.iter().map(|value| (*value).to_string()).collect(),
            env: BTreeMap::new(),
            stdin: stdin.map(str::to_string),
            prompt: "original".to_string(),
            prompt_mode,
            working_directory: ".".to_string(),
            prompt_acceptance: Some(PromptAcceptanceCandidate {
                prompt: "original".to_string(),
                mailbox_delivery_correlation: Some(MailboxDeliveryCorrelation {
                    delivery_nonce: "delivery-123".to_string(),
                }),
            }),
            completion_registration_authority: None,
        }
    }

    #[test]
    fn prompt_replacement_clears_mailbox_delivery_correlation() {
        let candidate = launch_candidate(&["provider", "original"], None, PromptMode::Arg);
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
        assert_eq!(transformed.prompt_acceptance, None);
    }

    #[test]
    fn argv_replacement_clears_mailbox_delivery_correlation() {
        let candidate = launch_candidate(&["provider", "original"], None, PromptMode::Arg);
        let transformed = apply_policy_transform(
            candidate,
            PolicyEvaluateResult {
                accepted: true,
                argv: Some(vec!["provider".to_string(), "replacement".to_string()]),
                env: None,
                stdin: None,
                prompt: None,
                diagnostics: Vec::new(),
                markers: Vec::new(),
            },
        )
        .unwrap();

        assert_eq!(transformed.prompt_acceptance, None);
    }

    #[test]
    fn stdin_replacement_clears_mailbox_delivery_correlation() {
        let candidate = launch_candidate(&["provider"], Some("original"), PromptMode::Stdin);
        let transformed = apply_policy_transform(
            candidate,
            PolicyEvaluateResult {
                accepted: true,
                argv: None,
                env: None,
                stdin: Some("replacement".to_string()),
                prompt: None,
                diagnostics: Vec::new(),
                markers: Vec::new(),
            },
        )
        .unwrap();

        assert_eq!(transformed.prompt_acceptance, None);
    }

    #[test]
    fn byte_identical_carriers_retain_prompt_acceptance_eligibility() {
        let candidate = launch_candidate(&["provider"], Some("original"), PromptMode::Stdin);
        let transformed = apply_policy_transform(
            candidate,
            PolicyEvaluateResult {
                accepted: true,
                argv: Some(vec!["provider".to_string()]),
                env: Some(BTreeMap::from([(
                    "POLICY_ENV".to_string(),
                    "changed".to_string(),
                )])),
                stdin: Some("original".to_string()),
                prompt: Some("original".to_string()),
                diagnostics: Vec::new(),
                markers: Vec::new(),
            },
        )
        .unwrap();

        assert_eq!(
            transformed.prompt_acceptance,
            Some(PromptAcceptanceCandidate {
                prompt: "original".to_string(),
                mailbox_delivery_correlation: Some(MailboxDeliveryCorrelation {
                    delivery_nonce: "delivery-123".to_string(),
                }),
            })
        );
    }
}
