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
//!   - component: crates/oulipoly-config/src/providers/entry.rs
//!     role: intrinsic-surface
//!     Domain: provider-entry-runtime-shape
//!     Owns:
//!       - ProviderEntry public account schema and runtime ProviderConfig construction
//!       - Provider account child-environment additions and removals
//!       - provider command/argument family detection subordinate to account validation
//!       - tool restriction duplicate detection for provider account command surfaces
//! ```
//!
use crate::model::{
    InvocationMode, PromptMode, ProviderConfig, ResumeAcceptanceRules, ResumeStrategy,
    SessionCapture, SessionStorage, ToolRestrictionKind, ToolRestrictions,
};
use std::collections::BTreeMap;

/// One entry in `providers.toml`, keyed by the provider name.
#[derive(Debug, Clone)]
pub struct ProviderEntry {
    /// Shell command that prints JSON on stdout describing rolling-quota
    /// windows. Empty if the provider has no quota check wired up.
    pub quota_script: Option<String>,
    /// Optional shell command that hits the provider's API and triggers the
    /// CLI's own OAuth token refresh (e.g. `claude auth status`,
    /// `codex login status`). Run when `quota_script` fails or returns an
    /// empty windows list on a previously-populated provider, then
    /// `quota_script` is retried once. Provider-agnostic: the runner does
    /// not implement OAuth itself; it delegates to the CLI.
    pub auth_refresh_command: Option<String>,
    /// Base executable for this provider account. Model-specific flags are
    /// appended from the selected model TOML at spawn time.
    pub command: Option<String>,
    pub args: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub unset_environment: Vec<String>,
    pub interactive_args: Option<Vec<String>>,
    pub prompt_mode: PromptMode,
    pub resume: Option<ResumeStrategy>,
    pub session_capture: Option<SessionCapture>,
    pub resume_acceptance: Option<ResumeAcceptanceRules>,
    pub session_storage: Option<SessionStorage>,
    pub system_prompt_override: Option<String>,
    pub tool_restrictions: Option<ToolRestrictions>,
    pub invocation_mode: InvocationMode,
}

impl Default for ProviderEntry {
    fn default() -> Self {
        Self {
            quota_script: None,
            auth_refresh_command: None,
            command: None,
            args: Vec::new(),
            environment: BTreeMap::new(),
            unset_environment: Vec::new(),
            interactive_args: None,
            prompt_mode: PromptMode::Stdin,
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: InvocationMode::Headless,
        }
    }
}
#[allow(dead_code)]
impl ProviderEntry {
    pub(super) fn validate(&self, name: &str) -> Result<(), String> {
        if let Some(resume) = &self.resume {
            resume
                .validate()
                .map_err(|e| format_provider_context_error(name, &e))?;
        }
        if let Some(capture) = &self.session_capture {
            capture
                .validate()
                .map_err(|e| format_provider_context_error(name, &e))?;
        }
        if let Some(storage) = &self.session_storage {
            storage
                .validate()
                .map_err(|e| format_provider_context_error(name, &e))?;
        }
        self.validate_tool_restrictions(name)?;
        Ok(())
    }

    fn validate_tool_restrictions(&self, name: &str) -> Result<(), String> {
        let Some(restrictions) = &self.tool_restrictions else {
            return Ok(());
        };

        validate_tool_restriction_family(name, self, restrictions)?;
        match restrictions.kind {
            ToolRestrictionKind::Claude => validate_claude_entry_tool_restrictions(
                name,
                self.command.as_deref(),
                &self.args,
                self.interactive_args.as_deref(),
                restrictions,
            )?,
            ToolRestrictionKind::Codex => validate_codex_entry_tool_restrictions(
                name,
                self.command.as_deref(),
                &self.args,
                self.interactive_args.as_deref(),
                restrictions,
            )?,
        }

        Ok(())
    }

    pub fn effective_provider(
        &self,
        name: &str,
        model_provider: Option<&ProviderConfig>,
    ) -> Result<(ProviderConfig, PromptMode), String> {
        let command = self
            .command
            .clone()
            .ok_or_else(|| format_provider_missing_command_error(name))?;
        Ok((
            map_provider_entry_to_provider_config(self, name, command, model_provider),
            self.prompt_mode,
        ))
    }
}

