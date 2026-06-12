use oulipoly_config::ModelConfig;

pub(in crate::run::balancing) fn diagnostic_exhaustion_category(input: &str) -> Option<String> {
    super::super::super::predicate::diagnostic_input_is_exhaustion(input)
        .then(crate::quota_zero_turn::quota_exhausted_category)
}

pub(in crate::run::balancing) fn quota_retry_budget(model: &ModelConfig) -> usize {
    model.providers.len().max(1) + 1
}

pub(in crate::run::balancing) fn model_provider_names(model: &ModelConfig) -> Vec<String> {
    model
        .providers
        .iter()
        .map(|provider| provider.name.clone())
        .collect()
}
