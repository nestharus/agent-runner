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
//!   - component: crates/oulipoly-config/src/model/error.rs
//!     role: intrinsic-surface
//!     Domain: model-error-reporting
//!     Owns:
//!       - ModelError parse, validation, IO, and provider implementation reference error surface
//!       - std::io and ProviderImplementationRefError carriers subordinated to model loading failures
//!       - model-name remapping for diagnostics emitted from model TOML processing
//! ```
//!
use crate::provider_implementation_ref::ProviderImplementationRefError;
use std::fmt;

#[derive(Debug)]
pub enum ModelError {
    Io(std::io::Error),
    Toml(String, String),
    ProviderImplementationRef {
        model: String,
        source: ProviderImplementationRefError,
    },
    InvocationModeIsRootOnly {
        model: String,
        provider: String,
    },
    Other(String, String),
}

impl ModelError {
    pub(super) fn with_model_name(self, name: &str) -> Self {
        match self {
            Self::Toml(model, err) if model == "<unknown>" => Self::Toml(name.to_string(), err),
            Self::ProviderImplementationRef { model, source } if model == "<unknown>" => {
                Self::ProviderImplementationRef {
                    model: name.to_string(),
                    source,
                }
            }
            Self::InvocationModeIsRootOnly { model, provider } if model == "<unknown>" => {
                Self::InvocationModeIsRootOnly {
                    model: name.to_string(),
                    provider,
                }
            }
            Self::Other(model, err) if model == "<unknown>" => {
                Self::Other(name.to_string(), err.replace("<unknown>", name))
            }
            other => other,
        }
    }

    pub fn contains(&self, needle: &str) -> bool {
        self.to_string().contains(needle)
    }
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "io error reading model TOML: {err}"),
            Self::Toml(model, err) => write!(f, "toml parse error in model {model}: {err}"),
            Self::ProviderImplementationRef { model, source } => {
                write!(f, "model {model}: {source}")
            }
            Self::InvocationModeIsRootOnly { model, provider } => write!(
                f,
                "model {model}: provider {provider}: invocation_mode is root-only and must move to providers.toml"
            ),
            Self::Other(model, err) => write!(f, "model {model} {err}"),
        }
    }
}

impl std::error::Error for ModelError {}

impl From<std::io::Error> for ModelError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<ModelError> for String {
    fn from(value: ModelError) -> Self {
        value.to_string()
    }
}

impl PartialEq for ModelError {
    fn eq(&self, other: &Self) -> bool {
        use ModelError::*;
        match (self, other) {
            (Io(left), Io(right)) => left.to_string() == right.to_string(),
            (Toml(left_model, left_err), Toml(right_model, right_err)) => {
                left_model == right_model && left_err == right_err
            }
            (
                ProviderImplementationRef {
                    model: left_model,
                    source: left_source,
                },
                ProviderImplementationRef {
                    model: right_model,
                    source: right_source,
                },
            ) => left_model == right_model && left_source == right_source,
            (
                InvocationModeIsRootOnly {
                    model: left_model,
                    provider: left_provider,
                },
                InvocationModeIsRootOnly {
                    model: right_model,
                    provider: right_provider,
                },
            ) => left_model == right_model && left_provider == right_provider,
            (Other(left_model, left_err), Other(right_model, right_err)) => {
                left_model == right_model && left_err == right_err
            }
            _ => false,
        }
    }
}

impl Eq for ModelError {}
