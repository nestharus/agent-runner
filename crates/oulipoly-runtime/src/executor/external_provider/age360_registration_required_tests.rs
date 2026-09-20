use super::*;
use crate::provider_registry::ProviderRegistryOptions;
use crate::services::{ExecutorServiceRequest, MailboxDeliveryCorrelation};

#[test]
fn correlated_standalone_cannot_skip_registration_when_identity_is_absent_or_invalid() {
    for identity in [None, Some("invalid invocation identity")] {
        let provider = oulipoly_config::ProviderConfig::new("/never-run", vec![]);
        let model = oulipoly_config::ModelConfig {
            name: "fixture".into(),
            prompt_mode: oulipoly_config::PromptMode::Arg,
            providers: vec![provider],
            inputs: vec![],
            provider: None,
        };
        let mut context = crate::executor::external_provider_context_from_request(
            ExecutorServiceRequest::Facade {
                model,
                provider_index: 0,
                prompt: "exact".into(),
                working_dir: None,
                models_dir: None,
                extra_inputs: Default::default(),
                parent_invocation_env: identity.map(str::to_string),
            },
        )
        .unwrap();
        context.mailbox_delivery_correlation = Some(MailboxDeliveryCorrelation {
            delivery_nonce: "host-attempt".into(),
        });
        let error = attempt_account_dispatch(
            &ProviderRegistry::empty(ProviderRegistryOptions::default()),
            &context,
        )
        .unwrap_err();
        let ServiceError::Dependency { message } = error.service_error else {
            panic!("unexpected error type")
        };
        assert_eq!(
            message,
            "headless execution requires valid runtime registration identity"
        );
    }
}
