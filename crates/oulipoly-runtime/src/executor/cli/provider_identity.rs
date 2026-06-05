//! ## Declared roles
//!
//! Roles: parser, mapper, predicate, accessor, filter.
//!
//! - parser: [`shell_split`] tokenizes a `command` string into argv tokens
//!   using a double-quote-only shell-lite grammar.
//! - parser: [`provider_executable_name`] walks tokens after env-prefix and
//!   option skipping to locate the executable token.
//! - mapper: [`provider_name`] returns the last-token convention from
//!   `shell_split`.
//! - mapper: [`ProviderRecognizer::for_provider`] maps a [`ProviderConfig`]
//!   onto the bounded provider-identity domain.
//! - predicate: [`ProviderRecognizer::recognize`] dispatches the bounded
//!   identity to the matching terminal-signal recognizer.
//! - accessor: [`command_provider_basename`].
//! - filter: [`executable_token_from_tokens`].
//!
//! ## ACR-205 intrinsic-surface declaration (PP-002)
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/provider_identity.rs
//!     role: intrinsic-surface
//!     Domain: provider_identity
//!     Owns:
//!       - provider_name_prefix_recognition
//!       - provider_command_executable_token_recognition
//!       - command_provider_basename
//!       - shell_split
//!       - provider_policy_kind_fail_closed_inference
//!       - terminal_signal_recognizer_dispatch
//! ```
//!
//! The runtime owns the provider-identity domain as an intrinsic surface.
//! The bounded set of recognized providers is **exactly four**:
//!
//! - `claude` — selected when `provider.name` starts with `"claude"` OR when
//!   the recognized command executable basename starts with `"claude"`.
//! - `codex` — selected when `provider.name` starts with `"codex"` OR when
//!   the recognized command executable basename starts with `"codex"`.
//! - `opencode` — selected when `provider.name` starts with `"opencode"` OR when
//!   the recognized command executable basename starts with `"opencode"`.
//! - `openai_compat` — the default fallback for any provider that does not
//!   match Claude, Codex, or OpenCode.
//!
//! The recognition rule walks tokens after `shell_split`-splitting
//! `provider.command` and concatenating `provider.args`; `env`/`/.../env`
//! invocations, long options (`--foo`), short options (`-u`/`-e`/`-S` plus
//! their argument), other short options, and `KEY=value` env assignments
//! are skipped to find the first true executable token. The token is then
//! basename-normalized via [`command_provider_basename`].
//!
//! Fail-closed behavior: provider-policy attempts on a provider with policy
//! configured but an unrecognized identity return a canonical
//! `"defines policy but its ecosystem could not be inferred"` error and
//! emit no argv. Cross-kind misconfiguration (e.g., Claude `kind` with
//! Codex settings populated) returns a canonical kind-mismatch error.
//!
//! Any future widening of the runtime identity domain MUST update both
//! this declaration and `tests/age_164_c4_policy_identity.rs`.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/provider_identity.rs
//!     role: adapter
//!     Translates:
//!       - provider-command-token-contract
//!       - terminal-signal-recognizer-contract
//!       - provider-policy-kind-contract
//! ```

use crate::executor::terminal_signal::{TerminalSignal, TerminalSignalEvidence};
use oulipoly_config::{ProviderConfig, ToolRestrictionKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProviderRecognizer {
    Claude,
    Codex,
    OpenCode,
    OpenAiCompat,
}

impl ProviderRecognizer {
    pub(super) fn for_provider(provider: &ProviderConfig) -> Self {
        let command_provider = provider_executable_name(provider)
            .unwrap_or_else(|| command_provider_basename(provider_name(&provider.command)));
        if provider.name.starts_with("claude") || command_provider.starts_with("claude") {
            Self::Claude
        } else if provider.name.starts_with("codex") || command_provider.starts_with("codex") {
            Self::Codex
        } else if provider.name.starts_with("opencode") || command_provider.starts_with("opencode")
        {
            Self::OpenCode
        } else {
            Self::OpenAiCompat
        }
    }

    pub(super) fn recognize(&self, evidence: &TerminalSignalEvidence<'_>) -> TerminalSignal {
        use crate::executor::terminal_signal::TerminalSignalRecognizer;
        match self {
            Self::Claude => crate::executor::providers::claude::Recognizer.recognize(evidence),
            Self::Codex => crate::executor::providers::codex::Recognizer.recognize(evidence),
            Self::OpenCode => crate::executor::providers::opencode::Recognizer.recognize(evidence),
            Self::OpenAiCompat => {
                crate::executor::providers::openai_compat::Recognizer.recognize(evidence)
            }
        }
    }
}

pub fn shell_split(s: &str) -> Vec<String> {
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

pub fn provider_name(command: &str) -> String {
    shell_split(command)
        .last()
        .cloned()
        .unwrap_or_else(|| command.to_string())
}

pub(super) fn command_provider_basename(provider: String) -> String {
    std::path::Path::new(&provider)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(provider.as_str())
        .to_string()
}

pub(super) fn provider_executable_name(provider: &ProviderConfig) -> Option<String> {
    let tokens = provider_command_tokens(provider);
    executable_token_from_tokens(&tokens).map(command_provider_basename)
}

fn provider_command_tokens(provider: &ProviderConfig) -> Vec<String> {
    let mut tokens = shell_split(&provider.command);
    tokens.extend(provider.args.iter().cloned());
    tokens
}

fn executable_token_from_tokens(tokens: &[String]) -> Option<String> {
    let mut i = 0;
    while i < tokens.len() {
        let token = tokens[i].as_str();
        if token_is_env_invocation(token) {
            i += 1;
            continue;
        }
        if token_is_long_option(token) {
            i += 1;
            continue;
        }
        if let Some(skip_count) = short_option_skip_count(token) {
            i += skip_count;
            continue;
        }
        if token_is_env_assignment(token) {
            i += 1;
            continue;
        }

        return Some(token.to_string());
    }

    None
}

fn token_is_env_invocation(token: &str) -> bool {
    token == "env" || token.ends_with("/env")
}

fn token_is_long_option(token: &str) -> bool {
    token
        .strip_prefix("--")
        .is_some_and(|rest| !rest.is_empty())
}

fn short_option_skip_count(token: &str) -> Option<usize> {
    let rest = token.strip_prefix('-')?;
    Some(if matches!(rest, "u" | "e" | "S") {
        2
    } else {
        1
    })
}

fn token_is_env_assignment(token: &str) -> bool {
    token.contains('=') && token.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

pub(super) fn provider_policy_kind(provider: &ProviderConfig) -> Option<ToolRestrictionKind> {
    explicit_provider_policy_kind(provider).or_else(|| inferred_provider_policy_kind(provider))
}

fn explicit_provider_policy_kind(provider: &ProviderConfig) -> Option<ToolRestrictionKind> {
    provider
        .tool_restrictions
        .as_ref()
        .map(|restrictions| restrictions.kind)
}

fn inferred_provider_policy_kind(provider: &ProviderConfig) -> Option<ToolRestrictionKind> {
    let command_provider = provider_executable_name(provider)
        .unwrap_or_else(|| command_provider_basename(provider_name(&provider.command)));
    if provider.name.starts_with("claude") || command_provider.starts_with("claude") {
        Some(ToolRestrictionKind::Claude)
    } else if provider.name.starts_with("codex") || command_provider.starts_with("codex") {
        Some(ToolRestrictionKind::Codex)
    } else {
        None
    }
}