fn validate_tool_restriction_family(
    name: &str,
    entry: &ProviderEntry,
    restrictions: &ToolRestrictions,
) -> Result<(), String> {
    let Some(family) = provider_family(
        name,
        entry.command.as_deref(),
        &entry.args,
        entry.interactive_args.as_deref(),
    ) else {
        return Ok(());
    };
    if restrictions.kind == family {
        return Ok(());
    }
    Err(format_tool_restriction_kind_mismatch_error(
        name,
        family,
        restrictions.kind,
    ))
}

fn validate_claude_entry_tool_restrictions(
    name: &str,
    command: Option<&str>,
    args: &[String],
    interactive_args: Option<&[String]>,
    restrictions: &ToolRestrictions,
) -> Result<(), String> {
    validate_claude_tool_restriction_shape(name, restrictions)?;
    if let Some(command_tokens) = parse_command_tokens(command) {
        validate_claude_duplicates(name, &command_tokens, "command", restrictions)?;
    }
    validate_claude_duplicates(name, args, "args", restrictions)?;
    if let Some(interactive_args) = interactive_args {
        validate_claude_duplicates(name, interactive_args, "interactive_args", restrictions)?;
    }
    Ok(())
}

fn validate_claude_tool_restriction_shape(
    name: &str,
    restrictions: &ToolRestrictions,
) -> Result<(), String> {
    if !restrictions.codex.is_empty() {
        return Err(format_claude_kind_codex_not_empty_error(name));
    }
    if !restrictions.claude.allowed_tools.is_empty()
        && !restrictions.claude.disallowed_tools.is_empty()
    {
        return Err(format_claude_mutually_exclusive_tools_error(name));
    }
    Ok(())
}

fn validate_codex_entry_tool_restrictions(
    name: &str,
    command: Option<&str>,
    args: &[String],
    interactive_args: Option<&[String]>,
    restrictions: &ToolRestrictions,
) -> Result<(), String> {
    validate_codex_inactive_restrictions_empty(name, restrictions)?;
    if let Some(command_tokens) = parse_command_tokens(command) {
        validate_codex_duplicates(name, &command_tokens, "command", restrictions)?;
    }
    validate_codex_duplicates(name, args, "args", restrictions)?;
    if let Some(interactive_args) = interactive_args {
        validate_codex_duplicates(name, interactive_args, "interactive_args", restrictions)?;
    }
    validate_codex_tool_restriction_allowlists(name, restrictions)
}

fn validate_codex_inactive_restrictions_empty(
    name: &str,
    restrictions: &ToolRestrictions,
) -> Result<(), String> {
    if !restrictions.claude.is_empty() {
        return Err(format_codex_kind_claude_not_empty_error(name));
    }
    Ok(())
}

fn validate_codex_tool_restriction_allowlists(
    name: &str,
    restrictions: &ToolRestrictions,
) -> Result<(), String> {
    validate_codex_config_pair_allowlist(name, &restrictions.codex.config_pairs)?;
    validate_codex_disabled_feature_allowlist(name, &restrictions.codex.disabled_features)
}

fn validate_codex_config_pair_allowlist(name: &str, pairs: &[String]) -> Result<(), String> {
    for pair in pairs {
        let key = parse_config_pair_key(pair);
        if CODEX_CONFIG_PAIR_ALLOWLIST.contains(&key) {
            continue;
        }
        return Err(format_codex_config_pair_allowlist_error(name, key));
    }
    Ok(())
}

fn validate_codex_disabled_feature_allowlist(
    name: &str,
    features: &[String],
) -> Result<(), String> {
    for feature in features {
        if CODEX_DISABLED_FEATURE_ALLOWLIST.contains(&feature.as_str()) {
            continue;
        }
        return Err(format_codex_disabled_feature_allowlist_error(name, feature));
    }
    Ok(())
}

fn parse_command_tokens(command: Option<&str>) -> Option<Vec<String>> {
    command.map(shell_split)
}

fn parse_config_pair_key(pair: &str) -> &str {
    pair.split_once('=').map(|(key, _)| key).unwrap_or(pair)
}

fn map_provider_entry_to_provider_config(
    entry: &ProviderEntry,
    name: &str,
    command: String,
    model_provider: Option<&ProviderConfig>,
) -> ProviderConfig {
    ProviderConfig {
        name: name.to_string(),
        command,
        args: map_effective_args(&entry.args, model_provider),
        environment: entry.environment.clone(),
        unset_environment: entry.unset_environment.clone(),
        interactive_args: map_effective_interactive_args(
            entry.interactive_args.as_deref(),
            model_provider,
        ),
        resume: entry.resume.clone(),
        session_capture: entry.session_capture.clone(),
        resume_acceptance: entry.resume_acceptance.clone(),
        session_storage: entry.session_storage.clone(),
        system_prompt_override: entry.system_prompt_override.clone(),
        tool_restrictions: entry.tool_restrictions.clone(),
        invocation_mode: entry.invocation_mode,
    }
}

