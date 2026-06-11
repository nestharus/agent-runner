//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - mapper
//! - parser
//!
//! Role set: { accessor, formatter, mapper, parser }
//!
//! Provider/account discovery DTOs stored and loaded through `StateDb`.

use serde::{Deserialize, Serialize};

/// The type of a model parameter, stored as JSON in SQLite.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParamType {
    /// A parameter that accepts one of a fixed set of values.
    Enum { options: Vec<String> },
    /// A free-form string parameter.
    String,
    /// A numeric parameter with optional bounds.
    Number { min: Option<f64>, max: Option<f64> },
    /// A boolean flag parameter.
    Boolean,
}

/// How a parameter maps to CLI flags when invoking the model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CliMapping {
    /// The CLI flag, e.g. "--temperature" or "-m".
    pub flag: String,
    /// A template for the value, e.g. "{value}" or "model:{value}".
    pub value_template: String,
}

/// A model discovered from a CLI provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredModel {
    pub canonical_name: String,
    pub provider: String,
    pub discovered_at: String,
    pub cli_version: String,
}

/// A parameter for a discovered model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelParameter {
    pub name: String,
    pub display_name: String,
    pub param_type: ParamType,
    pub description: String,
    pub cli_mapping: CliMapping,
}

/// How an account authenticates with its provider CLI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthMethod {
    /// CLI handles the OAuth flow (browser redirect, token exchange).
    OAuth,
    /// Authentication via an API key, stored in an env var or config file.
    ApiKey {
        env_var: String,
        config_path: Option<String>,
    },
    /// Authentication via a CLI-specific config file.
    ConfigFile { path: String },
}

impl AuthMethod {
    /// Serialize to a JSON string for SQLite storage.
    pub(super) fn to_db_string(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| r#"{"type":"oauth"}"#.to_string())
    }

    /// Deserialize from a JSON string stored in SQLite.
    pub(super) fn from_db_string(s: &str) -> Self {
        serde_json::from_str(s).unwrap_or(AuthMethod::OAuth)
    }
}

/// Whether the account's authentication credentials are currently valid.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthStatus {
    Valid,
    Expired,
    Unknown,
    NoAuth,
}

impl AuthStatus {
    pub(super) fn as_str(&self) -> &'static str {
        match self {
            AuthStatus::Valid => "valid",
            AuthStatus::Expired => "expired",
            AuthStatus::Unknown => "unknown",
            AuthStatus::NoAuth => "no_auth",
        }
    }

    pub(super) fn from_str(s: &str) -> Self {
        match s {
            "valid" => AuthStatus::Valid,
            "expired" => AuthStatus::Expired,
            "no_auth" => AuthStatus::NoAuth,
            _ => AuthStatus::Unknown,
        }
    }
}

/// A CLI tool that can execute AI model requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliProviderRecord {
    pub cli_name: String,
    pub display_name: String,
    pub installed: bool,
    pub version: Option<String>,
    pub config_dir: Option<String>,
    pub last_synced: Option<String>,
}

/// An authenticated profile within a provider CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountRecord {
    pub id: String,
    pub provider: String,
    pub profile_name: String,
    pub auth_method: AuthMethod,
    pub auth_status: AuthStatus,
    pub created_at: String,
}
