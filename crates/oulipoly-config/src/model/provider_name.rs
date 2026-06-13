//! ## Declared roles
//!
//! - filter
//! - mapper
//! - orchestration
//! - parser
//! - predicate
//!
//! Role set: { filter, mapper, orchestration, parser, predicate }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-config/src/model/provider_name.rs
//!     role: intrinsic-surface
//!     Domain: model-provider-name-derivation
//!     Owns:
//!       - provider-name derivation from command and argument token streams
//!       - shell-split command token interpretation subordinate to model provider naming
//! ```
//!
/// Derive a provider name from a command + args vector.
///
/// The heuristic picks the first token that looks like an executable name —
/// skipping flags, `env` and its flag arguments (e.g. the VAR in `-u VAR`),
/// and env-var assignments of the form `FOO=bar`. Returns the command string
/// itself as a last-resort fallback.
pub fn derive_provider_name(command: &str, args: &[String]) -> String {
    let tokens = parse_provider_name_tokens(command, args);
    let candidates = executable_candidate_tokens(&tokens);
    map_provider_name(candidates, command)
}

fn parse_provider_name_tokens(command: &str, args: &[String]) -> Vec<String> {
    let command_tokens = crate::providers::shell_split(command);
    let mut all: Vec<String> = Vec::with_capacity(command_tokens.len() + args.len());
    if command_tokens.is_empty() {
        all.push(command.to_string());
    } else {
        all.extend(command_tokens);
    }
    all.extend(args.iter().cloned());
    all
}

#[derive(Clone, Copy)]
enum ExecutableTokenClass {
    Wrapper,
    Flag,
    FlagWithValue,
    EnvAssignment,
    Executable,
}

fn executable_candidate_tokens(tokens: &[String]) -> Vec<&String> {
    let mut candidates = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let token_class = parse_executable_token_class(tokens[i].as_str());
        if is_executable_token_class(token_class) {
            candidates.push(&tokens[i]);
        }
        i += executable_token_advance(token_class);
    }
    candidates
}

fn parse_executable_token_class(token: &str) -> ExecutableTokenClass {
    if token == "env" || token.ends_with("/env") {
        return ExecutableTokenClass::Wrapper;
    }
    if token.strip_prefix("--").is_some() {
        return ExecutableTokenClass::Flag;
    }
    if let Some(rest) = token.strip_prefix('-') {
        return parse_short_flag_token_class(rest);
    }
    if is_uppercase_env_assignment(token) {
        return ExecutableTokenClass::EnvAssignment;
    }
    ExecutableTokenClass::Executable
}

fn parse_short_flag_token_class(rest: &str) -> ExecutableTokenClass {
    if matches!(rest, "u" | "e" | "S") {
        return ExecutableTokenClass::FlagWithValue;
    }
    ExecutableTokenClass::Flag
}

fn is_uppercase_env_assignment(token: &str) -> bool {
    token.contains('=') && token.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

fn is_executable_token_class(token_class: ExecutableTokenClass) -> bool {
    matches!(token_class, ExecutableTokenClass::Executable)
}

fn executable_token_advance(token_class: ExecutableTokenClass) -> usize {
    match token_class {
        ExecutableTokenClass::FlagWithValue => 2,
        _ => 1,
    }
}

fn map_provider_name(candidates: Vec<&String>, fallback: &str) -> String {
    candidates
        .first()
        .map(|token| (*token).clone())
        .unwrap_or_else(|| fallback.to_string())
}
