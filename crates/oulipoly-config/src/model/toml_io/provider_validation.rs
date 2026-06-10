//! ## Declared roles
//!
//! `formatter`, `mapper`, `parser`, `predicate`, `validator`

use crate::claude_tool_filter::{ClaudeToolFilterShape, validate_proxy_claude_filter_shape};
use std::collections::HashSet;

use super::super::{
    ModelConfig, ModelError, ProviderConfig, ToolRestrictionKind, derive_provider_name,
};

pub(super) fn validate_proxy_claude_model_shape(
    model_name: &str,
    model: &ModelConfig,
    providers: &crate::providers::ProvidersConfig,
) -> Result<(), ModelError> {
    for model_provider in &model.providers {
        if !is_claude_provider(model_provider, providers) {
            continue;
        }
        let Some(effective) = resolve_effective_provider_for_model(model_provider, providers)
        else {
            continue;
        };
        let argv = build_provider_argv_tokens(&effective);
        let shape = claude_tool_filter_shape(&argv);
        validate_proxy_claude_filter_shape(effective.invocation_mode, shape)
            .map_err(|err| proxy_claude_model_error(model_name, model_provider, err))?;
    }
    Ok(())
}

fn is_claude_provider(
    model_provider: &ProviderConfig,
    providers: &crate::providers::ProvidersConfig,
) -> bool {
    let Some(root) = providers.get(&model_provider.name) else {
        return false;
    };
    root_provider_family_starts_with(root, &model_provider.name, "claude")
        || model_provider.name.starts_with("claude")
}

fn root_provider_family_starts_with(
    root: &crate::providers::ProviderEntry,
    fallback_name: &str,
    expected_prefix: &str,
) -> bool {
    provider_family_basename(root_provider_family(root, fallback_name)).starts_with(expected_prefix)
}

fn root_provider_family(root: &crate::providers::ProviderEntry, fallback_name: &str) -> String {
    derive_provider_name(root.command.as_deref().unwrap_or(fallback_name), &root.args)
}

fn provider_family_basename(family: String) -> String {
    family.rsplit('/').next().unwrap_or(&family).to_string()
}

fn resolve_effective_provider_for_model(
    model_provider: &ProviderConfig,
    providers: &crate::providers::ProvidersConfig,
) -> Option<ProviderConfig> {
    providers
        .effective_provider(model_provider)
        .ok()
        .map(|(effective, _)| effective)
}

fn build_provider_argv_tokens(provider: &ProviderConfig) -> Vec<String> {
    let mut argv = crate::providers::shell_split(&provider.command);
    argv.extend(provider.args.iter().cloned());
    if let Some(interactive_args) = provider.interactive_args.as_ref() {
        argv.extend(interactive_args.iter().cloned());
    }
    argv
}

fn claude_tool_filter_shape(argv: &[String]) -> ClaudeToolFilterShape {
    ClaudeToolFilterShape::detect_in_argv(argv).unwrap_or(ClaudeToolFilterShape::NoFilter)
}

