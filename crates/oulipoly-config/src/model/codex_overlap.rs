//! ## Declared roles
//!
//! - accessor
//! - filter
//! - formatter
//! - mapper
//! - orchestration
//! - parser
//! - predicate
//! - validator
//!
//! Role set: { accessor, filter, formatter, mapper, orchestration, parser, predicate, validator }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-config/src/model/codex_overlap.rs
//!     role: intrinsic-surface
//!     Domain: model-codex-overlap-validation
//!     Owns:
//!       - Codex root/model argument overlap validation
//!       - Codex typed-policy config pair overlap validation
//!       - Codex interactive argument overlap validation
//!       - provider family interpretation subordinate to Codex overlap checks
//! ```
//!

use std::collections::HashSet;

use super::config::ModelConfig;
use super::provider_config::ProviderConfig;
use super::provider_name::derive_provider_name;
use super::tool_restrictions::{ToolRestrictionKind, ToolRestrictions};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum CodexArgPart {
    Standalone(String),
    Pair { flag: String, value: String },
}

enum CodexArgToken<'a> {
    Standalone(&'a str),
    Pair { flag: &'a str, value: &'a str },
}

struct CodexProviderFamily<'a> {
    model_provider: &'a ProviderConfig,
    root_codex: &'a crate::providers::ProviderEntry,
    root_family: String,
}

struct PolicyConfigPair<'a> {
    display: &'a String,
    key: &'a str,
}

impl CodexArgPart {
    fn display(&self) -> String {
        match self {
            Self::Standalone(token) => token.clone(),
            Self::Pair { flag, value } => format!("({flag}, {value})"),
        }
    }
}

pub(super) fn split_codex_arg_parts(args: &[String]) -> Vec<CodexArgPart> {
    parse_codex_arg_tokens(args)
        .into_iter()
        .map(map_codex_arg_token)
        .collect()
}

fn parse_codex_arg_tokens(args: &[String]) -> Vec<CodexArgToken<'_>> {
    let mut tokens = Vec::new();
    let mut i = 0;

    while let Some(parsed) = parse_next_arg_token(args, &mut i) {
        tokens.push(parsed);
    }

    tokens
}

fn parse_next_arg_token<'a>(args: &'a [String], index: &mut usize) -> Option<CodexArgToken<'a>> {
    if is_arg_index_exhausted(args, *index) {
        return None;
    }
    let (parsed, advance) = parse_arg_token_at(args, *index);
    *index += advance;
    Some(parsed)
}

fn is_arg_index_exhausted(args: &[String], index: usize) -> bool {
    index >= args.len()
}

fn parse_arg_token_at(args: &[String], index: usize) -> (CodexArgToken<'_>, usize) {
    let token = args[index].as_str();
    if is_config_flag(token) {
        return parse_config_arg_token(args, index);
    }
    (parse_standalone_arg_token(token), 1)
}

fn is_config_flag(token: &str) -> bool {
    matches!(token, "-c" | "--config")
}

fn parse_config_arg_token(args: &[String], index: usize) -> (CodexArgToken<'_>, usize) {
    let flag = args[index].as_str();
    let Some(value) = args.get(index + 1) else {
        return (CodexArgToken::Standalone(flag), 1);
    };
    (
        CodexArgToken::Pair {
            flag,
            value: value.as_str(),
        },
        2,
    )
}

fn parse_standalone_arg_token(token: &str) -> CodexArgToken<'_> {
    CodexArgToken::Standalone(token)
}

fn map_codex_arg_token(token: CodexArgToken<'_>) -> CodexArgPart {
    match token {
        CodexArgToken::Standalone(token) => CodexArgPart::Standalone(token.to_string()),
        CodexArgToken::Pair { flag, value } => CodexArgPart::Pair {
            flag: flag.to_string(),
            value: value.to_string(),
        },
    }
}

pub(super) fn codex_arg_overlap(root_args: &[String], model_args: &[String]) -> Option<String> {
    let root_parts = split_codex_arg_parts(root_args);
    let model_parts = split_codex_arg_parts(model_args);
    first_duplicate_codex_arg_part(&root_parts, model_parts).map(format_codex_arg_part)
}

