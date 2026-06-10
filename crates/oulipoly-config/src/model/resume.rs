//! ## Declared roles
//!
//! `validator`
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-config/src/model/resume.rs
//!     role: intrinsic-surface
//!     Domain: model_provider_session_config
//!     Owns:
//!       - provider resume strategy (resume command shape)
//! ```

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResumeStrategy {
    pub kind: ResumeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subcommand: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResumeKind {
    Flag,
    Subcommand,
}

impl ResumeStrategy {
    pub fn validate(&self) -> Result<(), String> {
        match self.kind {
            ResumeKind::Flag => {
                if self.flag.is_none() {
                    return Err("resume.kind = flag requires `flag`".into());
                }
                if self.subcommand.is_some() {
                    return Err("resume.kind = flag does not allow `subcommand`".into());
                }
                Ok(())
            }
            ResumeKind::Subcommand => {
                if !matches!(self.subcommand.as_ref(), Some(parts) if !parts.is_empty()) {
                    return Err("resume.kind = subcommand requires non-empty `subcommand`".into());
                }
                if self.flag.is_some() {
                    return Err("resume.kind = subcommand does not allow `flag`".into());
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResumeAcceptanceRules {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_output_patterns: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejected_output_patterns: Option<Vec<String>>,
}