fn proxy_claude_model_error(
    model_name: &str,
    model_provider: &ProviderConfig,
    err: crate::claude_tool_filter::ClaudeToolFilterError,
) -> ModelError {
    ModelError::Other(
        model_name.to_string(),
        format!("provider {}: {err}", model_provider.name),
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(in crate::model) enum CodexArgPart {
    Standalone(String),
    Pair { flag: String, value: String },
}

impl CodexArgPart {
    fn display(&self) -> String {
        match self {
            Self::Standalone(token) => token.clone(),
            Self::Pair { flag, value } => format!("({flag}, {value})"),
        }
    }
}

pub(in crate::model) fn split_codex_arg_parts(args: &[String]) -> Vec<CodexArgPart> {
    let mut parts = Vec::new();
    let mut i = 0;

    while i < args.len() {
        let token = &args[i];
        if matches!(token.as_str(), "-c" | "--config") {
            if let Some(value) = args.get(i + 1) {
                parts.push(CodexArgPart::Pair {
                    flag: token.clone(),
                    value: value.clone(),
                });
                i += 2;
            } else {
                parts.push(CodexArgPart::Standalone(token.clone()));
                i += 1;
            }
        } else {
            parts.push(CodexArgPart::Standalone(token.clone()));
            i += 1;
        }
    }

    parts
}

pub(in crate::model) fn codex_arg_overlap(
    root_args: &[String],
    model_args: &[String],
) -> Option<String> {
    let root_parts = split_codex_arg_parts(root_args)
        .into_iter()
        .collect::<HashSet<_>>();

    split_codex_arg_parts(model_args)
        .into_iter()
        .find(|part| root_parts.contains(part))
        .map(|part| part.display())
}

fn codex_config_pair_key(value: &str) -> &str {
    value.split_once('=').map(|(key, _)| key).unwrap_or(value)
}

fn codex_model_config_pair_keys(args: &[String]) -> HashSet<String> {
    split_codex_arg_parts(args)
        .into_iter()
        .filter_map(|part| match part {
            CodexArgPart::Pair { value, .. } => Some(codex_config_pair_key(&value).to_string()),
            CodexArgPart::Standalone(_) => None,
        })
        .collect()
}

fn codex_typed_policy_overlap(
    root: &crate::providers::ProviderEntry,
    model_args: &[String],
) -> Option<String> {
    let policy_pairs = root
        .tool_restrictions
        .as_ref()
        .filter(|restrictions| restrictions.kind == ToolRestrictionKind::Codex)?
        .codex
        .config_pairs
        .iter()
        .collect::<Vec<_>>();
    if policy_pairs.is_empty() {
        return None;
    }

    let model_keys = codex_model_config_pair_keys(model_args);
    policy_pairs
        .into_iter()
        .find(|pair| model_keys.contains(codex_config_pair_key(pair)))
        .map(|pair| pair.to_string())
}

pub(crate) fn validate_codex_model_arg_overlap(
    model_name: &str,
    model: &ModelConfig,
    providers: &crate::providers::ProvidersConfig,
) -> Result<(), String> {
    for model_provider in model.providers.iter() {
        let Some(root_codex) = providers.get(&model_provider.name) else {
            continue;
        };
        if !is_codex_provider(model_provider, root_codex) {
            continue;
        }

        if let Some(display) = codex_arg_overlap(&root_codex.args, &model_provider.args) {
            return Err(format_codex_args_overlap_error(
                model_name,
                model_provider,
                &display,
            ));
        }

        if let Some(display) = codex_typed_policy_overlap(root_codex, &model_provider.args) {
            return Err(format_codex_typed_policy_overlap_error(
                model_name,
                model_provider,
                &display,
            ));
        }

        if let Some(display) = codex_interactive_args_overlap(root_codex, model_provider) {
            return Err(format_codex_interactive_args_overlap_error(
                model_name,
                model_provider,
                &display,
            ));
        }
    }

    Ok(())
}

fn is_codex_provider(
    model_provider: &ProviderConfig,
    root_provider: &crate::providers::ProviderEntry,
) -> bool {
    root_provider_family(root_provider, &model_provider.name).starts_with("codex")
        || model_provider.name.starts_with("codex")
}

fn codex_interactive_args_overlap(
    root: &crate::providers::ProviderEntry,
    model_provider: &ProviderConfig,
) -> Option<String> {
    let root_interactive_args = root.interactive_args.as_deref()?;
    let model_interactive_args = model_provider.interactive_args.as_deref()?;
    if root_interactive_args.is_empty() || model_interactive_args.is_empty() {
        return None;
    }
    codex_arg_overlap(root_interactive_args, model_interactive_args)
}

fn format_codex_args_overlap_error(
    model_name: &str,
    model_provider: &ProviderConfig,
    display: &str,
) -> String {
    format!(
        "Model {model_name} provider {}: args token \"{display}\" duplicates root [{}].args; leave provider-level Codex flags in providers.toml and keep only model-specific flags in model TOML; run `agents migrate-config` to repair existing files.",
        model_provider.name, model_provider.name
    )
}

fn format_codex_typed_policy_overlap_error(
    model_name: &str,
    model_provider: &ProviderConfig,
    display: &str,
) -> String {
    format!(
        "Model {model_name} provider {}: typed-policy config pair \"{display}\" duplicates model args; leave provider-level Codex policy in providers.toml and keep only model-specific flags in model TOML.",
        model_provider.name
    )
}

fn format_codex_interactive_args_overlap_error(
    model_name: &str,
    model_provider: &ProviderConfig,
    display: &str,
) -> String {
    format!(
        "Model {model_name} provider {}: interactive_args token \"{display}\" duplicates root [{}].interactive_args; leave provider-level Codex flags in providers.toml and keep only model-specific flags in model TOML; run `agents migrate-config` to repair existing files.",
        model_provider.name, model_provider.name
    )
}
