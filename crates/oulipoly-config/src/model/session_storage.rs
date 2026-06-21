//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - mapper
//! - orchestration
//! - predicate
//! - validator
//!
//! Role set: { accessor, formatter, mapper, orchestration, predicate, validator }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-config/src/model/session_storage.rs
//!     role: intrinsic-surface
//!     Domain: model-session-storage
//!     Owns:
//!       - SessionStorage locator routing configuration for script, Claude Code, and Codex sessions
//!       - ScriptSessionStorageType normalized script storage discriminant
//!       - PathBuf tilde expansion and shell-word rendering subordinate to session-storage commands
//! ```
//!
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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
            SessionStorage::Script { cwd_script, .. } => stored_script_cwd(cwd_script),
            SessionStorage::ClaudeCode { projects_dir } => {
                format_claude_code_cwd_script(projects_dir)
            }
            SessionStorage::Codex { sessions_dir } => format_codex_cwd_script(sessions_dir),
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
            SessionStorage::Script { storage_type, .. } => stored_script_storage_type(storage_type),
            SessionStorage::ClaudeCode { .. } => map_claude_code_storage_type(),
            SessionStorage::Codex { .. } => map_codex_storage_type(),
        }
    }
}

fn stored_script_cwd(cwd_script: &str) -> String {
    cwd_script.to_string()
}

fn stored_script_storage_type(
    storage_type: &Option<ScriptSessionStorageType>,
) -> Option<ScriptSessionStorageType> {
    *storage_type
}

fn map_claude_code_storage_type() -> Option<ScriptSessionStorageType> {
    Some(ScriptSessionStorageType::ClaudeCode)
}

fn map_codex_storage_type() -> Option<ScriptSessionStorageType> {
    Some(ScriptSessionStorageType::CodexSession)
}

fn shell_word(input: &str) -> String {
    if is_shell_safe(input) {
        return input.to_string();
    }
    quote_shell_word(input)
}

fn format_claude_code_cwd_script(projects_dir: &Path) -> String {
    format!(
        "claude-code-cwd {}",
        shell_word(&projects_dir.display().to_string())
    )
}

fn format_codex_cwd_script(sessions_dir: &Path) -> String {
    format!(
        "codex-cwd {}",
        shell_word(&sessions_dir.display().to_string())
    )
}

fn is_shell_safe(input: &str) -> bool {
    input
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | '~'))
}

fn quote_shell_word(input: &str) -> String {
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