fn first_duplicate_codex_arg_part(
    root_parts: &[CodexArgPart],
    model_parts: Vec<CodexArgPart>,
) -> Option<CodexArgPart> {
    let root_part_set = codex_arg_part_set(root_parts);
    find_duplicate_codex_arg_part(model_parts, &root_part_set)
}

fn codex_arg_part_set(root_parts: &[CodexArgPart]) -> HashSet<CodexArgPart> {
    root_parts.iter().cloned().collect()
}

fn find_duplicate_codex_arg_part(
    model_parts: Vec<CodexArgPart>,
    root_parts: &HashSet<CodexArgPart>,
) -> Option<CodexArgPart> {
    model_parts
        .into_iter()
        .find(|part| root_parts.contains(part))
}

fn format_codex_arg_part(part: CodexArgPart) -> String {
    part.display()
}

fn codex_config_pair_key(value: &str) -> &str {
    value.split_once('=').map(|(key, _)| key).unwrap_or(value)
}

fn map_codex_arg_part_config_key(part: CodexArgPart) -> Option<String> {
    match part {
        CodexArgPart::Pair { value, .. } => Some(codex_config_pair_key(&value).to_string()),
        CodexArgPart::Standalone(_) => None,
    }
}

fn codex_model_config_pair_keys(args: &[String]) -> HashSet<String> {
    split_codex_arg_parts(args)
        .into_iter()
        .filter_map(map_codex_arg_part_config_key)
        .collect()
}

fn policy_config_pair_slice_for_root(root: &crate::providers::ProviderEntry) -> Option<&[String]> {
    let restrictions = root_tool_restrictions(root)?;
    let codex_restrictions = filter_codex_tool_restrictions(restrictions)?;
    Some(policy_config_pairs(codex_restrictions))
}

fn root_tool_restrictions(root: &crate::providers::ProviderEntry) -> Option<&ToolRestrictions> {
    root.tool_restrictions.as_ref()
}

fn filter_codex_tool_restrictions(restrictions: &ToolRestrictions) -> Option<&ToolRestrictions> {
    is_codex_tool_restrictions(restrictions).then_some(restrictions)
}

fn is_codex_tool_restrictions(restrictions: &ToolRestrictions) -> bool {
    restrictions.kind == ToolRestrictionKind::Codex
}

fn policy_config_pairs(restrictions: &ToolRestrictions) -> &[String] {
    restrictions.codex.config_pairs.as_slice()
}

fn is_empty_policy_pairs(policy_pairs: &[String]) -> bool {
    policy_pairs.is_empty()
}

fn parse_policy_config_pairs(policy_pairs: &[String]) -> Vec<PolicyConfigPair<'_>> {
    policy_pairs.iter().map(parse_policy_config_pair).collect()
}

fn parse_policy_config_pair(pair: &String) -> PolicyConfigPair<'_> {
    PolicyConfigPair {
        display: pair,
        key: codex_config_pair_key(pair),
    }
}

fn filter_overlapping_policy_pairs<'a>(
    policy_pairs: Vec<PolicyConfigPair<'a>>,
    model_keys: &HashSet<String>,
) -> Vec<PolicyConfigPair<'a>> {
    policy_pairs
        .into_iter()
        .filter(|pair| model_keys.contains(pair.key))
        .collect()
}

fn first_overlapping_policy_pair(
    policy_pairs: Vec<PolicyConfigPair<'_>>,
) -> Option<PolicyConfigPair<'_>> {
    policy_pairs.into_iter().next()
}

fn map_policy_pair_display(pair: PolicyConfigPair<'_>) -> String {
    pair.display.to_string()
}

fn codex_typed_policy_overlap(
    root: &crate::providers::ProviderEntry,
    model_args: &[String],
) -> Option<String> {
    let policy_pairs = non_empty_policy_config_pairs(root)?;
    let model_keys = codex_model_config_pair_keys(model_args);
    let policy_pairs = parse_policy_config_pairs(policy_pairs);
    let policy_pairs = filter_overlapping_policy_pairs(policy_pairs, &model_keys);
    first_overlapping_policy_pair(policy_pairs).map(map_policy_pair_display)
}

