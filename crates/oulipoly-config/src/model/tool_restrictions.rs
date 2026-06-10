//! ## Declared roles
//!
//! `accessor`, `formatter`, `predicate`

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct ToolRestrictions {
    pub kind: ToolRestrictionKind,
    #[serde(default, skip_serializing_if = "ClaudeRestrictions::is_empty")]
    pub claude: ClaudeRestrictions,
    #[serde(default, skip_serializing_if = "CodexRestrictions::is_empty")]
    pub codex: CodexRestrictions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolRestrictionKind {
    #[default]
    Claude,
    Codex,
}

impl fmt::Display for ToolRestrictionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Claude => f.write_str("claude"),
            Self::Codex => f.write_str("codex"),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaudeRestrictions {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disallowed_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub disable_slash_commands: bool,
}

impl ClaudeRestrictions {
    pub fn is_empty(&self) -> bool {
        self.disallowed_tools.is_empty()
            && self.allowed_tools.is_empty()
            && !self.disable_slash_commands
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodexRestrictions {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config_pairs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_features: Vec<String>,
}

impl CodexRestrictions {
    pub fn is_empty(&self) -> bool {
        self.config_pairs.is_empty() && self.disabled_features.is_empty()
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}