fn map_effective_args(
    base_args: &[String],
    model_provider: Option<&ProviderConfig>,
) -> Vec<String> {
    let mut args = base_args.to_vec();
    if let Some(model_provider) = model_provider {
        args.extend(model_provider.args.clone());
    }
    args
}

fn map_effective_interactive_args(
    base_interactive_args: Option<&[String]>,
    model_provider: Option<&ProviderConfig>,
) -> Option<Vec<String>> {
    let base_interactive_args = base_interactive_args?;
    Some(map_effective_interactive_args_from_base(
        base_interactive_args,
        model_provider,
    ))
}

fn map_effective_interactive_args_from_base(
    base_interactive_args: &[String],
    model_provider: Option<&ProviderConfig>,
) -> Vec<String> {
    let mut args = base_interactive_args.to_vec();
    if let Some(model_provider) = model_provider
        && let Some(model_args) = model_provider.interactive_args.as_ref()
    {
        args.extend(model_args.clone());
    }
    args
}

fn format_tool_restriction_kind_mismatch_error(
    name: &str,
    discovered: ToolRestrictionKind,
    configured: ToolRestrictionKind,
) -> String {
    format!(
        "providers.toml provider {name}: tool_restrictions.kind mismatch: discovered provider family {discovered}, configured kind {configured}"
    )
}

fn format_claude_kind_codex_not_empty_error(name: &str) -> String {
    format!(
        "providers.toml provider {name}: tool_restrictions.codex must be empty when kind = \"claude\""
    )
}

fn format_claude_mutually_exclusive_tools_error(name: &str) -> String {
    format!(
        "providers.toml provider {name}: tool_restrictions.claude.allowed_tools and tool_restrictions.claude.disallowed_tools are mutually exclusive"
    )
}

fn format_codex_kind_claude_not_empty_error(name: &str) -> String {
    format!(
        "providers.toml provider {name}: tool_restrictions.claude must be empty when kind = \"codex\""
    )
}

fn format_codex_config_pair_allowlist_error(name: &str, key: &str) -> String {
    format!(
        "providers.toml provider {name}: tool_restrictions.codex.config_pairs key {key:?} has no allowlisted Codex config pair"
    )
}

fn format_codex_disabled_feature_allowlist_error(name: &str, feature: &str) -> String {
    format!(
        "providers.toml provider {name}: tool_restrictions.codex.disabled_features feature {feature:?} is not allowlisted"
    )
}

fn format_provider_missing_command_error(name: &str) -> String {
    format!("provider {name} has no command in providers.toml")
}

fn format_provider_context_error(name: &str, error: &str) -> String {
    format!("providers.toml provider {name}: {error}")
}

#[allow(dead_code)]
const CODEX_CONFIG_PAIR_ALLOWLIST: &[&str] = &[];
#[allow(dead_code)]
const CODEX_DISABLED_FEATURE_ALLOWLIST: &[&str] = &["web_search"];

#[allow(dead_code)]
pub(super) fn provider_family(
    name: &str,
    command: Option<&str>,
    args: &[String],
    interactive_args: Option<&[String]>,
) -> Option<ToolRestrictionKind> {
    let executable = provider_executable_token(name, command, args, interactive_args);
    let basename = parse_executable_basename(&executable);
    map_executable_basename_to_tool_restriction_kind(basename)
}