fn non_empty_policy_config_pairs(root: &crate::providers::ProviderEntry) -> Option<&[String]> {
    filter_non_empty_policy_pairs(policy_config_pair_slice_for_root(root)?)
}

fn filter_non_empty_policy_pairs(policy_pairs: &[String]) -> Option<&[String]> {
    (!is_empty_policy_pairs(policy_pairs)).then_some(policy_pairs)
}

pub(crate) fn validate_codex_model_arg_overlap(
    model_name: &str,
    model: &ModelConfig,
    providers: &crate::providers::ProvidersConfig,
) -> Result<(), String> {
    for (model_provider, root_codex) in codex_provider_pairs(model, providers) {
        validate_codex_provider_args_overlap(model_name, model_provider, root_codex)?;
        validate_codex_provider_policy_overlap(model_name, model_provider, root_codex)?;
        validate_codex_provider_interactive_overlap(model_name, model_provider, root_codex)?;
    }

    Ok(())
}

fn codex_provider_pairs<'a>(
    model: &'a ModelConfig,
    providers: &'a crate::providers::ProvidersConfig,
) -> Vec<(&'a ProviderConfig, &'a crate::providers::ProviderEntry)> {
    let root_pairs = model_provider_root_pairs(model, providers);
    let family_pairs = parse_codex_provider_families(root_pairs);
    let codex_families = filter_codex_provider_families(family_pairs);
    map_codex_provider_pairs(codex_families)
}

fn model_provider_root_pairs<'a>(
    model: &'a ModelConfig,
    providers: &'a crate::providers::ProvidersConfig,
) -> Vec<(&'a ProviderConfig, &'a crate::providers::ProviderEntry)> {
    model
        .providers
        .iter()
        .filter_map(|model_provider| model_provider_root_pair(model_provider, providers))
        .collect()
}

fn model_provider_root_pair<'a>(
    model_provider: &'a ProviderConfig,
    providers: &'a crate::providers::ProvidersConfig,
) -> Option<(&'a ProviderConfig, &'a crate::providers::ProviderEntry)> {
    let root = root_provider_entry(providers, &model_provider.name)?;
    Some(map_model_provider_root_pair(model_provider, root))
}

fn root_provider_entry<'a>(
    providers: &'a crate::providers::ProvidersConfig,
    name: &str,
) -> Option<&'a crate::providers::ProviderEntry> {
    providers.get(name)
}

fn map_model_provider_root_pair<'a>(
    model_provider: &'a ProviderConfig,
    root: &'a crate::providers::ProviderEntry,
) -> (&'a ProviderConfig, &'a crate::providers::ProviderEntry) {
    (model_provider, root)
}

fn parse_codex_provider_families<'a>(
    root_pairs: Vec<(&'a ProviderConfig, &'a crate::providers::ProviderEntry)>,
) -> Vec<CodexProviderFamily<'a>> {
    root_pairs
        .into_iter()
        .map(|(model_provider, root_codex)| parse_codex_provider_family(model_provider, root_codex))
        .collect()
}

fn parse_codex_provider_family<'a>(
    model_provider: &'a ProviderConfig,
    root_codex: &'a crate::providers::ProviderEntry,
) -> CodexProviderFamily<'a> {
    let root_family = derive_provider_name(
        root_codex
            .command
            .as_deref()
            .unwrap_or(&model_provider.name),
        &root_codex.args,
    );
    CodexProviderFamily {
        model_provider,
        root_codex,
        root_family,
    }
}

fn filter_codex_provider_families(
    family_pairs: Vec<CodexProviderFamily<'_>>,
) -> Vec<CodexProviderFamily<'_>> {
    family_pairs
        .into_iter()
        .filter(is_codex_provider_family_pair)
        .collect()
}

fn is_codex_provider_family_pair(family: &CodexProviderFamily<'_>) -> bool {
    is_codex_provider_family(&family.root_family, &family.model_provider.name)
}

fn is_codex_provider_family(root_family: &str, model_provider_name: &str) -> bool {
    root_family.starts_with("codex") || model_provider_name.starts_with("codex")
}

fn map_codex_provider_pairs<'a>(
    family_pairs: Vec<CodexProviderFamily<'a>>,
) -> Vec<(&'a ProviderConfig, &'a crate::providers::ProviderEntry)> {
    family_pairs
        .into_iter()
        .map(map_codex_provider_pair)
        .collect()
}

