//! ## Declared roles
//!
//! `accessor`, `formatter`, `mapper`, `predicate`, `validator`
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-config/src/model/session_storage.rs
//!     role: intrinsic-surface
//!     Domain: model_provider_session_config
//!     Owns:
//!       - session_storage configuration (locator routing)
//! ```

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionStorage {
    Script {
        cwd_script: String,
        #[serde(default)]
        transcript_script: Option<String>,
        #[serde(default)]
        storage_type: Option<ScriptSessionStorageType>,
    },
    ClaudeCode {
        projects_dir: PathBuf,
    },
    Codex {
        sessions_dir: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScriptSessionStorageType {
    ClaudeCode,
    CodexSession,
}

impl SessionStorage {
    pub fn expand_tilde(self) -> Self {
        match self {
            SessionStorage::Script {
                cwd_script,
                transcript_script,
                storage_type,
            } => SessionStorage::Script {
                cwd_script,
                transcript_script,
                storage_type,
            },
            SessionStorage::ClaudeCode { projects_dir } => SessionStorage::ClaudeCode {
                projects_dir: expand_leading_tilde(projects_dir),
            },
            SessionStorage::Codex { sessions_dir } => SessionStorage::Codex {
                sessions_dir: expand_leading_tilde(sessions_dir),
            },
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            SessionStorage::Script {
                cwd_script,
                transcript_script,
                storage_type,
            } => {
                if cwd_script.trim().is_empty() {
                    return Err("session_storage.kind = script requires `cwd_script`".into());
                }
                if transcript_script
                    .as_ref()
                    .is_some_and(|script| script.trim().is_empty())
                {
                    return Err(
                        "session_storage.kind = script requires non-empty `transcript_script`"
                            .into(),
                    );
                }
                if transcript_script.is_some() != storage_type.is_some() {
                    return Err(
                        "session_storage.kind = script requires `transcript_script` and `storage_type` together"
                            .into(),
                    );
                }
            }
            SessionStorage::ClaudeCode { projects_dir } => {
                if projects_dir.as_os_str().is_empty() {
                    return Err("session_storage.kind = claude_code requires `projects_dir`".into());
                }
            }
            SessionStorage::Codex { sessions_dir } => {
                if sessions_dir.as_os_str().is_empty() {
                    return Err("session_storage.kind = codex requires `sessions_dir`".into());
                }
            }
        }
        Ok(())
    }

    pub fn cwd_script(&self) -> String {
        match self {
            SessionStorage::Script { cwd_script, .. } => cwd_script.clone(),
            SessionStorage::ClaudeCode { projects_dir } => {
                format!(
                    "claude-code-cwd {}",
                    shell_word(&projects_dir.display().to_string())
                )
            }
            SessionStorage::Codex { sessions_dir } => {
                format!(
                    "codex-cwd {}",
                    shell_word(&sessions_dir.display().to_string())
                )
            }
        }
    }

    pub fn transcript_script(&self) -> Option<&str> {
        match self {
            SessionStorage::Script {
                transcript_script, ..
            } => transcript_script.as_deref(),
            _ => None,
        }
    }

    pub fn script_storage_type(&self) -> Option<ScriptSessionStorageType> {
        match self {
            SessionStorage::Script { storage_type, .. } => *storage_type,
            SessionStorage::ClaudeCode { .. } => Some(ScriptSessionStorageType::ClaudeCode),
            SessionStorage::Codex { .. } => Some(ScriptSessionStorageType::CodexSession),
        }
    }
}

fn shell_word(input: &str) -> String {
    if input
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | '~'))
    {
        return input.to_string();
    }
    format!("'{}'", input.replace('\'', r#"'\''"#))
}

fn expand_leading_tilde(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    let Some(home) = dirs::home_dir() else {
        return path;
    };
    if raw == "~" {
        return home;
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return home.join(rest);
    }
    path
}
