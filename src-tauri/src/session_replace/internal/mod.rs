use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use super::ReplaceError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CanonicalRecord {
    pub session_id: String,
    pub provider_name: String,
    pub turn_id: String,
    pub role: String,
    pub timestamp: String,
    #[serde(default)]
    pub content: Vec<ContentChunk>,
    #[serde(default)]
    pub source: Value,
    #[serde(default)]
    pub unsupported_record: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentChunk {
    Text { text: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageType {
    ClaudeCode,
    CodexSession,
    Other,
}

impl StorageType {
    pub fn as_str(&self) -> &'static str {
        match self {
            StorageType::ClaudeCode => "claude_code",
            StorageType::CodexSession => "codex_session",
            StorageType::Other => "other",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionMetadata {
    pub session_id: String,
    pub chain_id: String,
    pub active_segment_id: i64,
    pub provider_name: String,
    pub storage_type: StorageType,
    pub jsonl_path: PathBuf,
}

pub struct SessionLock {
    path: PathBuf,
    token: String,
}

impl SessionLock {
    pub fn acquire(data_root: &Path, session_id: &str) -> Result<Self, ReplaceError> {
        let lock_dir = data_root.join("locks");
        fs::create_dir_all(&lock_dir).map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to create lock dir: {e}"),
        })?;
        let path = lock_dir.join(format!("session-{session_id}.lock"));
        let token = Uuid::new_v4().to_string();
        let now = Utc::now();
        let expires_at = now + chrono::Duration::minutes(5);

        if let Ok(bytes) = fs::read(&path)
            && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
        {
            let active = value
                .get("expires_at")
                .and_then(Value::as_str)
                .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
                .map(|dt| dt.with_timezone(&Utc) > now)
                .unwrap_or(true);
            if active {
                return Err(ReplaceError::SessionBusy {
                    token: value
                        .get("token_hash")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string(),
                    expires_at: value
                        .get("expires_at")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                });
            }
            let _ = fs::remove_file(&path);
        }

        let payload = serde_json::json!({
            "version": 1,
            "session_id": session_id,
            "token_hash": format!("sha256:{token}"),
            "created_at": now.to_rfc3339(),
            "expires_at": expires_at.to_rfc3339(),
            "owner_pid": std::process::id(),
        });
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    ReplaceError::SessionBusy {
                        token: "unknown".to_string(),
                        expires_at: "".to_string(),
                    }
                } else {
                    ReplaceError::OperationalError {
                        message: format!("failed to create lock: {e}"),
                    }
                }
            })?;
        serde_json::to_writer(&file, &payload).map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to write lock: {e}"),
        })?;
        file.sync_all()
            .map_err(|e| ReplaceError::OperationalError {
                message: format!("failed to fsync lock: {e}"),
            })?;
        Ok(Self { path, token })
    }
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        let should_remove = fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|value| {
                value
                    .get("token_hash")
                    .and_then(Value::as_str)
                    .map(|token_hash| token_hash == format!("sha256:{}", self.token))
            })
            .unwrap_or(false);
        if should_remove {
            let _ = fs::remove_file(&self.path);
        }
    }
}