fn map_codex_provider_pair<'a>(
    family: CodexProviderFamily<'a>,
) -> (&'a ProviderConfig, &'a crate::providers::ProviderEntry) {
    (family.model_provider, family.root_codex)
}

fn validate_codex_provider_args_overlap(
    model_name: &str,
    model_provider: &ProviderConfig,
    root_codex: &crate::providers::ProviderEntry,
) -> Result<(), String> {
    let Some(display) = codex_arg_overlap(&root_codex.args, &model_provider.args) else {
        return Ok(());
    };
    Err(format_codex_args_duplicate_error(
        model_name,
        &model_provider.name,
        &display,
    ))
}

fn validate_codex_provider_policy_overlap(
    model_name: &str,
    model_provider: &ProviderConfig,
    root_codex: &crate::providers::ProviderEntry,
) -> Result<(), String> {
    let Some(display) = codex_typed_policy_overlap(root_codex, &model_provider.args) else {
        return Ok(());
    };
    Err(format_codex_policy_duplicate_error(
        model_name,
        &model_provider.name,
        &display,
    ))
}

fn validate_codex_provider_interactive_overlap(
    model_name: &str,
    model_provider: &ProviderConfig,
    root_codex: &crate::providers::ProviderEntry,
) -> Result<(), String> {
    let Some((root_interactive_args, model_interactive_args)) =
        codex_interactive_arg_pair(root_codex, model_provider)
    else {
        return Ok(());
    };
    let Some(display) = codex_arg_overlap(root_interactive_args, model_interactive_args) else {
        return Ok(());
    };
    Err(format_codex_interactive_duplicate_error(
        model_name,
        &model_provider.name,
        &display,
    ))
}

fn codex_interactive_arg_pair<'a>(
    root_codex: &'a crate::providers::ProviderEntry,
    model_provider: &'a ProviderConfig,
) -> Option<(&'a [String], &'a [String])> {
    let root_interactive_args = root_interactive_args(root_codex)?;
    let model_interactive_args = model_interactive_args(model_provider)?;
    let root_interactive_args = filter_non_empty_interactive_args(root_interactive_args)?;
    let model_interactive_args = filter_non_empty_interactive_args(model_interactive_args)?;
    Some(map_interactive_arg_pair(
        root_interactive_args,
        model_interactive_args,
    ))
}

fn root_interactive_args(root_codex: &crate::providers::ProviderEntry) -> Option<&[String]> {
    root_codex.interactive_args.as_deref()
}

fn model_interactive_args(model_provider: &ProviderConfig) -> Option<&[String]> {
    model_provider.interactive_args.as_deref()
}

fn filter_non_empty_interactive_args(args: &[String]) -> Option<&[String]> {
    (!args.is_empty()).then_some(args)
}

fn map_interactive_arg_pair<'a>(
    root_interactive_args: &'a [String],
    model_interactive_args: &'a [String],
) -> (&'a [String], &'a [String]) {
    (root_interactive_args, model_interactive_args)
}

fn format_codex_args_duplicate_error(
    model_name: &str,
    provider_name: &str,
    display: &str,
) -> String {
    format!(
        "Model {model_name} provider {provider_name}: args token \"{display}\" duplicates root [{provider_name}].args; leave provider-level Codex flags in providers.toml and keep only model-specific flags in model TOML; run `agents migrate-config` to repair existing files."
    )
}

fn format_codex_policy_duplicate_error(
    model_name: &str,
    provider_name: &str,
    display: &str,
) -> String {
    format!(
        "Model {model_name} provider {provider_name}: typed-policy config pair \"{display}\" duplicates model args; leave provider-level Codex policy in providers.toml and keep only model-specific flags in model TOML."
    )
}

fn format_codex_interactive_duplicate_error(
    model_name: &str,
    provider_name: &str,
    display: &str,
) -> String {
    format!(
        "Model {model_name} provider {provider_name}: interactive_args token \"{display}\" duplicates root [{provider_name}].interactive_args; leave provider-level Codex flags in providers.toml and keep only model-specific flags in model TOML; run `agents migrate-config` to repair existing files."
    )
}