#[allow(dead_code)]
fn executable_token<'a>(tokens: &'a [&'a str]) -> Option<&'a str> {
    tokens
        .iter()
        .enumerate()
        .find_map(|(index, token)| is_executable_token(index, tokens, token).then_some(*token))
}

fn is_executable_token(index: usize, tokens: &[&str], token: &str) -> bool {
    !is_env_wrapper_token(token)
        && !is_flag_token(token)
        && !is_flag_operand(index, tokens)
        && !is_uppercase_assignment(token)
}

fn is_env_wrapper_token(token: &str) -> bool {
    token == "env" || token.ends_with("/env")
}

fn is_flag_token(token: &str) -> bool {
    token.strip_prefix('-').is_some()
}

fn is_flag_operand(index: usize, tokens: &[&str]) -> bool {
    let mut cursor = 0;
    while cursor < tokens.len() {
        if is_operand_taking_flag_token(tokens[cursor]) {
            if cursor + 1 == index {
                return true;
            }
            cursor += 2;
        } else {
            cursor += 1;
        }
    }
    false
}

fn is_operand_taking_flag_token(token: &str) -> bool {
    token
        .strip_prefix('-')
        .is_some_and(|rest| matches!(rest, "u" | "e" | "S"))
}

fn is_uppercase_assignment(token: &str) -> bool {
    token.contains('=') && token.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

fn provider_executable_token(
    name: &str,
    command: Option<&str>,
    args: &[String],
    interactive_args: Option<&[String]>,
) -> String {
    let tokens = provider_invocation_tokens(command, args, interactive_args);
    let token_refs = map_string_tokens_to_refs(&tokens);
    executable_token(&token_refs).unwrap_or(name).to_string()
}

fn provider_invocation_tokens(
    command: Option<&str>,
    args: &[String],
    interactive_args: Option<&[String]>,
) -> Vec<String> {
    let mut tokens = Vec::new();
    if let Some(command) = command {
        tokens.extend(shell_split(command));
    }
    tokens.extend(args.iter().cloned());
    if let Some(interactive_args) = interactive_args {
        tokens.extend(interactive_args.iter().cloned());
    }
    tokens
}

fn map_string_tokens_to_refs(tokens: &[String]) -> Vec<&str> {
    tokens.iter().map(String::as_str).collect()
}

fn parse_executable_basename(executable: &str) -> &str {
    executable.rsplit('/').next().unwrap_or(executable)
}

fn map_executable_basename_to_tool_restriction_kind(basename: &str) -> Option<ToolRestrictionKind> {
    if basename.starts_with("claude") {
        Some(ToolRestrictionKind::Claude)
    } else if basename.starts_with("codex") {
        Some(ToolRestrictionKind::Codex)
    } else {
        None
    }
}

pub(crate) fn shell_split(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in s.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

#[allow(dead_code)]
fn validate_claude_duplicates(
    name: &str,
    args: &[String],
    field: &str,
    restrictions: &ToolRestrictions,
) -> Result<(), String> {
    validate_claude_tool_duplicates(
        name,
        args,
        field,
        "tool_restrictions.claude.allowed_tools",
        &["--allowedTools", "--allowed-tools"],
        &restrictions.claude.allowed_tools,
    )?;
    validate_claude_tool_duplicates(
        name,
        args,
        field,
        "tool_restrictions.claude.disallowed_tools",
        &["--disallowedTools", "--disallowed-tools"],
        &restrictions.claude.disallowed_tools,
    )
}

#[allow(dead_code)]
fn validate_claude_tool_duplicates(
    name: &str,
    args: &[String],
    arg_field: &str,
    policy_field: &str,
    flags: &[&str],
    policy_tools: &[String],
) -> Result<(), String> {
    for (flag, raw_value) in claude_flag_values(args, flags) {
        let parsed_tools = parse_comma_separated_payload(raw_value);
        for raw_tool in filter_non_empty_tokens(parsed_tools) {
            if policy_tools.iter().any(|tool| tool == raw_tool) {
                return Err(format_claude_duplicate_tool_error(
                    name,
                    policy_field,
                    raw_tool,
                    arg_field,
                    &flag,
                ));
            }
        }
    }
    Ok(())
}

#[allow(dead_code)]
fn claude_flag_values<'a>(args: &'a [String], flags: &[&str]) -> Vec<(String, &'a str)> {
    collect_flag_values(args, flags)
}

#[allow(dead_code)]
fn validate_codex_duplicates(
    name: &str,
    args: &[String],
    field: &str,
    restrictions: &ToolRestrictions,
) -> Result<(), String> {
    validate_codex_config_duplicates(name, args, field, &restrictions.codex.config_pairs)?;
    validate_codex_disabled_feature_duplicates(
        name,
        args,
        field,
        &restrictions.codex.disabled_features,
    )
}

#[allow(dead_code)]
fn validate_codex_config_duplicates(
    name: &str,
    args: &[String],
    field: &str,
    policy_pairs: &[String],
) -> Result<(), String> {
    for (flag, raw_value) in codex_flag_values(args, &["-c", "--config"]) {
        let raw_key = parse_config_pair_key(raw_value);
        for policy_pair in policy_pairs {
            let policy_key = parse_config_pair_key(policy_pair);
            if policy_key == raw_key {
                return Err(format_codex_duplicate_config_key_error(
                    name, raw_key, field, &flag,
                ));
            }
        }
    }
    Ok(())
}

#[allow(dead_code)]
fn validate_codex_disabled_feature_duplicates(
    name: &str,
    args: &[String],
    field: &str,
    policy_features: &[String],
) -> Result<(), String> {
    for (flag, raw_value) in codex_flag_values(args, &["--disable"]) {
        let parsed_features = parse_comma_separated_payload(raw_value);
        for raw_feature in filter_non_empty_tokens(parsed_features) {
            if policy_features.iter().any(|feature| feature == raw_feature) {
                return Err(format_codex_duplicate_disabled_feature_error(
                    name,
                    raw_feature,
                    field,
                    &flag,
                ));
            }
        }
    }
    Ok(())
}

#[allow(dead_code)]
fn codex_flag_values<'a>(args: &'a [String], flags: &[&str]) -> Vec<(String, &'a str)> {
    collect_flag_values(args, flags)
}

fn collect_flag_values<'a>(args: &'a [String], flags: &[&str]) -> Vec<(String, &'a str)> {
    matching_flag_positions(args, flags)
        .into_iter()
        .filter_map(|position| parse_flag_value_at(args, position))
        .map(map_flag_value_pair)
        .collect()
}

