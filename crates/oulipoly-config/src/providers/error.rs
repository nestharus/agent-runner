//! ## Declared roles
//!
//! - formatter
//! - mapper
//! - predicate
//!
//! Role set: { formatter, mapper, predicate }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-config/src/providers/error.rs
//!     role: intrinsic-surface
//!     Domain: providers-load-error-reporting
//!     Owns:
//!       - LoadError parse, IO, invocation-mode, and provider policy error surface
//!       - std::io error carrier subordinated to providers.toml loading failures
//!       - conversion and equality semantics for provider load diagnostics
//! ```
//!
use std::fmt;

#[derive(Debug)]
pub enum LoadError {
    Io(std::io::Error),
    Toml(String),
    InvocationMode { provider: String, value: String },
    UnsafeProxyClaudeToolsRestrict { provider: String },
    Other(String),
}

impl LoadError {
    pub fn contains(&self, needle: &str) -> bool {
        self.to_string().contains(needle)
    }
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "io error reading providers.toml: {err}"),
            Self::Toml(err) => write!(f, "toml parse error in providers.toml: {err}"),
            Self::InvocationMode { provider, value } => write!(
                f,
                "provider {provider}: invalid invocation_mode `{value}`; valid modes are: `headless`, `proxy`"
            ),
            Self::UnsafeProxyClaudeToolsRestrict { provider } => write!(
                f,
                "provider {provider}: proxy-mode Claude must not use `--tools mcp__...`; use `--allowedTools` or omit the filter (AGE-104)"
            ),
            Self::Other(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<std::io::Error> for LoadError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<LoadError> for String {
    fn from(value: LoadError) -> Self {
        value.to_string()
    }
}

impl PartialEq for LoadError {
    fn eq(&self, other: &Self) -> bool {
        use LoadError::*;
        match (self, other) {
            (Io(left), Io(right)) => left.to_string() == right.to_string(),
            (Toml(left), Toml(right)) => left == right,
            (
                InvocationMode {
                    provider: left_provider,
                    value: left_value,
                },
                InvocationMode {
                    provider: right_provider,
                    value: right_value,
                },
            ) => left_provider == right_provider && left_value == right_value,
            (
                UnsafeProxyClaudeToolsRestrict { provider: left },
                UnsafeProxyClaudeToolsRestrict { provider: right },
            ) => left == right,
            (Other(left), Other(right)) => left == right,
            _ => false,
        }
    }
}

impl Eq for LoadError {}
