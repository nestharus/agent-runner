//! ## Declared roles
//!
//! `validator`, `filter`, `parser`.

use super::RoutingError;
use oulipoly_config::ModelConfig;
use std::ffi::OsString;

pub(super) fn fresh_run_pin_provider() -> Option<String> {
    pin_provider_from_args(std::env::args_os().skip(1))
}

fn pin_provider_from_args(args: impl IntoIterator<Item = OsString>) -> Option<String> {
    let args = args.into_iter().take_while(|arg| arg != "--");
    pin_provider_value_token(args)
        .and_then(|value| value.into_string().ok())
        .filter(|value| !value.is_empty() && !value.starts_with('-'))
}

fn pin_provider_value_token(args: impl IntoIterator<Item = OsString>) -> Option<OsString> {
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy();
        if let Some(value) = text.strip_prefix("--pin-provider=") {
            return Some(OsString::from(value));
        }
        if text == "--pin-provider" {
            return args.next();
        }
    }
    None
}

pub(super) fn select_pinned_provider(
    model: &ModelConfig,
    eligible_indices: &[usize],
    target_provider: &str,
) -> Result<usize, RoutingError> {
    let provider_index = model
        .providers
        .iter()
        .position(|provider| provider.name == target_provider)
        .ok_or_else(|| RoutingError::PinnedProviderNotInModel {
            model_name: model.name.clone(),
            target_provider: target_provider.to_string(),
            provider_names: model
                .providers
                .iter()
                .map(|provider| provider.name.clone())
                .collect(),
        })?;

    if eligible_indices.contains(&provider_index) {
        Ok(provider_index)
    } else {
        Err(RoutingError::PinnedProviderIneligible {
            model_name: model.name.clone(),
            target_provider: target_provider.to_string(),
        })
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