fn matching_flag_positions(args: &[String], flags: &[&str]) -> Vec<usize> {
    let mut positions = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if is_separated_flag_position(args, i, flags) {
            positions.push(i);
            i += 2;
            continue;
        }
        if is_inline_flag_position(args, i, flags) {
            positions.push(i);
        }
        i += 1;
    }
    positions
}

fn is_separated_flag_position(args: &[String], position: usize, flags: &[&str]) -> bool {
    flags.contains(&args[position].as_str()) && args.get(position + 1).is_some()
}

fn is_inline_flag_position(args: &[String], position: usize, flags: &[&str]) -> bool {
    parse_inline_flag(args[position].as_str()).is_some_and(|(flag, _)| flags.contains(&flag))
}

struct ParsedFlagValue<'a> {
    flag: &'a str,
    value: &'a str,
}

fn parse_flag_value_at(args: &[String], position: usize) -> Option<ParsedFlagValue<'_>> {
    parse_inline_flag_value_at(args, position)
        .or_else(|| parse_separated_flag_value_at(args, position))
}

fn parse_separated_flag_value_at(args: &[String], position: usize) -> Option<ParsedFlagValue<'_>> {
    Some(ParsedFlagValue {
        flag: args.get(position)?.as_str(),
        value: args.get(position + 1)?.as_str(),
    })
}

fn parse_inline_flag_value_at(args: &[String], position: usize) -> Option<ParsedFlagValue<'_>> {
    parse_inline_flag(args.get(position)?.as_str())
        .map(|(flag, value)| ParsedFlagValue { flag, value })
}

fn parse_inline_flag(arg: &str) -> Option<(&str, &str)> {
    arg.split_once('=')
}

fn map_flag_value_pair(parsed: ParsedFlagValue<'_>) -> (String, &str) {
    (parsed.flag.to_string(), parsed.value)
}

fn parse_comma_separated_payload(payload: &str) -> Vec<&str> {
    payload.split(',').map(str::trim).collect()
}

fn filter_non_empty_tokens(tokens: Vec<&str>) -> Vec<&str> {
    tokens
        .into_iter()
        .filter(|token| !token.is_empty())
        .collect()
}

fn format_claude_duplicate_tool_error(
    name: &str,
    policy_field: &str,
    tool: &str,
    arg_field: &str,
    flag: &str,
) -> String {
    format!(
        "providers.toml provider {name}: {policy_field} contains duplicate tool {tool:?} already present in {arg_field} flag {flag}"
    )
}

fn format_codex_duplicate_config_key_error(
    name: &str,
    key: &str,
    field: &str,
    flag: &str,
) -> String {
    format!(
        "providers.toml provider {name}: tool_restrictions.codex.config_pairs contains duplicate key {key:?} already present in {field} flag {flag}"
    )
}

fn format_codex_duplicate_disabled_feature_error(
    name: &str,
    feature: &str,
    field: &str,
    flag: &str,
) -> String {
    format!(
        "providers.toml provider {name}: tool_restrictions.codex.disabled_features contains duplicate feature {feature:?} already present in {field} flag {flag}"
    )
}
