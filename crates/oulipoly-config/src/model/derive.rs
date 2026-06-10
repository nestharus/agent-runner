//! ## Declared roles
//!
//! `mapper`, `predicate`
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-config/src/model/derive.rs
//!     role: intrinsic-surface
//!     Domain: model_provider_session_config
//!     Owns:
//!       - provider derivation
//! ```

/// Derive a provider name from a command + args vector.
///
/// The heuristic picks the first token that looks like an executable name —
/// skipping flags, `env` and its flag arguments (e.g. the VAR in `-u VAR`),
/// and env-var assignments of the form `FOO=bar`. Returns the command string
/// itself as a last-resort fallback.
pub fn derive_provider_name(command: &str, args: &[String]) -> String {
    let command_tokens = provider_command_tokens(command);
    let all = provider_candidate_tokens(command, &command_tokens, args);

    let mut i = 0;
    while i < all.len() {
        let t = all[i];
        if provider_token_is_env_wrapper(t) {
            i += 1;
            continue;
        }

        if provider_token_is_long_flag(t) {
            i += 1;
            continue;
        }

        if let Some(rest) = t.strip_prefix('-') {
            if env_short_flag_consumes_value(rest) {
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }

        if provider_token_is_env_assignment(t) {
            i += 1;
            continue;
        }

        return t.to_string();
    }
    command.to_string()
}

fn provider_command_tokens(command: &str) -> Vec<String> {
    crate::providers::shell_split(command)
}

fn provider_candidate_tokens<'a>(
    command: &'a str,
    command_tokens: &'a [String],
    args: &'a [String],
) -> Vec<&'a str> {
    let mut all = Vec::with_capacity(command_tokens.len() + args.len());
    if command_tokens.is_empty() {
        all.push(command);
    } else {
        all.extend(command_tokens.iter().map(String::as_str));
    }
    all.extend(args.iter().map(String::as_str));
    all
}

fn provider_token_is_env_wrapper(token: &str) -> bool {
    token == "env" || token.ends_with("/env")
}

fn provider_token_is_long_flag(token: &str) -> bool {
    token
        .strip_prefix("--")
        .is_some_and(|rest| !rest.is_empty())
}

fn env_short_flag_consumes_value(flag_without_dash: &str) -> bool {
    matches!(flag_without_dash, "u" | "e" | "S")
}

fn provider_token_is_env_assignment(token: &str) -> bool {
    token.contains('=') && token.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}
