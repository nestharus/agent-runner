use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use super::ReplaceError;

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

    pub fn to_export(&self) -> crate::session_export::SessionStorageType {
        match self {
            StorageType::ClaudeCode => crate::session_export::SessionStorageType::ClaudeCode,
            StorageType::CodexSession => crate::session_export::SessionStorageType::CodexSession,
            StorageType::Other => crate::session_export::SessionStorageType::Other,
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
    pub fn acquire(
        data_root: &Path,
        provider_name: &str,
        session_id: &str,
    ) -> Result<Self, ReplaceError> {
        let lock_dir = data_root.join("locks");
        fs::create_dir_all(&lock_dir).map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to create lock dir: {e}"),
        })?;
        let _guard = LockAcquireGuard::acquire(&lock_dir)?;
        let path = lock_dir.join(lock_file_name(provider_name, session_id));
        let token = Uuid::new_v4().to_string();
        let now = Utc::now();
        let expires_at = now + chrono::Duration::minutes(5);

        match fs::read(&path) {
            Ok(bytes) => {
                let value = serde_json::from_slice::<Value>(&bytes).map_err(|e| {
                    ReplaceError::OperationalError {
                        message: format!("failed to parse lock metadata {}: {e}", path.display()),
                    }
                })?;
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
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(ReplaceError::OperationalError {
                    message: format!("failed to read lock: {e}"),
                });
            }
        }

        let payload = serde_json::json!({
            "version": 1,
            "provider_name": provider_name,
            "session_id": session_id,
            "token_hash": lock_token_hash(&token),
            "created_at": now.to_rfc3339(),
            "expires_at": expires_at.to_rfc3339(),
            "owner_pid": std::process::id(),
        });
        let tmp = lock_dir.join(format!(
            ".{}.{}.tmp",
            lock_file_name(provider_name, session_id),
            token
        ));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| ReplaceError::OperationalError {
                message: format!("failed to create lock temp: {e}"),
            })?;
        if let Err(e) = serde_json::to_writer(&file, &payload) {
            drop(file);
            let _ = fs::remove_file(&tmp);
            return Err(ReplaceError::OperationalError {
                message: format!("failed to write lock: {e}"),
            });
        }
        if let Err(e) = file.sync_all() {
            drop(file);
            let _ = fs::remove_file(&tmp);
            return Err(ReplaceError::OperationalError {
                message: format!("failed to fsync lock: {e}"),
            });
        }
        drop(file);
        if let Err(e) = fs::hard_link(&tmp, &path) {
            let _ = fs::remove_file(&tmp);
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                return Err(ReplaceError::SessionBusy {
                    token: "unknown".to_string(),
                    expires_at: "".to_string(),
                });
            }
            return Err(ReplaceError::OperationalError {
                message: format!("failed to publish lock: {e}"),
            });
        }
        let _ = fs::remove_file(&tmp);
        if let Err(e) = sync_dir(&lock_dir) {
            let _ = fs::remove_file(&path);
            let _ = sync_dir(&lock_dir);
            return Err(e);
        }
        Ok(Self { path, token })
    }

    pub fn any_active_for_session(
        data_root: &Path,
        session_id: &str,
    ) -> Result<bool, ReplaceError> {
        let lock_dir = data_root.join("locks");
        let session = sanitize_lock_component(session_id);
        let suffix = format!("-session-{session}.lock");
        let now = Utc::now();
        let entries = match fs::read_dir(&lock_dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => {
                return Err(ReplaceError::OperationalError {
                    message: format!("failed to read lock dir: {e}"),
                });
            }
        };
        for entry in entries {
            let path = entry
                .map_err(|e| ReplaceError::OperationalError {
                    message: format!("failed to read lock dir entry: {e}"),
                })?
                .path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !name.starts_with("provider-") || !name.ends_with(&suffix) {
                continue;
            }
            let Ok(bytes) = fs::read(&path) else {
                return Ok(true);
            };
            let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                return Ok(true);
            };
            let active = value
                .get("expires_at")
                .and_then(Value::as_str)
                .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
                .map(|dt| dt.with_timezone(&Utc) > now)
                .unwrap_or(true);
            if active {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        let should_remove = fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|value| {
                let expires_active = value
                    .get("expires_at")
                    .and_then(Value::as_str)
                    .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
                    .map(|dt| dt.with_timezone(&Utc) > Utc::now())
                    .unwrap_or(false);
                expires_active
                    .then(|| {
                        value
                            .get("token_hash")
                            .and_then(Value::as_str)
                            .map(|token_hash| token_hash == lock_token_hash(&self.token))
                    })
                    .flatten()
            })
            .unwrap_or(false);
        if should_remove {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn lock_file_name(provider_name: &str, session_id: &str) -> String {
    let provider = sanitize_lock_component(provider_name);
    let session = sanitize_lock_component(session_id);
    format!("provider-{provider}-session-{session}.lock")
}

fn lock_token_hash(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

fn sanitize_lock_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn sync_dir(path: &Path) -> Result<(), ReplaceError> {
    let file = File::open(path).map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to open lock dir for fsync: {e}"),
    })?;
    file.sync_all().map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to fsync lock dir: {e}"),
    })
}

struct LockAcquireGuard {
    path: PathBuf,
}

impl LockAcquireGuard {
    fn acquire(lock_dir: &Path) -> Result<Self, ReplaceError> {
        let path = lock_dir.join(".acquire.guard");
        match fs::create_dir(&path) {
            Ok(()) => Ok(Self { path }),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let stale = fs::metadata(&path)
                    .and_then(|metadata| metadata.modified())
                    .ok()
                    .and_then(|modified| modified.elapsed().ok())
                    .map(|age| age > std::time::Duration::from_secs(60))
                    .unwrap_or(false);
                if stale {
                    let _ = fs::remove_dir(&path);
                    fs::create_dir(&path).map_err(|e| ReplaceError::SessionBusy {
                        token: "unknown".to_string(),
                        expires_at: format!("lock acquire guard busy: {e}"),
                    })?;
                    Ok(Self { path })
                } else {
                    Err(ReplaceError::SessionBusy {
                        token: "unknown".to_string(),
                        expires_at: "lock acquire guard busy".to_string(),
                    })
                }
            }
            Err(e) => Err(ReplaceError::OperationalError {
                message: format!("failed to create lock acquire guard: {e}"),
            }),
        }
    }
}

impl Drop for LockAcquireGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.path);
    }
}
