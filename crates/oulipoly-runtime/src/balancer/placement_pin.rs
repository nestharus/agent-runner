//! ## Declared roles
//!
//! `validator`, `filter`, `orchestration`, `mapper`.

use super::routing_error::{RoutingError, pinned_provider_not_in_model_error};
use oulipoly_config::ModelConfig;
use std::ffi::OsString;

pub(super) fn fresh_run_pin_provider() -> Option<String> {
    pin_provider_from_args(std::env::args_os().skip(1))
}

fn pin_provider_from_args(args: impl IntoIterator<Item = OsString>) -> Option<String> {
    let args = args_before_prompt_separator(args);
    pin_provider_value_token(args.as_slice())
        .and_then(pin_value_from_os_string)
        .and_then(validate_pin_provider_value)
}

fn args_before_prompt_separator(args: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    args.into_iter().take_while(|arg| arg != "--").collect()
}

fn pin_provider_value_token(args: &[OsString]) -> Option<OsString> {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy();
        if let Some(value) = text.strip_prefix("--pin-provider=") {
            return Some(OsString::from(value));
        }
        if text == "--pin-provider" {
            return args.next().cloned();
        }
    }
    None
}

fn pin_value_from_os_string(value: OsString) -> Option<String> {
    value.into_string().ok()
}

fn validate_pin_provider_value(value: String) -> Option<String> {
    (!value.is_empty() && !value.starts_with('-')).then_some(value)
}

pub(super) fn select_pinned_provider(
    model: &ModelConfig,
    eligible_indices: &[usize],
    target_provider: &str,
) -> Result<usize, RoutingError> {
    let provider_index = pinned_provider_index(model, target_provider)?;
    validate_pinned_provider_eligibility(model, eligible_indices, target_provider, provider_index)?;
    Ok(provider_index)
}

fn pinned_provider_index(
    model: &ModelConfig,
    target_provider: &str,
) -> Result<usize, RoutingError> {
    model
        .providers
        .iter()
        .position(|provider| provider.name == target_provider)
        .ok_or_else(|| pinned_provider_not_in_model_error(model, target_provider))
}

fn validate_pinned_provider_eligibility(
    model: &ModelConfig,
    eligible_indices: &[usize],
    target_provider: &str,
    provider_index: usize,
) -> Result<(), RoutingError> {
    if pinned_provider_is_eligible(eligible_indices, provider_index) {
        return Ok(());
    }
    Err(pinned_provider_ineligible_error(model, target_provider))
}

fn pinned_provider_is_eligible(eligible_indices: &[usize], provider_index: usize) -> bool {
    eligible_indices.contains(&provider_index)
}

fn pinned_provider_ineligible_error(model: &ModelConfig, target_provider: &str) -> RoutingError {
    RoutingError::PinnedProviderIneligible {
        model_name: model.name.clone(),
        target_provider: target_provider.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_space_separated_pin_provider() {
        assert_eq!(
            pin_provider_from_args(args(&["--model", "fixture", "--pin-provider", "provider2"])),
            Some("provider2".to_string())
        );
    }

    #[test]
    fn parses_equals_pin_provider() {
        assert_eq!(
            pin_provider_from_args(args(&["--pin-provider=provider2"])),
            Some("provider2".to_string())
        );
    }

    #[test]
    fn ignores_prompt_separator_tail() {
        assert_eq!(
            pin_provider_from_args(args(&["--", "--pin-provider", "provider2"])),
            None
        );
    }
}
